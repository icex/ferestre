#!/usr/bin/env python3
"""Loopback HTTP and HTTPS servers for tests/xnetworking_cert.c.

  https_fixture.py <port file> <cert> <key> [<cert> <key> ...]

Serves "ok" over plain HTTP, and over HTTPS once for each synthetic
certificate given (a certificate file may hold the issuers to send after it),
on loopback only: 127.0.0.1 and, when the host has it, ::1 on the same port, so
"localhost" reaches it whichever address Wine tries first. Writes
"<http port> <https port> ..." to the port file, HTTPS ports in the order of
the certificates, once all are listening.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
import socket
import ssl
import sys
import threading

port_file = sys.argv[1]
pairs = sys.argv[2:]
if not pairs or len(pairs) % 2:
    sys.exit("usage: https_fixture.py <port file> <cert> <key> [<cert> <key> ...]")


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


def listen(context=None):
    servers = [ThreadingHTTPServer(("127.0.0.1", 0), Handler)]
    port = servers[0].server_address[1]
    try:
        servers.append(Server6(("::1", port), Handler))
    except OSError:
        pass
    for server in servers:
        if context:
            server.socket = context.wrap_socket(server.socket, server_side=True)
        serve(server)
    return port


https_ports = []
for cert, key in zip(pairs[0::2], pairs[1::2]):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    https_ports.append(listen(context))
http_port = listen()

with open(port_file + ".tmp", "w") as f:
    f.write(" ".join(str(port) for port in [http_port] + https_ports) + "\n")
os.replace(port_file + ".tmp", port_file)
threading.Event().wait()
