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
BUILD_DIR=${BUILD_DIR:-${XODUS_SRC_DIR:-$HOME/src}/xodus-build}
WINE_SRC=${WINE_SRC:-${XODUS_SRC_DIR:-$HOME/src}/xodus-proton/wine}
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

# Compute the identity strings the DLL should derive, straight from the fixture.
read -r EXPECT_FAMILY EXPECT_FULL EXPECT_AUMID < <(python3 - "$WORK/pkg/AppxManifest.xml" <<'PY'
import hashlib, re, sys
xml = open(sys.argv[1]).read()
def attr(a):
    m = re.search(r'<Identity[^>]*\b%s="([^"]+)"' % a, xml, re.S)
    return m.group(1) if m else None
name = attr("Name"); pub = attr("Publisher")
ver  = attr("Version") or "1.0.0.0"
arch = attr("ProcessorArchitecture") or "x64"
appm = re.search(r'<Application[^>]*\bId="([^"]+)"', xml, re.S)
app = appm.group(1) if appm else "App"
h = hashlib.sha256(pub.encode('utf-16-le')).digest()[:8]
alpha = "0123456789abcdefghjkmnpqrstvwxyz"
v = int.from_bytes(h, 'big')
pubid = ''.join(alpha[(v >> (59 - 5*i)) & 31] for i in range(12)) + alpha[(v & 15) << 1]
family = f"{name}_{pubid}"
full = f"{name}_{ver}_{arch}__{pubid}"
aumid = f"{family}!{app}"
print(family, full, aumid)
PY
)
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
if grep -q "^PASSED" "$WORK/packaged.txt" && grep -q "^PASSED" "$WORK/bare.txt"; then
    printf '\033[32m==> ALL APPMODEL TESTS PASSED\033[0m\n'
    exit 0
fi
printf '\033[31m==> APPMODEL TESTS FAILED (packaged rc=%s bare rc=%s)\033[0m\n' "$pk" "$ba"
exit 1
