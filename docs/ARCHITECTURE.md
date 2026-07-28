# FerroMix architecture (v3.0 — single-process patchbay)

## One process

FerroMix is one binary (`ferromix2`, crate `mixer-gui-iced`). Launch it and it
owns PipeWire for as long as it's open; close it and every FerroMix device/
link is torn down, PipeWire goes straight back to stock behavior. That's a
deliberate choice, not a limitation — earlier builds split this into a
long-lived `ferromix2-daemon` (owning PipeWire, meant to survive the GUI
closing) talking to a disposable GUI over a Unix socket, but that split only
ever existed for standalone testing of the two halves. The real product goal
is simpler: one launch, and closing it means audio goes back to normal.

Concretely, `mixer-gui-iced/src/link.rs::start()` does exactly what the old
daemon's `main()` used to — load `Config`, construct `mixer_pw::PwBackend`,
call `mixer_core::Engine::spawn(...)` — and hands the resulting `EngineHandle`
straight to the GUI. The GUI still never touches PipeWire itself, only the
`AudioBackend` trait below; the seam that used to be a socket is now just an
`Arc<Mutex<MixerState>>` clone. `mixer_core::mock::MockBackend` (see
`mixer-core/src/mock.rs`) exists for backend-agnostic testing the same way —
swap which backend `Engine::spawn` gets and nothing else changes.

Because FerroMix now owns its own process lifetime end to end, it's also
responsible for noticing when its PipeWire connection dies (deliberately, via
the RESET AUDIO button or a sample-rate change, or externally) and rebuilding
itself — see "Reconnect" under Threading below. The old design could rely on
a service supervisor (systemd) for this; there isn't one anymore.

## The one abstraction: `AudioBackend`

`mixer-core::backend::AudioBackend` is the seam. The engine and GUI know
nothing about PipeWire — only this trait. Two implementations:

- `mixer_core::mock::MockBackend` — fake apps, animated levels (any OS)
- `mixer_pw::PwBackend` — the real thing (Linux)

## The declarative reconciler (the important part)

`mixer-pw` never treats a command as "do X once". Commands mutate a **Desired**
model (strips, buses, assignments, app routes, volumes). A `reconcile()` pass
then converges the actual graph toward Desired, and it re-runs on **every
registry change** — node added, port added, link added, anything.

Consequences:

- App restarts: the app's stream node reappears → rule matches → route
  re-applied. Links are re-derived from ground truth, not remembered.
- Device hotplug: bus target resolution runs again → bus migrates.
- Process restart (including the in-place reconnect above): virtual devices
  are deliberately **not** `object.linger`'d (`mixer-pw/src/virtual_dev.rs`)
  — they die with the process/connection and get rebuilt fresh from `Desired`
  on the next start, rather than lingering server-side and risking the
  reconciler adopting a stale duplicate. Simpler, and it's what makes closing
  FerroMix a genuinely clean teardown instead of leaking nodes.
- Exclusivity: if WirePlumber re-links a routed app to the default sink, the
  next reconcile destroys that link again. Self-healing by construction.

## Audio path

```
app stream ──link──▶ strip (null sink, monitor.channel-volumes=true)
                        │ monitor ports (post-fader)
                        ├──link──▶ bus A (null sink) ──link──▶ hardware sink
                        └──link──▶ bus B (Audio/Source/Virtual = virtual mic)
```

- **Volume/mute**: SPA `Props` pods (`channelVolumes`, cubic taper `ui³`;
  `mute`) set on our nodes. Monitors follow volume, so faders shape routing.
- **VU meters**: one *passive* capture stream per strip/bus taps the signal,
  computes a peak in the RT callback, and ships throttled `Level` events.
  Passive = meters never force the graph to run.
- **Recording**: a non-passive capture stream on a bus → `hound` WAV
  (32-bit float, 48 kHz stereo; PipeWire converts).
- **Playback**: an output stream targeting a strip, advertising the WAV's own
  rate/channels and letting PipeWire resample.

## Threading

- PipeWire loop thread: all proxies, registry mirror, reconciler (Rc/RefCell,
  single-threaded by design). Commands arrive via `pipewire::channel`.
- Stream process callbacks: RT threads; touch only their own user data and an
  mpsc sender.
- Engine thread (`mixer-core`): consumes backend events + GUI commands,
  owns the `MixerState` snapshot behind an `Arc<Mutex>`.
- Iced/UI thread: `App`'s ~60 Hz (`iced::time::every(16ms)`) tick calls
  `EngineHandle::snapshot()` directly — a mutex lock + clone, no IPC — and
  sends `Command`s the same way. Boring on purpose; used to be a 30 Hz
  `GetState` poll over a Unix socket with length-prefixed bincode frames,
  same shape, just in-process now.

### Reconnect

If the PipeWire core reports an error on connection id 0 (the connection
itself is gone — happens on every deliberate `pipewire.socket` restart, e.g.
RESET AUDIO or a sample-rate change, not just an external one),
`mixer-pw/src/worker.rs` sends `BackendEvent::Disconnected`, quits its own
mainloop, and the worker thread ends — it does **not** exit the process
(there's no supervisor to restart it into). `mixer-core::engine::run()` sets
`MixerState.backend_alive = false` and keeps running; further commands just
no-op against the now-dead `PwCmd` channel. The GUI's `Message::Tick` handler
in `mixer-gui-iced/src/main.rs` notices `backend_alive == false`, shows
"reconnecting…" in the header, and after `RECONNECT_DELAY` calls
`link::start()` again — a fresh `PwBackend` + `Engine::spawn` reading
`Config` from disk, which already has the last-autosaved routing, so it comes
back exactly as it was.

## Per-strip DSP

Each strip can own a `libpipewire-module-filter-chain` instance (loaded via a
small unsafe FFI shim in `mixer-pw/src/dsp.rs` — the `pipewire` crate has no
safe binding for `pw_context_load_module`), spliced in as
`source → dsp.in → [gate_l/gate_r → sc4 compressor] → dsp.out → strip device`
instead of the direct `source → strip device` link, via two extra `Slot`
variants in the reconciler (`DspIn`/`DspOut`). Verified end-to-end against a
live daemon (`pw-dump`/`pw-link -l`), which caught four real bugs the SPA-JSON
generator's unit tests didn't (they only checked string formatting, not that
the plugin/labels/ports/units were real):

- The gate is a PipeWire builtin, `type = builtin`, `label = noisegate` — NOT
  `label = gate`, which doesn't exist and fails to load.
- The gate is MONO (one "In"/"Out" pair), so a stereo strip needs two
  instances (`gate_l`, `gate_r`), not one.
- The gate's `Open Threshold`/`Close Threshold` ports are LINEAR AMPLITUDE
  (SPA range 0.0..1.0), not dB — passing a raw dB value silently clamps to
  the port's minimum. Converted via `db_to_lin` (`10^(db/20)`).
- PipeWire's builtin filter-graph has no compressor at all, so that stage
  uses the SC4 LADSPA plugin (`plugin = sc4_1882`, `label = sc4`) from
  `ladspa-swh-plugins` (a runtime dependency) — genuinely stereo
  (`Left/Right input`/`Left/Right output`), confirmed via `analyseplugin
  sc4_1882.so`.

A knob change reloads the module (destroy + recreate with new args baked into
the SPA-JSON) rather than pushing live params into the running chain's
internal nodes — simpler, and the cost is a few ms of dropout on that one
strip. Confirmed the reload path destroys the old module cleanly (fresh node
ids, no leaked/duplicate `ferromix.dsp.*` nodes).

## Config → live state

`config.toml` is the source of truth at startup; the Iced GUI autosaves
(`Command::Save`) ~1.5s after the last change, and folds live
fader/mute/assign/name/dsp state back into it. Rules are edited in the file
and take effect on the next engine start or app appearance.
