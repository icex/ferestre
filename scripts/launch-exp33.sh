#!/bin/bash
# Clair Obscur: Expedition 33 (Xbox GDK build). Suitable as a Steam shortcut
# target -- keep the path stable, the shortcut's appid is derived from it.
. "$(dirname "$0")/xodus-env.sh"
exec "$(dirname "$0")/launch-gdk.sh" "${EXP33_DIR:-$XODUS_GAMES_DIR/exp33}" \
    'Sandfall\Binaries\WinGDK\SandFall-WinGDK-Shipping.exe' \
    "${EXP33_PREFIX:-$XODUS_GAMES_DIR/exp33-proton}" "$XODUS_GAMES_DIR/exp33-launch.log"
