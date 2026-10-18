#!/bin/bash
set -euo pipefail
REPO_DIR=${REPO_DIR:-$(cd "$(dirname "$0")/.." && pwd)}
. "$REPO_DIR/scripts/xodus-env.sh"
BUILD_DIR=${BUILD_DIR:-$XODUS_BUILD_DIR}
ARCH=${ACCOUNT_TEST_ARCH:-x86_64}
case "$ARCH" in
    x86_64) COMPILER=x86_64-w64-mingw32-gcc; export WINEARCH=win64 ;;
    i386) COMPILER=i686-w64-mingw32-gcc; export WINEARCH=win32 ;;
    *) exit 2 ;;
esac
OBJ=$BUILD_DIR/obj-wine-$ARCH
TOOL_DIR=${TOOL_DIR:?Set TOOL_DIR to the runtime base to test}
WORK=$(mktemp -d "$BUILD_DIR/.account-roles-tests.XXXXXX")
cleanup() {
    if [ -n "${WINE:-}" ]; then "$WINE"server -k 2>/dev/null || true; fi
    if [ -n "${FIXTURE:-}" ]; then kill -TERM "$FIXTURE" 2>/dev/null || true; wait "$FIXTURE" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT
"$REPO_DIR/tools/in-container.sh" "$COMPILER" -O1 -D__WINESRC__ \
    -I"$OBJ/dlls/xgameruntime" -I"$BUILD_DIR/src-wine/dlls/xgameruntime" \
    -o "$WORK/account_roles.exe" "$REPO_DIR/tests/account_roles.c" -luuid
cp -a --reflink=auto "$TOOL_DIR/files" "$WORK/wine"
rm -f "$WORK/wine/lib/wine/$ARCH-windows/xgameruntime.dll" "$WORK/wine/lib/wine/$ARCH-unix/xgameruntime.so"
cp "${ACCOUNT_TEST_DLL:-$OBJ/dlls/xgameruntime/$ARCH-windows/xgameruntime.dll}" "$WORK/wine/lib/wine/$ARCH-windows/xgameruntime.dll"
cp "${ACCOUNT_TEST_UNIXLIB:-$OBJ/dlls/xgameruntime/xgameruntime.so}" "$WORK/wine/lib/wine/$ARCH-unix/xgameruntime.so"
WINE=$WORK/wine/bin/wine
export WINEPREFIX=$WORK/prefix WINEDEBUG=-all
export XODUS_RUNTIME_DIR=$WORK/service
sed 's/9TESTSTORE0ID/9TESTROLE0ID/g' "$REPO_DIR/tests/fixtures/MicrosoftGame.config" > "$WORK/MicrosoftGame.config"
printf '%s\n' '{"store":1,"player":1,"same":0,"licensed":1,"broken":0}' > "$WORK/account-control.json"
python3 "$REPO_DIR/tests/account_roles_fixture.py" "$XODUS_RUNTIME_DIR" "$WORK/account-control.json" &
FIXTURE=$!
for ((i=0;i<100;i++)); do [ -S "$XODUS_RUNTIME_DIR/xodus.sock" ] && break; sleep 0.05; done
(cd "$WORK" && timeout 300 "$WINE" "$WORK/account_roles.exe")
# Credentials a title hands the runtime, and the tokens the service hands
# back, must not reach a debug log even with tracing on; their presence and
# length may.
printf '%s\n' '{"store":1,"player":1,"same":1,"licensed":1,"broken":0}' > "$WORK/account-control.json"
(cd "$WORK" && WINEDEBUG=+gdkc,+storetrace timeout 120 "$WINE" "$WORK/account_roles.exe" --log-probe) 2> "$WORK/trace.log"
grep -q 'serviceTicket present' "$WORK/trace.log" || { echo "FAIL: the trace did not record the Store ID calls"; exit 1; }
if grep -aqE 'SYNTHETIC_(TICKET|PUBLISHER|TOKEN)_DO_NOT_LOG' "$WORK/trace.log"; then
    echo "FAIL: a credential reached the debug log"; exit 1
fi
echo "ok: credentials stay out of the debug log"
