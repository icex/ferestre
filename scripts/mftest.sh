#!/bin/sh
# Decode a video through Wine's Media Foundation using the Xodus Proton build,
# with the same converter demotion the game launcher applies.
#
#   mftest.sh <video file>        (WINEPREFIX defaults to the exp33 prefix)
. "$(dirname "$0")/xodus-env.sh"
export WINEPREFIX="${WINEPREFIX:-$XODUS_GAMES_DIR/exp33-prefix}"
export GST_PLUGIN_FEATURE_RANK="${GST_PLUGIN_FEATURE_RANK:-protonvideoconverter:NONE,protonaudioconverter:NONE,protonaudioconverterbin:NONE,protondemuxer:NONE}"
P=$XODUS_PROTON_DIR/files
[ -x "$P/bin/wine" ] || { echo "!! no Xodus Proton wine at $P (set XODUS_PROTON_DIR)" >&2; exit 1; }
export GST_PLUGIN_PATH="$P/lib/x86_64-linux-gnu/gstreamer-1.0"
export LD_LIBRARY_PATH="$P/lib/x86_64-linux-gnu:$P/lib64${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export MEDIACONV_BLANK_VIDEO_FILE="$P/share/media/blank.mkv"
export MEDIACONV_BLANK_AUDIO_FILE="$P/share/media/blank.ptna"
exec "$P/bin/wine" "$XODUS_REPO_DIR/tools/mftest.exe" "$@"
