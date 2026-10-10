#!/bin/bash
# Regression tests for the xgameruntime work in this repo.
#
# Builds the test binary against the current sources, runs it under the locally
# built Xodus Wine against a synthetic fixture and a scratch save root, and
# reports pass/fail. Exit status is non-zero if anything failed, so it drops
# into CI or a pre-commit check.
#
#   tests/run-tests.sh                 # build DLL is assumed current; just test
#   REBUILD=1 tests/run-tests.sh       # rebuild xgameruntime.dll first
#
# Env overrides: BUILD_DIR, WINE_SRC, TOOL_DIR (see defaults below).

set -uo pipefail

REPO_DIR=${REPO_DIR:-$(cd "$(dirname "$0")/.." && pwd)}
BUILD_DIR=${BUILD_DIR:-${XODUS_SRC_DIR:-$HOME/src}/xodus-build}
WINE_SRC=${WINE_SRC:-${XODUS_SRC_DIR:-$HOME/src}/xodus-proton/wine}
TOOL_DIR=${TOOL_DIR:-${XODUS_STEAM_DIR:-$HOME/.steam/steam}/compatibilitytools.d/xodus}
OBJ=$BUILD_DIR/obj-wine-x86_64
INCUBE=$REPO_DIR/tools/in-container.sh
# The build container only mounts ${XODUS_SRC_DIR:-$HOME/src}, so the compiler's output
# directory has to live there too, not under /tmp.
WORK=$(mktemp -d "$BUILD_DIR/.xgr-tests.XXXXXX")
cleanup() {
    if [ -n "${FIXTURE_PID:-}" ]; then kill -TERM "$FIXTURE_PID" 2>/dev/null; wait "$FIXTURE_PID" 2>/dev/null; fi
    [ -n "${WINE:-}" ] && WINEPREFIX="$WORK/prefix" "$WINE"server -k 2>/dev/null
    rm -rf "$WORK" 2>/dev/null
}
trap cleanup EXIT

say() { printf '\033[1m:: %s\033[0m\n' "$*"; }
fail() { printf '\033[31m!! %s\033[0m\n' "$*" >&2; exit 1; }

DLL=${XGR_TEST_DLL:-$OBJ/dlls/xgameruntime/x86_64-windows/xgameruntime.dll}

if [ "${REBUILD:-0}" != "0" ]; then
    say "rebuilding xgameruntime.dll"
    rsync -a --exclude .git "$WINE_SRC/dlls/xgameruntime/" "$BUILD_DIR/src-wine/dlls/xgameruntime/"
    echo "WORKDIR=$OBJ $INCUBE make -j$(nproc) dlls/xgameruntime/x86_64-windows/xgameruntime.dll dlls/xgameruntime/xgameruntime.so" \
        | newgrp docker || fail "build failed"
fi
[ -f "$DLL" ] || fail "$DLL not found (run with REBUILD=1)"

say "compiling the test binary"
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -o "$WORK/xgr_tests.exe" "$REPO_DIR/tests/xgr_tests.c" \
    -D__WINESRC__ -DCOBJMACROS -I"$OBJ/dlls/xgameruntime" -I"$WINE_SRC/dlls/xgameruntime" \
    -luuid 2>&1 | grep -viE 'COBJMACROS redefined|this is the location|note:' || true
[ -f "$WORK/xgr_tests.exe" ] || fail "test binary did not build"

# Compiled, never run here: it needs a Vulkan GPU. Building it on every run is
# what keeps a test nobody runs routinely from quietly stopping compiling.
say "compiling the D3D12 allocator lifetime test (not run: it needs a GPU)"
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -o "$WORK/d3d12_allocator_lifetime.exe" \
    "$REPO_DIR/tests/d3d12_allocator_lifetime.c" 2>&1 | grep -v 'note:' || true
[ -f "$WORK/d3d12_allocator_lifetime.exe" ] || fail "the D3D12 allocator lifetime test did not build"

# Fixtures: a MicrosoftGame.config in the cwd, and a scratch save root.
cp "$REPO_DIR/tests/fixtures/MicrosoftGame.config" "$WORK/"
cp "$DLL" "$WORK/xgameruntime.dll"
mkdir -p "$WORK/wgs"

# The tests assert the derived package family name; compute the expected value
# the same way the DLL does (SHA-256 over the UTF-16 publisher, Crockford b32).
EXPECTED_FAMILY=$(python3 - "$WORK/MicrosoftGame.config" <<'PY'
import hashlib, re, sys
xml = open(sys.argv[1]).read()
name = re.search(r'<Identity[^>]*\bName="([^"]+)"', xml).group(1)
pub = re.search(r'<Identity[^>]*\bPublisher="([^"]+)"', xml).group(1)
h = hashlib.sha256(pub.encode('utf-16-le')).digest()[:8]
alpha = "0123456789abcdefghjkmnpqrstvwxyz"
v = int.from_bytes(h, 'big')
suffix = ''.join(alpha[(v >> (59 - 5*i)) & 31] for i in range(12)) + alpha[(v & 15) << 1]
print(f"{name}_{suffix}")
PY
)
say "expected package family: $EXPECTED_FAMILY"

# Wine resolves a builtin DLL by name and loads its own installed copy,
# ignoring WINEDLLPATH and any path we pass -- so to test the freshly built
# xgameruntime.dll we make a hardlinked copy of the Proton files (cheap, same
# filesystem) and swap our DLL into its builtin location. The installed Proton
# is untouched: rm+cp breaks the hardlink for that one file only.
[ -x "$TOOL_DIR/files/bin/wine" ] || fail "no installed Proton wine at $TOOL_DIR"
say "staging a wine copy with the freshly built DLL"
cp -al "$TOOL_DIR/files" "$WORK/wine" || fail "could not hardlink-copy the wine tree"
BUILTIN=$WORK/wine/lib/wine/x86_64-windows/xgameruntime.dll
rm -f "$BUILTIN"
cp "$DLL" "$BUILTIN"
# The new IPC function changes the DLL/Unix-library pair; always stage both.
UNIXLIB=$WORK/wine/lib/wine/x86_64-unix/xgameruntime.so
rm -f "$UNIXLIB"
cp "$OBJ/dlls/xgameruntime/xgameruntime.so" "$UNIXLIB" || fail "missing matching Unix library"
WINE=$WORK/wine/bin/wine

say "running tests"
export WINEPREFIX="$WORK/prefix"
export WINEDEBUG=-all,fixme-all
export XGR_WGS_ROOT="Z:$(echo "$WORK/wgs" | sed 's#/#\\#g')"
export XGR_EXPECTED_FAMILY=$EXPECTED_FAMILY


say "starting the synthetic license service"
SERVICE_TEST_BIN=${SERVICE_TEST_BIN:-$("$REPO_DIR/tests/build-license-service-fixture.sh")} || fail "service fixture build failed"
export XDG_RUNTIME_DIR="$WORK/rt"
python3 "$REPO_DIR/tests/license_token_fixture.py" "$XDG_RUNTIME_DIR" "$SERVICE_TEST_BIN" "$WORK/license-transcript.json" &
FIXTURE_PID=$!
for _ in $(seq 1 100); do [ -f "$XDG_RUNTIME_DIR/ready" ] && break; sleep 0.1; done
[ -f "$XDG_RUNTIME_DIR/ready" ] || fail "service fixture did not start"

( cd "$WORK" && timeout 180 "$WINE" "$WORK/xgr_tests.exe" ) 2>/dev/null | \
    grep -avE '^ntsync|radv is not|^wine:|wineserver' | tee "$WORK/out.txt"
rc=${PIPESTATUS[0]}
if [ "$rc" = 0 ]; then
    ( cd "$WORK" && XGR_STRICT_MANUAL_QUEUES=1 timeout 30 "$WINE" "$WORK/xgr_tests.exe" --manual-queues ) 2>/dev/null | \
        grep -avE '^ntsync|radv is not|^wine:|wineserver' | tee "$WORK/manual-out.txt"
    rc=${PIPESTATUS[0]}
fi
kill -TERM "$FIXTURE_PID" 2>/dev/null
wait "$FIXTURE_PID"
fixture_rc=$?
FIXTURE_PID=
cat "$WORK/license-transcript.json"
[ "$fixture_rc" = 0 ] || rc=1

echo
if [ "$rc" = "0" ] && grep -q "^PASSED" "$WORK/out.txt"; then
    printf '\033[32m==> ALL TESTS PASSED\033[0m\n'
    exit 0
fi
printf '\033[31m==> TESTS FAILED (rc=%s)\033[0m\n' "$rc"
exit 1
