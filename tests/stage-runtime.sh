#!/bin/sh
# An independent runtime tree: reflinks where supported, copies across filesystems.
set -eu
[ "$#" -eq 2 ] && [ -d "$1" ] && [ ! -e "$2" ] || {
    echo 'usage: stage-runtime.sh SOURCE NEW_DESTINATION' >&2
    exit 2
}
cp -a --reflink=auto "$1" "$2"
