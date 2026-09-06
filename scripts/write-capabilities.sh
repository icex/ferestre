#!/bin/bash
# Write the capability manifest a runtime build publishes about itself.
#
# The launcher decides whether a title can run by comparing what the recipe
# needs against what the runtime provides. Without this file it falls back to
# probing, which finds four capabilities out of twelve because most of them
# leave no signature a generic probe could recognise -- so every title carries
# "could not be found" wording it does not deserve, and the launcher cannot tell
# "missing" from "we did not look properly".
#
# This does not take the build's word for anything. Every capability in
# titles/capabilities.toml carries `verify` markers: a file inside the installed
# runtime and a string that is only there once the patch is. Each is checked
# against the real tree, and only what matches is written down. The result is a
# manifest that was measured rather than asserted, which is the only kind worth
# reading.
#
#   scripts/write-capabilities.sh [runtime-dir]
#
# Defaults to $XODUS_PROTON_DIR. Exits non-zero if the runtime is missing or
# unreadable; a capability that fails its check is reported and omitted, which
# is not an error -- a slimmed or older build genuinely has fewer.

set -euo pipefail
. "$(dirname "$0")/xodus-env.sh"

REPO_DIR=${REPO_DIR:-$XODUS_REPO_DIR}
TOOL_DIR=${1:-${XODUS_PROTON_DIR:-}}
REGISTRY=$REPO_DIR/titles/capabilities.toml

[ -n "$TOOL_DIR" ] || { echo "!! no runtime directory: pass one or set XODUS_PROTON_DIR" >&2; exit 1; }
[ -d "$TOOL_DIR" ] || { echo "!! $TOOL_DIR is not a directory" >&2; exit 1; }
[ -f "$REGISTRY" ] || { echo "!! $REGISTRY is missing" >&2; exit 1; }

exec python3 - "$REGISTRY" "$TOOL_DIR" <<'PY'
import json
import os
import subprocess
import sys
import tomllib

registry, tool_dir = sys.argv[1], sys.argv[2]
with open(registry, "rb") as f:
    capabilities = tomllib.load(f)["capability"]

def contains(path, needle):
    """Whether a marker string is in a file.

    `strings` rather than a substring search, because these are ELF and PE
    binaries where a literal can sit anywhere, and reading a 200 MB DLL into
    memory to search it is not worth doing when the tool exists. Falls back to
    a byte search so this still works on a machine without binutils.
    """
    full = os.path.join(tool_dir, path)
    if not os.path.isfile(full):
        return False
    try:
        out = subprocess.run(
            ["strings", "-a", full], capture_output=True, check=True
        ).stdout
        return needle.encode() in out
    except (FileNotFoundError, subprocess.CalledProcessError):
        with open(full, "rb") as fh:
            return needle.encode() in fh.read()

provided, missing = [], []
for capability in capabilities:
    name = capability["name"]
    markers = capability.get("verify") or []
    if not markers:
        # A capability with no way to check it cannot be published: the whole
        # point is that the manifest is measured.
        missing.append((name, "no verify markers in the registry"))
        continue
    failed = [m for m in markers if not contains(m["file"], m["contains"])]
    if failed:
        missing.append((name, f"{failed[0]['file']} lacks {failed[0]['contains']!r}"))
    else:
        provided.append(name)

out_path = os.path.join(tool_dir, "files", "share", "ferestre", "capabilities.json")
os.makedirs(os.path.dirname(out_path), exist_ok=True)
with open(out_path, "w") as f:
    json.dump({"schema": 1, "capabilities": sorted(provided)}, f, indent=2)
    f.write("\n")

print(f":: {len(provided)}/{len(capabilities)} capabilities verified -> {out_path}")
for name, why in missing:
    print(f"   not provided: {name} ({why})")
PY
