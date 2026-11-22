#!/bin/bash
# Block until the launcher's install queue finishes another title.
#
#   tools/wait-for-install.sh
#
# Prints "installed: <PRODUCTID>.json" (one line per new record) and exits 0
# when a title gets an install record it did not have when this started, or
# "queue empty" when nothing is left running or pending. Meant to be run in the
# background by whoever tests titles as they arrive, then followed by
# tools/headless-title.sh <PRODUCTID>.
#
# Reads only ~/.local/state/ferestre/installed/ and install-queue.json, once a
# minute. Changes nothing.
state=${FERESTRE_STATE_DIR:-$HOME/.local/state/ferestre}
seen=$(ls "$state/installed/" 2>/dev/null)
while :; do
    now=$(ls "$state/installed/" 2>/dev/null)
    new=$(comm -13 <(echo "$seen") <(echo "$now"))
    if [ -n "$new" ]; then echo "$new" | sed 's/^/installed: /'; exit 0; fi
    left=$(python3 -c 'import json,sys
print(sum(j["status"] in ("running","pending") for j in json.load(open(sys.argv[1]))["jobs"]))' \
        "$state/install-queue.json" 2>/dev/null)
    if [ "$left" = 0 ]; then echo "queue empty"; exit 0; fi
    sleep 60
done
