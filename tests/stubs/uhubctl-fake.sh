#!/bin/bash
# Fake uhubctl for tests: canned multi-device bench.
# - no action args  -> print listing (two J-Links, two PPK2s, keyboard hub)
# - "-a off|on"     -> emulate the power request round-trip, exit 0
# - "--fail" in args -> exit 1 (permission-style failure)
LISTING='Current status for hub 1-11 [2109:2817 VIA Labs, Inc. USB2.0 Hub, USB 2.10, 4 ports, ppps]
  Port 1: 0103 power enable connect [1915:c00a Nordic Semiconductor PPK2 D4184E334B85]
  Port 2: 0100 power
  Port 3: 0507 power highspeed suspend enable connect [2109:2817 VIA Labs, Inc. USB2.0 Hub, USB 2.10, 4 ports, ppps]
  Port 4: 1103 power indicator enable connect [1366:1024 SEGGER J-Link_Apollo510B_r2.0 001160002965]
Current status for hub 1-12 [2109:2817 VIA Labs, Inc. USB2.0 Hub, USB 2.10, 4 ports, ppps]
  Port 1: 0103 power enable connect [1366:1024 SEGGER J-Link 001160003881]
  Port 2: 0103 power enable connect [1915:c00a Nordic Semiconductor PPK2 AABBCCDDEEFF]'
for a in "$@"; do
  if [ "$a" = "--fail" ]; then echo "permission denied" >&2; exit 1; fi
done
ACTION=""
LOC=""
PREV=""
for a in "$@"; do
  if [ "$PREV" = "-l" ]; then LOC="$a"; fi
  if [ "$PREV" = "-a" ]; then ACTION="$a"; fi
  PREV="$a"
done
if [ -z "$ACTION" ]; then
  echo "$LISTING"
  exit 0
fi
echo "Current status for hub $LOC [fake hub]"
echo "Sent power $ACTION request"
echo "New status for hub $LOC [fake hub]"
exit 0
