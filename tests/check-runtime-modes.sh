#!/bin/bash
# Run inside the Proton SDK, using its own headless compositor and private prefix.
set -euo pipefail
runtime=$(realpath "${1:?runtime directory required}")
source_dir=$(cd "$(dirname "$0")" && pwd)
test_root=$(mktemp -d)
weston_pid=
cleanup() {
    if [ -d "$test_root/compat/pfx" ]; then
        WINEPREFIX="$test_root/compat/pfx" "$runtime/files/bin/wineserver" -k >/dev/null 2>&1 || true
    fi
    if [ -n "$weston_pid" ]; then
        kill "$weston_pid" 2>/dev/null || true
        wait "$weston_pid" 2>/dev/null || true
    fi
    rm -rf "$test_root"
}
trap cleanup EXIT
mkdir -m 700 "$test_root/runtime" "$test_root/home" "$test_root/compat" "$test_root/steam"
export XDG_RUNTIME_DIR="$test_root/runtime" HOME="$test_root/home"
export XDG_CONFIG_HOME="$test_root/home/config" XDG_DATA_HOME="$test_root/home/data"
unset DISPLAY WINEPREFIX
weston --backend=headless --renderer=pixman --width=7680 --height=2160 \
    --socket=ferestre-mode-test --idle-time=0 --log="$test_root/weston.log" &
weston_pid=$!
for attempt in $(seq 1 100); do
    [ ! -S "$XDG_RUNTIME_DIR/ferestre-mode-test" ] || break
    if ! kill -0 "$weston_pid" 2>/dev/null; then cat "$test_root/weston.log"; exit 1; fi
    sleep .1
done
test -S "$XDG_RUNTIME_DIR/ferestre-mode-test"
x86_64-w64-mingw32-gcc -std=c89 -Wall -Werror -mwindows \
    "$source_dir/runtime-display-modes.c" -o "$test_root/probe.exe" -luser32
export WAYLAND_DISPLAY=ferestre-mode-test XDG_SESSION_TYPE=wayland
export STEAM_COMPAT_DATA_PATH="$test_root/compat" STEAM_COMPAT_CLIENT_INSTALL_PATH="$test_root/steam"
export PROTON_USE_XALIA=0 WINEDEBUG=-all
export WINEDLLOVERRIDES='winex11.drv=d;winewayland.drv=b'
export FERESTRE_MODE_PROBE_OUTPUT="Z:$test_root/modes.log"
status=0
timeout 90 "$runtime/proton" run "$test_root/probe.exe" >"$test_root/proton.log" 2>&1 || status=$?
if [ -f "$test_root/modes.log" ]; then cat "$test_root/modes.log"; fi
if [ "$status" -ne 0 ]; then cat "$test_root/proton.log" "$test_root/weston.log"; fi
exit "$status"
