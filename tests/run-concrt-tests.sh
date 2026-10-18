#!/bin/bash
# The concurrent runtime tests, on their own: the concurrent_vector segment
# test and the concurrent_queue iterator test, run under Wine against a given
# concrt140.dll.
#
#   tests/run-concrt-tests.sh                       # the build tree's builtin
#   tests/run-concrt-tests.sh --builtin DLL         # another builtin build
#   tests/run-concrt-tests.sh --native DLL          # also Microsoft's, as a reference
#
# Unlike tests/run-tests.sh this needs no license service, fixture or rebuilt
# xgameruntime, and it writes nothing into the build tree: the test binaries
# and the scratch prefix live under $TMPDIR (default /var/tmp).
#
# A builtin DLL is tested the only way Wine will load one, by swapping it into
# a copy of the installed Proton's own lib directory -- Wine resolves a builtin
# by name and ignores any path it is given. A native DLL is loaded by path with
# a native load-order override, which is how the expected behaviour (the
# allocator is given element counts, the segment layout operator[] relies on)
# was checked against Microsoft's implementation.
#
# Env overrides: BUILD_DIR, TOOL_DIR, STEAMRT_IMAGE.

set -uo pipefail

REPO_DIR=$(cd "$(dirname "$0")/.." && pwd)
. "$REPO_DIR/scripts/xodus-env.sh"
BUILD_DIR=${BUILD_DIR:-$XODUS_BUILD_DIR}
TOOL_DIR=${TOOL_DIR:-${XODUS_STEAM_DIR:-$HOME/.steam/steam}/compatibilitytools.d/xodus}
IMAGE=${STEAMRT_IMAGE:-registry.gitlab.steamos.cloud/proton/steamrt4/sdk/x86_64:4.0.20260331.220802-0}
BUILTIN=$BUILD_DIR/obj-wine-x86_64/dlls/concrt140/x86_64-windows/concrt140.dll
NATIVE=

say() { printf '\033[1m:: %s\033[0m\n' "$*"; }
fail() { printf '\033[31m!! %s\033[0m\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case $1 in
        --builtin) BUILTIN=$2; shift 2 ;;
        --native) NATIVE=$2; shift 2 ;;
        *) fail "unknown argument: $1" ;;
    esac
done
[ -f "$BUILTIN" ] || fail "no builtin concrt140.dll at $BUILTIN (build it, or pass --builtin)"
[ -z "$NATIVE" ] || [ -f "$NATIVE" ] || fail "no native concrt140.dll at $NATIVE"
[ -x "$TOOL_DIR/files/bin/wine" ] || fail "no installed Proton wine at $TOOL_DIR"
BUILTIN=$(realpath "$BUILTIN")

WORK=$(mktemp -d "${TMPDIR:-/var/tmp}/ferestre-concrt.XXXXXX")
cleanup() {
    [ -x "$WORK/wine/bin/wineserver" ] && WINEPREFIX="$WORK/prefix" "$WORK/wine/bin/wineserver" -k 2>/dev/null
    rm -rf "$WORK" 2>/dev/null
}
trap cleanup EXIT

say "compiling the tests"
docker run --rm -v "$REPO_DIR:$REPO_DIR:ro" -v "$WORK:$WORK" -w "$WORK" \
    --user "$(id -u):$(id -g)" -e HOME=/tmp "$IMAGE" sh -c "
    x86_64-w64-mingw32-gcc -O1 -Wall -Werror -o concrt_vector_segments.exe $REPO_DIR/tests/concrt_vector_segments.c &&
    x86_64-w64-mingw32-gcc -O1 -o concrt_queue_iterator.exe $REPO_DIR/tests/concrt_queue_iterator.c" \
    || fail "the tests did not build"

# Hardlinks where the trees share a filesystem, reflinks or a plain copy
# otherwise. Either way the installed Proton is untouched: rm + cp replaces the
# one file in the copy.
say "staging a wine copy with $BUILTIN"
cp -al "$TOOL_DIR/files" "$WORK/wine" 2>/dev/null || {
    rm -rf "$WORK/wine"
    cp -a --reflink=auto "$TOOL_DIR/files" "$WORK/wine" || fail "could not copy the wine tree"
}
rm -f "$WORK/wine/lib/wine/x86_64-windows/concrt140.dll"
cp "$BUILTIN" "$WORK/wine/lib/wine/x86_64-windows/concrt140.dll"

# A console test needs no display, and none is offered: nothing may open a
# window on the desktop. mscoree and mshtml are disabled so creating the
# prefix does not offer to install Mono or Gecko.
export WINEPREFIX="$WORK/prefix"
export WINEDEBUG=-all
run() {
    ( cd "$WORK" && env -u DISPLAY -u WAYLAND_DISPLAY timeout 120 "$WORK/wine/bin/wine" "$@" ) 2>/dev/null |
        grep -avE '^ntsync|radv is not|^wine:|wineserver'
    return "${PIPESTATUS[0]}"
}
WINEDLLOVERRIDES="mscoree,mshtml=" env -u DISPLAY -u WAYLAND_DISPLAY "$WORK/wine/bin/wineboot" -u >/dev/null 2>&1

rc=0
say "concurrent_vector segments, builtin"
WINEDLLOVERRIDES="mscoree,mshtml=" run concrt_vector_segments.exe | tail -n 40 || rc=1
say "concurrent_queue iterators, builtin"
WINEDLLOVERRIDES="mscoree,mshtml=" run concrt_queue_iterator.exe || rc=1

if [ -n "$NATIVE" ]; then
    mkdir -p "$WORK/native"
    cp "$NATIVE" "$WORK/native/concrt140.dll"
    say "concurrent_vector segments, native reference $NATIVE"
    WINEDLLOVERRIDES="mscoree,mshtml=;concrt140=n" run concrt_vector_segments.exe 'native\concrt140.dll' | tail -n 40 || rc=1
    say "concurrent_queue iterators, native reference"
    WINEDLLOVERRIDES="mscoree,mshtml=;concrt140=n" run concrt_queue_iterator.exe 'native\concrt140.dll' || rc=1
fi

echo
if [ "$rc" = 0 ]; then
    printf '\033[32m==> ALL CONCRT TESTS PASSED\033[0m\n'
    exit 0
fi
printf '\033[31m==> CONCRT TESTS FAILED\033[0m\n'
exit 1
