"""Offline CLI and transport tests for xsts_probe.py."""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock
import urllib.error
import xml.etree.ElementTree as ET


TOOL = Path(__file__).with_name("xsts_probe.py")
DEFAULT_URL = "https://title.mgt.xboxlive.com/titles/current/endpoints"
REPLY = (
    "<XSTSTokenResponse><Token>XBL3.0 x=PRIVATE_HASH;PRIVATE_TOKEN</Token>"
    "<Signature>PRIVATE_SIGNATURE</Signature><Xuid>PRIVATE_XUID</Xuid>"
    "<Gamertag>PRIVATE_GAMERTAG</Gamertag><Expiry>2000000000</Expiry>"
    "</XSTSTokenResponse>"
).encode()


def receive_exact(connection, size):
    result = bytearray()
    while len(result) < size:
        part = connection.recv(size - len(result))
        if not part:
            raise EOFError("incomplete test request")
        result.extend(part)
    return bytes(result)


class ProbeWireTests(unittest.TestCase):
    def invoke(self, arguments, config=None, reply=REPLY, magic=0x58445358):
        with tempfile.TemporaryDirectory() as folder:
            game = Path(folder, "game")
            game.mkdir()
            if config is not None:
                (game / "MICROSOFTGAME.CONFIG").write_text(config)
                arguments = [*arguments, "--game-dir", str(game)]
            captured = []
            errors = []
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
                listener.bind(str(Path(folder, "xodus.sock")))
                listener.listen(1)
                listener.settimeout(3)

                def serve():
                    try:
                        with listener.accept()[0] as connection:
                            header = receive_exact(connection, 8)
                            captured.append((header, receive_exact(connection, struct.unpack("<IHH", header)[2])))
                            connection.sendall(struct.pack("<IHH", magic, 6, len(reply)) + reply)
                    except Exception as error:
                        errors.append(type(error).__name__)

                thread = threading.Thread(target=serve, daemon=True)
                thread.start()
                result = subprocess.run(
                    [sys.executable, str(TOOL), *arguments],
                    env={**os.environ, "XDG_RUNTIME_DIR": folder},
                    capture_output=True, text=True, timeout=5,
                )
                thread.join(4)
            self.assertFalse(errors, errors)
            self.assertTrue(captured, result.stderr)
            return result, captured[0]

    def test_positional_url_is_xml_escaped_and_title_metadata_is_forwarded(self):
        url = "https://service.example.test/path?one=1&two=2"
        result, (header, payload) = self.invoke([url], config=(
            "<Game><TitleId>1234ABCD</TitleId><MSAAppId>synthetic-client</MSAAppId></Game>"
        ))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(struct.unpack("<IHH", header)[:2], (0x58445358, 5))
        try:
            request = ET.fromstring(payload)
        except ET.ParseError:
            self.fail("the positional URL must be serialized as valid XML")
        self.assertEqual(request.findtext("Url"), url)
        self.assertEqual(request.findtext("Method"), "GET")
        self.assertEqual(request.findtext("TitleId"), str(0x1234ABCD))
        self.assertEqual(request.findtext("TitleClientId"), "synthetic-client")

    def test_default_url_and_legacy_config_do_not_expose_account_fields(self):
        result, (_, payload) = self.invoke([], config="<Game><TitleId>1234ABCD</TitleId></Game>")
        self.assertEqual(result.returncode, 0, result.stderr)
        request = ET.fromstring(payload)
        self.assertEqual(request.findtext("Url"), DEFAULT_URL)
        self.assertEqual(request.findtext("TitleId"), str(0x1234ABCD))
        self.assertEqual(request.findtext("TitleClientId", ""), "")
        for secret in ["PRIVATE_HASH", "PRIVATE_TOKEN", "PRIVATE_SIGNATURE", "PRIVATE_XUID", "PRIVATE_GAMERTAG"]:
            self.assertNotIn(secret, result.stdout + result.stderr)

    def test_invalid_header_and_xml_are_reported_without_tracebacks_or_payloads(self):
        for reply, magic in [(REPLY, 0), (b"<PRIVATE_TOKEN", 0x58445358)]:
            with self.subTest(magic=magic):
                result, _ = self.invoke([], reply=reply, magic=magic)
                self.assertEqual(result.returncode, 1)
                self.assertNotIn("PRIVATE_", result.stdout + result.stderr)
                self.assertNotIn("Traceback", result.stderr)


class ProbeUnitTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location("xsts_probe", TOOL)
        cls.probe = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.probe)

    def test_uid_fallback_uses_current_user(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(os, "getuid", return_value=4321):
            self.assertEqual(self.probe.socket_path(), "/run/user/4321/xodus.sock")

    def test_invalid_title_ids_are_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            config = Path(folder, "MicrosoftGame.config")
            for title_id in ["0", "-1", "100000000", "not-hex"]:
                config.write_text(f"<Game><TitleId>{title_id}</TitleId></Game>")
                with self.subTest(title_id=title_id), self.assertRaises(ValueError):
                    self.probe.read_title_metadata(Path(folder))

    def test_endpoint_check_uses_only_fixed_get_and_does_not_follow_redirects(self):
        opener = mock.Mock()
        response = mock.MagicMock()
        response.status = 200
        response.read.return_value = json.dumps({"EndPoints": [{}, {}]}).encode()
        response.__enter__.return_value = response
        opener.open.return_value = response
        output = io.StringIO()
        with mock.patch.object(self.probe.urllib.request, "build_opener", return_value=opener) as build, contextlib.redirect_stdout(output):
            self.assertTrue(self.probe.check_endpoints(ET.fromstring(REPLY)))
        request = opener.open.call_args.args[0]
        self.assertEqual(request.full_url, DEFAULT_URL)
        self.assertEqual(request.get_method(), "GET")
        self.assertEqual(request.get_header("Authorization"), "XBL3.0 x=PRIVATE_HASH;PRIVATE_TOKEN")
        self.assertEqual(request.get_header("Signature"), "PRIVATE_SIGNATURE")
        handler = build.call_args.args[0]
        self.assertIsNone(handler.redirect_request(request, None, 302, "Found", {}, "https://other.example.test/"))
        self.assertIn("HTTP 200", output.getvalue())
        self.assertIn("2", output.getvalue())
        self.assertNotIn("PRIVATE_", output.getvalue())

    def test_endpoint_check_rejects_custom_url_before_requesting_credentials(self):
        with mock.patch.object(self.probe, "request_token") as request, contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as error:
                self.probe.main(["https://other.example.test/", "--check-endpoints"])
        self.assertEqual(error.exception.code, 2)
        request.assert_not_called()


if __name__ == "__main__":
    unittest.main()
