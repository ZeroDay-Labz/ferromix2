# FerroMix architecture (v0.2 — source→bus patchbay)

## Why a daemon/GUI split

Voicemeeter dies when you close its window. FerroMix routing must not. The
daemon (`ferromix-daemon`) owns everything PipeWire; the GUI is a disposable
viewer/remote. Close it, reopen it, run two of them — audio never blinks.
On Windows (or `--mock`), the GUI instead hosts the engine in-process against
`MockBackend`, which is how the UI gets developed without a Linux box.

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

## Config → live state

`config.toml` is the source of truth at startup; the GUI's `SAVE` folds live
fader/mute/assign state back into it. Rules are edited in the file (v0.1) and
take effect on the next engine start or app appearance.
