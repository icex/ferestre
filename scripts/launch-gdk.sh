#!/bin/bash
# Launch an Xbox GDK / Store (MSIXVC) title through Xodus + Proton.
#
#   launch-gdk.sh <game dir> <exe, backslash path relative to game dir> <proton prefix dir> [log file]
#
# The executable is encrypted on disk and only exists decrypted inside a memfd
# that xodus-cli hands to Wine, so the game cannot be started directly -- it
# has to go through `xodus-cli run`. This wraps that up so a per-game one-liner
# can be a Steam shortcut or a desktop launcher.

set -u
. "$(dirname "$0")/xodus-env.sh"

GAME_DIR=$1
EXE=$2
PREFIX=$3
LOG=${4:-$XODUS_GAMES_DIR/$(basename "$GAME_DIR")-launch.log}
SHIM=$(cd "$(dirname "$0")" && pwd)/proton-wine-shim.sh

xodus_require "xodus-cli"   "$XODUS_CLI_DIR/xodus-cli" "build it: cargo build --release in the xodus-cli checkout, or set XODUS_CLI_DIR"
xodus_require "Xodus Proton" "$XODUS_PROTON_DIR"        "run scripts/install-xodus-proton.sh, or set XODUS_PROTON_DIR"
xodus_require "game dir"    "$GAME_DIR"                 "download it first: scripts/get-game.sh <product id> \"$GAME_DIR\""

mkdir -p "$(dirname "$LOG")" "$PREFIX"
exec > >(tee -a "$LOG") 2>&1
echo "=== launch $(date) ==="

# Steam starts shortcuts with its own environment: the overlay is injected via
# LD_PRELOAD, LD_LIBRARY_PATH points into the Steam runtime, and STEAM_COMPAT_*
# describe *Steam's* idea of the app. All of that collides with the Proton
# instance we start ourselves, so drop it and set our own. SteamAppId,
# SteamGameId and SteamClientLaunch stay: Steam Input applies the shortcut's
# controller configuration only to a process carrying them.
unset LD_PRELOAD LD_LIBRARY_PATH
unset STEAM_COMPAT_DATA_PATH STEAM_COMPAT_CLIENT_INSTALL_PATH
unset STEAM_COMPAT_TRANSCODED_MEDIA_PATH STEAM_COMPAT_MEDIA_PATH
unset SteamAppUser SteamEnv
unset WINEPREFIX WINEDLLPATH WINELOADER WINESERVER

# Steam also attaches its Vulkan layers (overlay + Fossilize pipeline capture)
# through the environment. Fossilize records pipelines for a Steam appid that
# does not really own this process, and its capture layer sits in the same
# vkd3d-proton path the game renders through -- the log fills with
# "pipeline handle is not registered" and the run dies. None of it is wanted
# for a launcher that drives Proton itself.
unset ENABLE_VK_LAYER_VALVE_steam_fossilize_1 ENABLE_VK_LAYER_VALVE_steam_overlay_1
unset VK_LAYER_PATH VK_INSTANCE_LAYERS VK_LOADER_LAYERS_ENABLE
unset STEAM_COMPAT_SHADER_PATH STEAM_FOSSILIZE_DUMP_PATH
export DISABLE_VK_LAYER_VALVE_steam_overlay_1=1
export DISABLE_VK_LAYER_VALVE_steam_fossilize_1=1

# GDK/UWP titles (Minecraft) require the GameInput runtime; enable our
# gameinput.dll implementation, which is otherwise behind an allow-list.
export WINE_GAMEINPUT=1

export STEAM_COMPAT_DATA_PATH=$PREFIX
export STEAM_COMPAT_CLIENT_INSTALL_PATH=${XODUS_STEAM_DIR:-$HOME/.steam/steam}
export PROTON_DIR=$XODUS_PROTON_DIR

# XUID the Windows install saved under (the prefix of its save folder names).
# Existing save folders are found by SCID regardless; this only names new ones
# the same way so they can be copied back to Windows as-is. Set XGR_XUID to
# the 16-hex-digit prefix of your Windows save folders to enable it.
[ -n "${XGR_XUID:-}" ] && export XGR_XUID

# `ferestre run` stops the service it started when the last title using it
# exits, and counts the titles by a shared lock next to the socket. Hold the
# same lock while this title runs, so a service reused here is not stopped
# under it. Only this shell holds it: the service and the title get the
# descriptor closed, or a service started here would count as a title forever.
SERVICE_DIR=${XODUS_RUNTIME_DIR:-${XDG_RUNTIME_DIR:-/run/user/$(id -u)}}
lock_fd=
if mkdir -p "$SERVICE_DIR" && exec {lock_fd}>>"$SERVICE_DIR/users.lock"; then
    if ! flock -s "$lock_fd"; then
        echo "warning: could not lock $SERVICE_DIR/users.lock; a ferestre launch finishing may stop the service under this title" >&2
    fi
fi
close_lock() { if [ -n "$lock_fd" ]; then "$@" {lock_fd}>&-; else "$@"; fi; }

# The service backs xgameruntime's Xbox-side calls; start it if it isn't up.
# "Up" means something accepts connections on the socket this title will use.
# Any process merely named xodus-service proves nothing: `ferestre run` keeps
# its own under a per-title directory, and another user's is no use here.
SOCK=$SERVICE_DIR/xodus.sock
service_up() {
    python3 -I -c 'import socket, sys
s = socket.socket(socket.AF_UNIX)
s.settimeout(1)
try:
    s.connect(sys.argv[1])
except OSError:
    sys.exit(1)' "$SOCK" 2>/dev/null
}
# `ferestre run` takes the same lock around its own check and start, so the
# two never both start a service on one socket.
start_fd=
exec {start_fd}>>"$SERVICE_DIR/start.lock" && flock -x "$start_fd" || start_fd=
if ! service_up; then
    # A service that was killed leaves its socket behind and a new one cannot
    # bind over it. Only ever remove a socket, never some other file.
    [ -S "$SOCK" ] && rm -f "$SOCK"
    # The subshell closes the lock descriptors and then becomes the service,
    # so nothing else lives on holding them. The log can name accounts and
    # titles: it goes next to the socket, readable by this user only.
    (
        if [ -n "$lock_fd" ]; then exec {lock_fd}>&-; fi
        if [ -n "$start_fd" ]; then exec {start_fd}>&-; fi
        umask 077
        export XODUS_RUNTIME_DIR=$SERVICE_DIR
        log=$SERVICE_DIR/xodus-service.log
        # One written by an older version of this script may be world-readable.
        : >"$log" && chmod 600 "$log"
        exec nohup "$XODUS_CLI_DIR/xodus-service" >"$log" 2>&1
    ) &
    service_pid=$!
    for _ in $(seq 100); do
        service_up && break
        kill -0 "$service_pid" 2>/dev/null || break
        sleep 0.1
    done
    service_up || echo "warning: xodus-service is not listening on $SOCK; see $SERVICE_DIR/xodus-service.log" >&2
fi
if [ -n "$start_fd" ]; then exec {start_fd}>&-; fi

echo "launching $EXE from $GAME_DIR (prefix $PREFIX)..."
close_lock "$XODUS_CLI_DIR/xodus-cli" run -e "$EXE" "$GAME_DIR" "$SHIM"
rc=$?
echo "=== exited rc=$rc $(date) ==="
exit $rc
