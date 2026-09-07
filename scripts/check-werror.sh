#!/bin/bash
# Compile every file the patches touch the way a clone of Wine compiles it.
#
#   scripts/check-werror.sh [object ...]
#
# Wine's configure turns on -Werror when it finds a .git, on the reasoning that
# anyone building from a checkout is developing. That makes the local build and
# the CI build disagree in a way that is invisible until it is expensive:
#
#   * The build tree here works from $OBJ/src-wine, a *copy* with no .git, so
#     warnings stay warnings and everything compiles.
#   * The runtime workflow clones the fork, so -Werror is on, and a warning is
#     a failed build -- 48 minutes in, after the bundled ffmpeg, kaldi and vosk
#     have all been built and thrown away.
#
# That is what happened to `patches/wine/0006`: a declaration below a statement,
# which is a warning here and `error: ISO C90 forbids mixed declarations and
# code` there. One line, an hour to find out.
#
# So compile the patched translation units, in the same container, with the
# flags the Makefile already uses plus -Werror. About a minute against a warm
# build tree, and it answers the same question.
#
# Needs a configured build tree ($XODUS_BUILD_DIR, default ~/src/xodus-proton/
# build-ferestre) -- it reuses that tree's own Makefile rather than guessing at
# include paths, which is the only way the flags are honestly the same.

set -euo pipefail
. "$(dirname "$0")/xodus-env.sh"

BUILD_DIR=${BUILD_DIR:-$XODUS_BUILD_DIR}
OBJ_DIR=$BUILD_DIR/obj-wine-x86_64
SRC_DIR=${XODUS_SRC_DIR:-$HOME/src}/xodus-proton
IMAGE=${STEAMRT_IMAGE:-registry.gitlab.steamos.cloud/proton/steamrt4/sdk/x86_64:4.0.20260331.220802-0}

if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then
    C_B=$'\033[1m'; C_R=$'\033[31m'; C_G=$'\033[32m'; C_0=$'\033[0m'
else
    C_B=; C_R=; C_G=; C_0=
fi
say() { printf '%s:: %s%s\n' "$C_B" "$*" "$C_0" >&2; }
die() { printf '%s!! %s%s\n' "$C_R" "$1" "$C_0" >&2; shift; for l in "$@"; do printf '   %s\n' "$l" >&2; done; exit 1; }

[ -f "$OBJ_DIR/Makefile" ] || die \
    "no configured Wine build at $OBJ_DIR" \
    "This reuses a real build tree's Makefile so the flags are the ones that" \
    "are actually used. Configure and build the runtime once first:" \
    "  scripts/install-xodus-proton.sh"

# The objects for every .c the wine and xgameruntime series touch. Derived from
# the patches rather than listed, so a patch that starts touching a new file is
# checked without anyone remembering to add it here.
REPO=$(cd "$(dirname "$0")/.." && pwd)
objects() {
    local p f
    for p in "$REPO"/patches/wine/*.patch; do
        # `+++ b/dlls/foo/bar.c` -> the object the Makefile builds from it.
        while read -r f; do
            case $f in
                # Unix-side sources build to an object beside themselves; PE
                # sources build under x86_64-windows/. media-converter is
                # neither -- it is compiled into two different unix libraries,
                # so ask the Makefile which objects it has rather than guess.
                dlls/ntdll/unix/*.c) printf '%s\n' "${f%.c}.o" ;;
                dlls/winegstreamer/media-converter/*.c)
                    grep -oE "^dlls/[a-z0-9_]+/media-converter/$(basename "$f" .c)\.o" \
                        "$OBJ_DIR/Makefile" | sort -u ;;
                dlls/*/*.c)
                    printf '%s/x86_64-windows/%s.o\n' "$(dirname "$f")" "$(basename "$f" .c)" ;;
            esac
        done < <(sed -n 's|^+++ b/\(.*\.c\)$|\1|p' "$p")
    done
    for p in "$REPO"/patches/xgameruntime/*.patch; do
        while read -r f; do
            case $f in
                unixlib.c) printf 'dlls/xgameruntime/unixlib.o\n' ;;
                *.c) printf 'dlls/xgameruntime/x86_64-windows/%s.o\n' "${f%.c}" ;;
            esac
        done < <(sed -n 's|^+++ b/\(.*\.c\)$|\1|p' "$p")
    done
}

if [ $# -gt 0 ]; then
    OBJECTS=("$@")
else
    mapfile -t OBJECTS < <(objects | sort -u)
fi
[ ${#OBJECTS[@]} -gt 0 ] || die "no objects to check"

# The Makefile's own warning flags, plus -Werror. Taken from the file rather
# than written out, because a flag list copied here would drift from the one
# the build uses and quietly check something else.
EXTRA=$(sed -n 's/^EXTRACFLAGS = //p' "$OBJ_DIR/Makefile" | head -1)
[ -n "$EXTRA" ] || die "no EXTRACFLAGS in $OBJ_DIR/Makefile; has the build system changed?"

say "checking ${#OBJECTS[@]} objects with -Werror"
for o in "${OBJECTS[@]}"; do printf '   %s\n' "$o" >&2; done

# Removed first: a warm object is not recompiled, and a check that compiles
# nothing passes for the wrong reason.
( cd "$OBJ_DIR" && rm -f "${OBJECTS[@]}" )

log=$(mktemp)
trap 'rm -f "$log"' EXIT
if docker run --rm \
        -v "$SRC_DIR:$SRC_DIR" -v "$BUILD_DIR:$BUILD_DIR" \
        ${CCACHE_DIR:+-v "$CCACHE_DIR:$CCACHE_DIR" -e CCACHE_DIR} \
        -e HOME -e USER -u "$(id -u):$(id -g)" \
        -w "$OBJ_DIR" "$IMAGE" \
        make -k -j"$(nproc)" EXTRACFLAGS="$EXTRA -Werror" "${OBJECTS[@]}" \
        >"$log" 2>&1; then
    printf '   %sok%s   every patched file compiles as a clone of Wine compiles it\n' "$C_G" "$C_0" >&2
    exit 0
fi

printf '   %s!!%s   -Werror rejected something the local build only warns about:\n' "$C_R" "$C_0" >&2
grep -E 'error:|\*\*\*' "$log" | head -20 | sed 's/^/        /' >&2
exit 1
