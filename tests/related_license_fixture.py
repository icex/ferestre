#!/usr/bin/env python3
"""Account-free entitlement IPC fixture; no credentials or Store HTTP."""
import pathlib
import socket
import struct
import sys
import threading

root = pathlib.Path(sys.argv[1])
root.mkdir()
server = socket.socket(socket.AF_UNIX)
server.bind(str(root / 'xodus.sock'))
server.listen()
(root / 'ready').touch()


def handle(client):
    def read(size):
        data = b''
        while len(data) < size:
            part = client.recv(size - len(data))
            if not part:
                raise EOFError
            data += part
        return data
    try:
        magic, version = struct.unpack('<II', read(8))
        if magic == 0x58445345 and version == 1:
            size, = struct.unpack('<I', read(4))
            product = read(size).decode('ascii')
            active = product == '9TESTPARENT0' or (product == '9TESTADDON01' and not (root / 'revoked').exists())
            client.sendall(struct.pack('<II', 1, active))
        elif magic == 0x58445341 and version == 1:
            client.sendall(struct.pack('<IIII', 1, 1, 1, 0))
    finally:
        client.close()


while True:
    client, _ = server.accept()
    threading.Thread(target=handle, args=(client,), daemon=True).start()
