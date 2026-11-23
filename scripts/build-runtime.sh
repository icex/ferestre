#!/bin/bash
# Build the runtime's dist tree from the locked Proton fork, the way CI does.
#
#   scripts/build-runtime.sh <work dir> [proton ref] [--until clone|configure|build]
#
# Clones Proton (with Wine, xgameruntime and vkd3d-proton as its recursive
# submodules) into <work dir>/proton, refuses a source graph that differs from
# sources.lock.json, configures with ccache, runs `make dist`, and packs the
# result as <work dir>/out/dist.tar.zst with the SDK image name beside it in
# <work dir>/out/sdk-image. The version, the checks against the built tree,
# the capability manifest and the .tar.xz are the `package` job's, so that a
# tag that promotes a main build runs the same checks on the same bytes as one
# that built them.
#
# Everything that decides what `make dist` produces is in this file or in the
# Proton commit, and scripts/runtime-inputs.py hashes this file's blob id
# together with the four pins. Change a flag here and the digest changes, so
# no build made with the old flags is ever promoted.
#
# The proton ref defaults to the pin. Another ref (a manual run's) skips the
# lock check, and its output is never saved or promoted (runtime.yml).
#
# ccache: Proton's Makefile mounts $CCACHE_DIR into its build container and
# passes every CCACHE_* variable through. Paths inside the build tree are
# absolute, so a cache only hits from a build at the same path: CI keeps the
# work directory at the same place on every hosted runner.
#
# Needs git, docker (Proton builds inside its SDK image), python3 and zstd.
# --until stops after a stage, for trying the script out locally.
set -euo pipefail

work=${1:?usage: build-runtime.sh <work dir> [proton ref] [--until clone|configure|build]}
shift
ref=
until=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --until) until=${2:?}; shift 2 ;;
        -*) echo "unknown option $1" >&2; exit 2 ;;
        *) ref=$1; shift ;;
    esac
done
case "$until" in ''|clone|configure|build) ;; *) echo "bad --until $until" >&2; exit 2 ;; esac

here=$(cd "$(dirname "$0")/.." && pwd)
pins() { python3 "$here/scripts/source-pins.py" "$@"; }
pinned=$(pins get proton commit)
ref=${ref:-$pinned}
mkdir -p "$work"
work=$(cd "$work" && pwd)
stamp() { printf '::  %s %s\n' "$(date -u +%H:%M:%S)" "$*"; }

stamp "clone Proton $ref"
url=$(pins get proton repository)
rm -rf "$work/proton"
# Git LFS off: the only LFS files in the graph are OpenXR's PDFs and models
# and GStreamer's test media, none of which the build reads. Pointer files
# are checked out instead of downloading them, with or without git-lfs
# installed. `git -c` reaches the submodule clones too.
nolfs=(-c filter.lfs.smudge= -c filter.lfs.process= -c filter.lfs.required=false)
git "${nolfs[@]}" clone --quiet "$url" "$work/proton"
git -C "$work/proton" checkout --quiet "$ref"
# The complete locked source graph, without local changes.
git "${nolfs[@]}" -C "$work/proton" submodule update --init --recursive --depth 1 --jobs 8
echo ":: proton   $(git -C "$work/proton" rev-parse HEAD)"
echo ":: wine     $(git -C "$work/proton/wine" rev-parse HEAD)"
echo ":: xgr      $(git -C "$work/proton/wine/dlls/xgameruntime" rev-parse HEAD)"
echo ":: vkd3d    $(git -C "$work/proton/vkd3d-proton" rev-parse HEAD)"

# Refuse a source graph that differs from the lock before compiling anything.
#
# Wine's configure turns on -Werror when it finds a .git, so this clone
# compiles more strictly than a local build tree made from a copy;
# scripts/check-werror.sh reproduces that in about a minute.
if [ "$ref" = "$pinned" ]; then
    pins verify proton --directory "$work/proton"
    pins verify wine --directory "$work/proton/wine"
    pins verify xgameruntime --directory "$work/proton/wine/dlls/xgameruntime"
    pins verify vkd3d-proton --directory "$work/proton/vkd3d-proton"
else
    echo "::warning::building Proton $ref, not the pinned $pinned: lock not checked"
fi

# Ferestre's own fixes to the submodules, on the checked source graph.
for dir in "$here"/packaging/runtime-patches/*/; do
    sub=$(basename "$dir")
    for patch in "$dir"*.patch; do
        [ -e "$patch" ] || continue
        echo ":: patch    $sub/$(basename "$patch")"
        git -C "$work/proton/$sub" apply "$patch"
    done
done
[ "$until" = clone ] && exit 0

stamp configure
mkdir -p "$work/proton/build"
(cd "$work/proton/build" && ../configure.sh --enable-ccache --build-name=ferestre)
[ "$until" = configure ] && exit 0

stamp "make dist on $(nproc) cores"
make -C "$work/proton/build" -j"$(nproc)" dist
df -h "$work"
[ "$until" = build ] && exit 0

stamp pack
mkdir -p "$work/out"
make --silent --no-print-directory -C "$work/proton/build" get-steamrt-image | tail -n 1 > "$work/out/sdk-image"
test -s "$work/out/sdk-image" || { echo "!! no SDK image name in the build tree" >&2; exit 1; }
# Fixed order and owners, so the same tree packs to the same bytes.
tar --sort=name --owner=0 --group=0 --numeric-owner -C "$work/proton/build" -cf - dist |
    zstd -q -T0 -6 -o "$work/out/dist.tar.zst"
ls -l "$work/out"
stamp finished
