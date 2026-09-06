#!/bin/bash
# Download and decrypt a Microsoft Store / Xbox GDK game you own, ready to launch.
#
#   scripts/get-game.sh <product id | bedrock> [destination dir]
#
# Uses `xodus-cli streaming`, which streams the .msixvc from Microsoft's CDN,
# fetches your licence key (CIK) with your own account, and decrypts + extracts
# in one pass. Nothing is bypassed: it needs an account that owns the title.
#
# Product ids are the 12-character code in a Store URL, e.g.
#   https://apps.microsoft.com/detail/9NBLGGH2JHXJ  ->  9NBLGGH2JHXJ
#
# Known ids:   bedrock  9NBLGGH2JHXJ   Minecraft for Windows (Bedrock)

set -u
. "$(dirname "$0")/xodus-env.sh"

say()  { printf '\033[1m:: %s\033[0m\n' "$*"; }
fail() { printf '\033[31m!! %s\033[0m\n' "$*" >&2; exit 1; }

[ $# -ge 1 ] || { sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }

case "$1" in
    bedrock|minecraft) PRODUCT=9NBLGGH2JHXJ; DEFAULT_DEST=$XODUS_GAMES_DIR/bedrock/game ;;
    *)                 PRODUCT=$1;           DEFAULT_DEST=$XODUS_GAMES_DIR/$(printf '%s' "$1" | tr 'A-Z' 'a-z') ;;
esac
DEST=${2:-$DEFAULT_DEST}

xodus_require "xodus-cli" "$XODUS_CLI_DIR/xodus-cli" \
    "build it: 'cargo build --release' in the xodus-cli checkout, or set XODUS_CLI_DIR"

# Refuse to start on a full or unwritable disk rather than dying mid-stream.
mkdir -p "$DEST" || fail "cannot create $DEST"
[ -w "$DEST" ] || fail "$DEST is not writable"
free_gb=$(df -Pk "$DEST" | awk 'NR==2{printf "%d", $4/1024/1024}')
say "product $PRODUCT -> $DEST  (${free_gb} GB free; Store titles range from ~3 GB to ~150 GB)"
[ "$free_gb" -ge 5 ] || fail "less than 5 GB free at $DEST; free some space or pick another destination"

say "streaming, decrypting and extracting (this is the full download; be patient)"
if ! "$XODUS_CLI_DIR/xodus-cli" streaming "$PRODUCT" "$DEST"; then
    cat >&2 <<'EOF'
!! streaming failed. Common causes:
   - not logged in:            run  xodus-cli login   (opens a Microsoft sign-in)
   - account does not own it:  the Store product must be purchased on this account
   - "device group is full":   remove an old device at account.microsoft.com/devices
   - network / CDN hiccup:     just run this command again, it resumes
EOF
    exit 1
fi

# A finished game dir has the streamed image plus a manifest beside the exe.
[ -f "$DEST/.xodus-streaming.msixvc" ] || fail "download did not complete (no .xodus-streaming.msixvc in $DEST)"
if ls "$DEST"/[Aa]ppx[Mm]anifest.xml "$DEST"/MicrosoftGame.[Cc]onfig >/dev/null 2>&1; then
    say "done. manifest found; the game's package identity will be derived from it at launch."
else
    say "done, but no AppxManifest.xml / MicrosoftGame.config at the top level -- the exe may live in a subfolder; pass its backslash path to launch-gdk.sh."
fi
say "next: create a launcher like scripts/launch-bedrock.sh, or run:"
echo "      scripts/launch-gdk.sh \"$DEST\" '<Main.exe>' \"$XODUS_GAMES_DIR/$(basename "$DEST")-proton\""
