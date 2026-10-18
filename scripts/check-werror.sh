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
# Both architectures. The clone builds a 32-bit tree too (obj-wine-i386: i386
# PE objects and 32-bit unix objects, also with -Werror), and a warning that
# only exists there -- a pointer cast to a 32-bit integer, a %ld given a
# 64-bit value -- is just as fatal to the release build.
#
# Needs a configured build tree ($XODUS_BUILD_DIR, default ~/src/xodus-build)
# -- it reuses that tree's own Makefile rather than guessing at include paths,
# which is the only way the flags are honestly the same.

set -euo pipefail
. "$(dirname "$0")/xodus-env.sh"

BUILD_DIR=${BUILD_DIR:-$XODUS_BUILD_DIR}
OBJ_DIR=$BUILD_DIR/obj-wine-x86_64
OBJ32_DIR=$BUILD_DIR/obj-wine-i386
SRC_DIR=$XODUS_SRC_DIR/xodus-proton
IMAGE=${STEAMRT_IMAGE:-registry.gitlab.steamos.cloud/proton/steamrt4/sdk/x86_64:4.0.20260331.220802-0}

if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then
    C_B=$'\033[1m'; C_R=$'\033[31m'; C_G=$'\033[32m'; C_0=$'\033[0m'
else
    C_B=; C_R=; C_G=; C_0=
fi
say() { printf '%s:: %s%s\n' "$C_B" "$*" "$C_0" >&2; }
die() { printf '%s!! %s%s\n' "$C_R" "$1" "$C_0" >&2; shift; for l in "$@"; do printf '   %s\n' "$l" >&2; done; exit 1; }

for d in "$OBJ_DIR" "$OBJ32_DIR"; do
    [ -f "$d/Makefile" ] || die \
        "no configured Wine build at $d" \
        "This reuses a real build tree's Makefile so the flags are the ones that" \
        "are actually used. Configure and build the runtime once first:" \
        "  scripts/install-xodus-proton.sh"
done

# The objects for every .c the wine and xgameruntime series touch. Derived from
# the patches rather than listed, so a patch that starts touching a new file is
# checked without anyone remembering to add it here.
REPO=$(cd "$(dirname "$0")/.." && pwd)
# objects PE_DIR MAKEFILE: PE_DIR is x86_64-windows or i386-windows, and
# MAKEFILE is the tree's own, for the sources only it can place.
objects() {
    local pe=$1 makefile=$2 p f
    for p in "$REPO"/patches/wine/*.patch; do
        # `+++ b/dlls/foo/bar.c` -> the object the Makefile builds from it.
        while read -r f; do
            case $f in
                # Unix-side sources build to an object beside themselves; PE
                # sources build under <arch>-windows/. media-converter is
                # neither -- it is compiled into two different unix libraries,
                # so ask the Makefile which objects it has rather than guess.
                dlls/ntdll/unix/*.c) printf '%s\n' "${f%.c}.o" ;;
                dlls/winegstreamer/media-converter/*.c)
                    grep -oE "^dlls/[a-z0-9_]+/media-converter/$(basename "$f" .c)\.o" \
                        "$makefile" | sort -u ;;
                dlls/*/*.c)
                    printf '%s/%s/%s.o\n' "$(dirname "$f")" "$pe" "$(basename "$f" .c)" ;;
            esac
        done < <(sed -n 's|^+++ b/\(.*\.c\)$|\1|p' "$p")
    done
    for p in "$REPO"/patches/xgameruntime/*.patch; do
        while read -r f; do
            case $f in
                unixlib.c) printf 'dlls/xgameruntime/unixlib.o\n' ;;
                *.c) printf 'dlls/xgameruntime/%s/%s.o\n' "$pe" "${f%.c}" ;;
            esac
        done < <(sed -n 's|^+++ b/\(.*\.c\)$|\1|p' "$p")
    done
}

# Arguments name 64-bit objects, the way they always have; the same objects
# are then checked in the 32-bit tree where it builds them. Some modules exist
# only in the 64-bit build, and the 32-bit Makefile has no rule for their
# objects; asking make for one would fail for a reason that has nothing to do
# with warnings, so those are listed as skipped instead.
if [ $# -gt 0 ]; then
    OBJECTS=("$@")
else
    mapfile -t OBJECTS < <(objects x86_64-windows "$OBJ_DIR/Makefile" | sort -u)
fi
[ ${#OBJECTS[@]} -gt 0 ] || die "no objects to check"
OBJECTS32=()
SKIPPED32=()
for o in "${OBJECTS[@]}"; do
    o32=${o//\/x86_64-windows\//\/i386-windows\/}
    if grep -q -- "^$o32:" "$OBJ32_DIR/Makefile"; then
        OBJECTS32+=("$o32")
    else
        SKIPPED32+=("$o32")
    fi
done

# The flags every compile rule ends with, plus -Werror. makedep writes each
# rule's warning flags out literally and closes it with $(CFLAGS) for unix
# objects and $(<arch>_CFLAGS) for PE objects, so these -- and not EXTRACFLAGS,
# which no rule refers to -- are the variables that reach the compiler. Taken
# from the file rather than written out, because a flag list copied here would
# drift from the one the build uses and quietly check something else.
make_flags() {
    local makefile=$1 pe_arch=$2
    printf '%s\n' \
        "CFLAGS=$(sed -n 's/^CFLAGS = //p' "$makefile" | head -1) -Werror" \
        "${pe_arch}_CFLAGS=$(sed -n "s/^${pe_arch}_CFLAGS = //p" "$makefile" | head -1) -Werror"
}
for m in "$OBJ_DIR/Makefile:x86_64" "$OBJ32_DIR/Makefile:i386"; do
    grep -q '^CFLAGS = ' "${m%:*}" && grep -q "^${m##*:}_CFLAGS = " "${m%:*}" \
        || die "no CFLAGS or ${m##*:}_CFLAGS in ${m%:*}; has the build system changed?"
done
mapfile -t FLAGS64 < <(make_flags "$OBJ_DIR/Makefile" x86_64)
mapfile -t FLAGS32 < <(make_flags "$OBJ32_DIR/Makefile" i386)

say "checking ${#OBJECTS[@]} 64-bit and ${#OBJECTS32[@]} 32-bit objects with -Werror"
for o in "${OBJECTS[@]}"; do printf '   %s\n' "$o" >&2; done
for o in "${OBJECTS32[@]}"; do printf '   i386: %s\n' "$o" >&2; done
for o in "${SKIPPED32[@]}"; do printf '   i386: %s (not built in the 32-bit tree)\n' "$o" >&2; done

# Removed first: a warm object is not recompiled, and a check that compiles
# nothing passes for the wrong reason.
( cd "$OBJ_DIR" && rm -f "${OBJECTS[@]}" )
if [ ${#OBJECTS32[@]} -gt 0 ]; then ( cd "$OBJ32_DIR" && rm -f "${OBJECTS32[@]}" ); fi

CCACHE_DIR=${CCACHE_DIR:-$BUILD_DIR/.ccache-check-werror}
mkdir -p "$CCACHE_DIR"
export CCACHE_DIR

# The sources and the dependency headers each Makefile names, read-only. They
# sit inside BUILD_DIR for a normal build tree, but naming them separately lets
# BUILD_DIR be a copy of just the object trees, so the check can run without
# writing to the tree it was configured in.
CONFIGURED_IN=$(dirname "$(sed -n 's/^srcdir = //p' "$OBJ_DIR/Makefile" | head -1)")
ro_mount=()
case "$CONFIGURED_IN/" in
    "$BUILD_DIR"/*) ;;
    *) [ -d "$CONFIGURED_IN" ] && ro_mount=(-v "$CONFIGURED_IN:$CONFIGURED_IN:ro") ;;
esac

log=$(mktemp)
trap 'rm -f "$log"' EXIT
compile() {
    local obj_dir=$1; shift
    docker run --rm \
        -v "$SRC_DIR:$SRC_DIR" -v "$BUILD_DIR:$BUILD_DIR" "${ro_mount[@]}" \
        ${CCACHE_DIR:+-v "$CCACHE_DIR:$CCACHE_DIR" -e CCACHE_DIR} \
        -e HOME -e USER -u "$(id -u):$(id -g)" \
        -w "$obj_dir" "$IMAGE" \
        make -k -j"$(nproc)" "$@" >>"$log" 2>&1
}
status=0
compile "$OBJ_DIR" "${FLAGS64[@]}" "${OBJECTS[@]}" || status=1
if [ ${#OBJECTS32[@]} -gt 0 ]; then
    compile "$OBJ32_DIR" "${FLAGS32[@]}" "${OBJECTS32[@]}" || status=1
fi
if [ "$status" -eq 0 ]; then
    printf '   %sok%s   every patched file compiles as a clone of Wine compiles it, on both architectures\n' "$C_G" "$C_0" >&2
    exit 0
fi

printf '   %s!!%s   -Werror rejected something the local build only warns about:\n' "$C_R" "$C_0" >&2
grep -E 'error:|\*\*\*' "$log" | head -20 | sed 's/^/        /' >&2
exit 1
