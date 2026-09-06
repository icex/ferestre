#!/usr/bin/env python3
"""Exercise the endpoints the title uses, with tokens from xodus-service.

Checks the whole chain end to end without starting the game: a token is
minted for each URL, the request is signed where a policy demands it, and the
status is reported. Token material is never printed.
"""
import os, socket, struct, re, json, sys, urllib.request, urllib.error

XML_MAGIC, REQ = 0x58445358, 5
XUID = "2533270000000000"

def xml_escape(t):
    for a, b in (("&", "&amp;"), ("<", "&lt;"), (">", "&gt;"), ('"', "&quot;"), ("'", "&apos;")):
        t = t.replace(a, b)
    return t

def token_for(url, method="GET", body=b""):
    import base64
    b64 = base64.b64encode(body).decode() if body else ""
    url = xml_escape(url)
    p = (f"<XSTSTokenRequest><Url>{url}</Url><RelyingParty></RelyingParty>"
         f"<Method>{method}</Method><BodyBase64>{b64}</BodyBase64></XSTSTokenRequest>").encode()
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.settimeout(90)
    s.connect(os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/run/user/1000"), "xodus.sock"))
    s.sendall(struct.pack("<IHH", XML_MAGIC, REQ, len(p)) + p)
    def rx(n):
        b = b""
        while len(b) < n:
            c = s.recv(n - len(b))
            if not c: raise EOFError("service closed")
            b += c
        return b
    _, _, size = struct.unpack("<IHH", rx(8))
    if not size: return "", ""
    body_s = rx(size).decode("utf-8", "replace")
    g = lambda t: (re.search(f"<{t}>(.*?)</{t}>", body_s, re.S) or [None, ""])[1]
    return g("Token"), g("Signature")

CASES = [
    ("profile",      "POST", "https://profile.xboxlive.com/users/batch/profile/settings",
     json.dumps({"userIds":[XUID],"settings":["Gamertag"]}).encode(), {"x-xbl-contract-version":"3"}),
    ("achievements", "GET",  f"https://achievements.xboxlive.com/users/xuid({XUID})/achievements?titleId=896928775&maxItems=1",
     b"", {"x-xbl-contract-version":"2"}),
    ("privacy",      "GET",  f"https://privacy.xboxlive.com/users/xuid({XUID})/people/avoid",
     b"", {"x-xbl-contract-version":"2"}),
    ("peoplehub",    "GET",  f"https://peoplehub.xboxlive.com/users/xuid({XUID})/people/friends",
     b"", {"x-xbl-contract-version":"7","Accept-Language":"en-US"}),
    ("presence get", "GET",  f"https://userpresence.xboxlive.com/users/xuid({XUID})?level=all",
     b"", {"x-xbl-contract-version":"3"}),
    ("presence set", "POST", f"https://userpresence.xboxlive.com/users/xuid({XUID})/devices/current/titles/current",
     json.dumps({"id":"896928775","state":"active"}).encode(), {"x-xbl-contract-version":"3"}),
    ("realms",       "GET",  "https://bedrock.frontendlegacy.realms.minecraft-services.net/worlds",
     b"", {"client-version":"1.26.45","charset":"utf-8"}, "https://pocket.realms.minecraft.net/"),
    ("userstats",    "POST", "https://userstats.xboxlive.com/batch?operation=read",
     json.dumps({"arrangebyfield":"xuid","xuids":[XUID],"stats":[{"name":"MinutesPlayed","titleid":"896928775"}]}).encode(),
     {"x-xbl-contract-version":"2"}),
]

fails = 0
for case in CASES:
    name, method, url, body, headers = case[:5]
    token_url = case[5] if len(case) > 5 else url
    try:
        tok, sig = token_for(token_url, method, body)
    except Exception as e:
        print(f"{name:14} TOKEN ERROR {e}"); fails += 1; continue
    if not tok:
        print(f"{name:14} NO TOKEN"); fails += 1; continue
    req = urllib.request.Request(url, data=body or None, method=method)
    req.add_header("Authorization", tok)
    for k, v in headers.items(): req.add_header(k, v)
    if body: req.add_header("Content-Type", "application/json")
    if sig: req.add_header("Signature", sig)
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            print(f"{name:14} HTTP {r.status}  (token {len(tok)}, sig {len(sig)})")
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", "replace")[:90].replace("\n", " ")
        print(f"{name:14} HTTP {e.code}  {detail}")
        if e.code not in (204, 404): fails += 1
    except Exception as e:
        print(f"{name:14} ERROR {e}"); fails += 1
print(f"\n{'ALL ENDPOINTS OK' if not fails else str(fails) + ' FAILING'}")
sys.exit(1 if fails else 0)
