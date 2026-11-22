#!/bin/bash
# Launch one installed title off-screen, watch it, screenshot it, stop it.
#
#   tools/headless-title.sh <product-id> [seconds]
#
# The title runs through the real launcher (`ferestre run`) inside a headless
# gamescope at 1280x720, in a systemd user scope of its own. Nothing appears on
# the desktop and nothing takes the keyboard focus, so it is safe to run while
# someone is working. Every 30 seconds it records which game executables are
# alive in the scope and takes a gamescope screenshot. At the end (default
# 300 s), or as soon as the scope exits by itself, it stops only the processes
# in its own scope.
#
# Output goes to a fresh private directory (mode 700), printed on the last
# line, holding:
#   summary.txt     the display used, then one line per sample: t=<s> exe=[...]
#                   and "scope exited at t=<s>" if the game went away early
#   shot-NNN.png    a screenshot at second NNN (check these by eye)
#   launch.log      what `ferestre run` printed
#   game.stderr     the launcher's per-title log, ~/.local/state/ferestre/launches/<id>.stderr
#
# A title that is still listed at the last sample and whose screenshots show
# the game is "launches and runs". An exit status of 255 in game.stderr after
# the final sample is this script stopping it, not a crash.
#
# Environment:
#   FERESTRE   the launcher to run (default ~/.local/bin/ferestre.AppImage)
#   OUT        output directory instead of a fresh one under /var/tmp
#   SIZE       WxH of the virtual display (default 1280x720)
#
# Needs gamescope (with gamescopectl) and a systemd user session. The title
# must already be installed, and the launcher must be able to run it, using its
# recipe and runtime selection exactly as a player's launch would.
set -u
id=${1:?usage: headless-title.sh <product-id> [seconds]}
secs=${2:-300}
ferestre=${FERESTRE:-$HOME/.local/bin/ferestre.AppImage}
size=${SIZE:-1280x720}
umask 077
out=${OUT:-$(mktemp -d "/var/tmp/ferestre-title-$id.XXXXXX")}
mkdir -p "$out"
unit=ferestre-title-$(echo "$id" | tr 'A-Z' 'a-z')-$$
rt=${XDG_RUNTIME_DIR:?needs a user session}

displays() { ls "$rt" | grep -E '^gamescope-[0-9]+$'; }
scope_procs() {
    local cg
    cg=/sys/fs/cgroup$(systemctl --user show -p ControlGroup --value "$unit.scope" 2>/dev/null)
    [ "$cg" != /sys/fs/cgroup ] && [ -d "$cg" ] || return 0
    find "$cg" -name cgroup.procs -exec cat {} + 2>/dev/null
}

before=$(displays)
# Low CPU and I/O weight: the machine is usually doing something else too.
systemd-run --user --scope --unit="$unit" --collect -p CPUWeight=50 -p IOWeight=50 -- \
    env -u WAYLAND_DISPLAY gamescope --backend headless -w "${size%x*}" -h "${size#*x}" -r 60 -- \
    "$ferestre" run "$id" > "$out/launch.log" 2>&1 &
sleep 8
display=$(comm -13 <(echo "$before") <(displays) | tail -1)
echo "display=$display" > "$out/summary.txt"

# Wine's own helpers are always there; what matters is whether the game is.
helpers='wineserver|services\.exe|winedevice|plugplay|explorer\.exe|rpcss|svchost|conhost|tabtip|xalia|steam\.exe|start\.exe|wineboot'
t=0
while [ "$t" -lt "$secs" ]; do
    sleep 30; t=$((t + 30))
    exe=$(for p in $(scope_procs); do tr '\0' ' ' < "/proc/$p/cmdline" 2>/dev/null; echo; done \
        | grep -i '\.exe' | grep -v -i -E "$helpers" | sed 's/.*\\//' | cut -c1-80 | sort -u | tr '\n' ';')
    [ -n "$display" ] && GAMESCOPE_WAYLAND_DISPLAY=$display timeout 10 \
        gamescopectl screenshot "$out/shot-$(printf %03d "$t").png" > /dev/null 2>&1
    echo "t=$t exe=[$exe]" >> "$out/summary.txt"
    if ! systemctl --user is-active -q "$unit.scope"; then
        echo "scope exited at t=$t" >> "$out/summary.txt"
        break
    fi
done

# The launcher's own stop path first (xodus-cli on TERM shuts the game down),
# then the whole scope. Only this scope: a game the user is playing is not in it.
for p in $(scope_procs); do
    grep -q -a xodus-cli "/proc/$p/cmdline" 2>/dev/null && kill -TERM "$p" 2>/dev/null
done
for _ in $(seq 20); do systemctl --user is-active -q "$unit.scope" || break; sleep 1; done
systemctl --user stop "$unit.scope" 2>/dev/null
cp "$HOME/.local/state/ferestre/launches/$id.stderr" "$out/game.stderr" 2>/dev/null
cat "$out/summary.txt"
echo "$out"
