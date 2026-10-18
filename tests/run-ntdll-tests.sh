#!/bin/bash
# Regression tests for the ntdll patches whose behaviour a title can see:
# the mapped-image filename fallback (patches/wine/0010), the code-byte dump
# under +seh (0007) and FileAllocationInformation (0012).
#
#   tests/run-ntdll-tests.sh              # test the currently built ntdll
#   REBUILD=1 tests/run-ntdll-tests.sh    # rebuild ntdll first
#
# Same staging trick as run-tests.sh: Wine loads a core builtin from its own
# tree, so we hardlink-copy the installed Proton and swap our fresh ntdll.dll
# and ntdll.so into it. The two halves have to match, so both are staged.

set -uo pipefail

REPO_DIR=${REPO_DIR:-$(cd "$(dirname "$0")/.." && pwd)}
. "$REPO_DIR/scripts/xodus-env.sh"
BUILD_DIR=${BUILD_DIR:-$XODUS_BUILD_DIR}
WINE_SRC=${WINE_SRC:-$XODUS_SRC_DIR/xodus-proton/wine}
TOOL_DIR=${TOOL_DIR:-${XODUS_STEAM_DIR:-$HOME/.steam/steam}/compatibilitytools.d/xodus}
OBJ=$BUILD_DIR/obj-wine-x86_64
INCUBE=$REPO_DIR/tools/in-container.sh
WORK=$(mktemp -d "$BUILD_DIR/.ntdll-tests.XXXXXX")

cleanup() {
    [ -n "${WINE:-}" ] && WINEPREFIX="$WORK/prefix" "$WINE"server -k 2>/dev/null
    rm -rf "$WORK" 2>/dev/null
}
trap cleanup EXIT

say() { printf '\033[1m:: %s\033[0m\n' "$*"; }
fail() { printf '\033[31m!! %s\033[0m\n' "$*" >&2; exit 1; }

DLL=$OBJ/dlls/ntdll/x86_64-windows/ntdll.dll
SO=$OBJ/dlls/ntdll/ntdll.so

if [ "${REBUILD:-0}" != "0" ]; then
    say "rebuilding ntdll"
    rsync -a --exclude .git "$WINE_SRC/dlls/ntdll/" "$BUILD_DIR/src-wine/dlls/ntdll/"
    echo "WORKDIR=$OBJ $INCUBE make -j$(nproc) dlls/ntdll/x86_64-windows/ntdll.dll dlls/ntdll/ntdll.so" \
        | newgrp docker || fail "build failed"
fi
[ -f "$DLL" ] && [ -f "$SO" ] || fail "$DLL or $SO not found (run with REBUILD=1)"

say "compiling the test binary"
"$INCUBE" x86_64-w64-mingw32-gcc -O1 -o "$WORK/ntdll_tests.exe" \
    "$REPO_DIR/tests/ntdll_tests.c" 2>&1 | grep -viE 'note:' || true
[ -f "$WORK/ntdll_tests.exe" ] || fail "test binary did not build"

[ -x "$TOOL_DIR/files/bin/wine" ] || fail "no installed Proton wine at $TOOL_DIR"
say "staging a wine copy with the freshly built ntdll"
cp -al "$TOOL_DIR/files" "$WORK/wine" || fail "could not hardlink-copy the wine tree"
rm -f "$WORK/wine/lib/wine/x86_64-windows/ntdll.dll" "$WORK/wine/lib/wine/x86_64-unix/ntdll.so"
cp "$DLL" "$WORK/wine/lib/wine/x86_64-windows/ntdll.dll"
cp "$SO" "$WORK/wine/lib/wine/x86_64-unix/ntdll.so"
WINE=$WORK/wine/bin/wine

export WINEPREFIX="$WORK/prefix"
export WINEDEBUG=-all,fixme-all
# Nothing here talks to the Xodus service; keep it from finding the real one.
export XODUS_RUNTIME_DIR="$WORK/rt"

filter() { grep -avE '^ntsync|radv is not|^wine:|wineserver'; }
results=()
record() {  # name, output file
    if grep -q "^PASSED" "$2"; then results+=("ok   $1"); else results+=("FAIL $1"); FAILED=1; fi
}
FAILED=0

for mode in mapped-name alloc; do
    say "running $mode"
    ( cd "$WORK" && timeout 120 "$WINE" "$WORK/ntdll_tests.exe" "$mode" ) 2>/dev/null | filter | tee "$WORK/$mode.txt"
    record "$mode" "$WORK/$mode.txt"
done

# +seh makes ntdll dump the code bytes around each fault. The dump itself is
# what is under test, so its log line is checked too: the readable bytes of
# the ud2 page are still shown, the unreadable ones as "??".
say "running seh-dump under WINEDEBUG=+seh"
( cd "$WORK" && WINEDEBUG=-all,+seh timeout 120 "$WINE" "$WORK/ntdll_tests.exe" seh-dump ) \
    2>"$WORK/seh-dump.log" | filter | tee "$WORK/seh-dump.txt"
record seh-dump "$WORK/seh-dump.txt"
if grep -aq -- '--- Code .*: ?? ?? .*?? 0f 0b c3 ' "$WORK/seh-dump.log"; then
    results+=("ok   seh-dump log shows the readable bytes of the ud2 page")
else
    results+=("FAIL seh-dump log shows the readable bytes of the ud2 page"); FAILED=1
    grep -a -- '--- Code' "$WORK/seh-dump.log" | head -5
fi

# The main image mapped from a memfd, as the client hands a decrypted title
# over: the file on disk is a decoy of the same size, and the image Wine runs
# exists only behind the descriptor named in WINE_DLL_FILE_MAP.
say "running mapped-name-memfd"
mkdir -p "$WORK/memfd"
python3 - "$WORK/ntdll_tests.exe" "$WORK/memfd/ntdll_tests.exe" "$WINE" > "$WORK/mapped-name-memfd.txt" 2>/dev/null <<'PY'
import os, subprocess, sys
real_path, decoy_path, wine = sys.argv[1:4]
real = open(real_path, "rb").read()
with open(decoy_path, "wb") as f:
    f.write(b"\0" * len(real))
fd = os.memfd_create("ntdll_tests", 0)
os.write(fd, real)
os.set_inheritable(fd, True)
dos = "Z:" + decoy_path.replace("/", "\\")
env = dict(os.environ, WINE_DLL_FILE_MAP=f"{fd}:\\??\\{dos}")
r = subprocess.run(["timeout", "120", wine, dos, "mapped-name-memfd"], env=env,
                   pass_fds=(fd,), cwd=os.path.dirname(decoy_path),
                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
sys.stdout.buffer.write(r.stdout)
PY
filter < "$WORK/mapped-name-memfd.txt"
record mapped-name-memfd "$WORK/mapped-name-memfd.txt"

echo
printf '   %s\n' "${results[@]}"
if [ "$FAILED" = 0 ]; then
    printf '\033[32m==> ALL NTDLL TESTS PASSED\033[0m\n'
    exit 0
fi
printf '\033[31m==> NTDLL TESTS FAILED\033[0m\n'
exit 1
