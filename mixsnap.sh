#!/usr/bin/env bash
# One-shot FerroMix graph snapshot. Read-only, changes nothing.
# Usage: set up your routing in the GUI, then run:  ./mixsnap.sh
echo "=========== FERROMIX SNAPSHOT $(date -Is) ==========="
echo
echo "### 1. FerroMix nodes that exist"
pw-cli ls Node 2>/dev/null | grep -iE "ferromix\.(strip|bus)" | sed 's/^[[:space:]]*//'
echo
echo "### 2. ALL links involving ferromix (out -> in)"
pw-link -l 2>/dev/null | grep -iE "ferromix" | sed 's/^[[:space:]]*//'
echo
echo "### 3. Does a B bus reach your speakers? (should be EMPTY)"
pw-link -l 2>/dev/null | grep -iE "ferromix.bus.[345]" | grep -iE "corsair|analog|hdmi|speaker|playback" | sed 's/^[[:space:]]*//'
echo "   ^ if any lines above, a B bus is leaking to your output"
echo
echo "### 4. Which app captures each B bus (its virtual mic)"
for b in 3 4 5; do
  echo "  B$((b-2)) = ferromix.bus.$b:"
  pw-dump 2>/dev/null | grep -B30 '"ferromix.bus.'$b'"' | grep -iE "discord|webrtc|chromium|firefox|zoom|phone|call|linphone" | grep -i "node.name" | head -3 | sed 's/^/     /'
done
echo
echo "### 5. Sink/source RUNNING state (is audio actually flowing?)"
pactl list short sinks sources 2>/dev/null | grep -i ferromix
echo
echo "=========== END — paste everything above ==========="
