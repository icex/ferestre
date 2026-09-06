#!/bin/bash
# Minecraft for Windows (Bedrock) -- Xbox GDK + UWP hybrid from the Microsoft
# Store, decrypted with `scripts/get-game.sh 9NBLGGH2JHXJ` into bedrock/game.
# The main executable is encrypted on disk, so it launches through
# `xodus-cli run` + Proton exactly like the GDK titles.
. "$(dirname "$0")/xodus-env.sh"
exec "$(dirname "$0")/launch-gdk.sh" "${BEDROCK_DIR:-$XODUS_GAMES_DIR/bedrock/game}" \
    'Minecraft.Windows.exe' \
    "${BEDROCK_PREFIX:-$XODUS_GAMES_DIR/bedrock-proton}" "$XODUS_GAMES_DIR/bedrock-launch.log"
