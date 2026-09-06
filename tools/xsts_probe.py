#!/usr/bin/env python3
"""Ask xodus-service for an Xbox Live XSTS token for a given URL.

Prints whether a usable XBL3.0 header came back and the identity attached to
it. Never prints token material; the XUID is masked.
"""
import os, socket, struct, sys, re, datetime

XML_MAGIC = 0x58445358
XSTS_TOKEN_REQUEST = 5

url = sys.argv[1] if len(sys.argv) > 1 else "https://b980a380.minecraft.playfabapi.com/"
sock_path = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/run/user/1000"), "xodus.sock")

payload = f"<XSTSTokenRequest><Url>{url}</Url><RelyingParty></RelyingParty></XSTSTokenRequest>".encode()
msg = struct.pack("<IHH", XML_MAGIC, XSTS_TOKEN_REQUEST, len(payload)) + payload

s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(120)
s.connect(sock_path)
print(f":: XSTS_TOKEN_REQUEST url={url}")
s.sendall(msg)


def recv_exact(n):
    buf = b""
    while len(buf) < n:
        c = s.recv(n - len(buf))
        if not c:
            raise EOFError(f"closed after {len(buf)}/{n}")
        buf += c
    return buf


magic, mtype, size = struct.unpack("<IHH", recv_exact(8))
print(f":: reply magic=0x{magic:08x} type={mtype} size={size}")
if not size:
    print("!! empty body -- request failed (see service log)")
    sys.exit(1)

text = recv_exact(size).decode(errors="replace")


def field(n):
    m = re.search(rf"<{n}>(.*?)</{n}>", text, re.S)
    return m.group(1) if m else None


token, xuid, gtg, exp = field("Token"), field("Xuid"), field("Gamertag"), field("Expiry")
mask = (xuid[:4] + "…" + xuid[-3:]) if xuid and len(xuid) > 8 else (xuid or "")
try:
    when = datetime.datetime.fromtimestamp(int(exp)).isoformat(sep=" ", timespec="seconds")
except Exception:
    when = exp

print()
print(f"   Header form : {'XBL3.0 x=<uhs>;<token>' if token and token.startswith('XBL3.0 x=') else repr(token[:20]) if token else 'MISSING'}")
print(f"   Header size : {len(token) if token else 0} chars")
print(f"   Gamertag    : {gtg or '(none)'}")
print(f"   XUID        : {mask or '(none)'}")
print(f"   Expires     : {when}")
print()
ok = bool(token and token.startswith("XBL3.0 x="))
print("==> XSTS TOKEN OBTAINED" if ok else "==> NO USABLE TOKEN")
sys.exit(0 if ok else 1)
