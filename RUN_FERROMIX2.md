# FerroMix2 — v2.0.0

Renamed from ferromix. New binaries, socket, and config path so it's a clean
separate project (won't collide with the old one).

## Run

Terminal 1 — daemon (owns audio):
    cargo run -p mixer-daemon        # binary: ferromix2-daemon

Terminal 2 — Iced GUI:
    cargo run -p mixer-gui-iced      # binary: ferromix2

If the window doesn't appear on KDE Wayland:
    WINIT_UNIX_BACKEND=x11 cargo run -p mixer-gui-iced

## New in v2.0.0
- Renamed to FerroMix2 (socket: ferromix2.sock, config: ~/.config/ferromix2/)
- Iced GUI with live daemon connection, real strips + buses
- GATE + COMP knobs on every strip (click label to toggle)
- DSP backend foundation: per-strip filter-chain (builtin gate + compressor),
  knob→parameter mapping tested (gate -60..-20dB, comp 1.5:1..8:1)

## What's wired vs pending
- Knobs: toggle on/off works and persists; the value is stored and mapped to
  real gate/comp parameters. LIVE audio processing (inserting the filter-chain
  into the running graph) is the next backend round — the SPA-JSON generator
  and tests are done, it needs wiring into the reconciler.
- Fader drag, device dropdowns, SEND TO APP, matrix, settings: round 2 UI work.

## Notes
- The old egui GUI (`mixer-gui`) has been removed — `mixer-gui-iced` is now
  the only GUI and has full feature parity (rename, record-arm, log tab,
  UI-scale, recordings-dir editing, add-strip).
- DSP tests: cargo test -p mixer-pw --lib dsp

## v2.0.1 — routing bleed fix (critical)
Fixed the standoff where B-buses were created with node.autoconnect=false
(telling WirePlumber "don't touch") while bus-listener assignment relied on
target.object metadata (asking WirePlumber to route) — which it refused, so
apps' mics fell back to grabbing the raw default source (the Spotify+mic bleed
into Discord seen in qpwgraph).

Now FerroMix draws the B-bus → app-mic link ITSELF (Slot::BusListener), so it
never depends on WirePlumber cooperating. Log line: "MIC LINK bus.N -> <app>".

## Still pending (next UI round)
- Device dropdowns on strips + A1/A2/A3 hardware-out row (can't pick hw yet)
- Draggable faders, SEND TO APP picker, matrix + settings tabs
- Low-latency quantum setting (the Linux equivalent of ASIO)

## v2.1.0 — the interactive UI round
Everything is now wired and controllable:
- INPUT SOURCE dropdown on every strip (pick your mic or an app)
- HARDWARE OUT row: A1/A2/A3 device dropdowns across the top
- SEND TO APP dropdown on every B bus (assign an app's mic)
- Draggable faders with scroll-wheel support (hover + scroll to nudge)
- Draggable/scrollable GATE + COMP knobs (scroll to set amount, click to toggle)
- A1/A2/A3 + B1/B2/B3 send pills on every strip
- MONITOR ON row on B buses
- Full-width layout using the whole window

## Still pending (next round)
- MATRIX tab (grid view) + SETTINGS tab (recordings dir, feedback guard,
  UI scale, low-latency quantum = the Linux "ASIO" equivalent)
- App icon

## v2.2.0 — matrix + settings + bus-mute fix
- MATRIX tab: full patch grid (strips × buses), click any cell to route.
  Cyan = hardware, violet = virtual mic, ✕ = feedback blocked.
- SETTINGS tab: feedback-guard toggle, recordings folder, the low-latency
  quantum command (Linux "ASIO" equivalent), and a routing primer.
- FIX: muting a B-bus now cuts its output links — muting B2 actually stops
  your voice reaching Discord (was only setting a flag before).

## Understanding the B-bus meter (not a bug)
B2's meter rising when YOU speak is correct: a B-bus is a virtual MIC carrying
what you SEND to the app. Discord's INCOMING call audio arrives on the WEBRTC
*strip* — that strip's meter moves when someone talks in VC, and that strip -> A1
is how you hear them. To stop hearing Discord, unroute the WEBRTC strip from A1;
to stop Discord hearing you, mute B2.

## v2.3.0 — interaction + a diagnostic
- Knobs now: click+drag vertically to set, scroll to fine-tune, right-click resets.
  New look: gradient glow ring + tick marks.
- Faders: scroll to nudge, right-click snaps back to 0.0 dB (unity).
- Added trace_strip.sh — run it WHILE an app plays audio; it proves whether
  the app's output is reaching its FerroMix strip. THIS is how we fix the
  "meter doesn't move" issue for real.

## Diagnosing "app audio doesn't reach the strip"
For a strip to show an app's audio, the app's OUTPUT must point at that strip's
sink (named "FerroMix Input N") — exactly like Voicemeeter needs the app's
output set to "Voicemeeter Input". FerroMix also tries to pull apps assigned via
the dropdown. To see where the break is, run ./trace_strip.sh while audio plays
and paste the output.

## v2.4.0 — SET AS DEFAULT (the fix for "app audio doesn't reach the strip")
The Iced GUI was missing the default-device buttons. Now:
- Every strip has "SET AS DEFAULT" — makes it the system default OUTPUT. Any app
  on "default" (Spotify, etc.) then flows into that strip automatically. THIS is
  why Spotify's meter wasn't moving — it was on default with nowhere to route.
- Every B bus has "SET AS DEF MIC" — makes it the system default INPUT.

### To get Spotify (or any "default" app) onto a strip, either:
  A) Set the app's OUTPUT to "FerroMix Input N" in the app/KDE audio settings, OR
  B) Click SET AS DEFAULT on the strip you want desktop audio on — then every
     default app lands there automatically.

## v2.5.0 — persistence, mix-minus fix, live DSP, full visual overhaul
- FIX: the Iced GUI never sent Command::Save — every fader/route/rename was
  lost on daemon restart. Now autosaves ~1.5s after the last change, plus a
  header SAVE button that shows dirty state.
- FIX (mix-minus): `apply_bus_listeners` (the B-bus -> app-mic link) was only
  called from a few command handlers, never from node-removal events — an
  app's capture stream disappearing/reappearing (reconnect) could permanently
  strand a bus's mix-minus feed. Folded into a `reconcile_all` that runs on
  every convergence pass, matching the reconciler's own declarative design.
  Also fixed `resolve_capture` to fall back to substring matching like
  `resolve_source` already did. New `trace_mixminus.sh` diagnostic.
- DSP is now LIVE: gate/compressor actually process audio, verified end-to-end
  against a running daemon (`pw-dump`/`pw-link -l` confirm `ferromix.dsp.N.in`
  sits between the source and the strip, and `.out` feeds the strip). Four
  real bugs found by loading the module for real instead of trusting the
  SPA-JSON generator's unit tests:
  1. The gate's builtin label is `noisegate`, not `gate` (module refused to
     load: "cannot create label gate").
  2. Its threshold control keys are case-sensitive: `Open Threshold`/`Close
     Threshold`, not `Open threshold`/`Close threshold`.
  3. Those threshold ports are LINEAR AMPLITUDE (SPA range 0.0..1.0), not dB —
     the original code passed raw dB values (e.g. -60.0), which silently
     clamped to the port's minimum, so the gate loaded fine but every
     threshold setting behaved identically. Now converted via `db_to_lin`.
  4. PipeWire's builtin filter-graph has NO compressor at all, and the gate
     turns out to be MONO while a proper compressor needs stereo — so the
     graph is now two `noisegate` instances (`gate_l`/`gate_r`) feeding the
     SC4 LADSPA compressor's Left/Right inputs (`ladspa-swh-plugins`, new
     runtime dependency; exact port names confirmed with `analyseplugin
     sc4_1882.so`), not a single mono gate->comp chain.
  A knob change reloads the strip's filter-chain module (destroy + recreate
  with the new values baked into fresh SPA-JSON) rather than pushing live
  params into the running chain's internal nodes — simpler and safe, costs a
  few ms of dropout on that strip only. Confirmed the reload path replaces
  cleanly (new node ids each time, no leaked/duplicate `ferromix.dsp.*` nodes).
- Full visual overhaul: bundled Inter font, an SVG icon set replacing the
  Unicode glyphs, a design-token module (spacing/radius/type scale) instead
  of scattered magic numbers, hand-drawn canvas faders (matching the DSP
  knob's glow language) replacing the restyled built-in slider, a bloom
  highlight on the VU meter's peak segment, and a resizable window with
  strip/bus cards that shrink responsively instead of clipping.
- New controls: click-to-rename strip/bus headers, REC arm buttons, an
  ACTIVITY LOG tab (was tracked in state, never shown), a UI-scale stepper,
  and an editable recordings-dir field — all previously-unwired `Command`s.
- The old egui GUI (`mixer-gui`) is removed; `mixer-gui-iced` has full parity.
