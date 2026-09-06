#!/bin/bash
# Launch Expedition 33 briefly with the GDK and GStreamer channels enabled,
# then report what the save and video paths actually did.
#
#   verify-gdk.sh [seconds]
#
# Leaves nothing running: several instances at once starve the GPU and look
# exactly like the game being slow.

. "$(dirname "$0")/xodus-env.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
SECS=${1:-70}
LOG=$XODUS_GAMES_DIR/verify.log

"$HERE/kill-gdk.sh" >/dev/null 2>&1
: > "$LOG"

WINEDEBUG="+gdkc,fixme-all" GST_DEBUG="GST_ELEMENT_FACTORY:4,protonmediaconverter:4" \
    timeout "$SECS" "$HERE/launch-exp33.sh" >"$LOG" 2>&1

echo "=== provider / package identity ==="
grep -aE "resolve_package_identity|create_provider|index_read" "$LOG" | head -5

echo
echo "=== XGameSave activity ==="
grep -aoE "x_game_save_XGameSave[A-Za-z]+" "$LOG" | sort | uniq -c | sort -rn | head -15

echo
echo "=== containers the title was shown ==="
grep -a "report_container" "$LOG" | head -8
echo "  (total reported: $(grep -ac 'report_container' "$LOG"))"

echo
echo "=== blobs read ==="
grep -aE "read blob|enumerate_blobs" "$LOG" | head -8

echo
echo "=== save errors ==="
grep -aE "err:gdkc|warn:gdkc" "$LOG" | head -8

echo
echo "=== video: which decoder was chosen ==="
grep -aE "protonvideoconverter|avdec_h264|qtdemux|Setting rank" "$LOG" | head -10

echo
echo "=== crashes ==="
grep -aE "EXCEPTION_|LowLevelFatalError|Fatal error|exited rc=" "$LOG" | head -5

"$HERE/kill-gdk.sh"
