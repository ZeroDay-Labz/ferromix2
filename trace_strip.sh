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
        name=p.get("node.name","?")
        app=p.get("application.name","?")
        binary=p.get("application.process.binary","?")
        print(f"  [{st:9}] {name:30} app={app}  binary={binary}")
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
echo "   (checks the 'default' METADATA object's target.object key per node"
echo "    id — that's the actual mechanism FerroMix and pw-metadata use, NOT"
echo "    a property on the stream node itself, which is always empty)"
python3 -c '
import subprocess, re, json

dump = json.loads(subprocess.run(["pw-dump"], capture_output=True, text=True).stdout or "[]")
names = {}
apps = {}
for o in dump:
    p = (o.get("info") or {}).get("props") or {}
    if p.get("media.class") == "Stream/Output/Audio":
        names[o["id"]] = p.get("node.name", "?")
        apps[o["id"]] = p.get("application.name", "?")

targets = {}
try:
    meta = subprocess.run(["pw-metadata"], capture_output=True, text=True).stdout
    for line in meta.splitlines():
        m = re.search(r"id:(\d+) key:.target.object. value:.\"?([^\x27\"]+)\"?.", line)
        if m and "target.object" in line:
            nid = int(m.group(1))
            val = m.group(2)
            targets[nid] = val
except Exception as e:
    print(f"  (could not read pw-metadata: {e})")

for nid, app in apps.items():
    tgt = targets.get(nid, "(default — no target.object set)")
    name = names.get(nid, "?")
    print(f"  {app:20} [{name}, id={nid}] -> target: {tgt}")
'
echo
echo "======== paste everything above ========"
