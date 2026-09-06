#!/bin/sh
# Run the XGameSave harness against a freshly built xgameruntime.dll (from
# WINEDLLPATH) in a bare Wine prefix, with saves rooted in a scratch dir.
#
#   XGSTEST_DIR=<scratch dir holding dll/xgstest.exe and dll/xgameruntime.dll>
. "$(dirname "$0")/xodus-env.sh"
T=${XGSTEST_DIR:-$XODUS_GAMES_DIR/wgs-test}
export WINEPREFIX="${WINEPREFIX:-$XODUS_GAMES_DIR/exp33-prefix}"
export WINEDLLPATH="$T/dll"
# Z: is Wine's view of /, so a unix path becomes Z:\path\with\backslashes
export XGR_WGS_ROOT="Z:$(printf '%s' "$T" | sed 's#/#\\#g')"
[ -n "${XGR_XUID:-}" ] && export XGR_XUID
export WINEDEBUG=${WINEDEBUG:-+gdkc}
exec "${WINE_BIN:-$XODUS_PROTON_DIR/files/bin/wine}" "$T/dll/xgstest.exe" "$@"
