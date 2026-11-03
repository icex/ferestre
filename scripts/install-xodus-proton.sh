#!/bin/bash
# Build and install the locked Proton fork, then measure its capabilities.
# The build copies sources using preserved timestamps. Invalidate its source
# and object stamps and touch the fork's modified files so a refreshed checkout
# cannot silently install objects compiled from an older commit.

set -eu
. "$(dirname "$0")/xodus-env.sh"

BUILD_DIR=${BUILD_DIR:-$XODUS_BUILD_DIR}
REPO_DIR=${REPO_DIR:-$XODUS_REPO_DIR}
J=${J:-16}

cd "$BUILD_DIR"

# Validate the tree this Makefile will actually copy, before changing stamps.
PROTON_SRC=$(sed -n 's/^SRCDIR[[:space:]]*:=[[:space:]]*//p' Makefile | tail -n 1)
[ -n "$PROTON_SRC" ] && [ -d "$PROTON_SRC" ] || { echo "!! configured Proton source is missing" >&2; exit 1; }
WINE_SRC=$PROTON_SRC/wine
VKD3D_SRC=$PROTON_SRC/vkd3d-proton
for component in proton wine xgameruntime vkd3d-proton; do
    case $component in
        proton) source_dir=$PROTON_SRC ;;
        wine) source_dir=$WINE_SRC ;;
        xgameruntime) source_dir=$WINE_SRC/dlls/xgameruntime ;;
        vkd3d-proton) source_dir=$VKD3D_SRC ;;
    esac
    python3 "$REPO_DIR/scripts/source-pins.py" verify "$component" --directory "$source_dir"
done

echo ":: invalidating Wine and vkd3d-proton source/build stamps"
rm -f .wine-source .wine-x86_64-build .wine-x86_64-post-build .wine-x86_64-dist
# Both architectures of vkd3d-proton rebuild on every install as a result. That
# costs minutes, and is the price of never shipping a d3d12core.dll without
# the locked allocator-lifetime fix.
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
# and several source changes behind while reporting success, and the only visible sign
# was capabilities.json dropping from 19 to 15 -- which is easy to read as a
# verification problem rather than as the build not having happened.
#
# So give the files the fork changes a current timestamp, in the submodule,
# before the rsync copies them. Touching them in src-wine or src-vkd3d-proton is
# no good: the refresh overwrites those timestamps on the way in.
echo ":: marking fork sources for rebuild"
mark_sources() {
    local label=$1 dir=$2 f n=0
    if [ ! -d "$dir" ]; then
        echo "   no $label source checkout at $dir" >&2
        return
    fi
    while IFS= read -r f; do
        if [ -f "$dir/$f" ]; then touch "$dir/$f"; n=$((n + 1)); fi
    done < <(python3 "$REPO_DIR/scripts/source-pins.py" modified "$label")
    echo "   $label: $n files"
}
mark_sources wine "$WINE_SRC"
mark_sources xgameruntime "$WINE_SRC/dlls/xgameruntime"
mark_sources vkd3d-proton "$VKD3D_SRC"

# The build runs in containers. `newgrp docker` is only for a session that
# predates its docker group membership; it does nothing for podman (there is no
# such group to switch to) or for a user who can already reach the engine, and
# fails outright on a host without the group.
ENGINE=$(sed -n 's/^CONTAINER_ENGINE[[:space:]]*:=[[:space:]]*//p' Makefile | tail -n 1)
ENGINE=${ENGINE:-docker}
in_docker_session() { case " $(id -nG) " in *" docker "*) return 0 ;; esac; return 1; }
build() {
    if [ "$ENGINE" = docker ] && ! docker info >/dev/null 2>&1 \
        && getent group docker >/dev/null && ! in_docker_session; then
        echo "make install -j$J" | newgrp docker
    else
        make install -j"$J"
    fi
}

echo ":: building and installing with $ENGINE (log: $BUILD_DIR/install.log)"
# The exit status of a pipeline is the last command's, so a failed build would
# otherwise be reported as success and the previous build left installed.
set -o pipefail
build 2>&1 | tee install.log | tail -3
set +o pipefail

# Check the tree `make install` just wrote, which it names in its
# log, not XODUS_PROTON_DIR: that may well point at a packaged, read-only copy.
# TOOL_DIR still overrides it, for an install moved somewhere else by hand.
INSTALLED=$(sed -n 's/^Installed Proton to //p' install.log | tail -n 1)
if [ -z "$INSTALLED" ]; then
    NAME=$(sed -n 's/^BUILD_NAME[[:space:]]*:=[[:space:]]*//p' Makefile | tail -n 1)
    INSTALLED=$HOME/.steam/root/compatibilitytools.d/${NAME:-xodus}
fi
TOOL_DIR=${TOOL_DIR:-$INSTALLED}

echo ":: verifying the installed fork script"
cd "$TOOL_DIR"
grep -q "close_fds=False" proton || { echo "!! fork script lacks inherited-fd support; GDK titles will not launch" >&2; exit 1; }

D=$TOOL_DIR/files/lib/wine/x86_64-windows/xgameruntime.dll
if strings "$D" | grep -q "unrecognised index"; then
    echo ":: xgameruntime carries the XGameSave implementation"
else
    echo "!! xgameruntime looks stale -- saves will not work" >&2
    exit 1
fi

# The same check for the vkd3d-proton fork: its option name is compiled into
# d3d12core.dll's configuration table, so its absence means the build did not
# include the locked allocator-lifetime fix.
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
