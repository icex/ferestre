#!/bin/bash
# Regression tests for dlls/gameinput (WINE_GAMEINPUT opt-in, IGameInput_v2,
# virtual keyboard/mouse devices and readings, dispatcher).
#
#   tests/run-gameinput-tests.sh              # test the currently built gameinput.dll
#   REBUILD=1 tests/run-gameinput-tests.sh    # rebuild it first
#
# Same staging as the other suites: hardlink-copy the installed Proton and swap
# the freshly built gameinput.dll into its builtin location, so we test the
# build and not whatever is installed.

set -uo pipefail

REPO_DIR=${REPO_DIR:-$(cd "$(dirname "$0")/.." && pwd)}
BUILD_DIR=${BUILD_DIR:-${XODUS_BUILD_DIR:-$HOME/src/xodus-build}}
WINE_SRC=${WINE_SRC:-$HOME/src/xodus-proton/wine}
TOOL_DIR=${TOOL_DIR:-${XODUS_PROTON_DIR:-$HOME/.steam/steam/compatibilitytools.d/xodus}}
OBJ=$BUILD_DIR/obj-wine-x86_64
INCUBE=$REPO_DIR/tools/in-container.sh
WORK=$(mktemp -d "$BUILD_DIR/.gameinput-tests.XXXXXX")

cleanup() {
    [ -n "${WINE:-}" ] && WINEPREFIX="$WORK/prefix" "$WINE"server -k 2>/dev/null
    rm -rf "$WORK" 2>/dev/null
}
trap cleanup EXIT

say() { printf '\033[1m:: %s\033[0m\n' "$*"; }
fail() { printf '\033[31m!! %s\033[0m\n' "$*" >&2; exit 1; }

DLL=$OBJ/dlls/gameinput/x86_64-windows/gameinput.dll

if [ "${REBUILD:-0}" != "0" ]; then
    say "rebuilding gameinput.dll"
    rsync -a --exclude .git "$WINE_SRC/dlls/gameinput/" "$BUILD_DIR/src-wine/dlls/gameinput/"
    echo "WORKDIR=$OBJ $INCUBE make -j$(nproc) dlls/gameinput/x86_64-windows/gameinput.dll" \
        | newgrp docker || fail "build failed"
fi
[ -f "$DLL" ] || fail "$DLL not found (run with REBUILD=1)"

say "compiling the test binary"
# IID_IUnknown comes from mingw's libuuid; the GameInput IIDs come from the
# generated header via initguid.h.
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -o "$WORK/gameinput_tests.exe" "$REPO_DIR/tests/gameinput_tests.c" \
    -I"$OBJ/include" -I"$WINE_SRC/include" -luuid 2>&1 | grep -viE 'note:' || true
[ -f "$WORK/gameinput_tests.exe" ] || fail "test binary did not build"

[ -x "$TOOL_DIR/files/bin/wine" ] || fail "no installed Proton wine at $TOOL_DIR"
say "staging a wine copy with the freshly built gameinput.dll"
cp -al "$TOOL_DIR/files" "$WORK/wine" || fail "could not hardlink-copy the wine tree"
BUILTIN=$WORK/wine/lib/wine/x86_64-windows/gameinput.dll
rm -f "$BUILTIN"
cp "$DLL" "$BUILTIN"
WINE=$WORK/wine/bin/wine

export WINEPREFIX="$WORK/prefix"
export WINEDEBUG=-all,fixme-all
unset WINE_GAMEINPUT WINE_GAMEINPUT_DEVQUERY SteamGameId SteamDeck

run() { ( cd "$WORK" && timeout 120 "$WINE" "$@" ) 2>/dev/null | grep -avE '^ntsync|radv is not|^wine:|wineserver'; }

say "running gated (no WINE_GAMEINPUT)"
run "$WORK/gameinput_tests.exe" gated | tee "$WORK/gated.txt"
echo
say "running enabled (WINE_GAMEINPUT=1)"
WINE_GAMEINPUT=1 run "$WORK/gameinput_tests.exe" enabled | tee "$WORK/enabled.txt"

echo
if grep -q "^PASSED" "$WORK/gated.txt" && grep -q "^PASSED" "$WORK/enabled.txt"; then
    printf '\033[32m==> ALL GAMEINPUT TESTS PASSED\033[0m\n'
    exit 0
fi
printf '\033[31m==> GAMEINPUT TESTS FAILED\033[0m\n'
exit 1
