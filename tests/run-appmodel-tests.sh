#!/bin/bash
# Regression tests for kernelbase/package.c (current-process package identity,
# the APPMODEL_ERROR_NO_PACKAGE fix for UWP/GDK apps).
#
#   tests/run-appmodel-tests.sh              # test the currently built kernelbase
#   REBUILD=1 tests/run-appmodel-tests.sh    # rebuild kernelbase.dll first
#
# Same staging trick as run-tests.sh: Wine loads a core builtin from its own
# tree, so we hardlink-copy the installed Proton and swap our fresh
# kernelbase.dll into it.

set -uo pipefail

REPO_DIR=${REPO_DIR:-$(cd "$(dirname "$0")/.." && pwd)}
. "$REPO_DIR/scripts/xodus-env.sh"
BUILD_DIR=${BUILD_DIR:-$XODUS_BUILD_DIR}
WINE_SRC=${WINE_SRC:-$XODUS_SRC_DIR/xodus-proton/wine}
TOOL_DIR=${TOOL_DIR:-${XODUS_STEAM_DIR:-$HOME/.steam/steam}/compatibilitytools.d/xodus}
OBJ=$BUILD_DIR/obj-wine-x86_64
INCUBE=$REPO_DIR/tools/in-container.sh
WORK=$(mktemp -d "$BUILD_DIR/.appmodel-tests.XXXXXX")

cleanup() {
    [ -n "${WINE:-}" ] && WINEPREFIX="$WORK/prefix" "$WINE"server -k 2>/dev/null
    rm -rf "$WORK" 2>/dev/null
}
trap cleanup EXIT

say() { printf '\033[1m:: %s\033[0m\n' "$*"; }
fail() { printf '\033[31m!! %s\033[0m\n' "$*" >&2; exit 1; }

DLL=$OBJ/dlls/kernelbase/x86_64-windows/kernelbase.dll

if [ "${REBUILD:-0}" != "0" ]; then
    say "rebuilding kernelbase.dll"
    rsync -a --exclude .git "$WINE_SRC/dlls/kernelbase/" "$BUILD_DIR/src-wine/dlls/kernelbase/"
    echo "WORKDIR=$OBJ $INCUBE make -j$(nproc) dlls/kernelbase/x86_64-windows/kernelbase.dll" \
        | newgrp docker || fail "build failed"
fi
[ -f "$DLL" ] || fail "$DLL not found (run with REBUILD=1)"

say "compiling the test binary"
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -o "$WORK/appmodel_tests.exe" \
    "$REPO_DIR/tests/appmodel_tests.c" 2>&1 | grep -viE 'note:' || true
[ -f "$WORK/appmodel_tests.exe" ] || fail "test binary did not build"

# Manifest fixture lives in its own dir so the "bare" run (exe dir has no
# manifest) is a genuine unpackaged process.
mkdir -p "$WORK/pkg"
cp "$REPO_DIR/tests/fixtures/AppxManifest.xml" "$WORK/pkg/"

# Compute the identity strings the DLL should derive, straight from a manifest.
# An XML parser rather than a pattern: it decodes escapes in attribute values
# and tells <Application> from <Applications>, the two things a hand-rolled
# reading gets wrong, so the expectation cannot share the DLL's mistakes.
expected_identity() {
    python3 - "$1" <<'PY'
import hashlib, sys
import xml.etree.ElementTree as ET
root = ET.parse(sys.argv[1]).getroot()
def first(tag):
    return next((e for e in root.iter() if e.tag.rsplit('}', 1)[-1] == tag), None)
ident = first("Identity")
name = ident.get("Name"); pub = ident.get("Publisher")
ver  = ident.get("Version") or "1.0.0.0"
arch = ident.get("ProcessorArchitecture") or "x64"
appel = first("Application")
app = appel.get("Id") if appel is not None and appel.get("Id") else "App"
h = hashlib.sha256(pub.encode('utf-16-le')).digest()[:8]
alpha = "0123456789abcdefghjkmnpqrstvwxyz"
v = int.from_bytes(h, 'big')
pubid = ''.join(alpha[(v >> (59 - 5*i)) & 31] for i in range(12)) + alpha[(v & 15) << 1]
family = f"{name}_{pubid}"
full = f"{name}_{ver}_{arch}__{pubid}"
aumid = f"{family}!{app}"
print(family, full, aumid)
PY
}
read -r EXPECT_FAMILY EXPECT_FULL EXPECT_AUMID < <(expected_identity "$WORK/pkg/AppxManifest.xml")
say "expected family: $EXPECT_FAMILY"
say "expected full:   $EXPECT_FULL"

[ -x "$TOOL_DIR/files/bin/wine" ] || fail "no installed Proton wine at $TOOL_DIR"
say "staging a wine copy with the freshly built kernelbase.dll"
cp -al "$TOOL_DIR/files" "$WORK/wine" || fail "could not hardlink-copy the wine tree"
BUILTIN=$WORK/wine/lib/wine/x86_64-windows/kernelbase.dll
rm -f "$BUILTIN"
cp "$DLL" "$BUILTIN"
WINE=$WORK/wine/bin/wine

export WINEPREFIX="$WORK/prefix"
export WINEDEBUG=-all,fixme-all
# Nothing inherited from the calling shell may change what is tested or reach
# the real service: a manifest would make the bare run a packaged one, and a
# terminal opened from a launched title carries the service socket and the
# profile variables. The service directory is an empty one of our own.
unset WINE_PACKAGE_MANIFEST WINEDLLOVERRIDES XODUS_STORE_PROFILE XODUS_PLAYER_PROFILE \
    XODUS_ACCOUNT_PROFILE FERESTRE_ACCOUNT_ROLES SteamGameId SteamDeck
mkdir -p "$WORK/rt"
export XODUS_RUNTIME_DIR="$WORK/rt"
DOS_MANIFEST="Z:$(echo "$WORK/pkg/AppxManifest.xml" | sed 's#/#\\#g')"

run() { ( cd "$WORK" && timeout 120 "$WINE" "$@" ) 2>/dev/null | grep -avE '^ntsync|radv is not|^wine:|wineserver'; }

say "running packaged-process tests"
WINE_PACKAGE_MANIFEST="$DOS_MANIFEST" \
EXPECT_FAMILY="$EXPECT_FAMILY" EXPECT_FULL="$EXPECT_FULL" EXPECT_AUMID="$EXPECT_AUMID" \
    run "$WORK/appmodel_tests.exe" packaged | tee "$WORK/packaged.txt"
pk=${PIPESTATUS[0]}

echo
say "running unpackaged-process tests"
run "$WORK/appmodel_tests.exe" bare | tee "$WORK/bare.txt"
ba=${PIPESTATUS[0]}

echo
# A manifest whose Publisher is written with XML escapes, under a directory
# whose DOS path is longer than MAX_PATH: the family name has to come from the
# decoded publisher, and an overlong WINE_PACKAGE_MANIFEST has to be read, not
# left in an uninitialised buffer.
LONG_DIR=$WORK/pkg-long
for i in 1 2 3 4 5 6 7 8 9; do LONG_DIR=$LONG_DIR/a-directory-name-that-is-long-$i; done
mkdir -p "$LONG_DIR"
cp "$REPO_DIR/tests/fixtures/AppxManifest-escaped.xml" "$LONG_DIR/AppxManifest.xml"
LONG_DOS_DIR="Z:$(echo "$LONG_DIR" | sed 's#/#\\#g')"
[ ${#LONG_DOS_DIR} -gt 280 ] || fail "the long path is only ${#LONG_DOS_DIR} characters"
read -r ESC_FAMILY ESC_FULL ESC_AUMID < <(expected_identity "$LONG_DIR/AppxManifest.xml")
say "running escaped-publisher tests from a ${#LONG_DOS_DIR}-character path (family $ESC_FAMILY)"
WINE_PACKAGE_MANIFEST="$LONG_DOS_DIR\\AppxManifest.xml" EXPECT_PATH="$LONG_DOS_DIR" \
EXPECT_FAMILY="$ESC_FAMILY" EXPECT_FULL="$ESC_FULL" EXPECT_AUMID="$ESC_AUMID" \
    run "$WORK/appmodel_tests.exe" packaged-other | tee "$WORK/escaped.txt"
es=${PIPESTATUS[0]}

echo
if grep -q "^PASSED" "$WORK/packaged.txt" && grep -q "^PASSED" "$WORK/bare.txt" &&
   grep -q "^PASSED" "$WORK/escaped.txt"; then
    printf '\033[32m==> ALL APPMODEL TESTS PASSED\033[0m\n'
    exit 0
fi
printf '\033[31m==> APPMODEL TESTS FAILED (packaged rc=%s bare rc=%s escaped rc=%s)\033[0m\n' "$pk" "$ba" "$es"
exit 1
