#!/bin/bash
# Replace a GDK title's XCurl.dll with a real Windows libcurl.
#
#   scripts/fix-xcurl.sh [game dir]        # default: the Bedrock install
#   scripts/fix-xcurl.sh --restore [dir]   # put Microsoft's back
#   LIBCURL=<libcurl-x64.dll> scripts/fix-xcurl.sh [dir]   # a build you fetched
#
# Safe to run again at any time, and meant to be run after every update: it
# substitutes whenever the DLL in place is Microsoft's, and leaves it alone
# when it already is not.
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

# The curl build to install, pinned by version and checksum: this DLL runs
# inside the title with the user's files reachable through Wine's Z: drive, so
# it is not taken on trust from whatever "latest" happens to be today. To move
# to a newer build, take the version and the sha256 from https://curl.se/windows/
# (each download has a .txt next to it with the hash), check that the title
# still starts and signs in, and change both lines together. curl.se prunes old
# builds after a few months; when the pinned one has gone, the download fails
# with a 404 and the message below says what to do.
CURL_WIN_VERSION=8.22.0_1
CURL_WIN_SHA256=7f23b039f6ea4197362d4468e1a0e71428201222e1bef3b680d5ef7b2aefb714
CURL_WIN_URL=https://curl.se/windows/dl-$CURL_WIN_VERSION/curl-$CURL_WIN_VERSION-win64-mingw.zip

RESTORE=0
if [ "${1:-}" = "--restore" ]; then RESTORE=1; shift; fi
GAME_DIR=${1:-${BEDROCK_DIR:-$XODUS_GAMES_DIR/bedrock/game}}
DLL=$GAME_DIR/XCurl.dll
BACKUP=$GAME_DIR/XCurl.dll.microsoft

[ -d "$GAME_DIR" ] || fail "no game directory at $GAME_DIR"

# Reads PE export and import tables. `classify` says what a DLL is: Microsoft's
# XCurl (it carries the xcurl_* extensions), something else that parses as a
# whole PE file (a substituted curl), or broken (missing, truncated or not a PE,
# as a copy cut short by a full disk would leave it). `check` makes sure the
# replacement exports everything the title's modules import from XCurl.
PE_PY=$(cat <<'PY'
import struct, sys, glob, os

def _pe(path):
    d = open(path, "rb").read()
    pe = struct.unpack_from("<I", d, 0x3c)[0]
    if d[:2] != b"MZ" or d[pe:pe+4] != b"PE\0\0":
        raise ValueError("not a PE image")
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
    return d, magic, ddir, off, cstr, secs

def exports(path):
    d, magic, ddir, off, cstr, _ = _pe(path)
    erva, _ = struct.unpack_from("<II", d, ddir)
    if not erva:
        return set()
    eo = off(erva)
    n, = struct.unpack_from("<I", d, eo+24)
    nr, = struct.unpack_from("<I", d, eo+32)
    no = off(nr)
    return {cstr(off(struct.unpack_from("<I", d, no+4*i)[0])) for i in range(n)}

def imports_from(path, want):
    d, magic, ddir, off, cstr, _ = _pe(path)
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

def classify(path):
    try:
        d, *_, secs = _pe(path)
        # Every section's raw data has to be in the file, or it was cut short.
        if any(rp + rs > len(d) for _, _, rp, rs in secs):
            return "broken"
        names = exports(path)
    except Exception:
        return "broken"
    return "microsoft" if any(n.startswith("xcurl_") for n in names) else "substituted"

if sys.argv[1] == "classify":
    print(classify(sys.argv[2]))
    sys.exit(0)

xcurl, libcurl, game_dir = sys.argv[2], sys.argv[3], sys.argv[4]
if classify(libcurl) != "substituted":
    print("   the replacement is not a complete libcurl build")
    sys.exit(1)
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
)
pe() { python3 -I -c "$PE_PY" "$@"; }

# Put a copy of $1 at $2 without ever leaving $2 half written: copy next to it,
# compare, then rename over it. Keeps the mode of the file it replaces.
put() {
    local src=$1 dst=$2 tmp
    tmp=$(mktemp "$(dirname "$dst")/.$(basename "$dst").XXXXXX") || return 1
    if cp "$src" "$tmp" && cmp -s "$src" "$tmp" \
        && if [ -e "$dst" ]; then chmod --reference="$dst" "$tmp"; else chmod '=rw' "$tmp"; fi \
        && mv -f "$tmp" "$dst"; then
        return 0
    fi
    rm -f "$tmp"
    return 1
}

if [ "$RESTORE" = 1 ]; then
    [ -f "$BACKUP" ] || fail "no backup at $BACKUP"
    [ "$(pe classify "$BACKUP")" = microsoft ] \
        || fail "$(basename "$BACKUP") is not a complete Microsoft XCurl.dll; reinstall or repair the title instead"
    put "$BACKUP" "$DLL" || fail "could not put Microsoft's XCurl.dll back in $GAME_DIR"
    say "restored Microsoft's XCurl.dll"
    exit 0
fi

[ -f "$DLL" ] || fail "no XCurl.dll in $GAME_DIR (this title may not use it)"

# Decide by what the DLL is, not by whether a backup exists: an update brings
# back a new Microsoft build next to the backup of the old one, and that has
# to be substituted again (and become the backup), not taken for done.
case $(pe classify "$DLL") in
    substituted)
        say "XCurl.dll is already substituted (original kept at $(basename "$BACKUP"))"
        exit 0
        ;;
    microsoft)
        REFRESH_BACKUP=1
        ;;
    *)
        # Left half written by an earlier run. Repairable only if the backup
        # really is Microsoft's: it is then kept as it is.
        [ "$(pe classify "$BACKUP" 2>/dev/null)" = microsoft ] \
            || fail "XCurl.dll in $GAME_DIR is damaged and there is no Microsoft backup to repair it from; repair the title"
        warn "XCurl.dll is damaged; substituting it again"
        REFRESH_BACKUP=0
        ;;
esac

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

if [ -n "${LIBCURL:-}" ]; then
    # A build fetched by hand: no download at all, so this works offline.
    [ -f "$LIBCURL" ] || fail "LIBCURL=$LIBCURL is not a file"
    say "using $LIBCURL"
    CABUNDLE=$(dirname "$LIBCURL")/curl-ca-bundle.crt
    [ -f "$CABUNDLE" ] || CABUNDLE=
else
    say "fetching curl $CURL_WIN_VERSION for Windows"
    if ! curl -fsSL -o "$WORK/curl.zip" "$CURL_WIN_URL"; then
        fail "download of $CURL_WIN_URL failed; fetch a win64 mingw build from https://curl.se/windows/ by hand, unpack it, and pass it with LIBCURL=<path to bin/libcurl-x64.dll>"
    fi
    got=$(sha256sum "$WORK/curl.zip" | cut -d' ' -f1)
    [ "$got" = "$CURL_WIN_SHA256" ] \
        || fail "the curl archive does not match its pinned checksum (got $got); not installing it"
    unzip -o -q "$WORK/curl.zip" -d "$WORK" || fail "could not unpack the archive"
    LIBCURL=$(find "$WORK" -name 'libcurl-x64.dll' | head -1)
    [ -f "$LIBCURL" ] || fail "no libcurl-x64.dll in the archive"
    CABUNDLE=$(find "$WORK" -name 'curl-ca-bundle.crt' | head -1)
fi

say "checking that it exports everything the title imports"
pe check "$DLL" "$LIBCURL" "$GAME_DIR" || fail "export check failed; not swapping"

say "substituting (Microsoft's build kept as XCurl.dll.microsoft)"
if [ "$REFRESH_BACKUP" = 1 ]; then
    put "$DLL" "$BACKUP" || fail "could not save Microsoft's XCurl.dll as $BACKUP; nothing was changed"
fi
put "$LIBCURL" "$DLL" || fail "could not install libcurl as $DLL; Microsoft's build is still in place"
if [ -n "$CABUNDLE" ]; then
    put "$CABUNDLE" "$GAME_DIR/curl-ca-bundle.crt" || fail "could not install curl-ca-bundle.crt in $GAME_DIR"
fi

say "done -- $(basename "$GAME_DIR") should now reach the network"
echo "   undo with: $0 --restore \"$GAME_DIR\""
