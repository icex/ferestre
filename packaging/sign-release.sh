#!/bin/bash
# Sign a release's artefacts with minisign, beside the artefacts themselves.
#
#   packaging/sign-release.sh DIR LABEL
#
# Every regular file in DIR gets a DIR/<file>.minisig. DIR must hold a
# SHA256SUMS that covers everything else in it, and it is checked first: the
# publish job signs artefacts that another job built and handed over, and a
# signature over a file that changed on the way would make the change look
# vouched for.
#
# The secret key comes from the environment, never from a file in the tree:
#
#   MINISIGN_SECRET_KEY   the whole secret key file (both lines), from a CI secret
#   MINISIGN_PASSWORD     its password; leave unset for a key made with -W
#
# Each signature's trusted comment is `file:<name> release:<LABEL>`. The
# trusted comment is covered by the signature, so a verifier that checks the
# file: field cannot be handed a validly signed but different artefact under
# this one's name.
#
# Optionally, FERESTRE_EXPECT_PUBKEY_IN names a file -- the launcher binary
# being published -- that must contain this key's public half in minisign's
# base64 form. The launcher refuses a runtime that does not verify against the
# key built into it, so publishing with a secret key that does not match that
# key would break every install; this is where that is caught instead.

set -euo pipefail

die() { printf '!! %s\n' "$1" >&2; shift; for l in "$@"; do printf '   %s\n' "$l" >&2; done; exit 1; }

[ $# -eq 2 ] || die "usage: $0 DIR LABEL"
DIR=$1
LABEL=$2
[ -d "$DIR" ] || die "not a directory: $DIR"
case "$LABEL" in
    *[!A-Za-z0-9._+-]*|"") die "not a usable release label: $LABEL" ;;
esac
command -v minisign >/dev/null 2>&1 || die "minisign is not installed"
[ -n "${MINISIGN_SECRET_KEY:-}" ] || die \
    "MINISIGN_SECRET_KEY is empty" \
    "set it from the repository secret of the same name"
[ -f "$DIR/SHA256SUMS" ] || die "no SHA256SUMS in $DIR"

# Everything except the signatures must be listed in SHA256SUMS, and every
# listed file must match. A file that is not listed would otherwise be signed
# without ever having been checked.
( cd "$DIR" && sha256sum --quiet --strict -c SHA256SUMS ) || die \
    "the artefacts do not match their SHA256SUMS" \
    "refusing to sign files that changed after they were built"
for f in "$DIR"/*; do
    name=$(basename "$f")
    case "$name" in *.minisig|SHA256SUMS) continue ;; esac
    [ -f "$f" ] || die "not a regular file: $name"
    awk -v n="$name" '{ sub(/^\.\//, "", $2); if ($2 == n) found = 1 } END { exit !found }' \
        "$DIR/SHA256SUMS" || die "$name is not listed in SHA256SUMS"
done

# The key lives only for the length of this script, readable only by us, and
# outside DIR so it cannot be uploaded with the artefacts.
umask 077
work=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/ferestre-sign.XXXXXX")
trap 'rm -rf "$work"' EXIT
printf '%s\n' "$MINISIGN_SECRET_KEY" > "$work/secret.key"

# minisign reads the password from standard input when that is not a
# terminal. An unencrypted key never asks, so the empty line is harmless.
with_password() { printf '%s\n' "${MINISIGN_PASSWORD:-}" | "$@"; }

with_password minisign -R -s "$work/secret.key" -p "$work/public.key" >/dev/null \
    || die "could not read the secret key (wrong MINISIGN_PASSWORD?)"
pubkey=$(sed -n 2p "$work/public.key")
printf ':: signing with public key %s\n' "$pubkey"

if [ -n "${FERESTRE_EXPECT_PUBKEY_IN:-}" ]; then
    [ -f "$FERESTRE_EXPECT_PUBKEY_IN" ] || die "no such file: $FERESTRE_EXPECT_PUBKEY_IN"
    grep -qF -- "$pubkey" "$FERESTRE_EXPECT_PUBKEY_IN" || die \
        "$(basename "$FERESTRE_EXPECT_PUBKEY_IN") does not contain the public key of the signing key" \
        "public key: $pubkey" \
        "The launcher verifies downloads against the key built into it, so these" \
        "signatures would be refused by the launcher being published. Either the" \
        "secret is the wrong key, or ferestre-core still carries a placeholder."
fi

for f in "$DIR"/*; do
    name=$(basename "$f")
    case "$name" in *.minisig) continue ;; esac
    rm -f "$f.minisig"
    with_password minisign -S -s "$work/secret.key" -m "$f" -x "$f.minisig" \
        -t "file:$name release:$LABEL" >/dev/null \
        || die "could not sign $name"
    # Checked with the public half, the way a downloader will check it.
    minisign -Vq -p "$work/public.key" -m "$f" -x "$f.minisig" \
        || die "the signature just made for $name does not verify"
    printf '   signed %s\n' "$name"
done
