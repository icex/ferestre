#!/bin/bash
# Build the ferestre AppImage.
#
#   packaging/appimage/build-appimage.sh [options]
#
# Assembles an AppDir, drops in the launcher (or a placeholder that drives the
# repository's shell scripts, when the launcher binary does not exist yet), and
# turns it into a single-file x86_64 AppImage.
#
# The patched Proton runtime is NOT in here. It is ~264 MiB compressed, it
# versions independently of the launcher, and bundling it would tie a 1 MB
# application to a 264 MB release train. It is built or downloaded separately.
#
# ---------------------------------------------------------------------------
# The libfuse2 problem, and why this script does not have it
# ---------------------------------------------------------------------------
# Classic AppImages embed a runtime linked against libfuse2, which Arch-family
# distributions have not shipped for years. The usual outcome is a first-time
# user seeing "dlopen(): error loading libfuse.so.2" and giving up.
#
# Two separate halves, solved separately:
#
#   Building.  appimagetool is itself an AppImage. We never let it mount
#              itself: it is unpacked once with --appimage-extract, which is a
#              plain userspace squashfs read and needs no FUSE at all, and the
#              unpacked tree is run directly. That also puts appimagetool's own
#              bundled mksquashfs, zsyncmake and desktop-file-validate on PATH,
#              so the build host needs none of them installed.
#
#   Running.   The AppImage we produce is built with --runtime-file against the
#              modern type2-runtime, which links libfuse 3 statically and uses
#              fusermount3. Verified on a host with libfuse3 and fusermount3 and
#              no libfuse2 or fusermount at all: it mounts and runs.
#
# Hosts with no FUSE whatsoever (most containers, some hardened setups) still
# cannot mount anything. For those the answer is --appimage-extract-and-run, and
# the self-test at the end of this script reports which of the two worked here
# so nobody has to discover it from a stack trace.

set -euo pipefail

# --- pinned build tools ------------------------------------------------------
#
# Pinned to tags rather than "continuous" so a build is reproducible and the
# checksums below mean something. Upstream has been known to re-upload assets
# under an existing tag; if that happens the checksum fails loudly with
# instructions rather than silently building against something else.
APPIMAGETOOL_VER=1.9.1
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/${APPIMAGETOOL_VER}/appimagetool-x86_64.AppImage"
APPIMAGETOOL_SHA=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0

RUNTIME_VER=20251108
RUNTIME_URL="https://github.com/AppImage/type2-runtime/releases/download/${RUNTIME_VER}/runtime-x86_64"
RUNTIME_SHA=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d

DEFAULT_APP_ID=io.github.icex.ferestre
DEFAULT_VERSION=0.0.0

# --- output ------------------------------------------------------------------
if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then
    C_B=$'\033[1m'; C_R=$'\033[31m'; C_Y=$'\033[33m'; C_0=$'\033[0m'
else
    C_B=; C_R=; C_Y=; C_0=
fi
say()  { printf '%s:: %s%s\n' "$C_B" "$*" "$C_0" >&2; }
warn() { printf '%s-- %s%s\n' "$C_Y" "$*" "$C_0" >&2; }
die()  { printf '%s!! %s%s\n' "$C_R" "$1" "$C_0" >&2; shift; for l in "$@"; do printf '   %s\n' "$l" >&2; done; exit 1; }

HERE=$(cd "$(dirname "$(readlink -f "$0")")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)

usage() {
    sed -n '2,4p' "$0" | sed 's/^# \{0,1\}//'
    cat <<EOF

options
  --out DIR          where the .AppImage lands       (default $REPO/out)
  --bin PATH         the launcher binary to package  (default: autodetect, else placeholder)
  --client PATH      xodus-cli binary to bundle
  --runtime DIR      patched Proton runtime directory to bundle
  --version V        version string                  (default: from Cargo.toml, else $DEFAULT_VERSION)
  --app-id ID        reverse-DNS application id      (default $DEFAULT_APP_ID)
  --update-info STR  appimagetool -u string; also writes a .zsync file
  --sign             sign the AppImage with gpg2 (needs a key set up)
  --offline          never use the network; the build tools must already be cached
  --keep-appdir      do not delete the staged AppDir afterwards
  --skip-self-test   do not run the produced AppImage
  -h, --help         this text

environment
  FERESTRE_BIN, FERESTRE_CLIENT_BIN, FERESTRE_RUNTIME_DIR, FERESTRE_OUT_DIR, FERESTRE_VERSION, FERESTRE_APP_ID, FERESTRE_UPDATE_INFO
  FERESTRE_CACHE_DIR         build-tool cache      (default \${XDG_CACHE_HOME:-\$HOME/.cache}/ferestre-appimage)
  FERESTRE_BUILD_DIR         AppDir staging        (default $REPO/build/appimage)
  FERESTRE_APPIMAGETOOL      a local appimagetool AppImage, instead of downloading
  FERESTRE_APPIMAGE_RUNTIME  a local type-2 runtime file, instead of downloading
  FERESTRE_APPIMAGETOOL_ARGS extra arguments passed through to appimagetool
EOF
}

# --- arguments ---------------------------------------------------------------
OUT_DIR=${FERESTRE_OUT_DIR:-$REPO/out}
BUILD_DIR=${FERESTRE_BUILD_DIR:-$REPO/build/appimage}
CACHE_DIR=${FERESTRE_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/ferestre-appimage}
LAUNCHER_BIN=${FERESTRE_BIN:-}
CLIENT_BIN=${FERESTRE_CLIENT_BIN:-}
BUNDLED_RUNTIME=${FERESTRE_RUNTIME_DIR:-}
VERSION=${FERESTRE_VERSION:-}
APP_ID=${FERESTRE_APP_ID:-$DEFAULT_APP_ID}
UPDATE_INFO=${FERESTRE_UPDATE_INFO:-}
OFFLINE=0 KEEP_APPDIR=0 SELF_TEST=1 SIGN=0

while [ $# -gt 0 ]; do
    case "$1" in
        --out)          OUT_DIR=${2:?--out needs a directory}; shift 2 ;;
        --bin)          LAUNCHER_BIN=${2:?--bin needs a path}; shift 2 ;;
        --client)       CLIENT_BIN=${2:?--client needs a path}; shift 2 ;;
        --runtime)      BUNDLED_RUNTIME=${2:?--runtime needs a directory}; shift 2 ;;
        --version)      VERSION=${2:?--version needs a value}; shift 2 ;;
        --app-id)       APP_ID=${2:?--app-id needs a value}; shift 2 ;;
        --update-info)  UPDATE_INFO=${2:?--update-info needs a value}; shift 2 ;;
        --sign)         SIGN=1; shift ;;
        --offline)      OFFLINE=1; shift ;;
        --keep-appdir)  KEEP_APPDIR=1; shift ;;
        --skip-self-test) SELF_TEST=0; shift ;;
        -h|--help)      usage; exit 0 ;;
        *)              die "unknown option: $1" "run with --help" ;;
    esac
done

# --- preflight ---------------------------------------------------------------
[ "$(uname -m)" = "x86_64" ] || die \
    "this builds an x86_64 AppImage and the host is $(uname -m)" \
    "Proton, the patched Wine and every supported title are x86_64 only, so there" \
    "is no other architecture worth producing. Cross-building is not supported here."

case "$APP_ID" in
    *[!A-Za-z0-9._-]*|"" ) die "not a usable application id: $APP_ID" \
        "use reverse-DNS, letters digits dot dash underscore only, e.g. io.github.you.ferestre" ;;
esac

[ -f "$REPO/scripts/xodus-env.sh" ] || die \
    "cannot find the repository root (looked at $REPO)" \
    "run this script from its place in the tree: packaging/appimage/build-appimage.sh"

missing=
for t in cp mkdir sed find chmod ln install du date grep readlink; do
    command -v "$t" >/dev/null 2>&1 || missing="$missing $t"
done
[ -z "$missing" ] || die "missing basic tools:$missing"

if command -v sha256sum >/dev/null 2>&1;    then sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1;     then sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
elif command -v openssl >/dev/null 2>&1;    then sha256() { openssl dgst -sha256 "$1" | sed 's/.*= //'; }
else die "no sha256 tool (need one of sha256sum, shasum, openssl)"; fi

if   command -v curl >/dev/null 2>&1; then DL=curl
elif command -v wget >/dev/null 2>&1; then DL=wget
else DL=; fi

# --- fetching the build tools ------------------------------------------------
# Downloads to a .part file and renames only after the checksum matches, so an
# interrupted build never leaves a truncated tool in the cache to be "reused".
fetch_pinned() {
    local name=$1 url=$2 want=$3 dest=$4 got q_curl q_wget

    if [ -f "$dest" ]; then
        got=$(sha256 "$dest")
        if [ "$got" = "$want" ] || [ -n "${FERESTRE_SKIP_SHA:-}" ]; then
            say "$name: cached"
            return 0
        fi
        warn "$name: cached copy has the wrong checksum, re-fetching"
        rm -f "$dest"
    fi

    [ "$OFFLINE" -eq 0 ] || die \
        "$name is not cached and --offline was given" \
        "fetch it once with network access, or place it yourself at:" \
        "  $dest" \
        "from: $url"

    [ -n "$DL" ] || die \
        "need curl or wget to download $name" \
        "or download it yourself to $dest, from:" \
        "  $url"

    say "$name: downloading $url"
    mkdir -p "$(dirname "$dest")"
    # A progress bar for a person, silence for a log or a CI job.
    if [ -t 2 ]; then q_curl=--progress-bar; q_wget=--show-progress
    else                q_curl=-sS;          q_wget=-q; fi
    case "$DL" in
        curl) curl -fL --retry 3 --connect-timeout 20 "$q_curl" -o "$dest.part" "$url" ;;
        wget) wget -q "$q_wget" --tries=3 --timeout=20 -O "$dest.part" "$url" ;;
    esac || { rm -f "$dest.part"; die \
        "could not download $name" \
        "url: $url" \
        "if this machine has no network, fetch it elsewhere and copy it to:" \
        "  $dest"; }

    got=$(sha256 "$dest.part")
    if [ "$got" != "$want" ] && [ -z "${FERESTRE_SKIP_SHA:-}" ]; then
        rm -f "$dest.part"
        die "$name failed its checksum" \
            "expected $want" \
            "got      $got" \
            "url      $url" \
            "Upstream sometimes re-uploads assets under an existing tag. If you have" \
            "checked the new file and trust it, update the pin at the top of this" \
            "script, or set FERESTRE_SKIP_SHA=1 for a one-off build, or point" \
            "FERESTRE_APPIMAGETOOL / FERESTRE_APPIMAGE_RUNTIME at a local copy you trust."
    fi
    mv "$dest.part" "$dest"
}

mkdir -p "$CACHE_DIR"

if [ -n "${FERESTRE_APPIMAGETOOL:-}" ]; then
    [ -f "$FERESTRE_APPIMAGETOOL" ] || die "FERESTRE_APPIMAGETOOL is set but $FERESTRE_APPIMAGETOOL does not exist"
    TOOL_IMG=$(readlink -f "$FERESTRE_APPIMAGETOOL")
    say "appimagetool: using $TOOL_IMG"
else
    TOOL_IMG="$CACHE_DIR/appimagetool-$APPIMAGETOOL_VER-x86_64.AppImage"
    fetch_pinned "appimagetool $APPIMAGETOOL_VER" "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA" "$TOOL_IMG"
fi

if [ -n "${FERESTRE_APPIMAGE_RUNTIME:-}" ]; then
    [ -f "$FERESTRE_APPIMAGE_RUNTIME" ] || die "FERESTRE_APPIMAGE_RUNTIME is set but $FERESTRE_APPIMAGE_RUNTIME does not exist"
    RUNTIME_FILE=$(readlink -f "$FERESTRE_APPIMAGE_RUNTIME")
    say "runtime: using $RUNTIME_FILE"
else
    RUNTIME_FILE="$CACHE_DIR/runtime-$RUNTIME_VER-x86_64"
    fetch_pinned "type2-runtime $RUNTIME_VER" "$RUNTIME_URL" "$RUNTIME_SHA" "$RUNTIME_FILE"
fi

# Unpack appimagetool once. --appimage-extract reads the squashfs in userspace,
# so this is the step that makes the whole build FUSE-free.
TOOL_ROOT="$CACHE_DIR/appimagetool-$APPIMAGETOOL_VER/squashfs-root"
if [ ! -x "$TOOL_ROOT/AppRun" ]; then
    say "unpacking appimagetool (no FUSE involved)"
    rm -rf "$(dirname "$TOOL_ROOT")"
    mkdir -p "$(dirname "$TOOL_ROOT")"
    chmod +x "$TOOL_IMG" 2>/dev/null || true
    ( cd "$(dirname "$TOOL_ROOT")" && "$TOOL_IMG" --appimage-extract >/dev/null ) || die \
        "could not unpack appimagetool" \
        "tried: $TOOL_IMG --appimage-extract" \
        "If it reported a FUSE error, the AppImage is a build this script has not" \
        "seen; delete $CACHE_DIR and try again, or set FERESTRE_APPIMAGETOOL to a" \
        "known-good appimagetool."
    [ -x "$TOOL_ROOT/AppRun" ] || die "appimagetool unpacked but has no AppRun at $TOOL_ROOT"
fi
# appimagetool's own bundled mksquashfs / zsyncmake / desktop-file-validate.
export PATH="$TOOL_ROOT/usr/bin:$PATH"

# --- what goes in usr/bin ----------------------------------------------------
PLACEHOLDER=0
if [ -z "$LAUNCHER_BIN" ]; then
    for cand in "$REPO/target/release/ferestre" "$REPO/target/debug/ferestre"; do
        if [ -x "$cand" ]; then LAUNCHER_BIN=$cand; break; fi
    done
fi
# The window, if it was built. Optional: it links GTK4 and libadwaita, which
# resolve against the host like the rest of this AppImage's dependencies, so a
# build without it is a valid CLI-only package rather than a broken one.
GUI_BIN=${FERESTRE_GUI_BIN:-}
if [ -z "$GUI_BIN" ]; then
    for cand in "$REPO/target/release/ferestre-gui" "$REPO/target/debug/ferestre-gui"; do
        if [ -x "$cand" ]; then GUI_BIN=$cand; break; fi
    done
fi
if [ -n "$LAUNCHER_BIN" ]; then
    [ -x "$LAUNCHER_BIN" ] || die "not an executable: $LAUNCHER_BIN"
    say "launcher: $LAUNCHER_BIN"
else
    PLACEHOLDER=1
    LAUNCHER_BIN=$HERE/ferestre-placeholder.sh
    warn "no launcher binary; packaging the placeholder shell command"
    warn "the AppImage will be marked -placeholder in its version and filename"
fi

# --- version -----------------------------------------------------------------
if [ -z "$VERSION" ]; then
    if [ -f "$REPO/Cargo.toml" ]; then
        VERSION=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$REPO/Cargo.toml" | head -1)
    fi
    [ -n "$VERSION" ] || VERSION=$DEFAULT_VERSION
    if [ "$PLACEHOLDER" -eq 1 ]; then VERSION="$VERSION-placeholder"; fi
    if command -v git >/dev/null 2>&1 && git -C "$REPO" rev-parse --short HEAD >/dev/null 2>&1; then
        # Dot-separated identifier, so the result stays a legal semver
        # pre-release and stays legal in a filename and a URL.
        VERSION="$VERSION.g$(git -C "$REPO" rev-parse --short HEAD)"
    fi
fi
say "version: $VERSION"

# --- stage the AppDir --------------------------------------------------------
mkdir -p "$BUILD_DIR"
BUILD_DIR=$(cd "$BUILD_DIR" && pwd)
APPDIR=$BUILD_DIR/$APP_ID.AppDir
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" \
         "$APPDIR/usr/lib/ferestre" \
         "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/icons/hicolor/256x256/apps" \
         "$APPDIR/usr/share/icons/hicolor/scalable/apps" \
         "$APPDIR/usr/share/metainfo" \
         "$APPDIR/usr/share/doc/ferestre"

install -m 0755 "$HERE/AppRun"      "$APPDIR/AppRun"
install -m 0755 "$LAUNCHER_BIN"     "$APPDIR/usr/bin/ferestre"
if [ -n "$CLIENT_BIN" ]; then
    [ -x "$CLIENT_BIN" ] || die "--client is not an executable: $CLIENT_BIN"
    say "client: $CLIENT_BIN"
    install -Dm755 "$CLIENT_BIN" "$APPDIR/usr/lib/ferestre/client/xodus-cli"
    if [ -x "$(dirname "$CLIENT_BIN")/xodus-service" ]; then
        install -Dm755 "$(dirname "$CLIENT_BIN")/xodus-service" \
            "$APPDIR/usr/lib/ferestre/client/xodus-service"
    fi
fi
if [ -n "$BUNDLED_RUNTIME" ]; then
    [ -d "$BUNDLED_RUNTIME" ] || die "--runtime is not a directory: $BUNDLED_RUNTIME"
    say "runtime: $BUNDLED_RUNTIME"
    cp -a "$BUNDLED_RUNTIME" "$APPDIR/usr/lib/ferestre/runtime"
    [ -f "$APPDIR/usr/lib/ferestre/runtime/files/lib/wine/x86_64-windows/xgameruntime.dll" ] || die \
        "--runtime does not look like a ferestre runtime: $BUNDLED_RUNTIME"
fi
if [ -n "$GUI_BIN" ]; then
    say "window: $GUI_BIN"
    install -m 0755 "$GUI_BIN"      "$APPDIR/usr/bin/ferestre-gui"
else
    warn "no window binary; this AppImage is command-line only"
fi

# The packaged command reports the version the package was built with rather
# than a constant compiled into it, so `ferestre version` and the AppImage filename
# can never disagree.
printf '%s\n' "$VERSION" > "$APPDIR/usr/lib/ferestre/VERSION"

# The scripts are the launch path, not documentation: launch-gdk.sh assembles
# the environment and proton-wine-shim.sh is what xodus-cli execs as its "wine"
# binary. patches/ comes along because install-xodus-proton.sh reads
# patches/proton out of it, and because someone holding only the AppImage should
# still be able to follow docs/RECIPES.md.
cp -a "$REPO/scripts"  "$APPDIR/usr/lib/ferestre/scripts"
cp -a "$REPO/patches"  "$APPDIR/usr/lib/ferestre/patches"
# The recipes are the canonical per-title data; the launch scripts are the
# fallback until the real launcher reads them directly.
cp -a "$REPO/titles"   "$APPDIR/usr/lib/ferestre/titles"
find "$APPDIR/usr/lib/ferestre/scripts" -name '*.sh' -exec chmod 0755 {} +

# Ship the licences with the thing they cover. patches/wine and
# patches/xgameruntime are LGPL-2.1-or-later and patches/xodus-cli is GPL-3.0;
# they travel as source diffs, and NOTICE is the map that says which is which.
for d in NOTICE LICENSE LICENSE.LGPL-2.1 README.md; do
    if [ -f "$REPO/$d" ]; then cp "$REPO/$d" "$APPDIR/usr/share/doc/ferestre/"; fi
done
[ -d "$REPO/docs" ] && cp "$REPO/docs"/*.md "$APPDIR/usr/share/doc/ferestre/" 2>/dev/null || true

# --- desktop entry, icon, metainfo ------------------------------------------
# Everything is named after the app id, so a fork that overrides it gets a
# consistent set rather than a desktop file pointing at someone else's icon.
subst_id() { sed "s/$DEFAULT_APP_ID/$APP_ID/g" "$1" > "$2"; }

subst_id "$HERE/$DEFAULT_APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
# The shared entry names `ferestre-gui`, which is right for a system install
# where that is a binary on PATH. In an AppImage it is not: an integrator
# rewrites `Exec=` to the AppImage's own path, so the entry point is AppRun,
# which opens the window when given no arguments. Point the entry at that, and
# fall back to the terminal command when no window was bundled -- an entry that
# launches something the package does not contain is indistinguishable from the
# whole thing being broken.
sed -i 's/^Exec=ferestre-gui$/Exec=ferestre/; s/^TryExec=ferestre-gui$/TryExec=ferestre/' \
    "$APPDIR/$APP_ID.desktop"
if [ -z "$GUI_BIN" ]; then
    sed -i 's/^Exec=ferestre$/Exec=ferestre doctor/; s/^Terminal=false$/Terminal=true/' \
        "$APPDIR/$APP_ID.desktop"
fi
cp "$APPDIR/$APP_ID.desktop"             "$APPDIR/usr/share/applications/$APP_ID.desktop"

cp "$HERE/$DEFAULT_APP_ID.png" "$APPDIR/$APP_ID.png"
cp "$HERE/$DEFAULT_APP_ID.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/$APP_ID.png"
cp "$HERE/../icons/hicolor/scalable/apps/$DEFAULT_APP_ID.svg" \
   "$APPDIR/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
ln -sf "$APP_ID.png" "$APPDIR/.DirIcon"

subst_id "$HERE/$DEFAULT_APP_ID.metainfo.xml" "$APPDIR/usr/share/metainfo/$APP_ID.metainfo.xml"
# The metainfo in the repo describes a placeholder development build; make the
# packaged copy describe the artefact actually being produced. Rewriting only
# the version and date used to leave a real release advertising itself as a
# placeholder -- software centres and Gear Lever show that text to users.
META="$APPDIR/usr/share/metainfo/$APP_ID.metainfo.xml"
if [ "$PLACEHOLDER" -eq 1 ]; then
    sed -i -e "s|<release version=\"[^\"]*\" date=\"[^\"]*\"|<release version=\"$VERSION\" date=\"$(date -u +%Y-%m-%d)\"|" "$META"
else
    # A real build: stamp it, drop the development marker, and replace the
    # placeholder prose rather than leaving it to contradict the artefact.
    python3 - "$META" "$VERSION" "$(date -u +%Y-%m-%d)" <<'PYEOF'
import re, sys
path, version, date = sys.argv[1], sys.argv[2], sys.argv[3]
xml = open(path).read()
xml = re.sub(
    r'<release version="[^"]*" date="[^"]*"[^>]*>.*?</release>',
    f'<release version="{version}" date="{date}">\n'
    f'      <description>\n'
    f'        <p>See the project\'s release notes for what changed in {version}.</p>\n'
    f'      </description>\n'
    f'    </release>',
    xml, count=1, flags=re.S)
open(path, "w").write(xml)
PYEOF
fi

# appimagetool still looks for the pre-2019 <id>.appdata.xml name and prints a
# five-line "AppStream metadata is missing" warning when it does not find one.
# The metadata is not missing; only the name it expects is. A symlink satisfies
# the check without shipping the document twice and getting them out of sync.
ln -sf "$APP_ID.metainfo.xml" "$APPDIR/usr/share/metainfo/$APP_ID.appdata.xml"

if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$APPDIR/$APP_ID.desktop" || die "the desktop entry is not valid"
fi
if command -v appstreamcli >/dev/null 2>&1; then
    appstreamcli validate --no-net "$APPDIR/usr/share/metainfo/$APP_ID.metainfo.xml" >/dev/null 2>&1 \
        || warn "appstream metainfo did not validate cleanly (not fatal; run appstreamcli validate to see why)"
fi

# --- build -------------------------------------------------------------------
mkdir -p "$OUT_DIR"
OUT_DIR=$(cd "$OUT_DIR" && pwd)
OUT_FILE="$OUT_DIR/ferestre-$VERSION-x86_64.AppImage"
rm -f "$OUT_FILE" "$OUT_FILE.zsync"

args=(--runtime-file "$RUNTIME_FILE")
if [ -n "$UPDATE_INFO" ]; then args+=(-u "$UPDATE_INFO"); fi
if [ "$SIGN" -eq 1 ]; then args+=(--sign); fi
# Deliberately unquoted: this is a caller-supplied argument list, not one word.
# shellcheck disable=SC2206
if [ -n "${FERESTRE_APPIMAGETOOL_ARGS:-}" ]; then args+=(${FERESTRE_APPIMAGETOOL_ARGS}); fi

# Run it from the output directory. With -u, appimagetool writes the .zsync
# file into the *current* directory rather than beside the AppImage it was told
# to produce, so a build started from anywhere else drops it there and leaves
# it behind. Every path handed to it is absolute for the same reason.
say "packing $OUT_FILE"
if ! ( cd "$OUT_DIR" && ARCH=x86_64 "$TOOL_ROOT/AppRun" "${args[@]}" "$APPDIR" "$OUT_FILE" >&2 ); then
    die "appimagetool failed" \
        "the staged AppDir was left at $APPDIR for inspection" \
        "re-run with --keep-appdir to keep it on success too"
fi
[ -f "$OUT_FILE" ] || die "appimagetool reported success but $OUT_FILE does not exist"
chmod +x "$OUT_FILE"

# --- self-test ---------------------------------------------------------------
# The point of this section is that nobody should learn about their host's FUSE
# situation from a stack trace. Try the normal path, then the fallback, and say
# plainly which worked.
fuse_ok=unknown
if [ "$SELF_TEST" -eq 1 ]; then
    say "self-test: running it the way a user would"
    if out=$("$OUT_FILE" version 2>&1); then
        fuse_ok=yes
        say "  mounted and ran: $(printf '%s' "$out" | tail -1)"
    elif printf '%s' "$out" | grep -qiE 'fuse|cannot mount'; then
        # The runtime could not mount the squashfs. That is the host's FUSE
        # setup, and it says nothing about whether the packaged command works.
        fuse_ok=no
        warn "  the AppImage could not mount itself here:"
        printf '%s\n' "$out" | sed 's/^/     /' >&2
        warn "  a property of this build host, not of the AppImage"
    else
        # It mounted and started; whatever ran inside is what returned nonzero.
        # Do not blame FUSE for that, or the final advice below is wrong.
        fuse_ok=yes
        warn "  it mounted, but the packaged command exited nonzero:"
        printf '%s\n' "$out" | sed 's/^/     /' >&2
    fi

    say "self-test: --appimage-extract-and-run (the no-FUSE fallback)"
    if out=$("$OUT_FILE" --appimage-extract-and-run version 2>&1); then
        say "  ran: $(printf '%s' "$out" | tail -1)"
    elif [ "$PLACEHOLDER" -eq 1 ]; then
        # The placeholder's command surface is ours, so this is a build bug.
        die "the AppImage does not run even with --appimage-extract-and-run" \
            "$out" \
            "the staged AppDir is at $APPDIR"
    else
        # A real launcher's command surface is not ours to assume; it may not
        # spell "version" the way the placeholder does. Report, do not fail.
        warn "  the packaged launcher did not answer 'version':"
        printf '%s\n' "$out" | sed 's/^/     /' >&2
        warn "  if that is just a different CLI, ignore it; the self-test cannot"
        warn "  then confirm anything beyond the AppImage having been produced"
    fi

    # Proves the shell scripts made it in and are readable from the mount.
    # Only asserted for the placeholder, whose subcommands this script defines.
    if [ "$PLACEHOLDER" -eq 1 ] \
       && ! "$OUT_FILE" --appimage-extract-and-run titles >/dev/null 2>&1; then
        die "the packaged command cannot read its own scripts directory" \
            "check usr/lib/ferestre/scripts in $APPDIR"
    fi
fi

[ "$KEEP_APPDIR" -eq 1 ] || rm -rf "$APPDIR"

# --- report ------------------------------------------------------------------
printf '\n'
say "built $OUT_FILE ($(du -h "$OUT_FILE" | cut -f1))"
if [ -f "$OUT_FILE.zsync" ]; then say "update file $OUT_FILE.zsync"; fi
if [ "$PLACEHOLDER" -eq 1 ]; then
    warn "this is a placeholder build: usr/bin/ferestre is a shell script over scripts/,"
    warn "not the launcher. Build the launcher and re-run, or pass --bin."
fi
case "$fuse_ok" in
    no)  warn "FUSE could not mount on this host. Tell users to run it as:"
         warn "    $(basename "$OUT_FILE") --appimage-extract-and-run <command>" ;;
    yes) say "FUSE mounting works here; no --appimage-extract-and-run needed" ;;
esac
printf '\n'
cat >&2 <<EOF
next:
  $OUT_FILE doctor     check this machine
  $OUT_FILE titles     what has a launch recipe
  $OUT_FILE help
EOF
