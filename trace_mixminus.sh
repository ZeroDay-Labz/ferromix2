#!/usr/bin/env bash
# Proves whether mix-minus (a B-bus virtual mic) is actually reaching the app
# that's supposed to be listening to it — AND that it's the ONLY thing
# reaching it. Companion to trace_strip.sh, which covers strip INPUTS; this
# covers the B-bus -> app-capture side. Read-only.
# Usage: ./trace_mixminus.sh   (run while the app you assigned a B-bus to,
# e.g. Discord, is open and its mic is presumably live)
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
        oid=o["id"]
        auto=p.get("node.autoconnect","?")
        linger=p.get("object.linger","?")
        cls=p.get("media.class","?")
        print(f"  id={oid:<5} {name:24} autoconnect={auto}  linger={linger}  class={cls}")
'
echo

echo "### 2. Actual links OUT of each B-bus right now (pw-link ground truth)"
pw-link -l 2>/dev/null | grep -B1 -i "ferromix.bus" | grep -v "^--" | sed 's/^/  /'
echo

echo "### 3. THE EXCLUSIVITY CHECK — for every app capture node fed by a"
echo "   FerroMix B-bus, is a B-bus the ONLY thing feeding it? A real mic"
echo "   or any other source showing up here means the app is hearing BOTH"
echo "   mixed together — this was the actual 'B-bus routing doesn't work'"
echo "   bug (WirePlumber auto-connects the app's real default mic before"
echo "   FerroMix's own link can land, and nothing used to cut it)."
python3 -c '
import subprocess, json

dump = json.loads(subprocess.run(["pw-dump"], capture_output=True, text=True).stdout or "[]")
names = {}
bus_ids = set()
cap_ids = []
for o in dump:
    p = (o.get("info") or {}).get("props") or {}
    name = p.get("node.name", "")
    if "node.name" in p:
        names[o["id"]] = name
    if name.startswith("ferromix.bus."):
        bus_ids.add(o["id"])
    if p.get("media.class") == "Stream/Input/Audio" and not name.startswith("ferromix."):
        cap_ids.append((o["id"], p.get("application.name") or p.get("application.process.binary", "?")))

# out_node -> set of in_nodes, from live link objects (ground truth).
feeds = {}
for o in dump:
    if o.get("type", "").endswith("Link"):
        info = o.get("info") or {}
        out_id, in_id = info.get("output-node-id"), info.get("input-node-id")
        feeds.setdefault(in_id, set()).add(out_id)

any_bus_listener = False
for cap_id, app in cap_ids:
    sources = feeds.get(cap_id, set())
    bus_sources = sources & bus_ids
    if not bus_sources:
        continue  # this app is not a FerroMix bus listener at all
    any_bus_listener = True
    stray = sources - bus_ids
    bus_names = ", ".join(names.get(b, str(b)) for b in bus_sources)
    print(f"  {app:20} [id={cap_id}] fed by bus(es): {bus_names}")
    if stray:
        stray_names = ", ".join(names.get(s, str(s)) for s in stray)
        print(f"    !! ALSO fed by: {stray_names}  <-- EXCLUSIVITY BROKEN, this is the bug")
    else:
        print(f"    OK — exclusively fed by FerroMix bus(es), nothing else")

if not any_bus_listener:
    print("  (no app currently has a FerroMix B-bus feeding its microphone —")
    print("   assign one via SEND TO APP on a bus card first)")
'
echo

echo "### 4. Daemon log lines to grep for (if piping journalctl/daemon stdout):"
echo "     'MIC LINK bus.N -> <app>'                = listener link was drawn"
echo "     'REDIRECT <app> off <node> (mic now ...)' = a stray link was cut"
echo "     '⚠ feedback loop blocked'                 = a send was refused by the guard"
echo "     'UNLINK ... -> ...'                       = a link was torn down"
echo

echo "======== paste everything above ========"
