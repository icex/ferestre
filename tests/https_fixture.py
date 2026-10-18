#!/usr/bin/env python3
"""Loopback HTTP and HTTPS servers for tests/xnetworking_cert.c.

  https_fixture.py <server cert> <server key> <port file>

Serves "ok" over HTTPS (with the given synthetic certificate) and over plain
HTTP, on loopback only: 127.0.0.1 and, when the host has it, ::1 on the same
port, so "localhost" reaches it whichever address Wine tries first. Writes
"<https port> <http port>" to the port file once both are listening.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
import socket
import ssl
import sys
import threading

cert, key, port_file = sys.argv[1:4]


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"ok"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


class Server6(ThreadingHTTPServer):
    address_family = socket.AF_INET6


def serve(server):
    threading.Thread(target=server.serve_forever, daemon=True).start()


context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(cert, key)

https = [ThreadingHTTPServer(("127.0.0.1", 0), Handler)]
https_port = https[0].server_address[1]
try:
    https.append(Server6(("::1", https_port), Handler))
except OSError:
    pass
for server in https:
    server.socket = context.wrap_socket(server.socket, server_side=True)
    serve(server)

plain = [ThreadingHTTPServer(("127.0.0.1", 0), Handler)]
http_port = plain[0].server_address[1]
try:
    plain.append(Server6(("::1", http_port), Handler))
except OSError:
    pass
for server in plain:
    serve(server)

with open(port_file + ".tmp", "w") as f:
    f.write("%d %d\n" % (https_port, http_port))
os.replace(port_file + ".tmp", port_file)
threading.Event().wait()
