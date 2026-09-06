#!/bin/sh
# Stand in for a plain `wine` binary, but run the game through Proton.
#
# `xodus-cli run` expects a wine executable and invokes it as `<wine> <exe>`,
# so this adapts that to `proton run <exe>` and sets up the compat environment
# Proton needs. Going through Proton gets DXVK/vkd3d installed into the prefix,
# ntsync, the shader cache and the media stack configured the way they expect,
# instead of hand-copying DLLs and setting GST_PLUGIN_PATH by hand.
#
# The decrypted images arrive as inherited file descriptors named in
# WINE_DLL_FILE_MAP, so nothing in this path may close them: `exec` keeps them,
# and Proton's run_proc() is patched with close_fds=False.

. "$(dirname "$0")/xodus-env.sh"

PROTON_DIR="${PROTON_DIR:-$XODUS_PROTON_DIR}"
[ -x "$PROTON_DIR/proton" ] || { echo "!! no Xodus Proton at '$PROTON_DIR' (set XODUS_PROTON_DIR)" >&2; exit 1; }

export STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$XODUS_GAMES_DIR/gdk-proton}"
export STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-${XODUS_STEAM_DIR:-$HOME/.steam/steam}}"

# Persist compiled pipelines; a UE5 title compiles thousands on first run.
# PROTON_NO_MEDIACONV=1 drops every MEDIACONV_* variable so GStreamer decodes
# the game's own video files instead of Proton's placeholder. Left OFF: removing
# the blank-media fallback as well makes video_conv_state_create() fail outright
# and the game then dies on a null dereference.
export PROTON_NO_MEDIACONV="${PROTON_NO_MEDIACONV:-0}"

# Use the system ffmpeg rather than Steam's.
#
# Steam ships an ffmpeg with no H.264 decoder and downloads that codec on
# demand; Proton puts that directory ahead of everything else on the library
# path. Launched outside Steam the download never happens, so winedmo cannot
# find an H.264 decoder and substitutes MEDIACONV_BLANK_VIDEO_FILE -- the
# colour bars seen in place of the game's loading-screen movies.
export PROTON_PREFER_SYSTEM_FFMPEG="${PROTON_PREFER_SYSTEM_FFMPEG:-1}"

# Play the game's own video rather than Proton's placeholder by demoting the
# converter elements by rank, so decodebin hands files to the real demuxer and
# decoder. REAL_VIDEO=0 restores the placeholder behaviour.
if [ "${REAL_VIDEO:-1}" != "0" ]; then
    export GST_PLUGIN_FEATURE_RANK="protonvideoconverter:NONE,protonaudioconverter:NONE,protonaudioconverterbin:NONE,protondemuxer:NONE"
fi

export VKD3D_SHADER_CACHE_PATH="${VKD3D_SHADER_CACHE_PATH:-$XODUS_GAMES_DIR/shadercache}"
export DXVK_STATE_CACHE_PATH="${DXVK_STATE_CACHE_PATH:-$XODUS_GAMES_DIR/shadercache}"
mkdir -p "$STEAM_COMPAT_DATA_PATH" "$VKD3D_SHADER_CACHE_PATH"

exec "$PROTON_DIR/proton" run "$@"
