#!/usr/bin/env bash
# Proves whether mix-minus (a B-bus virtual mic) is actually reaching the app
# that's supposed to be listening to it. Companion to trace_strip.sh, which
# only covers strip INPUTS — this covers the B-bus -> app-capture side.
# Read-only. Usage: ./trace_mixminus.sh   (run while the app you assigned a
# B-bus to, e.g. Discord, is open and its mic is presumably live)
echo "======== FERROMIX2 MIX-MINUS TRACE $(date +%H:%M:%S) ========"
echo

echo "### 1. FerroMix B-buses (virtual mics) and their autoconnect/linger props"
echo "   (expect node.autoconnect=false, object.linger=true — if autoconnect"
echo "    is missing/true, WirePlumber may be fighting our own links)"
pw-dump 2>/dev/null | python3 -c '
import sys,json
for o in json.load(sys.stdin):
    p=(o.get("info") or {}).get("props") or {}
    name=p.get("node.name","")
    if name.startswith("ferromix.bus.") or (name.startswith("ferromix.") and p.get("media.class")=="Audio/Source/Virtual"):
        print(f"  id={o[\"id\"]:<5} {name:24} autoconnect={p.get(\"node.autoconnect\",\"?\")}  linger={p.get(\"object.linger\",\"?\")}  class={p.get(\"media.class\",\"?\")}")
'
echo

echo "### 2. Actual links OUT of each B-bus right now (pw-link ground truth)"
pw-link -l 2>/dev/null | grep -B1 -i "ferromix.bus" | grep -v "^--" | sed 's/^/  /'
echo

echo "### 3. Apps with a live capture (microphone) stream, and where their"
echo "   target.object metadata currently points"
pw-dump 2>/dev/null | python3 -c '
import sys,json
d=json.load(sys.stdin)
for o in d:
    p=(o.get("info") or {}).get("props") or {}
    if p.get("media.class")=="Stream/Input/Audio":
        tgt=p.get("target.object") or p.get("node.target") or "(default / unset)"
        app=p.get("application.name") or p.get("application.process.binary","?")
        st=(o.get("info") or {}).get("state","")
        print(f"  [{st:9}] {app:20} node.name={p.get(\"node.name\",\"?\"):24} target.object={tgt}")
'
echo

echo "### 4. Strip -> bus sends the daemon believes are active (from the log,"
echo "   if you are piping journalctl/daemon stdout, grep for these lines"
echo "   yourself — this script cannot read the daemon's in-memory state):"
echo "     'MIC LINK bus.N -> <app>'      = listener link was drawn"
echo "     '⚠ feedback loop blocked'      = a send was refused by the guard"
echo "     'UNLINK ... -> ...'            = a link was torn down"
echo

echo "### 5. THE KEY QUESTION: for a B-bus you assigned as an app's mic, does"
echo "   section 2 show a real link INTO that app's capture node from"
echo "   section 3? If section 1/2 show no link at all, the listener link"
echo "   was never drawn (or was torn down and never redrawn) — check the"
echo "   daemon log for 'MIC LINK' around when you made the assignment."
echo "   If the link EXISTS but the app still doesn't hear anything, the"
echo "   B-bus itself may not be receiving any strip sends — check the"
echo "   PATCH MATRIX tab for that bus's column."
echo
echo "======== paste everything above ========"
