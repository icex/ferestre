#!/usr/bin/env python3
"""Probe title authentication without printing credentials or account identifiers.

Examples:
    xsts_probe.py --game-dir /path/to/game --check-endpoints
    xsts_probe.py 'wss://service.example.test/connect' --game-dir /path/to/game
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import socket
import struct
import sys
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET


XML_MAGIC = 0x58445358
XSTS_TOKEN_REQUEST = 5
TITLE_ENDPOINTS_URL = "https://title.mgt.xboxlive.com/titles/current/endpoints"


def socket_path():
    runtime_dir = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
    return os.path.join(runtime_dir, "xodus.sock")


def read_title_metadata(game_dir):
    candidates = [
        path for path in game_dir.iterdir()
        if path.name.casefold() == "microsoftgame.config" and path.is_file()
    ]
    if len(candidates) != 1:
        raise ValueError("game directory must contain one MicrosoftGame.config")
    root = ET.parse(candidates[0]).getroot()
    values = {
        element.tag.rsplit("}", 1)[-1].casefold(): (element.text or "").strip()
        for element in root.iter()
    }
    try:
        title_id = int(values.get("titleid", ""), 16)
    except ValueError:
        raise ValueError("TitleId must be a hexadecimal integer") from None
    if not 1 <= title_id <= 0xFFFFFFFF:
        raise ValueError("TitleId must be in 1..=4294967295")
    return title_id, values.get("msaappid", "")


def receive_exact(connection, size):
    result = bytearray()
    while len(result) < size:
        part = connection.recv(size - len(result))
        if not part:
            raise EOFError("incomplete service response")
        result.extend(part)
    return bytes(result)


def request_token(url, title=None):
    request = ET.Element("XSTSTokenRequest")
    for name, value in [
        ("Url", url),
        ("RelyingParty", ""),
        ("Method", "GET"),
        ("BodyBase64", ""),
        ("TitleId", str(title[0]) if title else ""),
        ("TitleClientId", title[1] if title else ""),
    ]:
        ET.SubElement(request, name).text = value
    payload = ET.tostring(request, encoding="utf-8")
    if len(payload) > 0xFFFF:
        raise ValueError("request exceeds the service message size limit")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(120)
        connection.connect(socket_path())
        connection.sendall(struct.pack("<IHH", XML_MAGIC, XSTS_TOKEN_REQUEST, len(payload)) + payload)
        magic, message_type, size = struct.unpack("<IHH", receive_exact(connection, 8))
        if magic != XML_MAGIC or message_type != XSTS_TOKEN_REQUEST + 1:
            raise ValueError("unexpected service response header")
        if not size:
            raise ValueError("service returned an empty response")
        return ET.fromstring(receive_exact(connection, size)), size


def usable_token(response):
    return (response.findtext("Token") or "").startswith("XBL3.0 x=")


def print_summary(response, size, title):
    if title:
        print(f"TitleId: 0x{title[0]:08X} ({title[0]})")
        print(f"MSAAppId: {title[1] or '(absent)'}")
    else:
        print("Title metadata: absent")
    print(f"Response size: {size} bytes")
    token = response.findtext("Token") or ""
    signature = response.findtext("Signature") or ""
    print(f"Token present: {bool(token)}; usable XBL header: {usable_token(response)}; size: {len(token)} chars")
    print(f"Signature present: {bool(signature)}; size: {len(signature)} chars")
    print(f"XUID present: {bool(response.findtext('Xuid'))}")
    print(f"Gamertag present: {bool(response.findtext('Gamertag'))}")
    try:
        expiry = datetime.datetime.fromtimestamp(
            int(response.findtext("Expiry") or ""), datetime.timezone.utc
        ).isoformat(timespec="seconds")
    except (ValueError, OverflowError, OSError):
        expiry = "unavailable"
    print(f"Expires: {expiry}")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, file_pointer, code, message, headers, new_url):
        return None


def check_endpoints(response):
    signature = response.findtext("Signature") or ""
    if not usable_token(response) or not signature:
        print("Endpoint check: usable token and signature required")
        return False
    request = urllib.request.Request(
        TITLE_ENDPOINTS_URL,
        method="GET",
        headers={
            "Authorization": response.findtext("Token"),
            "Signature": signature,
            "x-xbl-contract-version": "1",
        },
    )
    try:
        # Credentials are for this exact request and must not follow redirects.
        with urllib.request.build_opener(NoRedirect()).open(request, timeout=30) as result:
            print(f"Endpoint check: HTTP {result.status}")
            if result.status != 200:
                return False
            body = result.read(2 * 1024 * 1024 + 1)
            if len(body) > 2 * 1024 * 1024:
                raise ValueError("endpoint response is too large")
            endpoints = json.loads(body).get("EndPoints")
            if not isinstance(endpoints, list):
                raise ValueError("missing endpoint list")
            print(f"Endpoint count: {len(endpoints)}")
            return True
    except urllib.error.HTTPError as error:
        print(f"Endpoint check: HTTP {error.code}")
        error.close()
    except (OSError, ValueError, AttributeError):
        print("Endpoint check: request or response failed")
    return False


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("url", nargs="?", default=TITLE_ENDPOINTS_URL, help="URL to request authentication for")
    parser.add_argument("--game-dir", type=Path, help="installed game directory containing MicrosoftGame.config")
    parser.add_argument("--check-endpoints", action="store_true", help="perform a GET only to the fixed Microsoft title-management URL")
    args = parser.parse_args(argv)
    if args.check_endpoints and args.url != TITLE_ENDPOINTS_URL:
        parser.error("--check-endpoints requires the default title-management URL")
    try:
        title = read_title_metadata(args.game_dir) if args.game_dir else None
    except ValueError as error:
        parser.error(str(error))
    except (OSError, ET.ParseError):
        parser.error("could not read game metadata")
    try:
        response, size = request_token(args.url, title)
    except (OSError, EOFError, ValueError, ET.ParseError) as error:
        print(f"Token request failed ({type(error).__name__}); inspect the service log", file=sys.stderr)
        return 1
    print_summary(response, size, title)
    if not usable_token(response):
        return 1
    if args.check_endpoints and not check_endpoints(response):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
