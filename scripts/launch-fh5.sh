#!/bin/bash
# Forza Horizon 5 (Microsoft Store build). Drives with a restored Windows
# profile on the runtime capabilities listed in titles/9NNX1VVR3KNQ.toml.
# Forza Online reports its server is unavailable; see docs/RECIPES.md.
. "$(dirname "$0")/xodus-env.sh"
exec "$(dirname "$0")/launch-gdk.sh" "${FH5_DIR:-$XODUS_GAMES_DIR/fh5}" \
    'ForzaHorizon5.exe' \
    "${FH5_PREFIX:-$XODUS_GAMES_DIR/fh5-proton}" "$XODUS_GAMES_DIR/fh5-launch.log"
