#!/usr/bin/env python3
"""Synthetic xodus-service and Store licence-token endpoint for xgr_tests.

XStoreQueryLicenseTokenAsync asks xodus-service to sign a POST and then sends
that POST to the Store's licensing endpoint. The regression test has neither a
signed-in account nor Microsoft, so this stands in for both: it answers the
XSTS frame on the service socket and POST /v8.0/licenseToken over plain HTTP.

It also asserts what it was asked -- the body the runtime built out of the
caller's product ids, and the identity the service handed back -- because the
returned token only proves the answer travelled; the request is the part that
has to be exactly right.

Usage:
    license_token_server.py <runtime-dir> <transcript> <url-file>

    <runtime-dir>   becomes XDG_RUNTIME_DIR; the service socket is <dir>/xodus.sock
    <transcript>    what it saw, one line per observation
    <url-file>      written with the endpoint URL once it is listening

Exits 0 when it saw what the regression test expects, 1 otherwise (run-tests.sh
checks the status after SIGTERM). Never touches the network or an account.
"""

import base64
import http.server
import json
import os
import signal
import socket
import struct
import sys
import threading
import xml.etree.ElementTree as ET

XML_MAGIC = 0x58445358
XSTS_TOKEN_REQUEST = 5
XSTS_TOKEN_RESPONSE = XSTS_TOKEN_REQUEST + 1

# What the runtime is handed, and what xgr_tests.c compares its result against.
TOKEN = "XBL3.0 x=1;fixture-xsts-token"
SIGNATURE = "fixture-signature"
LICENSE_TOKEN = "eyJhbGciOiJSUzI1NiJ9.Zml4dHVyZS1saWNlbmNlLXRva2Vu.c2lnbmF0dXJl"

EXPECTED_STORE_ID = "9TESTSTORE0ID"                      # tests/fixtures/MicrosoftGame.config
EXPECTED_IDS = ["9TESTSTORE0ID", "9TESTADDON00"]
EXPECTED_CUSTOM = "regression-test"

_lock = threading.Lock()
_observations = []
_problems = []
_stop = threading.Event()
_license_requests = 0


def note(line):
    with _lock:
        _observations.append(line)
    print(f":: fixture {line}", flush=True)


def problem(line):
    with _lock:
        _problems.append(line)
        _observations.append(f"PROBLEM: {line}")
    print(f":: fixture PROBLEM: {line}", flush=True)


def receive_exact(connection, size):
    result = bytearray()
    while len(result) < size:
        part = connection.recv(size - len(result))
        if not part:
            raise EOFError("short read")
        result.extend(part)
    return bytes(result)


def send_frame(connection, message_type, payload):
    connection.sendall(struct.pack("<IHH", XML_MAGIC, message_type, len(payload)) + payload)


def check_body(text, where):
    """The body the runtime builds, checked field by field."""
    try:
        body = json.loads(text)
    except ValueError as error:
        problem(f"{where}: body is not JSON ({error})")
        return
    if body.get("parentProductId") != EXPECTED_STORE_ID:
        problem(f"{where}: parentProductId is {body.get('parentProductId')!r}")
    expect = json.dumps(EXPECTED_IDS, separators=(",", ":"))
    if body.get("relatedProductIds") != expect:
        problem(f"{where}: relatedProductIds is {body.get('relatedProductIds')!r}, wanted {expect!r}")
    if body.get("customDeveloperString") != EXPECTED_CUSTOM:
        problem(f"{where}: customDeveloperString is {body.get('customDeveloperString')!r}")
    if body.get("enforceSellableBy") is not True:
        problem(f"{where}: enforceSellableBy is {body.get('enforceSellableBy')!r}")


def _token_response():
    return (
        f"<XSTSTokenResponse><Token>{TOKEN}</Token><Signature>{SIGNATURE}</Signature>"
        f"<Xuid>1</Xuid><Gamertag>FixturePlayer</Gamertag><Expiry>4102444800</Expiry>"
        f"</XSTSTokenResponse>"
    )


def serve_service(socket_path, endpoint_url):
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(socket_path)
    listener.listen(4)
    listener.settimeout(0.5)
    while not _stop.is_set():
        try:
            connection, _ = listener.accept()
        except socket.timeout:
            continue
        except OSError:
            break
        with connection:
            connection.settimeout(30)
            try:
                magic, message_type, size = struct.unpack("<IHH", receive_exact(connection, 8))
                payload = receive_exact(connection, size)
            except (EOFError, OSError):
                problem("service: truncated frame")
                continue
            if magic != XML_MAGIC or message_type != XSTS_TOKEN_REQUEST:
                problem(f"service: frame magic {magic:#x} type {message_type}")
                continue
            request = ET.fromstring(payload)
            url = request.findtext("Url") or ""
            method = request.findtext("Method") or ""
            if url != endpoint_url:
                # The runtime resolves the signed-in identity against other
                # hosts; only the licence POST is this fixture's business.
                note(f"service: token for {method} {url} (not the licence endpoint)")
                send_frame(connection, XSTS_TOKEN_RESPONSE, _token_response().encode())
                continue
            note(f"service: token for {method} {url}")
            if method != "POST":
                problem(f"service: method is {method!r}")
            try:
                body = base64.b64decode(request.findtext("BodyBase64") or "")
            except ValueError as error:
                problem(f"service: BodyBase64 does not decode ({error})")
            else:
                check_body(body.decode("utf-8", "replace"), "service")
            send_frame(connection, XSTS_TOKEN_RESPONSE, _token_response().encode())
    listener.close()


class Handler(http.server.BaseHTTPRequestHandler):
    endpoint_path = "/v8.0/licenseToken"

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        note(f"http: POST {self.path} ({length} bytes)")
        if self.path != self.endpoint_path:
            problem(f"http: path is {self.path!r}")
        if self.headers.get("Authorization") != TOKEN:
            problem(f"http: Authorization is {self.headers.get('Authorization')!r}")
        if self.headers.get("Content-Type") != "application/json; charset=UTF-8":
            problem(f"http: Content-Type is {self.headers.get('Content-Type')!r}")
        check_body(body.decode("utf-8", "replace"), "http")

        # The first request is served; every later one is refused, which is how
        # xgr_tests exercises "an endpoint that does not answer is a failed
        # call, not an empty token" without touching the runtime's environment
        # (the test binary and the DLL do not share a CRT, so _putenv in one is
        # invisible to getenv in the other).
        with _lock:
            global _license_requests
            _license_requests += 1
            served = _license_requests == 1
        if served:
            payload = json.dumps({"licenseToken": LICENSE_TOKEN}).encode()
            self.send_response(200)
        else:
            note(f"http: refusing request {_license_requests}")
            payload = json.dumps({"error": "refused"}).encode()
            self.send_response(503)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args):
        pass


def main():
    if len(sys.argv) != 4:
        print(__doc__)
        return 2
    runtime_dir, transcript, url_file = sys.argv[1:]
    os.makedirs(runtime_dir, exist_ok=True)
    socket_path = os.path.join(runtime_dir, "xodus.sock")
    if os.path.exists(socket_path):
        os.unlink(socket_path)

    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    endpoint_url = f"http://127.0.0.1:{httpd.server_address[1]}{Handler.endpoint_path}"

    def stop(_signum, _frame):
        _stop.set()

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)

    service = threading.Thread(target=serve_service, args=(socket_path, endpoint_url), daemon=True)
    service.start()
    threading.Thread(target=httpd.serve_forever, kwargs={"poll_interval": 0.2}, daemon=True).start()
    with open(url_file, "w") as f:
        f.write(endpoint_url)
    note(f"listening: {endpoint_url}")

    _stop.wait(600)  # the harness always sends SIGTERM; this is the runaway case
    httpd.shutdown()
    with open(transcript, "w") as f:
        f.write("\n".join(_observations) + "\n")
    if _problems:
        print(f":: fixture saw {len(_problems)} problem(s); see {transcript}", flush=True)
        return 1
    note(f"clean: {len(_observations)} observations")
    return 0


if __name__ == "__main__":
    sys.exit(main())
