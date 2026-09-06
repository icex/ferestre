#!/bin/bash
# Forza Horizon 5 (Microsoft Store build). Downloads and decrypts fine but does
# NOT currently run: it fails inside its own in-binary code protection before
# any runtime API is reached (see README, "Forza Horizon 5"). Kept so the
# launch path is ready if that ever changes.
. "$(dirname "$0")/xodus-env.sh"
exec "$(dirname "$0")/launch-gdk.sh" "${FH5_DIR:-$XODUS_GAMES_DIR/fh5}" \
    'ForzaHorizon5.exe' \
    "${FH5_PREFIX:-$XODUS_GAMES_DIR/fh5-proton}" "$XODUS_GAMES_DIR/fh5-launch.log"
