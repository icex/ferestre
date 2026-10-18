#!/usr/bin/env python3
"""Bounded synthetic role/licence IPC server; never reads real credentials."""
import json
from pathlib import Path
import socket
import struct
import sys
import time

root = Path(sys.argv[1])
control = Path(sys.argv[2])
root.mkdir(parents=True, exist_ok=True)

def read_exact(connection, size):
    data = b""
    while len(data) < size:
        chunk = connection.recv(size - len(data))
        if not chunk:
            raise EOFError()
        data += chunk
    return data

with socket.socket(socket.AF_UNIX) as listener:
    listener.bind(str(root / "xodus.sock"))
    listener.listen()
    while True:
        connection, _ = listener.accept()
        with connection:
            connection.settimeout(5)
            try:
                magic, version = struct.unpack("<II", read_exact(connection, 8))
                assert version == 1
                state = json.loads(control.read_text())
                version = 99 if state["broken"] else 1
                if magic == 0x58445341:
                    connection.sendall(struct.pack("<IIII", version, state["store"], state["player"], state["same"]))
                elif magic == 0x58445345:
                    size, = struct.unpack("<I", read_exact(connection, 4))
                    assert size == 12
                    assert read_exact(connection, size) == b"9TESTROLE0ID"
                    time.sleep(0.4)
                    connection.sendall(struct.pack("<II", version, int(bool(state["store"] and state["licensed"]))))
                else:
                    raise AssertionError("unexpected protocol")
            except (EOFError, OSError):
                pass
