#!/bin/bash
# Stop every process a GDK launch leaves behind.
#
# Stray instances are easy to accumulate while testing, and several running at
# once starve the GPU -- which looks exactly like the game itself being slow.
#
# Two ways this script can shoot itself in the foot, both of which have
# happened: the patterns must not appear in its own command line, and neither
# this shell nor anything that launched it may be killed just because the game
# name appears in its arguments. So the patterns are assembled at run time and
# every ancestor of this process is excluded.
P1="SandFall-Win""GDK"
P2="Forza""Horizon5"
P3="xodus-cli"' run'
P4="proton-wine""-shim"
P5="Minecraft.""Windows.exe"

# every ancestor of this shell, so a caller that merely mentions the game is safe
ancestors=" "
p=$$
while [ -n "$p" ] && [ "$p" != "0" ] && [ "$p" != "1" ]; do
    ancestors="$ancestors$p "
    p=$(ps -o ppid= -p "$p" 2>/dev/null | tr -d ' ')
done

kill_matching() {
    local sig=$1 pat=$2 p
    for p in $(pgrep -f "$pat" 2>/dev/null); do
        case "$ancestors" in *" $p "*) continue ;; esac
        kill "$sig" "$p" 2>/dev/null
    done
}

for pat in "$P1" "$P2" "$P5" "$P3" "$P4"; do kill_matching -TERM "$pat"; done
sleep 2
for pat in "$P1" "$P2" "$P5"; do kill_matching -KILL "$pat"; done
sleep 1

left=0
for pat in "$P1" "$P2" "$P5"; do
    for p in $(pgrep -f "$pat" 2>/dev/null); do
        case "$ancestors" in *" $p "*) continue ;; esac
        left=$((left + 1))
    done
done
echo "remaining: $left"
