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
# LD_PRELOAD, LD_LIBRARY_PATH points into the Steam runtime, and SteamGameId /
# STEAM_COMPAT_* describe *Steam's* idea of the app. All of that collides with
# the Proton instance we start ourselves, so drop it and set our own.
unset LD_PRELOAD LD_LIBRARY_PATH
unset STEAM_COMPAT_DATA_PATH STEAM_COMPAT_CLIENT_INSTALL_PATH
unset STEAM_COMPAT_TRANSCODED_MEDIA_PATH STEAM_COMPAT_MEDIA_PATH
unset SteamAppId SteamGameId SteamAppUser SteamClientLaunch SteamEnv
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

# The service backs xgameruntime's Xbox-side calls; start it if it isn't up.
if ! pgrep -x xodus-service >/dev/null; then
    rm -f "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/xodus.sock"
    nohup "$XODUS_CLI_DIR/xodus-service" >"${TMPDIR:-/tmp}/xodus-service.log" 2>&1 &
    sleep 2
fi

echo "launching $EXE from $GAME_DIR (prefix $PREFIX)..."
"$XODUS_CLI_DIR/xodus-cli" run -e "$EXE" "$GAME_DIR" "$SHIM"
rc=$?
echo "=== exited rc=$rc $(date) ==="
exit $rc
