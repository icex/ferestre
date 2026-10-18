"""Offline tests for how the developer tools handle output, sockets and secrets.

    python3 -m unittest discover -s tools -p test_tool_hygiene.py

Nothing here reaches a real service, a game or the network: every socket and
HTTP server is a local fake, and the desktop drivers are only imported, never
run.
"""

import http.server
import importlib.util
import io
import contextlib
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock

TOOLS = Path(__file__).resolve().parent
REPO = TOOLS.parent
sys.path.insert(0, str(TOOLS))

import private_out  # noqa: E402

XML_MAGIC = 0x58445358


def load(path, name, environment):
    """Import a tool by path with `environment` in place, the way its module
    level code would see it when run."""
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    with mock.patch.dict(os.environ, environment, clear=True):
        spec.loader.exec_module(module)
    return module


def receive_exact(connection, size):
    result = bytearray()
    while len(result) < size:
        part = connection.recv(size - len(result))
        if not part:
            raise EOFError("incomplete test request")
        result.extend(part)
    return bytes(result)


class FakeService:
    """A stand-in xodus-service on a Unix socket: records each request and
    answers every one with `reply` (an empty body when it is empty)."""

    def __init__(self, path, reply=b""):
        self.requests = []
        self.reply = reply
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.listener.bind(str(path))
        self.listener.listen(16)
        self.listener.settimeout(0.2)
        self.stopping = threading.Event()
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self):
        while not self.stopping.is_set():
            try:
                connection = self.listener.accept()[0]
            except (socket.timeout, OSError):
                continue
            with connection:
                try:
                    magic, kind, size = struct.unpack("<IHH", receive_exact(connection, 8))
                    self.requests.append((kind, receive_exact(connection, size)))
                    connection.sendall(struct.pack("<IHH", XML_MAGIC, kind + 1, len(self.reply)) + self.reply)
                except (EOFError, OSError):
                    pass

    def close(self):
        self.stopping.set()
        self.thread.join(2)
        self.listener.close()


def clean_environment(**values):
    """This process's environment without any route to a real service."""
    environment = {key: value for key, value in os.environ.items()
                   if key not in ("XODUS_RUNTIME_DIR", "XDG_RUNTIME_DIR")}
    environment.update(values)
    return environment


class PrivateOutputTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="ferestre-tool-out-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.root.chmod(0o700)

    def test_a_missing_directory_is_created_for_this_user_only(self):
        made = private_out.private_dir(str(self.root / "out"), "unused")
        self.assertEqual(made, self.root / "out")
        self.assertEqual(made.stat().st_mode & 0o777, 0o700)

    def test_the_default_is_in_the_session_runtime_directory(self):
        with mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(self.root)}):
            self.assertEqual(private_out.private_dir(None, "ferestre-x"), self.root / "ferestre-x")

    def test_a_directory_others_can_write_is_refused(self):
        shared = self.root / "shared"
        shared.mkdir()
        shared.chmod(0o777)
        with self.assertRaises(SystemExit) as refused:
            private_out.private_dir(str(shared), "unused")
        self.assertIn("other users can write", str(refused.exception))

    def test_a_directory_someone_else_owns_is_refused(self):
        theirs = self.root / "theirs"
        theirs.mkdir(mode=0o700)
        with mock.patch.object(os, "geteuid", return_value=os.geteuid() + 1), self.assertRaises(SystemExit) as refused:
            private_out.private_dir(str(theirs), "unused")
        self.assertIn("belongs to uid", str(refused.exception))

    def test_a_link_to_an_open_directory_is_judged_by_where_it_leads(self):
        shared = self.root / "shared"
        shared.mkdir()
        shared.chmod(0o777)
        (self.root / "link").symlink_to(shared)
        with self.assertRaises(SystemExit):
            private_out.private_dir(str(self.root / "link"), "unused")

    def test_a_private_directory_inside_an_open_one_is_refused(self):
        # Whoever can write to the parent can rename ours away and put theirs
        # in its place; only a sticky parent such as /tmp prevents that.
        shared = self.root / "shared"
        shared.mkdir()
        shared.chmod(0o777)
        (shared / "out").mkdir(mode=0o700)
        with self.assertRaises(SystemExit) as refused:
            private_out.private_dir(str(shared / "out"), "unused")
        self.assertIn(str(shared), str(refused.exception))
        shared.chmod(0o1777)
        self.assertEqual(private_out.private_dir(str(shared / "out"), "unused"), shared / "out")


class DesktopDriverTests(unittest.TestCase):
    """The drivers only decide their output directory and their bot at import
    time and in _find_bot; nothing below launches anything."""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="ferestre-drivers-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.root.chmod(0o700)
        self.environment = {"HOME": str(self.root), "XDG_RUNTIME_DIR": str(self.root / "run"),
                            "XODUS_GAMES_DIR": str(self.root / "games")}
        (self.root / "run").mkdir(mode=0o700)

    def test_outputs_default_to_a_private_directory_not_tmp(self):
        for script, name in (("auto-join.py", "ferestre-autojoin"),
                             ("drive-aoe.py", "ferestre-drive-aoe"),
                             ("drive-fh5.py", "ferestre-drive-fh5")):
            with self.subTest(script=script):
                module = load(TOOLS / script, script.replace("-", "_")[:-3], self.environment)
                self.assertEqual(module.OUT, self.root / "run" / name)
                self.assertEqual(module.OUT.stat().st_mode & 0o777, 0o700)

    def test_an_output_directory_others_can_write_is_refused(self):
        shared = self.root / "shared"
        shared.mkdir()
        shared.chmod(0o777)
        for script, variable in (("auto-join.py", "XODUS_AUTOJOIN_OUT"),
                                 ("drive-aoe.py", "AOE_OUT"),
                                 ("drive-fh5.py", "FH5_OUT")):
            with self.subTest(script=script), self.assertRaises(SystemExit):
                load(TOOLS / script, script.replace("-", "_")[:-3], dict(self.environment, **{variable: str(shared)}))

    def test_auto_join_never_runs_a_bot_found_among_its_outputs(self):
        module = load(TOOLS / "auto-join.py", "auto_join", self.environment)
        (module.OUT / "bot.exe").write_text("planted")
        with mock.patch.dict(os.environ, self.environment, clear=True), self.assertRaises(SystemExit):
            module._find_bot()
        staged = self.root / "games" / "bedrock" / "game"
        staged.mkdir(parents=True)
        (staged / "_bot.exe").write_text("built by the developer")
        with mock.patch.dict(os.environ, self.environment, clear=True):
            self.assertEqual(module._find_bot(), staged / "_bot.exe")


class ProbeSocketTests(unittest.TestCase):
    """msa_probe.py and endpoint_suite.py ask the service the launcher started:
    XODUS_RUNTIME_DIR first, then the session directory, or --socket."""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="ferestre-probe-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.scoped = self.root / "ferestre-services" / ("0" * 32)
        self.scoped.mkdir(parents=True)
        self.session = self.root / "session"
        self.session.mkdir()

    def serve(self, path, reply=b""):
        service = FakeService(path, reply)
        self.addCleanup(service.close)
        return service

    def run_tool(self, tool, arguments, environment):
        return subprocess.run([sys.executable, str(tool), *arguments], env=environment,
                              capture_output=True, text=True, timeout=30)

    def test_msa_probe_prefers_xodus_runtime_dir(self):
        reply = b"<MSATokenResponse><Token>PRIVATE_TOKEN</Token><Expiry>2000000000</Expiry></MSATokenResponse>"
        service = self.serve(self.scoped / "xodus.sock", reply)
        result = self.run_tool(TOOLS / "msa_probe.py", [], clean_environment(
            XODUS_RUNTIME_DIR=str(self.scoped), XDG_RUNTIME_DIR=str(self.session)))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(len(service.requests), 1)
        self.assertNotIn("PRIVATE_TOKEN", result.stdout + result.stderr)

    def test_msa_probe_takes_a_socket_option(self):
        reply = b"<MSATokenResponse><Token>PRIVATE_TOKEN</Token></MSATokenResponse>"
        service = self.serve(self.root / "given.sock", reply)
        result = self.run_tool(TOOLS / "msa_probe.py", ["--socket", str(self.root / "given.sock")],
                               clean_environment(XDG_RUNTIME_DIR=str(self.session)))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(len(service.requests), 1)

    def test_endpoint_suite_prefers_xodus_runtime_dir(self):
        # An empty answer means no token, so no case goes on to the network.
        service = self.serve(self.scoped / "xodus.sock")
        result = self.run_tool(REPO / "tests" / "endpoint_suite.py", [], clean_environment(
            XODUS_RUNTIME_DIR=str(self.scoped), XDG_RUNTIME_DIR=str(self.session)))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(service.requests), 8, result.stdout)
        self.assertNotIn("TOKEN ERROR", result.stdout)

    def test_endpoint_suite_takes_a_socket_option(self):
        service = self.serve(self.root / "given.sock")
        result = self.run_tool(REPO / "tests" / "endpoint_suite.py", ["--socket", str(self.root / "given.sock")],
                               clean_environment(XDG_RUNTIME_DIR=str(self.session)))
        self.assertEqual(len(service.requests), 8, result.stdout)


class EndpointRedirectTests(unittest.TestCase):
    def test_credentials_never_follow_a_redirect(self):
        landed = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path == "/start":
                    self.send_response(302)
                    self.send_header("Location", f"http://127.0.0.1:{self.server.server_port}/elsewhere")
                    self.end_headers()
                    return
                landed.append(dict(self.headers))
                self.send_response(200)
                self.end_headers()

            def log_message(self, *args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        suite = load(REPO / "tests" / "endpoint_suite.py", "endpoint_suite", {})
        suite.CASES = [("redirected", "GET", f"http://127.0.0.1:{server.server_port}/start", b"", {})]
        output = io.StringIO()
        without_proxies = {key: value for key, value in os.environ.items() if "proxy" not in key.lower()}
        with mock.patch.dict(os.environ, without_proxies, clear=True), \
                mock.patch.object(suite, "token_for", return_value=("XBL3.0 x=PRIVATE;TOKEN", "PRIVATE_SIGNATURE")), \
                contextlib.redirect_stdout(output):
            status = suite.main(["--socket", str(Path(tempfile.gettempdir(), "no-such-service.sock"))])
        self.assertEqual(landed, [], "the redirect was followed")
        self.assertEqual(status, 1, output.getvalue())
        self.assertIn("redirect not followed", output.getvalue())
        self.assertNotIn("PRIVATE", output.getvalue())


@unittest.skipUnless(shutil.which("cc"), "needs a C compiler")
class XCurlTraceRedactionTests(unittest.TestCase):
    """The XCurl tracer's redaction, compiled natively on its own."""

    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix="ferestre-xcurl-")
        cls.binary = Path(cls.directory.name) / "redact"
        subprocess.run(["cc", "-DXCURL_REDACT_TEST", "-Wall", "-Werror", "-o", str(cls.binary),
                        str(TOOLS / "xcurl_trace.c")], check=True)

    @classmethod
    def tearDownClass(cls):
        cls.directory.cleanup()

    def redact(self, block, *urls):
        return subprocess.run([str(self.binary), *urls], input=block, capture_output=True,
                              check=True, timeout=10).stdout.decode()

    def test_credentials_in_headers_are_reduced_to_their_length(self):
        block = (b"POST /Client/LoginWithXbox?sig=QUERYSECRET HTTP/1.1\r\n"
                 b"Host: titleid.playfabapi.com\r\n"
                 b"X-Authorization: SESSIONTICKETSECRET\r\n"
                 b"X-EntityToken: ENTITYTOKENSECRET\r\n"
                 b"X-SecretKey: TITLESECRET\r\n"
                 b"Proxy-Authorization: Basic PROXYSECRET\r\n"
                 b"authorization: XBL3.0 x=XBLSECRET\r\n"
                 b"Signature: SIGNATURESECRET\r\n"
                 b"Cookie: COOKIESECRET\r\n"
                 b"Content-Type: application/json\r\n"
                 b"Location: https://cdn.example.test/file?se=2026&sig=LOCATIONSECRET\r\n\r\n")
        log = self.redact(block)
        self.assertNotIn("SECRET", log)
        for kept in ("X-Authorization: <redacted", "X-EntityToken: <redacted", "Proxy-Authorization: <redacted",
                     "Content-Type: application/json", "Host: titleid.playfabapi.com",
                     "POST /Client/LoginWithXbox?sig=<11> HTTP/1.1", "file?se=<4>&sig=<14>"):
            self.assertIn(kept, log)

    def test_logged_urls_keep_parameter_names_but_not_values(self):
        log = self.redact(b"", "https://store.blob.core.windows.net/c/f.msixvc?sv=2024&sig=SASSECRET&se=x",
                          "https://example.test/cb?access_token=TOKENSECRET#frag",
                          "https://example.test/plain/path")
        self.assertNotIn("SECRET", log)
        self.assertIn("f.msixvc?sv=<4>&sig=<9>&se=<1>", log)
        self.assertIn("cb?access_token=<11>#<4>", log)
        self.assertIn("--> GET https://example.test/plain/path", log)


if __name__ == "__main__":
    unittest.main()
