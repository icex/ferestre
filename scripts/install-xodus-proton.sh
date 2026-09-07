#!/bin/bash
# Build and install the Xodus Proton fork, re-apply the local patch, and write
# down what the result can do.
#
# Two things make this more than "make install":
#
#  - The build works from its own copy of the Wine tree ($OBJ/src-wine),
#    refreshed from the submodule only when the .wine-source stamp is missing.
#    Editing the submodule alone therefore changes nothing, and the stamps for
#    the stages that consume it have to go too or the install silently ships
#    the previous build.
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

echo ":: invalidating the wine source and build stamps"
rm -f .wine-source .wine-x86_64-build .wine-x86_64-post-build .wine-x86_64-dist

# Dropping the stamps makes the source stage run again; it does not make the
# compiler run again. That stage rsyncs the submodule over $BUILD_DIR/src-wine
# with -a, which preserves each file's modification time, so a source edited
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
# before the rsync copies them. Touching them in src-wine is no good: the
# refresh overwrites those timestamps on the way in.
echo ":: marking patched sources for rebuild"
WINE_SRC=${WINE_SRC:-$XODUS_SRC_DIR/xodus-proton/wine}
if [ -d "$WINE_SRC" ]; then
    # `if`, not `[ -f ] && touch`: under `set -e` a false test as the last
    # command of a loop body ends the script, so a patch naming a file that has
    # since moved would abort the install rather than be skipped.
    touched=0
    touch_series() {
        local dir=$1 p f
        shift
        for p in "$@"; do
            [ -e "$p" ] || continue
            while read -r f; do
                if [ -f "$dir/$f" ]; then
                    touch "$dir/$f"
                    touched=$((touched + 1))
                fi
            done < <(sed -n 's|^+++ b/\(.*\)$|\1|p' "$p")
        done
    }
    touch_series "$WINE_SRC" "$REPO_DIR"/patches/wine/*.patch
    touch_series "$WINE_SRC/dlls/xgameruntime" "$REPO_DIR"/patches/xgameruntime/*.patch
    echo "   $touched files"
else
    echo "   !! no wine tree at $WINE_SRC; set WINE_SRC if the build ships a stale runtime" >&2
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

# Last, because it describes what was just installed. Without it the launcher
# falls back to probing, finds a third of the capabilities, and tells the user
# every title "may not start" -- a build that cannot say what it provides is a
# build the launcher has to be pessimistic about.
echo ":: publishing the capability manifest"
"$REPO_DIR/scripts/write-capabilities.sh" "$TOOL_DIR"
