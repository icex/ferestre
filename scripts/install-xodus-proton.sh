#!/bin/bash
# Build and install the Xodus Proton fork, re-apply the local patch, and write
# down what the result can do.
#
# Two things make this more than "make install":
#
#  - The build works from its own copies of the Wine and vkd3d-proton trees
#    ($BUILD_DIR/src-wine, $BUILD_DIR/src-vkd3d-proton), refreshed from the
#    submodules only when their source stamps are missing. Editing a submodule
#    alone therefore changes nothing, and the stamps for the stages that
#    consume it have to go too or the install silently ships the previous
#    build.
#  - `make install` rsyncs dist/ over the compatibility tool with --delete,
#    which replaces the `proton` script -- including the close_fds=False change
#    that keeps the decrypted-image memfds alive. Without it every GDK title
#    fails to launch with exit code 1, so the patch goes back on afterwards.

set -eu
. "$(dirname "$0")/xodus-env.sh"

BUILD_DIR=${BUILD_DIR:-$XODUS_BUILD_DIR}
REPO_DIR=${REPO_DIR:-$XODUS_REPO_DIR}
# The tool may not exist yet on a first install; default to the Steam we found.
TOOL_DIR=${TOOL_DIR:-${XODUS_PROTON_DIR:-${XODUS_STEAM_DIR:-$HOME/.steam/steam}/compatibilitytools.d/xodus}}
PATCH=$REPO_DIR/patches/proton/0001-keep-inherited-fds-mediaconv-and-system-ffmpeg.patch
J=${J:-16}

cd "$BUILD_DIR"

echo ":: invalidating Wine and vkd3d-proton source/build stamps"
rm -f .wine-source .wine-x86_64-build .wine-x86_64-post-build .wine-x86_64-dist
# Both architectures of vkd3d-proton rebuild on every install as a result. That
# costs minutes, and is the price of never shipping a d3d12core.dll without
# patches/vkd3d-proton.
rm -f .vkd3d-proton-source .vkd3d-proton-post-source
for arch in i386 x86_64; do
    rm -f ".vkd3d-proton-$arch-build" ".vkd3d-proton-$arch-post-build" ".vkd3d-proton-$arch-dist"
done

# Dropping the stamps makes the source stage run again; it does not make the
# compiler run again. That stage rsyncs each submodule over its copy in
# $BUILD_DIR (src-wine, src-vkd3d-proton) with -a, which preserves each file's modification time, so a source edited
# hours ago arrives older than the object built from the previous version and
# make has nothing to do. The build then "succeeds" in seconds and installs
# exactly what was already there.
#
# It is a quiet failure and an expensive one: the runtime here sat four days
# and several patches behind while reporting success, and the only visible sign
# was capabilities.json dropping from 19 to 15 -- which is easy to read as a
# verification problem rather than as the build not having happened.
#
# So give the files the series touches a current timestamp, in the submodule,
# before the rsync copies them. Touching them in src-wine or src-vkd3d-proton is
# no good: the refresh overwrites those timestamps on the way in.
echo ":: marking patched sources for rebuild"
WINE_SRC=${WINE_SRC:-${XODUS_SRC_DIR:-$HOME/src}/xodus-proton/wine}
# A sibling of wine in the Proton tree, like every other Proton submodule.
VKD3D_SRC=${VKD3D_SRC:-$(dirname "$WINE_SRC")/vkd3d-proton}

# Touch every file a series patches, and say how many, per series: a count of
# zero for a series that has patches is the stale build announcing itself.
#
# `if`, not `[ -f ] && touch`: under `set -e` a false test as the last command
# of a loop body ends the script, so a patch naming a file that has since moved
# would abort the install rather than be skipped.
touch_series() {
    local label=$1 dir=$2 p f n=0
    shift 2
    for p in "$@"; do
        [ -e "$p" ] || continue
        while read -r f; do
            if [ -f "$dir/$f" ]; then
                touch "$dir/$f"
                n=$((n + 1))
            fi
        done < <(sed -n 's|^+++ b/\(.*\)$|\1|p' "$p")
    done
    echo "   $label: $n files"
}
if [ -d "$WINE_SRC" ]; then
    touch_series wine "$WINE_SRC" "$REPO_DIR"/patches/wine/*.patch
    touch_series xgameruntime "$WINE_SRC/dlls/xgameruntime" "$REPO_DIR"/patches/xgameruntime/*.patch
else
    echo "   !! no wine tree at $WINE_SRC; set WINE_SRC if the build ships a stale runtime" >&2
fi
if [ -d "$VKD3D_SRC" ]; then
    touch_series vkd3d-proton "$VKD3D_SRC" "$REPO_DIR"/patches/vkd3d-proton/*.patch
else
    echo "   !! no vkd3d-proton tree at $VKD3D_SRC; set VKD3D_SRC if the build ships a stale d3d12core.dll" >&2
fi

echo ":: building and installing (log: $BUILD_DIR/install.log)"
# The exit status of a pipeline is the last command's, so a failed build would
# otherwise be reported as success and the previous build left installed.
set -o pipefail
echo "make install -j$J" | newgrp docker 2>&1 | tee install.log | tail -3
set +o pipefail

echo ":: re-applying the proton patch"
cd "$TOOL_DIR"
if grep -q "close_fds=False" proton; then
    echo "   already patched"
else
    patch -p1 < "$PATCH"
    echo "   ok"
fi
grep -q "close_fds=False" proton || { echo "!! proton patch missing, GDK titles will not launch" >&2; exit 1; }

D=$TOOL_DIR/files/lib/wine/x86_64-windows/xgameruntime.dll
if strings "$D" | grep -q "unrecognised index"; then
    echo ":: xgameruntime carries the XGameSave implementation"
else
    echo "!! xgameruntime looks stale -- saves will not work" >&2
    exit 1
fi

# The same check for the vkd3d-proton series: its option name is compiled into
# d3d12core.dll's configuration table, so its absence means the build did not
# include patches/vkd3d-proton.
V=$TOOL_DIR/files/lib/wine/vkd3d-proton/x86_64-windows/d3d12core.dll
if strings "$V" | grep -q "retain_recording_allocators"; then
    echo ":: vkd3d-proton carries retain_recording_allocators"
else
    echo "!! vkd3d-proton looks stale -- engines that release a recording allocator will crash" >&2
    exit 1
fi

# Last, because it describes what was just installed. Without it the launcher
# falls back to probing, finds a third of the capabilities, and tells the user
# every title "may not start" -- a build that cannot say what it provides is a
# build the launcher has to be pessimistic about.
echo ":: publishing the capability manifest"
"$REPO_DIR/scripts/write-capabilities.sh" "$TOOL_DIR"
