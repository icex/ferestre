#!/bin/bash
# Compile the test-only service fixture; no credentials or production service.
set -euo pipefail
CLIENT_REPO=${CLIENT_REPO:-${XODUS_SRC_DIR:-$HOME/src}/xodus-cli}
(cd "$CLIENT_REPO" && cargo test --locked -p xodus-service --no-run --message-format=json) |
    python3 -c 'import json,sys
paths=[m["executable"] for line in sys.stdin if (m:=json.loads(line)).get("reason")=="compiler-artifact" and m.get("executable") and m.get("profile",{}).get("test")]
if len(paths)!=1: raise SystemExit("expected one service test executable")
print(paths[0])'
