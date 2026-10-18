#!/bin/sh
# Sign release artefacts with minisign, writing <file>.minisig beside each.
#
#   scripts/sign-release.sh <file>...
#
# The launcher downloads the runtime archive and refuses it unless
# <archive>.minisig verifies against the public key compiled into
# crates/ferestre-core/src/signing.rs, and unless the signature's trusted
# comment names the archive. This makes exactly such signatures and then checks
# each one against that compiled-in key, so a release signed with the wrong key
# fails here rather than on every user's machine.
#
# Environment:
#   MINISIGN_SECRET_KEY   the whole minisign secret key file (from the release
#                         secrets; never committed)
#   MINISIGN_PASSWORD     its password, when it has one
#   REQUIRE_SIGNATURE=1   fail when there is no key or the launcher still holds
#                         the placeholder key; set for tags. Without it, a run
#                         with no key only warns, so a manual run can still
#                         build everything else.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
source_file=${FERESTRE_SIGNING_SOURCE:-$here/../crates/ferestre-core/src/signing.rs}
require=${REQUIRE_SIGNATURE:-0}
placeholder=FERESTRE_RELEASE_PUBLIC_KEY_PLACEHOLDER

fail() {
    echo "sign-release: $*" >&2
    exit 1
}

[ "$#" -gt 0 ] || fail "usage: $0 <file>..."

if [ -z "${MINISIGN_SECRET_KEY:-}" ]; then
    [ "$require" = 1 ] && fail "no signing key: set the MINISIGN_SECRET_KEY secret"
    echo "sign-release: no signing key; artefacts are left unsigned" >&2
    exit 0
fi

public_key=$(sed -n 's/^pub const RELEASE_PUBLIC_KEY: &str = "\(.*\)";$/\1/p' "$source_file")
[ -n "$public_key" ] || fail "cannot find RELEASE_PUBLIC_KEY in $source_file"
if [ "$public_key" = "$placeholder" ]; then
    [ "$require" = 1 ] && fail "the launcher still holds the placeholder release key"
    echo "sign-release: the launcher holds the placeholder key; signatures are not checked" >&2
fi

command -v minisign >/dev/null 2>&1 || fail "minisign is not installed"

umask 077
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
printf '%s\n' "$MINISIGN_SECRET_KEY" > "$work/secret.key"
tab=$(printf '\t')

for file in "$@"; do
    [ -f "$file" ] || fail "$file: no such file"
    name=$(basename -- "$file")
    # The launcher requires file:<name> in the signed comment, so a signature
    # for one artefact cannot be presented for another. Stated explicitly
    # rather than relying on each minisign version's default comment.
    comment="timestamp:$(date +%s)${tab}file:${name}${tab}hashed"
    printf '%s\n' "${MINISIGN_PASSWORD:-}" |
        minisign -S -s "$work/secret.key" -m "$file" -x "$file.minisig" -t "$comment" >/dev/null ||
        fail "$file: signing failed"
    # Only pre-hashed signatures are accepted by the launcher.
    algorithm=$(sed -n 2p "$file.minisig" | base64 -d 2>/dev/null | head -c 2)
    [ "$algorithm" = ED ] || fail "$file: minisign wrote a legacy signature; use minisign 0.10 or later"
    if [ "$public_key" != "$placeholder" ]; then
        minisign -V -q -P "$public_key" -m "$file" -x "$file.minisig" ||
            fail "$file: the signature does not verify against the launcher's release key"
    fi
    echo "sign-release: signed $name"
done
