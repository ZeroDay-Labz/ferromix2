# FerroMix2 — v2.0.0

Renamed from ferromix. New binaries, socket, and config path so it's a clean
separate project (won't collide with the old one).

## Run

    cargo run -p mixer-gui-iced      # binary: ferromix2

One process, one command — as of v3.0.0 the GUI owns PipeWire directly (see
that changelog entry below), there's no separate daemon to start first.

If the window doesn't appear on Wayland (or crashes — see the v3.0.2
changelog entry below, which now handles the crash case automatically):
    WAYLAND_DISPLAY= WAYLAND_SOCKET= cargo run -p mixer-gui-iced
(`WINIT_UNIX_BACKEND=x11` — the traditional fix — does NOT work on the
winit version this app uses; see v3.0.2's entry for why.)

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

## v2.6.0 — routing was never actually exclusive (the real "nothing works" fix)

v2.5.0's stray-link redirect only covered app→strip (playback). Real use
turned up the same bug on the OTHER side, plus a separate mute bug — between
them these are almost certainly why faders/DSP/mute "did nothing" and B-bus
routing "didn't work at all" even though every topology check said the links
were correct.

- FIX (the big one): `apply_bus_listeners` (B-bus → app-mic, i.e. "send one
  app's audio to another app") only ever ADDED the link — it never checked
  whether the receiving app's mic ALSO had its real default microphone
  auto-connected (WirePlumber does this synchronously the instant the capture
  node appears, before FerroMix can react). The app heard its real mic and
  the B-bus mixed together, permanently. Fixed with the same
  redirect-off-the-stray-link approach already used for playback.
- FIX: muting an A-bus (hardware output) never actually cut the link to the
  hardware sink — only the mute *flag* was set, and (per the existing
  strip-mute finding) a null-sink's mute flag alone doesn't stop its monitor
  from emitting. Muting your main output bus did not silence your speakers.
- FIX (found immediately by live-testing the fix above): if the same app is
  assigned as listener to TWO B-buses at once (a real config the testing
  session had — Discord's mic on both B1 and B2), the redirect logic treated
  each bus as "stray" relative to the other and fought itself every
  reconcile pass, bouncing Discord's mic between B1 and B2 forever. Both
  `stray_destinations`/`stray_sources` now take the full set of an app's
  legitimate FerroMix targets, not just one, so multiple simultaneous
  assignments coexist instead of fighting.
- HARDENED: the playback redirect was a one-shot latch — if a stray link
  reappeared later without the app's node itself disappearing (a device/
  profile change, a role rescan), it would never be caught again. The
  redirect check now runs every reconcile pass (cheap no-op once clean);
  only the `target.object` metadata *write* stays latched, since repeating
  that specifically is what caused a destroy/recreate war in earlier testing.
- Added `packaging/wireplumber/91-ferromix-disable-role-loopbacks.conf` — an
  optional, documented, user-installed WirePlumber override that removes
  Fedora's role-based loopback sinks entirely, closing the playback-side race
  proactively instead of reactively. See README.md's Fedora section.
- Lesson learned, and applied going forward: `pw-dump`/`pw-link` showing a
  link exists is NOT sufficient verification — it doesn't show whether a
  COMPETING link also exists. Verification now checks exclusivity.

## v3.0.0 — daemon/GUI merge: one process, one launch

The `ferromix2-daemon` + `ferromix2` split only ever existed to test the GUI
against a running daemon independently. The actual goal is simpler than what
that split implied: one app, one launch, and closing it returns PipeWire to
completely stock behavior. So the daemon is gone — `ferromix2` now boots
`mixer-core::Engine` directly on the real PipeWire backend in-process
(`crates/mixer-gui-iced/src/link.rs`), the same call the daemon's `main()`
used to make. No socket, no systemd --user service, nothing left running
after the window closes (FerroMix's virtual devices were already never
`object.linger`'d, so process exit was always a clean teardown — there just
used to be a second process staying alive on purpose).

- Removed `mixer-daemon` and `mixer-core::ipc` (the Unix-socket/bincode
  protocol) entirely — nothing to keep in sync between two binaries anymore.
- The pending fix for a dead PipeWire connection (previously: exit the
  process so systemd restarts it) now instead sends
  `BackendEvent::Disconnected`, stops just the worker thread, and lets the
  GUI rebuild a fresh backend + engine in place after a short delay — a
  RESET AUDIO click or a sample-rate change (both of which restart PipeWire
  on purpose) no longer takes the whole window down with it. Shows briefly
  as "reconnecting…" in the header.
- Packaging follows: RPM/PKGBUILD ship one binary, no `.service` unit.

## v3.0.1 — hybrid-GPU Wayland crash, packaging cleanup

First real-world report from v3.0.0: a Fedora 44 install with an NVIDIA
discrete + Intel integrated GPU crashed on launch — `wgpu` picked an
adapter, then panicked importing a dmabuf for the window surface
("Fallback system failed to choose present mode. This is a bug."), a
wgpu/winit-level issue on certain hybrid-GPU + native-Wayland
combinations, not anything in FerroMix's own PipeWire/routing code (that
part of the log was completely healthy — strips linked, buses linked,
routing correct).

- FIX: `install_xwayland_fallback` in `main.rs` installs a panic hook at
  startup that, on any panic, relaunches the same binary once with
  `WINIT_UNIX_BACKEND=x11` forced (XWayland instead of native Wayland,
  sidestepping the dmabuf import path entirely) — guarded by
  `FERROMIX_RELAUNCHED` so a genuine unrelated crash doesn't loop, it
  just crashes normally on the second try same as before. No user action
  needed either way.
- The `.deb` package was accidentally named `mixer-gui-iced_*.deb`
  (cargo-deb defaults to the crate name) instead of `ferromix2_*.deb`.
- Stopped shipping the RPM's auto-generated `-debuginfo`/`-debugsource`
  subpackages — clutter next to a single self-contained binary.

## v3.0.2 — the actual XWayland fallback fix (v3.0.1's didn't work)

v3.0.1's relaunch mechanism itself fired correctly (confirmed live in the
same friend's log: panic caught, "retrying once under XWayland" logged,
a second process started) — but the fix it applied did nothing: setting
`WINIT_UNIX_BACKEND=x11` had no effect, the relaunched process still
logged "Using Wayland platform" and crashed identically.

Root cause: that env var is stale advice from a much older winit.
Checked winit 0.30.13's actual source
(`platform_impl/linux/mod.rs::EventLoop::new`) — it has no env-var
override at all anymore. Backend selection is purely: if
`WAYLAND_DISPLAY` or `WAYLAND_SOCKET` is set (and non-empty), use
Wayland; else if `DISPLAY` is set, use X11. `WINIT_UNIX_BACKEND` isn't
read anywhere in that path.

- FIX: the relaunch now clears `WAYLAND_DISPLAY`/`WAYLAND_SOCKET` from
  the child's environment instead of setting the dead env var — XWayland
  sessions still have `DISPLAY` set, which winit falls back to once
  neither Wayland variable is present. Verified the env-manipulation
  logic in isolation (not just reasoned about) before wiring it back in,
  same as v3.0.1's process — this time confirming the actual variables
  winit's own source checks, not assuming a plausible-looking env var
  still worked.
- Updated README/RUN_FERROMIX2.md's manual-workaround instructions to
  match (`WAYLAND_DISPLAY= WAYLAND_SOCKET= ferromix2`), since they
  carried the same stale `WINIT_UNIX_BACKEND=x11` advice.
