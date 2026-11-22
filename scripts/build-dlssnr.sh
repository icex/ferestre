#!/usr/bin/env bash
# Build the DLSSNR-AMD Linux package Ferestre hosts, from the commit pinned in
# sources.lock.json (`packages.dlssnr-amd`).
#
#   scripts/build-dlssnr.sh <output dir>
#
# Writes into <output dir>, which must be new or empty, exactly:
#   ferestre-dlssnr-<version>-x86_64.tar.gz
#   ferestre-dlssnr-<version>-i686.tar.gz
#   SOURCES.txt
#   SHA256SUMS            (last; lists the three files above)
# Every file in the output dir is in SHA256SUMS, because
# packaging/sign-release.sh refuses to sign a release holding an unlisted file.
#
# The build runs in the image upstream's own release job uses (ubuntu:24.04,
# pinned by digest below) with the same packages and flags, i686 included.
# ${FERESTRE_CONTAINER:-docker} runs it; podman works too. Needs the network
# (upstream's fetch_deps.sh downloads its pinned dependencies) and takes tens
# of minutes. Nothing is bind-mounted: the script goes in on stdin and the
# packages come out with `cp`, so a remote engine (a Docker context over ssh)
# works, and every file the host gets is owned by the host user.
#
# Upstream packs its tarball without sorting or fixed times; each one is
# repacked here with sorted names, root ownership and the pinned commit's time,
# without a top-level directory (like upstream's). The compiled PE files are not
# guaranteed byte-identical between builds, so the trust anchor is the SHA-256
# of the published asset pinned in ferestre-core, not a rebuild.
#
# No NVIDIA file is ever packaged: a tarball containing a model is refused.
set -euo pipefail

# Bumped deliberately, never followed: the index digest of ubuntu:24.04 when
# this was last reviewed (2026-11-22).
IMAGE=docker.io/library/ubuntu:24.04@sha256:9c7f200c07e3d4a03e7249d2c16bf66a51120fb876aa2242cf31451412543289
ARCHES=(x86_64 i686)
# Upstream's own check looks only for the last two names.
MODEL_NAMES=(CC_SILVER_AARDWOLD.bin CC_SMALL_FP8.bin dlssnr.bin dlssnr-small.bin)
PACKAGES=(ca-certificates git curl cmake ninja-build mingw-w64 g++-multilib python3 p7zip-full openssl osslsigncode)
OPTISCALER_SOURCE='OptiScaler (GPL-3.0) is shipped unmodified from its nightly-20261030 release; source: https://github.com/optiscaler/OptiScaler at e7e3872d410f704479d94d318d68d163b6b58947'

die() {
    echo "build-dlssnr: $*" >&2
    exit 1
}

# refuse_models <tarball>: fail when any entry is a model file.
refuse_models() {
    local listing name model
    listing=$(tar -tzf "$1") || die "$1 is not a readable tarball"
    while IFS= read -r name; do
        for model in "${MODEL_NAMES[@]}"; do
            if [[ ${name##*/} == "$model" ]]; then
                die "$1 contains $name; no NVIDIA model may be packaged"
            fi
        done
    done <<<"$listing"
}

# repack <source tarball> <destination> <unix time>: the same files, packed so
# that equal contents give an equal tarball.
repack() {
    local src=$1 dest=$2 mtime=$3 dir
    dir=$(mktemp -d "${TMPDIR:-/tmp}/ferestre-dlssnr-repack.XXXXXX")
    tar -xzf "$src" -C "$dir" --no-same-owner
    tar --format=gnu --sort=name --owner=0 --group=0 --numeric-owner \
        --mode='u+rwX,go+rX,go-w' --mtime="@$mtime" \
        -C "$dir" -cf - . | gzip -n -9 >"$dest"
    rm -rf -- "$dir"
}

# Inside the container: install upstream's build packages, fetch exactly the
# pinned commit, run upstream's release build, and leave the packages, the
# commit and its time in /work/result for the host to copy out.
inside() {
    local src=/work/src result=/work/result arch
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install -y --no-install-recommends "${PACKAGES[@]}"
    mkdir -p -- "$result"
    git init --quiet "$src"
    git -C "$src" fetch --quiet --depth 1 "$DLSSNR_REPOSITORY" "$DLSSNR_COMMIT"
    git -C "$src" checkout --quiet --detach FETCH_HEAD
    [[ $(git -C "$src" rev-parse HEAD) == "$DLSSNR_COMMIT" ]] ||
        die "fetched $(git -C "$src" rev-parse HEAD), not the pinned $DLSSNR_COMMIT"
    cd -- "$src"
    bash fetch_deps.sh
    bash linux/build/build_vulkan_loader.sh
    for arch in "${ARCHES[@]}"; do
        NR_VERSION=$DLSSNR_VERSION NR_ARCH=$arch NR_SIGN=1 bash linux/build/build_package.sh
        cp -- "linux/package/DLSSNR-AMD-Vulkan-Linux-$DLSSNR_VERSION-$arch.tar.gz" "$result/"
    done
    git rev-parse HEAD >"$result/commit"
    git show -s --format=%ct HEAD >"$result/mtime"
}

main() {
    if [[ ${1:-} == --inside && $# -eq 1 ]]; then
        inside
        return
    fi
    [[ $# -eq 1 && -n $1 && $1 != -* ]] || die "usage: scripts/build-dlssnr.sh <output dir>"

    local root pins repository commit version out work engine container arch built mtime
    root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
    pins=$root/scripts/source-pins.py
    repository=$(python3 "$pins" package dlssnr-amd repository)
    commit=$(python3 "$pins" package dlssnr-amd commit)
    version=$(python3 "$pins" package dlssnr-amd version)

    # Nothing unlisted may ever sit beside the sums.
    if [[ -e $1 && ! -d $1 ]]; then
        die "$1 is not a directory"
    fi
    if [[ -d $1 && -n $(ls -A -- "$1") ]]; then
        die "$1 is not empty"
    fi
    mkdir -p -- "$1"
    out=$(cd -- "$1" && pwd)

    work=$(mktemp -d "${TMPDIR:-/tmp}/ferestre-dlssnr.XXXXXX")
    engine=${FERESTRE_CONTAINER:-docker}
    container=ferestre-dlssnr-$$-$RANDOM
    # shellcheck disable=SC2064 # expand now: these are locals
    trap "rm -rf -- '$work'; '$engine' rm -f '$container' >/dev/null 2>&1 || true" EXIT
    echo ":: DLSSNR-AMD $version ($commit) in $IMAGE"
    "$engine" run -i --name "$container" \
        -e DLSSNR_REPOSITORY="$repository" -e DLSSNR_COMMIT="$commit" -e DLSSNR_VERSION="$version" \
        "$IMAGE" bash -c 'cat >/build-dlssnr.sh && bash /build-dlssnr.sh --inside' \
        <"$root/scripts/build-dlssnr.sh"
    "$engine" cp "$container:/work/result" "$work/result"

    [[ $(cat -- "$work/result/commit") == "$commit" ]] || die "the build did not use the pinned commit"
    mtime=$(cat -- "$work/result/mtime")
    [[ $mtime =~ ^[0-9]+$ ]] || die "no commit time from the build"
    # Both are checked before either is written, so a refused build leaves
    # nothing in the output dir.
    for arch in "${ARCHES[@]}"; do
        built=$work/result/DLSSNR-AMD-Vulkan-Linux-$version-$arch.tar.gz
        [[ -f $built ]] || die "upstream's build wrote no $built"
        refuse_models "$built"
    done
    for arch in "${ARCHES[@]}"; do
        repack "$work/result/DLSSNR-AMD-Vulkan-Linux-$version-$arch.tar.gz" \
            "$out/ferestre-dlssnr-$version-$arch.tar.gz" "$mtime"
    done

    {
        echo "DLSSNR-AMD $version for Ferestre"
        echo "Built from $repository at $commit"
        echo "in $IMAGE"
        echo "$OPTISCALER_SOURCE"
        echo "No NVIDIA file is included: the model is extracted at install time from the person's own nvngx_dlssnr.dll."
    } >"$out/SOURCES.txt"

    # Last write into the output dir: every file in it, SOURCES.txt included.
    (cd "$out" && sha256sum -- * > SHA256SUMS)
    cat -- "$out/SHA256SUMS"
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
    main "$@"
fi
