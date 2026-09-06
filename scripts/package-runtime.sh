#!/bin/bash
# Turn an installed runtime into the release tarball `ferestre-runtime-bin`
# expects.
#
# The contract it produces is documented in packaging/aur/README.md and asserted
# here rather than trusted: one top-level directory named for the version, a
# `compatibilitytool.vdf` that calls the tool `ferestre`, and the two things the
# PKGBUILD refuses to package without -- the `close_fds=False` patch in `proton`
# and a real `xgameruntime.dll`. Both have been shipped broken by hand before,
# and both look like the title crashing rather than like a packaging mistake.
#
# It also does the two things the roadmap measured and never implemented:
#
#   - drops wine-gecko and wine-mono, 434 MB between them. A native GDK title
#     needs neither: gecko is an embedded browser, mono is .NET. `--full` keeps
#     them for a title that turns out to want one.
#   - strips the ELF shared objects. Only those -- never the PE builtins, where
#     Wine resolves builtin DLLs through data a strip can remove.
#
#   scripts/package-runtime.sh [--full] [--out DIR] [runtime-dir]
#
# The source runtime is never modified: everything happens in a staging copy.

set -euo pipefail
. "$(dirname "$0")/xodus-env.sh"

REPO_DIR=${REPO_DIR:-$XODUS_REPO_DIR}
OUT_DIR=${OUT_DIR:-$REPO_DIR/out}
KEEP_DOTNET=0
SOURCE=""

while [ $# -gt 0 ]; do
    case $1 in
        --full) KEEP_DOTNET=1; shift ;;
        --out) OUT_DIR=$2; shift 2 ;;
        -*) echo "!! unknown option: $1" >&2; exit 2 ;;
        *) SOURCE=$1; shift ;;
    esac
done

SOURCE=${SOURCE:-${XODUS_PROTON_DIR:-}}
[ -n "$SOURCE" ] || { echo "!! no runtime: pass one or set XODUS_PROTON_DIR" >&2; exit 1; }
[ -d "$SOURCE" ] || { echo "!! $SOURCE is not a directory" >&2; exit 1; }
[ -f "$SOURCE/version" ] || { echo "!! $SOURCE/version is missing; is that a compatibility tool?" >&2; exit 1; }

say()  { printf '\033[1m:: %s\033[0m\n' "$*"; }
fail() { printf '\033[31m!! %s\033[0m\n' "$*" >&2; exit 1; }

# Upstream's version string is `xodus-bleeding-edge-11.0-20260803-3-g7c0b4354`.
# A pkgver cannot contain hyphens, so the AUR package uses <major>.<minor>.<date>.
RAW=$(cut -d' ' -f2- < "$SOURCE/version")
PKGVER=$(sed -nE 's/.*[^0-9]([0-9]+)\.([0-9]+)-([0-9]{8}).*/\1.\2.\3/p' <<<"$RAW")
[ -n "$PKGVER" ] || fail "cannot derive a pkgver from version string: $RAW"

NAME=ferestre-runtime-$PKGVER
STAGE=$(mktemp -d "${TMPDIR:-/tmp}/ferestre-runtime.XXXXXX")
trap 'rm -rf "$STAGE"' EXIT

say "packaging $RAW as $PKGVER"
before=$(du -sm "$SOURCE" | cut -f1)

# --exclude the caches: __pycache__ is regenerated, and proton.orig is the
# unpatched script kept for local reference, not something to ship.
rsync -a --exclude='__pycache__' --exclude='proton.orig' "$SOURCE/" "$STAGE/$NAME/"

if [ "$KEEP_DOTNET" -eq 0 ]; then
    say "dropping wine-gecko and wine-mono"
    rm -rf "$STAGE/$NAME/files/share/wine/gecko" "$STAGE/$NAME/files/share/wine/mono"
else
    say "keeping wine-gecko and wine-mono (--full)"
fi

say "stripping ELF shared objects"
stripped=0
skipped=0
while IFS= read -r -d '' f; do
    # PE builtins are deliberately excluded: Wine finds a builtin DLL through
    # data that a strip can remove, and a stripped builtin fails to load with an
    # error that looks like the title's fault.
    case $(file -b "$f") in
        ELF*not\ stripped*) ;;
        *) continue ;;
    esac
    # The runtime ships its libraries read-only (-r-xr-xr-x), and `strip` works
    # by writing a copy alongside, so it fails with "Permission denied" on every
    # one of them. Silently: it took stripping 9 files out of 82 to notice.
    mode=$(stat -c%a "$f")
    chmod u+w "$f"
    if strip --strip-unneeded "$f" 2>/dev/null; then
        stripped=$((stripped + 1))
    else
        skipped=$((skipped + 1))
    fi
    chmod "$mode" "$f"
done < <(find "$STAGE/$NAME/files" -type f -name '*.so*' -print0)
echo "   stripped $stripped shared objects"
[ "$skipped" -eq 0 ] || echo "   $skipped could not be stripped"

# The tool has to introduce itself as ferestre, or Steam lists it under the
# fork's name and two installs collide.
say "naming the tool"
sed -i 's/"xodus-proton"/"ferestre"/; s/"display_name" *"xodus"/"display_name" "Ferestre"/' \
    "$STAGE/$NAME/compatibilitytool.vdf"

say "publishing the capability manifest"
"$REPO_DIR/scripts/write-capabilities.sh" "$STAGE/$NAME" | sed 's/^/   /'

# What the PKGBUILD refuses to package without. Asserted here so a broken
# tarball is never published, rather than discovered by whoever installs it.
say "checking the shape the package expects"
grep -q "close_fds=False" "$STAGE/$NAME/proton" \
    || fail "proton lacks the close_fds patch -- every GDK title would fail to launch"
[ -s "$STAGE/$NAME/files/lib/wine/x86_64-windows/xgameruntime.dll" ] \
    || fail "xgameruntime.dll is missing or empty"
grep -q '"ferestre"' "$STAGE/$NAME/compatibilitytool.vdf" \
    || fail "compatibilitytool.vdf still names another tool"
[ -s "$STAGE/$NAME/files/share/ferestre/capabilities.json" ] \
    || fail "the capability manifest was not written"
echo "   ok"

mkdir -p "$OUT_DIR"
TARBALL=$OUT_DIR/$NAME-x86_64.tar.zst
say "writing $TARBALL"
tar --sort=name --owner=0 --group=0 --numeric-owner \
    -C "$STAGE" -cf - "$NAME" | zstd -19 -T0 -q -o "$TARBALL" -f

( cd "$OUT_DIR" && sha256sum "$(basename "$TARBALL")" > "$(basename "$TARBALL").sha256" )

after=$(du -sm "$STAGE/$NAME" | cut -f1)
packed=$(du -sm "$TARBALL" | cut -f1)
echo
printf '   installed  %5s MB  ->  %5s MB\n' "$before" "$after"
printf '   compressed %5s MB\n' "$packed"
echo
echo "   tag:   runtime-$PKGVER"
echo "   asset: $(basename "$TARBALL")"
echo "   sha:   $(cut -d' ' -f1 < "$TARBALL.sha256")"
