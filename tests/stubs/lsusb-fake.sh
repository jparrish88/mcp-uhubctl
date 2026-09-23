#!/bin/bash
# Fake lsusb for tests: reports VID:PIDs listed in $FAKE_PRESENT (space-separated).
# Usage: lsusb-fake.sh -d 1915:c00a
WANT=""
PREV=""
for a in "$@"; do
  if [ "$PREV" = "-d" ]; then WANT="$a"; fi
  PREV="$a"
done
for v in $FAKE_PRESENT; do
  if [ "$v" = "$WANT" ]; then
    echo "Bus 001 Device 099: ID $WANT Fake Device"
  fi
done
exit 0
