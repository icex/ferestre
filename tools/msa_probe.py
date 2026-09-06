#!/usr/bin/env python3
"""Probe xodus-service's MSA token endpoint from the Linux side.

Proves the sign-in path works with the user's own credentials before any Wine
plumbing exists. Deliberately never prints token material -- only whether one
came back, how long it is, and when it expires.
"""
import os, socket, struct, sys, datetime

XML_MAGIC = 0x58445358
MSA_TOKEN_REQUEST = 3

sock_path = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/run/user/1000"), "xodus.sock")

# Minecraft's MSA app id and full-trust flag, straight from its MicrosoftGame.Config.
client_id = sys.argv[1] if len(sys.argv) > 1 else "0000000040159362"
full_trust = "true"

payload = (
    "<MSATokenRequest>"
    f"<ClientId>{client_id}</ClientId>"
    "<AllowUi>false</AllowUi>"
    f"<MSAFullTrust>{full_trust}</MSAFullTrust>"
    "</MSATokenRequest>"
).encode()

msg = struct.pack("<IHH", XML_MAGIC, MSA_TOKEN_REQUEST, len(payload)) + payload

s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(60)
s.connect(sock_path)
print(f":: connected to {sock_path}")
print(f":: MSA_TOKEN_REQUEST client_id={client_id} full_trust={full_trust} ({len(payload)} byte body)")
s.sendall(msg)


def recv_exact(n):
    buf = b""
    while len(buf) < n:
        chunk = s.recv(n - len(buf))
        if not chunk:
            raise EOFError(f"socket closed after {len(buf)}/{n} bytes")
        buf += chunk
    return buf


hdr = recv_exact(8)
magic, mtype, size = struct.unpack("<IHH", hdr)
print(f":: reply magic=0x{magic:08x} type={mtype} size={size}")
body = recv_exact(size) if size else b""

if not size:
    print("!! empty body -- the service refused or failed the request")
    print("   (check the service log; usually means no signed-in user token)")
    sys.exit(1)

text = body.decode(errors="replace")
import re


def field(name):
    m = re.search(rf"<{name}>(.*?)</{name}>", text, re.S)
    return m.group(1) if m else None


token = field("Token")
expiry = field("Expiry")
device_rps = field("DeviceRps")
device_expiry = field("DeviceExpiry")


def when(ts):
    try:
        return datetime.datetime.fromtimestamp(int(ts)).isoformat(sep=" ", timespec="seconds")
    except Exception:
        return ts


print()
print(f"   Token        : {'PRESENT, %d chars' % len(token) if token else 'MISSING'}")
print(f"   Expiry       : {when(expiry) if expiry else 'MISSING'}")
print(f"   DeviceRps    : {'PRESENT, %d chars' % len(device_rps) if device_rps else 'absent'}")
print(f"   DeviceExpiry : {when(device_expiry) if device_expiry else 'absent'}")
print()
print("==> MSA TOKEN OBTAINED" if token else "==> NO TOKEN IN RESPONSE")
sys.exit(0 if token else 1)
