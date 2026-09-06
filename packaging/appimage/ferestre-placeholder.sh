#!/bin/bash
# Placeholder `ferestre` command.
#
# The real launcher is a Rust binary that does not exist yet (docs/ROADMAP.md,
# phase 2). Until it does, this stands in so the packaging can be built, run and
# tested rather than described: it is a thin, honest front end over the shell
# scripts in scripts/, with the same subcommand names the CLI is planned to use.
#
# build-appimage.sh installs this as usr/bin/ferestre only when no real binary is
# found. Point FERESTRE_BIN at target/release/ferestre and it is replaced.
#
# It also runs straight out of a git checkout:  packaging/appimage/ferestre-placeholder.sh doctor

set -u

VERSION_FALLBACK=0.0.0

# --- where the shell scripts are ---------------------------------------------
#
# Inside the AppImage they are at $APPDIR/usr/lib/ferestre/scripts; in a checkout
# they are at <repo>/scripts. Resolve, in order: an explicit override, the
# AppDir, then two guesses relative to this file.
self=$(readlink -f "$0")
selfdir=$(dirname "$self")
for cand in \
    "${FERESTRE_SCRIPTS_DIR:-}" \
    "${APPDIR:-/nonexistent}/usr/lib/ferestre/scripts" \
    "$selfdir/../lib/ferestre/scripts" \
    "$selfdir/../../scripts"
do
    [ -n "$cand" ] && [ -f "$cand/xodus-env.sh" ] && { SCRIPTS=$(cd "$cand" && pwd); break; }
done

if [ -z "${SCRIPTS:-}" ]; then
    echo "!! cannot find the ferestre scripts directory (looked for xodus-env.sh)" >&2
    echo "   set FERESTRE_SCRIPTS_DIR to the directory holding launch-gdk.sh" >&2
    exit 1
fi

# xodus-env.sh derives XODUS_REPO_DIR from $0, which would be *this* file and
# therefore wrong once the scripts live under usr/lib/ferestre. install-xodus-proton.sh
# reads patches/ out of that directory, so pin it to the scripts' actual parent.
export XODUS_REPO_DIR="${XODUS_REPO_DIR:-$(cd "$SCRIPTS/.." && pwd)}"

# --- output ------------------------------------------------------------------
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_OK=$'\033[32m'; C_WARN=$'\033[33m'; C_BAD=$'\033[31m'; C_DIM=$'\033[2m'; C_OFF=$'\033[0m'
else
    C_OK=; C_WARN=; C_BAD=; C_DIM=; C_OFF=
fi
fails=0
ok()   { printf '%s ok %s  %s\n'   "$C_OK"   "$C_OFF" "$*"; }
warn() { printf '%s -- %s  %s\n'   "$C_WARN" "$C_OFF" "$*"; }
bad()  { printf '%s !! %s  %s\n'   "$C_BAD"  "$C_OFF" "$*"; fails=$((fails + 1)); }
note() { printf '%s      %s%s\n'   "$C_DIM"  "$*" "$C_OFF"; }

# build-appimage.sh writes a VERSION file next to the scripts, so the packaged
# command cannot disagree with the filename of the package it came out of.
version() {
    if   [ -n "${FERESTRE_VERSION:-}" ];   then printf '%s\n' "$FERESTRE_VERSION"
    elif [ -f "$SCRIPTS/../VERSION" ]; then cat "$SCRIPTS/../VERSION"
    else printf '%s\n' "$VERSION_FALLBACK"; fi
}

usage() {
    cat <<EOF
ferestre $(version) — placeholder build

Runs Microsoft Store / Xbox GDK titles you own. The real launcher is not
written yet; this drives the project's shell scripts with the subcommand
names the launcher will use.

usage: ferestre <command> [arguments]

  doctor                      check this machine for everything a launch needs
  env                         print the resolved XODUS_* paths
  titles                      list the titles that have a launch recipe
  download <product-id> [dir] download and decrypt a title you own
  run <title>                 launch a title by recipe name (see: titles)
  run-raw <dir> <exe> <prefix>
                              launch an arbitrary package directory
  install-runtime             build and install the patched Proton runtime
  stop                        kill everything a launch left behind
  version                     print the version
  help                        this text

Paths come from XODUS_* environment variables; 'ferestre env' shows what they
resolve to. Product ids are the 12-character code in a Store URL.

Not written yet, and honestly so: listing what your account owns, delta
updates, and a GUI. See docs/ROADMAP.md.
EOF
}

# --- commands ----------------------------------------------------------------

cmd_env() {
    # Sourcing in a subshell keeps this process's environment clean.
    ( . "$SCRIPTS/xodus-env.sh"; env | grep -E '^(XODUS_|XGR_|FERESTRE_)' | sort )
}

cmd_titles() {
    local f name status product slug
    # titles/*.toml is the canonical source: one recipe per Store product id,
    # carrying the honest status the compatibility matrix is generated from.
    # The launch-*.sh scripts remain the fallback for a tree without recipes.
    local recipes="${FERESTRE_TITLES:-$SCRIPTS/../titles}"
    if [ -d "$recipes" ] && ls "$recipes"/*.toml >/dev/null 2>&1; then
        printf 'recipes in %s:\n\n' "$recipes"
        for f in "$recipes"/*.toml; do
            product=$(basename "$f" .toml)
            # capabilities.toml is the registry of what the runtime provides,
            # not a title.
            [ "$product" = "capabilities" ] && continue
            name=$(sed -n 's/^ *name *= *"\(.*\)"/\1/p' "$f" | head -1)
            slug=$(sed -n 's/^ *slug *= *"\(.*\)"/\1/p' "$f" | head -1)
            # state lives in the [status] table.
            status=$(sed -n 's/^ *state *= *"\(.*\)"/\1/p' "$f" | head -1)
            printf '  %-14s %-10s %-11s %s\n' \
                "$product" "${slug:-?}" "${status:-unknown}" "${name:-$product}"
        done
        printf '\nlaunch one with: ferestre run <product-id>\n'
        return 0
    fi

    printf 'recipes in %s:\n\n' "$SCRIPTS"
    for f in "$SCRIPTS"/launch-*.sh; do
        [ -f "$f" ] || continue
        name=$(basename "$f" .sh); name=${name#launch-}
        [ "$name" = "gdk" ] && continue   # the shared runner, not a title
        printf '  %-10s %s\n' "$name" \
            "$(sed -n '2s/^# //p' "$f" | sed -e 's/ -- .*//' -e 's/\. .*//')"
    done
    printf '\nlaunch one with: ferestre run <name>\n'
}

cmd_doctor() {
    # shellcheck disable=SC1091
    . "$SCRIPTS/xodus-env.sh"

    printf '\n%s\n' "== host =="
    if [ "$(uname -m)" = "x86_64" ]; then
        ok "x86_64"
    else
        bad "$(uname -m): the runtime, Proton and every supported title are x86_64 only"
    fi
    if [ -n "${APPIMAGE:-}" ]; then
        ok "running from an AppImage"
        note "$APPIMAGE"
    else
        warn "not running from an AppImage (that is fine)"
    fi

    printf '\n%s\n' "== client (xodus-cli) =="
    if [ -n "$XODUS_CLI_DIR" ] && [ -x "$XODUS_CLI_DIR/xodus-cli" ]; then
        ok "xodus-cli"
        note "$XODUS_CLI_DIR/xodus-cli"
    else
        bad "no xodus-cli"
        note "build it (cargo build --release in the xodus-cli checkout) or set XODUS_CLI_DIR"
    fi
    if [ -n "$XODUS_CLI_DIR" ] && [ -x "$XODUS_CLI_DIR/xodus-service" ]; then
        ok "xodus-service"
    else
        bad "no xodus-service (the GDK runtime's Xbox-side calls go through it)"
    fi

    printf '\n%s\n' "== runtime (patched Proton) =="
    if [ -n "$XODUS_PROTON_DIR" ] && [ -f "$XODUS_PROTON_DIR/proton" ]; then
        ok "compat tool"
        note "$XODUS_PROTON_DIR"
        # Without this patch every GDK title exits 1: Proton's run_proc() closes
        # the inherited fds that hold the decrypted executable.
        if grep -q "close_fds=False" "$XODUS_PROTON_DIR/proton" 2>/dev/null; then
            ok "proton keeps inherited file descriptors"
        else
            bad "proton closes inherited fds; every title will exit 1"
            note "re-run install-runtime, which re-applies patches/proton/0001-*.patch"
        fi
        if [ -f "$XODUS_PROTON_DIR/files/lib/wine/x86_64-windows/xgameruntime.dll" ]; then
            ok "xgameruntime.dll present"
        else
            bad "no xgameruntime.dll in the compat tool"
        fi
    else
        bad "no patched Proton"
        note "run: ferestre install-runtime   (or set XODUS_PROTON_DIR)"
    fi

    printf '\n%s\n' "== supporting =="
    if [ -n "$XODUS_STEAM_DIR" ] && [ -d "$XODUS_STEAM_DIR" ]; then
        ok "Steam install for STEAM_COMPAT_CLIENT_INSTALL_PATH"
        note "$XODUS_STEAM_DIR"
    else
        warn "no Steam install found; Proton wants STEAM_COMPAT_CLIENT_INSTALL_PATH"
        note "set XODUS_STEAM_DIR if yours is somewhere unusual"
    fi

    if mkdir -p "$XODUS_GAMES_DIR" 2>/dev/null && [ -w "$XODUS_GAMES_DIR" ]; then
        ok "games directory writable ($(df -Ph "$XODUS_GAMES_DIR" | awk 'NR==2{print $4}') free)"
        note "$XODUS_GAMES_DIR"
    else
        bad "games directory not writable: $XODUS_GAMES_DIR"
    fi

    if ls /usr/share/vulkan/icd.d/*.json >/dev/null 2>&1 \
       || ls /etc/vulkan/icd.d/*.json >/dev/null 2>&1; then
        ok "a Vulkan driver is installed"
    else
        bad "no Vulkan ICD found; the titles will not render"
    fi

    # The client stores account tokens in the keyring and currently aborts
    # outright when there is no Secret Service, rather than degrading.
    if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
        warn "no session D-Bus; sign-in needs a Secret Service (keyring) provider"
    elif command -v busctl >/dev/null 2>&1 \
         && busctl --user status org.freedesktop.secrets >/dev/null 2>&1; then
        ok "Secret Service available for the account keyring"
    else
        warn "could not confirm a Secret Service provider; sign-in may abort"
        note "unlock your keyring, or start gnome-keyring / kwallet"
    fi

    printf '\n'
    if [ "$fails" -eq 0 ]; then
        printf '%sready.%s  next: ferestre titles\n' "$C_OK" "$C_OFF"
    else
        printf '%s%d blocking problem(s) above.%s\n' "$C_BAD" "$fails" "$C_OFF"
    fi
    return "$fails"
}

require_script() {
    [ -x "$SCRIPTS/$1" ] && return 0
    echo "!! $SCRIPTS/$1 is missing or not executable" >&2
    exit 1
}

# --- dispatch ----------------------------------------------------------------
cmd=${1:-help}
[ $# -gt 0 ] && shift

case "$cmd" in
    doctor)          cmd_doctor ;;
    env)             cmd_env ;;
    titles)          cmd_titles ;;
    version|--version|-V)
                     version ;;
    help|--help|-h)  usage ;;

    download|install)
        [ $# -ge 1 ] || { echo "usage: ferestre download <product-id> [destination]" >&2; exit 2; }
        require_script get-game.sh
        exec "$SCRIPTS/get-game.sh" "$@"
        ;;

    run)
        [ $# -ge 1 ] || { echo "usage: ferestre run <title>   (see: ferestre titles)" >&2; exit 2; }
        # A recipe name becomes a filename, so keep it to something that cannot
        # walk out of the scripts directory.
        case "$1" in
            *[!a-z0-9-]*|-*|"") echo "!! not a recipe name: $1" >&2; exit 2 ;;
        esac
        title=$1; shift
        if [ ! -x "$SCRIPTS/launch-$title.sh" ]; then
            echo "!! no recipe for '$title'" >&2
            echo "   known:" >&2
            cmd_titles >&2
            exit 1
        fi
        exec "$SCRIPTS/launch-$title.sh" "$@"
        ;;

    run-raw)
        [ $# -ge 3 ] || { echo "usage: ferestre run-raw <game dir> <exe path in package> <prefix dir>" >&2; exit 2; }
        require_script launch-gdk.sh
        exec "$SCRIPTS/launch-gdk.sh" "$@"
        ;;

    install-runtime)
        require_script install-xodus-proton.sh
        # It builds inside a container from a Proton tree the packaged scripts
        # cannot carry, so say where that has to be rather than failing deep.
        if [ ! -d "${XODUS_BUILD_DIR:-$HOME/src/xodus-build}" ]; then
            echo "!! no Proton build tree at ${XODUS_BUILD_DIR:-$HOME/src/xodus-build}" >&2
            echo "   the runtime is built from source, not shipped in this package:" >&2
            echo "   see docs/RECIPES.md section 1, then set XODUS_BUILD_DIR" >&2
            exit 1
        fi
        exec "$SCRIPTS/install-xodus-proton.sh" "$@"
        ;;

    stop|kill)
        require_script kill-gdk.sh
        exec "$SCRIPTS/kill-gdk.sh" "$@"
        ;;

    *)
        echo "!! unknown command: $cmd" >&2
        usage >&2
        exit 2
        ;;
esac
