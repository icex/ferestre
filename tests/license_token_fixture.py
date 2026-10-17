#!/usr/bin/env python3
"""Account-free DLL -> Rust service -> HTTP fixture.

The Rust executable is built by build-license-service-fixture.sh. Its fixture
mode exists only under cfg(test), injects synthetic signing, and executes the
production licensing serializer/HTTP/parser. Production has no URL override.
"""
import base64
import http.server
import json
import os
from pathlib import Path
import signal
import ssl
import subprocess
import sys
import threading
import time

TOKEN = "eyJhbGciOiJSUzI1NiJ9.Zml4dHVyZS1saWNlbmNlLXRva2Vu.c2lnbmF0dXJl"


def main():
    runtime, executable, transcript = sys.argv[1:]
    root = Path(runtime)
    root.mkdir(parents=True, exist_ok=True)
    stop = threading.Event()
    problems = []
    seen = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            problems.append("redirect destination received GET")
            self.send_error(500)

        def do_POST(self):
            raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            if self.path != "/v8.0/licenseToken":
                problems.append("redirect destination received POST")
            if self.headers.get("Authorization") != "fixture-auth":
                problems.append("wrong synthetic Authorization")
            try:
                signed = base64.b64decode(self.headers.get("Signature", ""), validate=True)
                assert signed == raw
                body = json.loads(raw)
                assert body["parentProductId"] == "9TESTSTORE0ID"
                assert body["relatedProductIds"] == '["9TESTSTORE0ID","9TESTADDON00"]'
                assert body["enforceSellableBy"] is True
                custom = body["customDeveloperString"]
            except (ValueError, KeyError, AssertionError):
                problems.append("request did not match signed fixture input")
                self.send_error(400)
                return
            seen.append(custom)
            status = 200
            payload = json.dumps({"licenseToken": TOKEN}).encode()
            if custom == "slow":
                time.sleep(0.5)
            elif custom == "timeout":
                time.sleep(2)
            elif custom == "refused":
                status = 503
            elif custom == "empty":
                payload = b'{"licenseToken":""}'
            elif custom == "null":
                payload = b'{"licenseToken":null,"error":"secret"}'
            elif custom == "nested":
                payload = b'{"error":{"licenseToken":"a.b.c"}}'
            elif custom == "malformed":
                payload = b'not-json "licenseToken":"a.b.c"'
            elif custom == "truncated":
                payload = b'{"licenseToken":"SYNTHETIC_SECRET_DO_NOT_LOG'
            elif custom == "oversize":
                payload = b'a' * 65537
            elif custom.startswith("redirect-"):
                status = int(custom.split("-")[1])
                payload = b''
            self.send_response(status)
            if custom.startswith("redirect-"):
                self.send_header("Location", f"{scheme}://localhost:{httpd.server_port}/sink")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            try:
                self.wfile.write(payload)
            except (BrokenPipeError, ConnectionResetError):
                pass  # a timed-out request is expected to close the connection

        def log_message(self, *_):
            pass

    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    scheme = "https" if os.environ.get("LICENSE_FIXTURE_TLS") == "1" else "http"
    if scheme == "https":
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                        "-subj", "/CN=localhost", "-addext", "subjectAltName=DNS:localhost",
                        "-addext", "basicConstraints=critical,CA:FALSE",
                        "-keyout", str(root / "key.pem"), "-out", str(root / "cert.pem")],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(root / "cert.pem", root / "key.pem")
        httpd.socket = context.wrap_socket(httpd.socket, server_side=True)
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    env = dict(os.environ, XODUS_LICENSE_FIXTURE_DIR=str(root),
               XODUS_LICENSE_FIXTURE_URL=f"{scheme}://localhost:{httpd.server_port}/v8.0/licenseToken")
    if scheme == "https":
        env["XODUS_LICENSE_FIXTURE_CA"] = str(root / "cert.pem")
    signal.signal(signal.SIGTERM, lambda *_: stop.set())
    signal.signal(signal.SIGINT, lambda *_: stop.set())
    with (root / "service.log").open("w") as log:
        service = subprocess.Popen([executable, "connection::license::tests::offline_service_fixture",
                                    "--ignored", "--exact", "--nocapture"], env=env, stdout=log, stderr=log)
        try:
            for _ in range(100):
                if (root / "ready").exists():
                    break
                if service.poll() is not None:
                    raise RuntimeError("service fixture exited at startup")
                time.sleep(0.05)
            else:
                raise RuntimeError("service fixture did not start")
            while not stop.wait(0.2):
                if service.poll() is not None:
                    problems.append("service fixture exited during the test")
                    break
        finally:
            service.terminate()
            service.wait(timeout=10)
            httpd.shutdown()
    if "SYNTHETIC_SECRET_DO_NOT_LOG" in (root / "service.log").read_text():
        problems.append("response secret appeared in service output")
    if not seen:
        problems.append("no licensing HTTP requests reached the fixture")
    Path(transcript).write_text(json.dumps({"requests": seen, "problems": problems}, indent=2))
    return bool(problems)


if __name__ == "__main__":
    sys.exit(main())
