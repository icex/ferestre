#!/bin/bash
# Check that the patch series still applies, and says what it claims to say.
#
# This is the cheap half of testing the runtime. Building it takes an hour and a
# 26 GB tree; checking that its patches still apply takes a minute, and it is
# the check that has actually caught things:
#
#   - a client patch exported from a dirty tree carried the previous patch's
#     changes and could not apply after it (twice, on two different patches);
#   - a rewritten history invalidated every URL the AUR package fetched them by.
#
# What it does NOT do is prove the result builds or runs. That is
# `tests/run-*.sh`, which need the built tree.
#
#   scripts/check-patches.sh [--wine-repo DIR] [--client-repo DIR]
#
# Each series is checked against a pristine checkout of the commit it targets,
# in a throwaway worktree, so a tree that already has the changes applied cannot
# make a broken patch look fine.

set -euo pipefail

REPO_DIR=$(cd "$(dirname "$0")/.." && pwd)
WINE_REPO=${WINE_REPO:-${XODUS_SRC_DIR:-$HOME/src}/xodus-proton/wine}
CLIENT_REPO=${CLIENT_REPO:-${XODUS_SRC_DIR:-$HOME/src}/xodus-cli}
# The upstream commit the client series is written against. Documented in
# packaging/aur/ferestre-client/PKGBUILD, and pinned here so the two agree.
CLIENT_BASE=${CLIENT_BASE:-3e75c9f2d3aad2ea2fdc488d92d0163eb68c1a60}
# Same for the xgameruntime series, and for the same reason. Locally that
# repository is the build tree and its HEAD is the base; on a fresh clone it is
# whatever upstream has moved on to, which is how this check went red for five
# commits while every patch applied perfectly to the tree it was written for.
XGR_BASE=${XGR_BASE:-64aebcabb8c66121eae25d3bf0ace4b582ebb0da}

while [ $# -gt 0 ]; do
    case $1 in
        --wine-repo) WINE_REPO=$2; shift 2 ;;
        --client-repo) CLIENT_REPO=$2; shift 2 ;;
        *) echo "!! unknown argument: $1" >&2; exit 2 ;;
    esac
done

WORK=$(mktemp -d "${TMPDIR:-/tmp}/ferestre-patch-check.XXXXXX")
FAILED=0
cleanup() {
    [ -d "$WORK/wine" ] && git -C "$WINE_REPO" worktree remove --force "$WORK/wine" 2>/dev/null
    [ -d "$WORK/xgr" ] && git -C "$WINE_REPO/dlls/xgameruntime" worktree remove --force "$WORK/xgr" 2>/dev/null
    [ -d "$WORK/client" ] && git -C "$CLIENT_REPO" worktree remove --force "$WORK/client" 2>/dev/null
    rm -rf "$WORK" 2>/dev/null
}
trap cleanup EXIT

say()  { printf '\033[1m:: %s\033[0m\n' "$*"; }
ok()   { printf '   \033[32mok\033[0m   %s\n' "$*"; }
bad()  { printf '   \033[31m!!\033[0m   %s\n' "$*"; FAILED=$((FAILED + 1)); }

# --- the wine series ------------------------------------------------------
#
# Checked against the fork's own HEAD, because that is the tree the runtime is
# built from. The patches live as working-tree edits there, so a pristine
# worktree is the only place the question means anything.
if [ -d "$WINE_REPO/.git" ] || [ -f "$WINE_REPO/.git" ]; then
    say "wine series against $(git -C "$WINE_REPO" rev-parse --short HEAD)"
    git -C "$WINE_REPO" worktree add -q --detach "$WORK/wine" HEAD
    for p in "$REPO_DIR"/patches/wine/*.patch; do
        name=$(basename "$p")
        if git -C "$WORK/wine" apply --check "$p" 2>/dev/null; then
            git -C "$WORK/wine" apply "$p"
            ok "$name"
        else
            bad "$name does not apply"
        fi
    done
    # The series is not just individually applicable: together it must
    # reproduce the tree the runtime is actually built from. A patch that
    # applies but has drifted from the build would pass every other check --
    # this is what caught a rename leaking into a third-party project's source.
    #
    # Only meaningful where the repository IS a build tree, i.e. has the patches
    # applied as working-tree edits. A pristine clone (CI) has nothing to
    # reproduce, and comparing against it would fail for the wrong reason.
    # dlls/xgameruntime is excluded from the comparison because it is a
    # submodule with its own remote and its own patch series, checked separately
    # below. A worktree does not populate submodules, so comparing it here
    # reports every file in it as missing -- which looks like catastrophic drift
    # and is nothing at all.
    reproduces_build_tree() {
        diff -rq --exclude=.git --exclude=xgameruntime "$WORK/wine/dlls" "$WINE_REPO/dlls" >/dev/null 2>&1 \
            && diff -rq --exclude=.git --exclude=dlls "$WORK/wine" "$WINE_REPO" >/dev/null 2>&1
    }
    if [ -z "$(git -C "$WINE_REPO" status --porcelain 2>/dev/null | head -1)" ]; then
        say "   (pristine checkout, so nothing to reproduce -- applied-only check)"
    elif reproduces_build_tree; then
        ok "the series reproduces the build tree exactly"
    else
        bad "the series applies but does NOT reproduce the build tree:"
        { diff -rq --exclude=.git --exclude=xgameruntime "$WORK/wine/dlls" "$WINE_REPO/dlls" 2>/dev/null
          diff -rq --exclude=.git --exclude=dlls "$WORK/wine" "$WINE_REPO" 2>/dev/null
        } | head -10 | sed 's/^/        /'
    fi
else
    say "wine series: skipped, no repository at $WINE_REPO"
fi

# --- the xgameruntime series ----------------------------------------------
#
# Its own repository, pinned as a submodule of wine, so its patches are rooted
# at that repository's root rather than wine's. Applying them from the wine
# root fails with "No such file or directory", which reads like a broken patch
# and is not one.
XGR_REPO=$WINE_REPO/dlls/xgameruntime
if [ -e "$XGR_REPO/.git" ]; then
    if ! git -C "$XGR_REPO" cat-file -e "$XGR_BASE^{commit}" 2>/dev/null; then
        bad "the pinned base $XGR_BASE is not in $XGR_REPO (fetch it first)"
        XGR_BASE=""
    fi
fi
if [ -n "${XGR_BASE:-}" ] && [ -e "$XGR_REPO/.git" ]; then
    say "xgameruntime series against ${XGR_BASE:0:8}"
    git -C "$XGR_REPO" worktree add -q --detach "$WORK/xgr" "$XGR_BASE"
    for p in "$REPO_DIR"/patches/xgameruntime/*.patch; do
        name=$(basename "$p")
        if git -C "$WORK/xgr" apply --check "$p" 2>/dev/null; then
            git -C "$WORK/xgr" apply "$p"
            ok "$name"
        else
            bad "$name does not apply"
        fi
    done
    if [ "$(git -C "$XGR_REPO" rev-parse HEAD)" != "$XGR_BASE" ]; then
        say "   (checked out elsewhere, so nothing to reproduce -- applied-only check)"
    elif [ -z "$(git -C "$XGR_REPO" status --porcelain 2>/dev/null | head -1)" ]; then
        say "   (pristine checkout, so nothing to reproduce -- applied-only check)"
    elif diff -rq --exclude=.git "$WORK/xgr" "$XGR_REPO" >/dev/null 2>&1; then
        ok "the series reproduces the build tree exactly"
    else
        bad "the series applies but does NOT reproduce the build tree:"
        diff -rq --exclude=.git "$WORK/xgr" "$XGR_REPO" 2>/dev/null | head -6 | sed 's/^/        /'
    fi
else
    say "xgameruntime series: skipped, submodule not checked out"
fi

# --- the client series ----------------------------------------------------
#
# Against a pinned upstream commit rather than the fork's HEAD, because these
# patches are published for other people to apply to upstream.
if [ -d "$CLIENT_REPO/.git" ]; then
    say "client series against upstream ${CLIENT_BASE:0:8}"
    if git -C "$CLIENT_REPO" cat-file -e "$CLIENT_BASE^{commit}" 2>/dev/null; then
        git -C "$CLIENT_REPO" worktree add -q --detach "$WORK/client" "$CLIENT_BASE"
        # In order: each is written against the tree the previous one leaves.
        #
        # Every patch except 0001, which is an earlier export of work that 0002
        # already contains and fails to apply alongside it. Expressed as "skip
        # 0001" rather than as a numeric range: the range that used to be here
        # was 000[2-9], which stopped checking anything from 0010 onwards --
        # silently, and exactly when a tenth patch was added.
        for p in "$REPO_DIR"/patches/xodus-cli/*.patch; do
            case $(basename "$p") in 0001-*) continue ;; esac
            name=$(basename "$p")
            if git -C "$WORK/client" apply --check "$p" 2>/dev/null; then
                git -C "$WORK/client" apply "$p"
                ok "$name"
            else
                bad "$name does not apply"
            fi
        done
    else
        bad "upstream commit $CLIENT_BASE is not in $CLIENT_REPO (fetch it first)"
    fi
else
    say "client series: skipped, no repository at $CLIENT_REPO"
fi

echo
if [ "$FAILED" -eq 0 ]; then
    printf '\033[32mall patch series apply\033[0m\n'
else
    printf '\033[31m%s problem(s)\033[0m\n' "$FAILED" >&2
fi
exit "$FAILED"
