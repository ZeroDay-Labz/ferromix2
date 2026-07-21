# FerroMix architecture (v0.2 — source→bus patchbay)

## Why a daemon/GUI split

Voicemeeter dies when you close its window. FerroMix routing must not. The
daemon (`ferromix2-daemon`) owns everything PipeWire; the GUI
(`mixer-gui-iced`, binary `ferromix2`) is a disposable IPC client — it only
sends `Command`s and polls `MixerState` over the Unix socket, never touches
PipeWire directly. Close it, reopen it, run two of them — audio never blinks.
`mixer_core::mock::MockBackend` exists for backend-agnostic testing (see
`mixer-core/src/mock.rs`), but the current GUI has no in-process/`--mock` mode
of its own — that was an egui-GUI-era capability that didn't carry over when
`mixer-gui` was retired in favor of the pure-IPC-client Iced GUI.

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
- Daemon restarts: virtual devices linger server-side (`object.linger`), get
  **adopted by name** instead of duplicated; links (deliberately not lingered)
  are rebuilt from config.
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
- IPC: one thread per GUI client; 30 Hz `GetState` polling with
  length-prefixed bincode frames. Boring on purpose.

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
