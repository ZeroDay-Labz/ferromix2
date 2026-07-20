#!/usr/bin/env bash
# Proves whether an app's audio reaches its assigned strip. Read-only.
# Usage: ./trace_strip.sh          (run while an app is PLAYING audio)
echo "======== FERROMIX2 AUDIO-FLOW TRACE $(date +%H:%M:%S) ========"
echo
echo "### 1. Every app currently PLAYING audio (output streams)"
pw-dump 2>/dev/null | python3 -c '
import sys,json
for o in json.load(sys.stdin):
    p=(o.get("info") or {}).get("props") or {}
    if p.get("media.class")=="Stream/Output/Audio":
        st=(o.get("info") or {}).get("state","")
        print(f"  [{st:9}] {p.get(\"node.name\",\"?\"):30} app={p.get(\"application.name\",\"?\")}  binary={p.get(\"application.process.binary\",\"?\")}")
'
echo
echo "### 2. FerroMix strips and what feeds them RIGHT NOW"
for n in 0 1 2 3 4; do
  echo "  ferromix.strip.$n  <- fed by:"
  pw-link -l 2>/dev/null | grep -B1 "ferromix.strip.$n:playback" | grep -iv "ferromix.strip.$n\|^--\|monitor" | grep -iE "spotify|discord|webrtc|chrom|firefox|phone|call|input" | sed 's/^/       /' | sort -u
done
echo
echo "### 3. Are strips RUNNING (audio flowing) or IDLE?"
pactl list short sinks 2>/dev/null | grep ferromix.strip
echo
echo "### 4. THE KEY QUESTION: for each playing app, is its output"
echo "   pointed at a ferromix strip, or at hardware/default?"
pw-dump 2>/dev/null | python3 -c '
import sys,json
d=json.load(sys.stdin)
# map node id -> name
names={o["id"]:((o.get("info") or {}).get("props") or {}).get("node.name","?") for o in d}
for o in d:
    p=(o.get("info") or {}).get("props") or {}
    if p.get("media.class")=="Stream/Output/Audio":
        tgt=p.get("target.object") or p.get("node.target") or "(default)"
        app=p.get("application.name","?")
        print(f"  {app:20} -> target: {tgt}")
'
echo
echo "======== paste everything above ========"
