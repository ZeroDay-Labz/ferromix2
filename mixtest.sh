#!/usr/bin/env bash
# FerroMix routing test harness — run this, do the actions it prompts, paste output.
# Safe: it only READS the graph, makes no changes.
set -uo pipefail

line() { printf '─%.0s' {1..60}; echo; }
snap() {
  echo "### $1"
  echo "-- ferromix nodes --"
  pw-cli ls Node 2>/dev/null | grep -iE "ferromix\.(strip|bus)" | sed 's/^[[:space:]]*//'
  echo "-- links touching ferromix buses --"
  pw-link -l 2>/dev/null | grep -iE "ferromix\.bus" | sed 's/^[[:space:]]*//' | head -40
  echo "-- what apps read each B bus (their mic) --"
  for b in 3 4 5; do
    printf "B%s (bus.%s): " "$((b-2))" "$b"
    pw-link -l 2>/dev/null | grep -A1 "ferromix.bus.$b" | grep -iE "discord|webrtc|chromium|firefox|zoom|phone|call" | head -3 | tr '\n' ' '
    echo
  done
  line
}

echo "FERROMIX TEST HARNESS  ($(date -Is))"
echo "PipeWire: $(pw-cli info 0 2>/dev/null | grep -i version | head -1)"
line
snap "STATE 0: baseline (daemon running, no call yet)"

echo ">> TEST 1: In the GUI, put your MIC on strip 1 and light B1+B2 on it."
echo ">> Then press ENTER."
read -r
snap "STATE 1: mic -> B1+B2"

echo ">> TEST 2: Now click MUTE on the mic strip. Press ENTER."
read -r
snap "STATE 2: mic MUTED (expect: NO links from strip.0 to any bus)"

echo ">> TEST 3: Un-mute. Press ENTER."
read -r
snap "STATE 3: mic un-muted (links should return)"

echo "DONE. Copy everything above and paste it back."
