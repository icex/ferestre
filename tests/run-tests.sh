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
. "$REPO_DIR/scripts/xodus-env.sh"
BUILD_DIR=${BUILD_DIR:-$XODUS_BUILD_DIR}
WINE_SRC=${WINE_SRC:-$XODUS_SRC_DIR/xodus-proton/wine}
TOOL_DIR=${TOOL_DIR:-${XODUS_STEAM_DIR:-$HOME/.steam/steam}/compatibilitytools.d/xodus}
OBJ=$BUILD_DIR/obj-wine-x86_64
INCUBE=$REPO_DIR/tools/in-container.sh
# The build container only mounts the repository, source and build trees, so
# the compiler's output directory has to live in one of them, not under /tmp.
WORK=$(mktemp -d "$BUILD_DIR/.xgr-tests.XXXXXX")
cleanup() {
    if [ -n "${RELATED_FIXTURE_PID:-}" ]; then kill -TERM "$RELATED_FIXTURE_PID" 2>/dev/null; wait "$RELATED_FIXTURE_PID" 2>/dev/null; fi
    if [ -n "${CATALOG_FIXTURE_PID:-}" ]; then kill -TERM "$CATALOG_FIXTURE_PID" 2>/dev/null; wait "$CATALOG_FIXTURE_PID" 2>/dev/null; fi
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
    echo "WORKDIR=$OBJ $INCUBE make -j$(nproc) dlls/xgameruntime/x86_64-windows/xgameruntime.dll dlls/xgameruntime/xgameruntime.so dlls/concrt140/x86_64-windows/concrt140.dll" \
        | newgrp docker || fail "build failed"
fi
[ -f "$DLL" ] || fail "$DLL not found (run with REBUILD=1)"

say "compiling the test binary"
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -o "$WORK/xgr_tests.exe" "$REPO_DIR/tests/xgr_tests.c" \
    -D__WINESRC__ -DCOBJMACROS -I"$OBJ/dlls/xgameruntime" -I"$WINE_SRC/dlls/xgameruntime" \
    -luuid 2>&1 | grep -viE 'COBJMACROS redefined|this is the location|note:' || true
[ -f "$WORK/xgr_tests.exe" ] || fail "test binary did not build"

say "compiling the Store catalog fixture"
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -o "$WORK/store_catalog.exe" "$REPO_DIR/tests/store_catalog.c" \
    -D__WINESRC__ -I"$OBJ/dlls/xgameruntime" -I"$WINE_SRC/dlls/xgameruntime" -luuid || fail "catalog fixture did not build"

say "compiling the account-free PlayFab SDK fixture"
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -shared -o "$WORK/PlayFabCore.dll" "$REPO_DIR/tests/pfx_sdk_fixture.c" \
    -D__WINESRC__ -I"$OBJ/dlls/xgameruntime" -I"$WINE_SRC/dlls/xgameruntime" -ladvapi32 || fail "PFX fixture did not build"
cp "$WORK/PlayFabCore.dll" "$WORK/PlayFabGameSave.dll"

say "compiling the concurrent queue iterator test"
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -o "$WORK/concrt_queue_iterator.exe" "$REPO_DIR/tests/concrt_queue_iterator.c" || fail "iterator test did not build"
say "compiling the concurrent vector segment test"
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -Wall -Werror -o "$WORK/concrt_vector_segments.exe" \
    "$REPO_DIR/tests/concrt_vector_segments.c" || fail "vector segment test did not build"

# Compiled, never run here: it needs a Vulkan GPU. Building it on every run is
# what keeps a test nobody runs routinely from quietly stopping compiling.
say "compiling the D3D12 allocator lifetime test (not run: it needs a GPU)"
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -o "$WORK/d3d12_allocator_lifetime.exe" \
    "$REPO_DIR/tests/d3d12_allocator_lifetime.c" 2>&1 | grep -v 'note:' || true
[ -f "$WORK/d3d12_allocator_lifetime.exe" ] || fail "the D3D12 allocator lifetime test did not build"

# Also compiled and not run, for the same reason: the SRW lock soak takes
# minutes, and the WebSocket harness drives a title's own libHttpClient
# against tests/ws_server.py. The harness includes the widl header generated
# from the patched xgameruntime IDL, so an IDL change that breaks it shows here.
say "compiling the SRW lock stress test (not run: it is a long soak)"
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -o "$WORK/srwlock_stress.exe" "$REPO_DIR/tests/srwlock_stress.c" \
    || fail "the SRW lock stress test did not build"
say "compiling the WebSocket lifecycle harness (not run: it needs a title's libHttpClient)"
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -o "$WORK/websocket_harness.exe" "$REPO_DIR/tests/websocket_harness.c" \
    -D__WINESRC__ -I"$OBJ/dlls/xgameruntime" -I"$WINE_SRC/dlls/xgameruntime" || fail "the WebSocket harness did not build"

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
CONCRT=$WORK/wine/lib/wine/x86_64-windows/concrt140.dll
rm -f "$CONCRT"
cp "$OBJ/dlls/concrt140/x86_64-windows/concrt140.dll" "$CONCRT" || fail "missing built concurrent runtime"
WINE=$WORK/wine/bin/wine

say "running tests"
# Nothing inherited from the calling shell may lead the tests to the real
# service or account. A terminal opened from a launched title carries
# XODUS_RUNTIME_DIR and the profile variables, and the DLL asks the socket in
# XODUS_RUNTIME_DIR before the one in XDG_RUNTIME_DIR, so the fixture's
# directory is set as both below.
unset XODUS_RUNTIME_DIR XODUS_STORE_PROFILE XODUS_PLAYER_PROFILE XODUS_ACCOUNT_PROFILE \
    FERESTRE_ACCOUNT_ROLES WINE_PACKAGE_MANIFEST WINEDLLOVERRIDES XGR_XUID XGR_STORE_CATALOG \
    SteamGameId SteamDeck XGR_ACCOUNT_ROLES_V1 XGR_RELATED_PACKAGE_PATHS XODUS_RELATED_CONTENT
export WINEPREFIX="$WORK/prefix"
export WINEDEBUG=-all,fixme-all
export XGR_WGS_ROOT="Z:$(echo "$WORK/wgs" | sed 's#/#\\#g')"
export XGR_EXPECTED_FAMILY=$EXPECTED_FAMILY

if [ "${PROVIDER_ONLY:-0}" = 1 ]; then
    ( cd "$WORK" && timeout 30 "$WINE" "$WORK/xgr_tests.exe" --provider-data )
    exit $?
fi

# A separate process exercises its immutable related-package snapshot without
# changing what the existing current-package tests see.
python3 - "$WORK" <<'PYFIXTURE'
import pathlib,sys
root=pathlib.Path(sys.argv[1])
config=root/'MicrosoftGame.config'
(root/'base-config.backup').write_bytes(config.read_bytes())
config.write_text(config.read_text().replace('9TESTSTORE0ID','9TESTPARENT0'))
for name,store,parent in [('related-one','9TESTADDON01','9TESTPARENT0'),('related-two','9TESTADDON02','9TESTPARENT0'),('unrelated','9TESTOTHER00','9OTHERGAME00')]:
 d=root/name;d.mkdir()
 (d/'MicrosoftGame.config').write_text(f'<Game><Identity Name="Fixture.{name}" Version="1.2.3.4"/><StoreId>{store}</StoreId><AllowedProducts><AllowedProduct>{parent}</AllowedProduct></AllowedProducts><DesktopRegistration><MainPackageDependency Name="XodusTest.RegressionGame" /></DesktopRegistration><ShellVisuals DefaultDisplayName="Fixture content" Description="Fixture content" PublisherDisplayName="Fixture"/></Game>')
PYFIXTURE
RELATED_PATHS=$(printf '%s\n%s\n%s' "$WORK/related-one" "$WORK/related-two" "$WORK/unrelated")
( cd "$WORK" && XGR_RELATED_PACKAGE_PATHS="$RELATED_PATHS" timeout 30 "$WINE" "$WORK/xgr_tests.exe" --related-packages ) > "$WORK/related-out.txt" 2>&1
related_rc=$?
cat "$WORK/related-out.txt"
[ "$related_rc" = 0 ] || fail "related-package regression checks failed"
python3 "$REPO_DIR/tests/related_license_fixture.py" "$WORK/related-rt" &
RELATED_FIXTURE_PID=$!
for _ in $(seq 1 50); do [ -f "$WORK/related-rt/ready" ] && break; sleep 0.1; done
[ -f "$WORK/related-rt/ready" ] || fail "related licence fixture did not start"
( cd "$WORK" && XODUS_RUNTIME_DIR="$WORK/related-rt" XGR_ACCOUNT_ROLES_V1=1 XGR_LICENSE_FIXTURE_REVOKE="Z:$WORK/related-rt/revoked" XGR_RELATED_PACKAGE_PATHS="$RELATED_PATHS" timeout 30 "$WINE" "$WORK/xgr_tests.exe" --related-licenses ) > "$WORK/related-license-out.txt" 2>&1
related_rc=$?
cat "$WORK/related-license-out.txt"
[ "$related_rc" = 0 ] || fail "related licence regression checks failed"
kill -TERM "$RELATED_FIXTURE_PID"; wait "$RELATED_FIXTURE_PID" 2>/dev/null; unset RELATED_FIXTURE_PID
cp "$WORK/base-config.backup" "$WORK/MicrosoftGame.config"
[ "${RELATED_ONLY:-0}" != 1 ] || exit 0

say "starting the synthetic license service"
SERVICE_TEST_BIN=${SERVICE_TEST_BIN:-$("$REPO_DIR/tests/build-license-service-fixture.sh")} || fail "service fixture build failed"
export XDG_RUNTIME_DIR="$WORK/rt"
export XODUS_RUNTIME_DIR="$XDG_RUNTIME_DIR"
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
if [ "$rc" = 0 ]; then
    ( cd "$WORK" && timeout 30 "$WINE" "$WORK/xgr_tests.exe" --pfx ) 2>/dev/null | \
        grep -avE '^ntsync|radv is not|^wine:|wineserver' | tee "$WORK/pfx-out.txt"
    rc=${PIPESTATUS[0]}
fi
if [ "$rc" = 0 ]; then
    ( cd "$WORK" && timeout 30 "$WINE" "$WORK/concrt_queue_iterator.exe" ) 2>/dev/null | \
        grep -avE '^ntsync|radv is not|^wine:|wineserver' | tee "$WORK/iterator-out.txt"
    rc=${PIPESTATUS[0]}
fi
# The same builtin concrt140 against Microsoft's segment layout; see
# tests/run-concrt-tests.sh to compare it with a native DLL.
if [ "$rc" = 0 ]; then
    ( cd "$WORK" && timeout 120 "$WINE" "$WORK/concrt_vector_segments.exe" ) 2>/dev/null | \
        grep -avE '^ntsync|radv is not|^wine:|wineserver' | tail -5 | tee "$WORK/vector-out.txt"
    rc=${PIPESTATUS[0]}
fi
if [ "$rc" = 0 ]; then
    say "running catalog paging and malformed-response checks"
    mkdir -p "$WORK/catalog"
    sed 's/9TESTSTORE0ID/9TESTPARENT0/' "$WORK/MicrosoftGame.config" > "$WORK/catalog/MicrosoftGame.config"
    python3 "$REPO_DIR/tests/store_catalog_fixture.py" "$WORK/catalog-rt" &
    CATALOG_FIXTURE_PID=$!
    for _ in $(seq 1 100); do [ -f "$WORK/catalog-rt/ready" ] && break; sleep 0.1; done
    [ -f "$WORK/catalog-rt/ready" ] || fail "catalog fixture did not start"
    ( cd "$WORK/catalog" && XODUS_RUNTIME_DIR="$WORK/catalog-rt" timeout 30 "$WINE" "$WORK/store_catalog.exe" ) \
        2>/dev/null | tee "$WORK/catalog-out.txt"
    rc=${PIPESTATUS[0]}
    kill -TERM "$CATALOG_FIXTURE_PID" 2>/dev/null
    wait "$CATALOG_FIXTURE_PID" 2>/dev/null
    CATALOG_FIXTURE_PID=
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
