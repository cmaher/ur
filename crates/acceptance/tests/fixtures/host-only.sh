#!/bin/sh
# Hostexec fixture script.
# Writes $1 (the tag) to the file path given as $2 (the marker file).
# Runs on the host via builderd — used by acceptance tests to verify
# the full script dispatch flow.
if [ -z "$1" ] || [ -z "$2" ]; then
    echo "usage: host-only.sh <tag> <marker-file>" >&2
    exit 1
fi
printf '%s' "$1" > "$2"
echo "host-only: wrote tag '$1' to '$2'"
