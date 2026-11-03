#!/bin/sh
# Validate the lock locally; --remote also verifies every public fork and gitlink.
set -eu
REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
python3 "$REPO_DIR/scripts/source-pins.py" verify
if [ "${1:-}" = --remote ]; then
    python3 "$REPO_DIR/scripts/source-pins.py" verify-remote
elif [ "$#" -ne 0 ]; then
    printf 'usage: %s [--remote]\n' "$0" >&2
    exit 2
fi
