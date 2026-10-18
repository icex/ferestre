#!/usr/bin/env python3
"""Probe xodus-service's MSA token endpoint from the Linux side.

Proves the sign-in path works with the user's own credentials before any Wine
plumbing exists. Deliberately never prints token material -- only whether one
came back, how long it is, and when it expires.
"""
import argparse, os, socket, struct, sys, datetime
from xml.sax.saxutils import escape

XML_MAGIC = 0x58445358
MSA_TOKEN_REQUEST = 3


def socket_path(override=None):
    """The service socket, found the way the launcher and the DLL find it:
    XODUS_RUNTIME_DIR first, then the session's runtime directory."""
    if override:
        return override
    runtime_dir = (os.environ.get("XODUS_RUNTIME_DIR") or os.environ.get("XDG_RUNTIME_DIR")
                   or f"/run/user/{os.getuid()}")
    return os.path.join(runtime_dir, "xodus.sock")


parser = argparse.ArgumentParser(description=__doc__)
# Minecraft's MSA app id, straight from its MicrosoftGame.Config.
parser.add_argument("client_id", nargs="?", default="0000000040159362", help="MSA app id to ask for")
parser.add_argument("--socket", help="service socket to ask (default: $XODUS_RUNTIME_DIR, else $XDG_RUNTIME_DIR, /xodus.sock)")
args = parser.parse_args()
sock_path = socket_path(args.socket)
client_id = args.client_id
full_trust = "true"

payload = (
    "<MSATokenRequest>"
    f"<ClientId>{escape(client_id)}</ClientId>"
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
