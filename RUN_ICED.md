# FerroMix — Iced GUI (round 1)

This is the NEW Iced-based GUI, running alongside the existing engine.
The old egui GUI still works; this is additive and safe.

## Run it

Terminal 1 — the daemon (unchanged, owns audio):
    cargo run -p mixer-daemon

Terminal 2 — the NEW Iced console:
    cargo run -p mixer-gui-iced

It connects to the same daemon socket as the old GUI and shows your real
strips and buses live. The header shows ● LIVE when connected.

## If the window doesn't appear (KDE Wayland)

Force XWayland or software GL, same as before:
    WINIT_UNIX_BACKEND=x11 cargo run -p mixer-gui-iced
    WGPU_BACKEND=gl cargo run -p mixer-gui-iced

## What works in round 1
- Live connection to the daemon, real strips + buses
- Indigo/glass theme, cyan (A) / violet (B) accents
- Meters, dB readouts, send pills, mute buttons
- Console tab (matrix + settings come in round 2)

## What's next (round 2)
- Wire every control: faders drag, device dropdowns, SEND TO APP, record rack
- Port the matrix and settings tabs
- Then retire the egui crate
