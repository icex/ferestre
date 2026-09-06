#!/bin/bash
# Replace a GDK title's XCurl.dll with a real Windows libcurl.
#
#   scripts/fix-xcurl.sh [game dir]        # default: the Bedrock install
#   scripts/fix-xcurl.sh --restore [dir]   # put Microsoft's back
#
# Why this is needed
# ------------------
# XCurl.dll is Microsoft's HTTP transport for the GDK: a shim that exposes a
# libcurl API on top of WinHTTP. Under Wine it loads but never gets a request
# onto the wire -- a title using it shows zero TCP connections and retries for
# ever, so anything online (sign-in, marketplace, profile) hangs.
#
# It exports 57 ordinary curl_* symbols plus four xcurl_* extensions. What a
# title actually imports is only the standard ones (Minecraft's
# libHttpClient.GDK.dll imports 16, all curl_*), so a genuine libcurl built for
# Windows is a drop-in replacement. This script checks that claim against the
# real files rather than assuming it, and refuses to swap if anything is
# missing.
#
# This is a workaround, not a fix: the real answer is for XCurl's WinHTTP path
# to work under Wine.

set -uo pipefail
. "$(dirname "$0")/xodus-env.sh"

say()  { printf '\033[1m:: %s\033[0m\n' "$*"; }
warn() { printf '\033[33m** %s\033[0m\n' "$*"; }
fail() { printf '\033[31m!! %s\033[0m\n' "$*" >&2; exit 1; }

RESTORE=0
if [ "${1:-}" = "--restore" ]; then RESTORE=1; shift; fi
GAME_DIR=${1:-${BEDROCK_DIR:-$XODUS_GAMES_DIR/bedrock/game}}
DLL=$GAME_DIR/XCurl.dll
BACKUP=$GAME_DIR/XCurl.dll.microsoft

[ -d "$GAME_DIR" ] || fail "no game directory at $GAME_DIR"

if [ "$RESTORE" = 1 ]; then
    [ -f "$BACKUP" ] || fail "no backup at $BACKUP"
    cp "$BACKUP" "$DLL" && say "restored Microsoft's XCurl.dll"
    exit 0
fi

[ -f "$DLL" ] || fail "no XCurl.dll in $GAME_DIR (this title may not use it)"

# Already done? Microsoft's build is ~200 KB and carries the xcurl_* extensions.
if [ -f "$BACKUP" ] && ! cmp -s "$DLL" "$BACKUP"; then
    say "XCurl.dll is already substituted (original kept at $(basename "$BACKUP"))"
    exit 0
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

say "fetching the official curl build for Windows"
if ! curl -fsSL -o "$WORK/curl.zip" "https://curl.se/windows/latest.cgi?p=win64-mingw.zip"; then
    fail "download failed; fetch a win64 mingw build from https://curl.se/windows/ by hand and pass it with LIBCURL=<path to libcurl-x64.dll>"
fi
unzip -o -q "$WORK/curl.zip" -d "$WORK" || fail "could not unpack the archive"

LIBCURL=${LIBCURL:-$(find "$WORK" -name 'libcurl-x64.dll' | head -1)}
[ -f "$LIBCURL" ] || fail "no libcurl-x64.dll in the archive"
CABUNDLE=$(find "$WORK" -name 'curl-ca-bundle.crt' | head -1)

say "checking that it exports everything the title imports"
python3 - "$DLL" "$LIBCURL" "$GAME_DIR" <<'PY' || fail "export check failed; not swapping"
import struct, sys, glob, os

def _pe(path):
    d = open(path, "rb").read()
    pe = struct.unpack_from("<I", d, 0x3c)[0]
    nsec, = struct.unpack_from("<H", d, pe+6)
    optsz, = struct.unpack_from("<H", d, pe+20)
    magic, = struct.unpack_from("<H", d, pe+24)
    opt = pe + 24
    ddir = opt + (112 if magic == 0x20b else 96)
    secs = []
    so = opt + optsz
    for i in range(nsec):
        o = so + i*40
        vsize, vaddr, rawsz, rawptr = struct.unpack_from("<IIII", d, o+8)
        secs.append((vaddr, vsize, rawptr, rawsz))
    def off(rva):
        for va, vs, rp, rs in secs:
            if va <= rva < va + max(vs, rs):
                return rp + (rva - va)
    def cstr(o):
        return d[o:d.index(b"\0", o)].decode(errors="replace")
    return d, magic, ddir, off, cstr

def exports(path):
    d, magic, ddir, off, cstr = _pe(path)
    erva, _ = struct.unpack_from("<II", d, ddir)
    if not erva:
        return set()
    eo = off(erva)
    n, = struct.unpack_from("<I", d, eo+24)
    nr, = struct.unpack_from("<I", d, eo+32)
    no = off(nr)
    return {cstr(off(struct.unpack_from("<I", d, no+4*i)[0])) for i in range(n)}

def imports_from(path, want):
    d, magic, ddir, off, cstr = _pe(path)
    irva, _ = struct.unpack_from("<II", d, ddir+8)
    if not irva:
        return set()
    io = off(irva)
    names = set()
    while True:
        olt, tds, fc, nrva, fta = struct.unpack_from("<IIIII", d, io)
        if not any((olt, tds, fc, nrva, fta)):
            break
        if cstr(off(nrva)).lower() == want:
            t = off(olt or fta)
            while True:
                e, = struct.unpack_from("<Q" if magic == 0x20b else "<I", d, t)
                if not e:
                    break
                if not (e >> (63 if magic == 0x20b else 31)):
                    names.add(cstr(off(e & 0x7fffffff)+2))
                t += 8 if magic == 0x20b else 4
        io += 20
    return names

xcurl, libcurl, game_dir = sys.argv[1], sys.argv[2], sys.argv[3]
have = exports(libcurl)

# What every module in the game folder actually imports from XCurl.dll.
needed = set()
for f in glob.glob(os.path.join(game_dir, "*.dll")) + glob.glob(os.path.join(game_dir, "*.exe")):
    try:
        needed |= imports_from(f, "xcurl.dll")
    except Exception:
        pass

if not needed:
    print("   nothing statically imports XCurl.dll; substituting anyway (it may be loaded dynamically)")
missing = sorted(needed - have)
print(f"   imported from XCurl: {len(needed)}   provided by libcurl: {len(have)}   missing: {len(missing)}")
if missing:
    print("   missing:", ", ".join(missing))
    sys.exit(1)
sys.exit(0)
PY

say "substituting (Microsoft's build kept as XCurl.dll.microsoft)"
cp "$DLL" "$BACKUP"
cp "$LIBCURL" "$DLL"
[ -n "$CABUNDLE" ] && cp "$CABUNDLE" "$GAME_DIR/curl-ca-bundle.crt"

say "done -- $(basename "$GAME_DIR") should now reach the network"
echo "   undo with: $0 --restore \"$GAME_DIR\""
