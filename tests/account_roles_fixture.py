#!/usr/bin/env python3
"""Bounded synthetic role/licence/token IPC server; never reads real credentials.

The control file (JSON) sets the account state and, optionally, how the
service misbehaves:
  store, player, same, licensed  the account roles and the Store entitlement
  broken     answer role requests in an unsupported protocol version
  fault      "" (answer), "drop" (close without answering), "stall" (answer
             nothing for stall_s seconds, then close) or "hold" (answer nothing
             and wait for the client to hang up; the longest such wait is
             written to hold-max next to the control file)
  down       1 removes the socket, as a service that is not running
  xsts       token requests: "ok", "drop" or "stall"
Token requests are counted in xsts-requests next to the control file. A token
asked for with <ForceRefresh>true</ForceRefresh> says "forced", any other
"cached", and every token carries a marker that must never reach a log.
"""
import json
import os
from pathlib import Path
import socket
import struct
import sys
import threading
import time

root = Path(sys.argv[1])
control = Path(sys.argv[2])
root.mkdir(parents=True, exist_ok=True)
lock = threading.Lock()
counters = {"xsts": 0, "hold": 0.0}
TOKEN_SECRET = "SYNTHETIC_TOKEN_DO_NOT_LOG"


def state():
    defaults = {"broken": 0, "fault": "", "stall_s": 8, "down": 0, "xsts": "ok"}
    for _ in range(20):
        try:
            return {**defaults, **json.loads(control.read_text())}
        except (OSError, ValueError):
            time.sleep(0.01)
    return {**defaults, "store": 1, "player": 1, "same": 0, "licensed": 1}


def read_exact(connection, size):
    data = b""
    while len(data) < size:
        chunk = connection.recv(size - len(data))
        if not chunk:
            raise EOFError()
        data += chunk
    return data


def misbehave(connection, current):
    """True when the request was swallowed instead of answered."""
    fault = current["fault"]
    if fault == "drop":
        return True
    if fault == "stall":
        time.sleep(current["stall_s"])
        return True
    if fault == "hold":
        start = time.monotonic()
        connection.settimeout(60)
        try:
            while connection.recv(1):
                pass
        except OSError:
            pass
        held = time.monotonic() - start
        with lock:
            counters["hold"] = max(counters["hold"], held)
            (control.parent / "hold-max").write_text("%.2f" % counters["hold"])
        return True
    return False


def xsts(connection, kind, length):
    if kind & 0x8000:
        length, = struct.unpack("<I", read_exact(connection, 4))
    request = read_exact(connection, length).decode()
    with lock:
        counters["xsts"] += 1
        (control.parent / "xsts-requests").write_text(str(counters["xsts"]))
    current = state()
    if current["xsts"] == "drop":
        return
    if current["xsts"] == "stall":
        time.sleep(3)
        return
    forced = "<ForceRefresh>true</ForceRefresh>" in request
    # Large enough that a leaked copy per request shows in the process.
    token = "XBL3.0 x=1;%s;%s;%s" % ("forced" if forced else "cached", TOKEN_SECRET, "t" * 3000)
    body = ("<XSTSTokenResponse><Token>%s</Token><Xuid>2533274800000001</Xuid>"
            "<Gamertag>Fixture</Gamertag><Signature>fixture-signature</Signature>"
            "<Expiry>%d</Expiry></XSTSTokenResponse>" % (token, int(time.time()) + 3600)).encode()
    connection.sendall(struct.pack("<IHH", 0x58445358, 6, len(body)) + body)


def serve(connection):
    with connection:
        connection.settimeout(5)
        try:
            magic, version = struct.unpack("<II", read_exact(connection, 8))
            if magic == 0x58445358:
                xsts(connection, version & 0xffff, version >> 16)
                return
            assert version == 1
            current = state()
            reply = 99 if current["broken"] else 1
            if magic == 0x58445341:
                if misbehave(connection, current):
                    return
                connection.sendall(struct.pack("<IIII", reply, current["store"], current["player"], current["same"]))
            elif magic == 0x58445345:
                size, = struct.unpack("<I", read_exact(connection, 4))
                assert size == 12
                assert read_exact(connection, size) == b"9TESTROLE0ID"
                if misbehave(connection, current):
                    return
                time.sleep(0.4)
                connection.sendall(struct.pack("<II", reply, int(bool(current["store"] and current["licensed"]))))
            else:
                raise AssertionError("unexpected protocol")
        except (EOFError, OSError, AssertionError):
            pass


path = root / "xodus.sock"
listener = None
while True:
    down = state()["down"]
    if down and listener:
        listener.close()
        listener = None
        os.unlink(path)
    elif not down and not listener:
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(str(path))
        listener.listen()
        listener.settimeout(0.05)
    if not listener:
        time.sleep(0.05)
        continue
    try:
        connection, _ = listener.accept()
    except socket.timeout:
        continue
    connection.setblocking(True)
    threading.Thread(target=serve, args=(connection,), daemon=True).start()
