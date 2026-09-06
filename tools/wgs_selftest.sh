#!/bin/bash
# Write-path self test for the XGameSave implementation: create, update,
# add, delete-blob and delete a container in a scratch copy of a real save
# folder, validating the on-disk result with wgs_dump.py after each step.
#
#   XGSTEST=~/xbox-games/xgstest.sh WGS_DIR=~/xbox-games/wgs-test/<XUID>_<SCID> wgs_selftest.sh
set -u
SCID=${SCID:-00000000-0000-0000-0000-0000697f9ec3}
XGSTEST=${XGSTEST:-${XODUS_GAMES_DIR:-$HOME/xbox-games}/xgstest.sh}
WGS_DIR=${WGS_DIR:-${XODUS_GAMES_DIR:-$HOME/xbox-games}/wgs-test/0009ABCD12345678_000000000000000000000000697F9EC3}
DUMP="python3 $(dirname "$0")/wgs_dump.py"
export WINEDEBUG=${WINEDEBUG:--all,err+gdkc,warn+gdkc}

run() { echo "\$ $*"; timeout 120 "$XGSTEST" "$SCID" "$@" 2>&1 | grep -v "^ntsync\|radv\|InitializeApiImpl\|XUserAddResult\|InitializeProvider"; }
check() { $DUMP "$WGS_DIR" 2>/dev/null | grep -E "^container 'SelfTest'|^    blob|!!|^total|^index" | grep -A3 "SelfTest\|!!\|^total\|^index" | grep -v "^--"; }
py_hash() { python3 -c "
import sys
h=0
for b in open(sys.argv[1],'rb').read(): h=(h*31+b)&0xffffffff
print(f'{h:08x}', len(open(sys.argv[1],'rb').read()))" "$1"; }

A=${XODUS_GAMES_DIR:-$HOME/xbox-games}/wgs-test/baseline.txt
B=/etc/os-release
C=${XODUS_GAMES_DIR:-$HOME/xbox-games}/wgs-test/dll/xgameruntime.dll
winpath() { echo "Z:$(echo "$1" | sed 's#/#\\#g')"; }

echo "### expected hashes: A=$(py_hash $A) B=$(py_hash $B) C=$(py_hash $C)"
echo "### 1. create container with blob Data (A)";  run write SelfTest Data "$(winpath $A)" "Self Test Display"; check
echo "### 2. async update Data (B): generation bump"; run writeasync SelfTest Data "$(winpath $B)"; check
echo "### 3. add second blob Big (C, 1.6MB)";      run write SelfTest Big "$(winpath $C)"; check
echo "### 4. read both back";                       run read SelfTest
echo "### 5. read by name";                         run read SelfTest Big
echo "### 6. delete blob Data";                     run delblob SelfTest Data; check
echo "### 7. delete container";                     run delete SelfTest; check
echo "### 8. folder contents after delete (expect 21 entries: 20 + index):"; ls "$WGS_DIR" | wc -l
echo "### 9. diff against baseline (only index mtime may differ):"
$DUMP "$WGS_DIR" 2>/dev/null | diff <(grep -v "mtime=" ${XODUS_GAMES_DIR:-$HOME/xbox-games}/wgs-test/baseline.txt) <(grep -v "^  aumid" /dev/stdin | grep -v "mtime=" ) && echo "IDENTICAL apart from timestamps" 
