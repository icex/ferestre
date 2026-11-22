#!/usr/bin/env bash
# Point a Cargo target directory at a build directory that outlives the job.
#
#   ci-target-dir.sh <target path in the checkout> <name> <cap in GiB>
#   ci-target-dir.sh --env <name> <cap in GiB>
#
# Self-hosted runners keep their home directory between jobs, but checkout's
# clean step deletes target/ every time, and the GitHub cache cannot help them:
# each runner is its own user, so its CARGO_HOME, and with it every cache key
# and cache version, is its own, and none of them has ever hit. So the CI jobs
# build into ~/.cache/ferestre-ci/<name> instead, through a symlink at the
# usual path, which the tests that look for target/debug/... still find. Only
# what changed is rebuilt.
#
# The symlink only ever goes at the top of the checkout, where checkout's
# clean step removes it before the next job, a release included. Inside a
# submodule nothing removes it, so for third_party/xodus-cli use --env, which
# sets CARGO_TARGET_DIR for the rest of this job only.
#
# Used by .github/workflows/rust.yml on self-hosted runners only. The release
# and runtime workflows must never use it: a signed release builds in a clean
# checkout from main's sources, and tests/test_workflows.py checks that.
#
# There is no size limit in Cargo, and stale dependency versions pile up, so a
# directory over the cap, or on a disk with little room left, is emptied before
# the job and rebuilt from scratch.
set -euo pipefail

if [ "$#" -ne 3 ]; then
    printf 'usage: %s <target path>|--env <name> <cap GiB>\n' "$0" >&2
    exit 2
fi
link=$1 name=$2 cap_gib=$3
case "$name" in
    ''|*/*|.*) printf 'bad name: %s\n' "$name" >&2; exit 2 ;;
esac

root="${XDG_CACHE_HOME:-$HOME/.cache}/ferestre-ci"
dir="$root/$name"
mkdir -p "$dir"

# The runners share one host and one disk: below this much free space, give
# the directory back too, whatever its size.
min_free_gib=${FERESTRE_CI_MIN_FREE_GIB:-30}
used_kib=$(du -sk "$dir" | cut -f1)
free_kib=$(df -Pk "$root" | awk 'NR == 2 { print $4 }')
if [ "$used_kib" -gt $((cap_gib * 1024 * 1024)) ]; then
    echo "::notice::$dir holds $((used_kib / 1024)) MiB, over the ${cap_gib} GiB cap; starting it afresh"
    rm -rf "$dir"
    mkdir -p "$dir"
elif [ "$free_kib" -lt $((min_free_gib * 1024 * 1024)) ]; then
    echo "::notice::only $((free_kib / 1024)) MiB free under $root; emptying $dir"
    rm -rf "$dir"
    mkdir -p "$dir"
fi

if [ "$link" = --env ]; then
    echo "CARGO_TARGET_DIR=$dir" >> "${GITHUB_ENV:?}"
    echo "CARGO_TARGET_DIR=$dir ($(du -sh "$dir" | cut -f1))"
else
    case "$link" in
        */*) printf 'the link goes at the top of the checkout: %s\n' "$link" >&2; exit 2 ;;
    esac
    # A real directory left at the link path (an older job, or a build run
    # before this step) is replaced; a symlink is only the link.
    rm -rf "$link"
    ln -s "$dir" "$link"
    echo "$link -> $dir ($(du -sh "$dir" | cut -f1))"
fi
df -h "$root" | tail -n 1
