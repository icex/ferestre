# Shared path resolution for every script in this repo.  POSIX sh, meant to be
# sourced:   . "$(dirname "$0")/xodus-env.sh"
#
# Nothing here is tied to one machine: every location comes from an
# environment variable with a sensible default, and the defaults probe the
# places Steam and the Xodus tools normally live.  Override any XODUS_* value
# before sourcing (or in your shell profile) to relocate things.
#
#   XODUS_GAMES_DIR   where decrypted games, prefixes and logs go   (~/xbox-games)
#   XODUS_CLI_DIR     directory holding xodus-cli / xodus-service    (auto-detected)
#   XODUS_PROTON_DIR  the installed Xodus Proton compat tool         (auto-detected)
#   XODUS_STEAM_DIR   a Steam install, for STEAM_COMPAT_CLIENT_INSTALL_PATH
#   XODUS_BUILD_DIR   the Proton build tree                           (~/src/xodus-build)
#   XODUS_SRC_DIR     where the xodus-proton and xodus-cli trees live  (<repo>/third_party)
#   XODUS_REPO_DIR    this repository                                 (auto)
#   XGR_XUID          optional: XUID your Windows saves were made under

_first_existing() {
    # print the first argument that names an existing directory
    for _d in "$@"; do
        [ -n "$_d" ] && [ -d "$_d" ] && { printf '%s\n' "$_d"; return 0; }
    done
    return 1
}

XODUS_REPO_DIR=${XODUS_REPO_DIR:-$(cd "$(dirname "$0")/.." 2>/dev/null && pwd)}
# $0 is the sourcing script when one of ours sources this; from an interactive
# shell or `sh -c '. scripts/xodus-env.sh'` it is the shell itself, so fall
# back to the working directory when that is the repository.
if [ ! -f "$XODUS_REPO_DIR/scripts/xodus-env.sh" ] && [ -f "$PWD/scripts/xodus-env.sh" ]; then
    XODUS_REPO_DIR=$PWD
fi
XODUS_GAMES_DIR=${XODUS_GAMES_DIR:-$HOME/xbox-games}
XODUS_BUILD_DIR=${XODUS_BUILD_DIR:-$HOME/src/xodus-build}
# The Proton fork and the client are submodules of this repository; a checkout
# that predates that layout has them beside it in ~/src instead.
if [ -z "${XODUS_SRC_DIR:-}" ]; then
    XODUS_SRC_DIR=$(_first_existing "$XODUS_REPO_DIR/third_party" "$HOME/src") || XODUS_SRC_DIR=$XODUS_REPO_DIR/third_party
fi

if [ -z "${XODUS_CLI_DIR:-}" ]; then
    if command -v xodus-cli >/dev/null 2>&1; then
        XODUS_CLI_DIR=$(dirname "$(command -v xodus-cli)")
    else
        XODUS_CLI_DIR=$(_first_existing "$XODUS_SRC_DIR/xodus-cli/target/release" \
                                        "$HOME/src/xodus-cli/target/release" \
                                        "$XODUS_REPO_DIR/../xodus-cli/target/release") || XODUS_CLI_DIR=
    fi
fi

if [ -z "${XODUS_STEAM_DIR:-}" ]; then
    XODUS_STEAM_DIR=$(_first_existing "$HOME/.steam/steam" "$HOME/.local/share/Steam" \
                                      "$HOME/.steam/root" "$HOME/.var/app/com.valvesoftware.Steam/.local/share/Steam") || XODUS_STEAM_DIR=
fi

if [ -z "${XODUS_PROTON_DIR:-}" ]; then
    XODUS_PROTON_DIR=$(_first_existing "$XODUS_STEAM_DIR/compatibilitytools.d/xodus" \
                                       "$HOME/.steam/steam/compatibilitytools.d/xodus" \
                                       "$HOME/.local/share/Steam/compatibilitytools.d/xodus" \
                                       "$HOME/.steam/root/compatibilitytools.d/xodus") || XODUS_PROTON_DIR=
fi

export XODUS_REPO_DIR XODUS_GAMES_DIR XODUS_BUILD_DIR XODUS_SRC_DIR XODUS_CLI_DIR XODUS_STEAM_DIR XODUS_PROTON_DIR

# xodus_require <what> <path> [hint]  -- fail loudly with an actionable message
xodus_require() {
    if [ -z "$2" ] || [ ! -e "$2" ]; then
        printf '!! %s not found (%s)\n' "$1" "${2:-unset}" >&2
        [ -n "${3:-}" ] && printf '   %s\n' "$3" >&2
        exit 1
    fi
}
