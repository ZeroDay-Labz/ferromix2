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
- The old egui GUI (ferromix2-gui) still builds if you need it.
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
