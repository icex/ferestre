#!/bin/bash
# Put the desktop entry and the icon where a desktop environment looks.
#
# A packaged install does this for you. A build from source does not, so the
# window shows up in the taskbar as a generated letter tile and the launcher
# menu has no entry for it at all -- which is a poor look for a project whose
# icon is the thing its name is a joke about.
#
#   scripts/install-desktop.sh [--uninstall] [--prefix DIR]
#
# Installs into ~/.local/share by default: no root, and nothing outside the
# user's own home. `--uninstall` removes exactly what this put there.
#
# The entry points at whatever `ferestre-gui` the PATH finds, matching the
# packaged entry. Pass --prefix to install elsewhere.

set -euo pipefail

REPO_DIR=$(cd "$(dirname "$0")/.." && pwd)
PREFIX=${PREFIX:-${XDG_DATA_HOME:-$HOME/.local/share}}
ID=io.github.icex.ferestre
UNINSTALL=0

while [ $# -gt 0 ]; do
    case $1 in
        --uninstall) UNINSTALL=1; shift ;;
        --prefix) PREFIX=$2; shift 2 ;;
        *) echo "!! unknown argument: $1" >&2; exit 2 ;;
    esac
done

DESKTOP=$PREFIX/applications/$ID.desktop
ICON=$PREFIX/icons/hicolor/scalable/apps/$ID.svg

say() { printf '\033[1m:: %s\033[0m\n' "$*"; }

if [ "$UNINSTALL" -eq 1 ]; then
    say "removing"
    rm -fv "$DESKTOP" "$ICON"
else
    SOURCE_DESKTOP=$REPO_DIR/packaging/appimage/$ID.desktop
    SOURCE_ICON=$REPO_DIR/packaging/icons/hicolor/scalable/apps/$ID.svg
    [ -f "$SOURCE_DESKTOP" ] || { echo "!! $SOURCE_DESKTOP is missing" >&2; exit 1; }
    [ -f "$SOURCE_ICON" ] || { echo "!! $SOURCE_ICON is missing" >&2; exit 1; }

    say "installing into $PREFIX"
    install -Dm644 "$SOURCE_DESKTOP" "$DESKTOP"
    install -Dm644 "$SOURCE_ICON" "$ICON"
    echo "   $DESKTOP"
    echo "   $ICON"
fi

# Both are best-effort: the files are in place either way, and a desktop that
# does not ship these tools picks them up on its next scan.
command -v update-desktop-database >/dev/null \
    && update-desktop-database "$PREFIX/applications" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null \
    && gtk-update-icon-cache -qtf "$PREFIX/icons/hicolor" 2>/dev/null || true

if [ "$UNINSTALL" -eq 0 ]; then
    echo
    echo "   The entry runs whatever \`ferestre-gui\` is on your PATH."
    echo "   Running from a checkout instead? The window finds its own icon there,"
    echo "   so this is only needed for the launcher menu and the taskbar."
fi
