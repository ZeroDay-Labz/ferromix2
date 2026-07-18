#!/usr/bin/env bash
# Captures the phone echo loop. Run DURING a call (or *43 echo test).
echo "======== PHONE ECHO TRACE $(date -Is) ========"
echo
echo "### A. Every stream the softphone owns (playback AND capture)"
pw-dump 2>/dev/null | python3 -c '
import sys,json
d=json.load(sys.stdin)
for o in d:
    p=(o.get("info") or {}).get("props") or {}
    name=(p.get("node.name") or p.get("application.name") or "").lower()
    cls=p.get("media.class","")
    binary=(p.get("application.process.binary") or "").lower()
    if any(k in (name+binary) for k in ["phone","sip","linphone","zoiper","call","pjsip","baresip","2600","2602"]):
        print(f"  id={o.get(\"id\")}  class={cls:28}  name={p.get(\"node.name\",\"?\")}")
' 2>/dev/null
echo
echo "### B. What the phone strip (strip.1 = strip 2 in UI) sends to"
pw-link -l 2>/dev/null | grep -A6 "ferromix.strip.1:monitor" | grep -iE "bus" | sed 's/^/   /'
echo
echo "### C. What each B bus feeds (who captures it)"
for b in 3 4 5; do
  echo "  B$((b-2)) (bus.$b) captured by:"
  pw-link -l 2>/dev/null | grep -A2 "ferromix.bus.$b:capture" | grep -ivE "tap|ferromix.bus" | grep -iE "phone|sip|discord|webrtc|call|linphone|zoiper" | sed 's/^/     /'
done
echo
echo "### D. THE SMOKING GUN: is the phone's capture reading a bus the phone's audio also feeds?"
echo "   (if the phone appears in both B-above and a strip that sends to that same B, that's the loop)"
echo
echo "======== paste everything above ========"
