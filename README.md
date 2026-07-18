# FerroMix

**Voicemeeter for Linux.** A PipeWire mixer that routes any app to any output
or virtual mic, keeps routes alive by name (a VoIP call ending doesn't destroy
your patch), and refuses to build the feedback loops that ruin a mix-minus.

Built because the alternatives don't cut it: qpwgraph makes you redraw a line
every time a call ends; Pulsemeeter lumps everything into two virtual inputs
and looks like 2009.

## The model (identical to Voicemeeter)

```
INPUT STRIPS                        BUSES
  pick any source per strip:          A1  A2   hardware outputs (pick a device)
    • Virtual Input  (see below)      B1  B2   virtual mics (apps pick as input)
    • any hardware mic
    • any running app
```

Each strip has an assign stack — light `A1` and it plays out your headset;
light `B1` and Discord hears it. Exactly the Voicemeeter workflow:

| Strip | A1 | B1 | B2 | result |
|---|---|---|---|---|
| Mic | ● | ● | ● | you're heard in Discord **and** the SIP call |
| Browser | ● | ● | ● | browser audio to you + Discord + SIP |
| Discord | ● | — | ● | Discord audio into the SIP call (**B1 = echo, blocked**) |
| SIP phone | ● | ● | — | call audio into Discord (**B2 = echo back to caller, blocked**) |

**Mix-minus is safe by construction.** If a strip's app also *listens* to a
B-bus, sending that app back into the same bus is an echo loop. FerroMix
detects it, **refuses the link**, and paints the button red.

### Virtual Input (the VAIO trick)

FerroMix creates a `FerroMix Input` sink. Set it as your **system default
output** (Settings → Sound → Output) and everything you haven't routed per-app
lands on the "Virtual Input" strip — one fader for all your loose system audio.

### Routes stick

Assignments are keyed by **app name**, never by PipeWire ids. End a call, close
an app, restart the daemon — when it comes back, the reconciler re-links it.
The strip stays visible (marked offline) so your intent is never lost.

## Upgrading from an earlier build — read this

Versions before 0.5 created their virtual devices with `object.linger`, so the
devices **outlived the daemon** and every run left another copy behind
(`FerroMix A1`, `FerroMix A1-1`, …). Links landed on one copy while meters
watched another. v0.5 no longer lingers, and **sweeps any leftover `ferromix.*`
node at startup** — so it self-heals. Also delete your old config, since the
bus list and default gain changed:

```sh
rm -f ~/.config/ferromix/config.toml
systemctl --user restart ferromix
pw-cli ls Node | grep -i ferromix    # expect ONE node per bus, no "-1" copies
```

## Ubuntu / Debian

```sh
sudo apt install libpipewire-0.3-dev clang pkg-config build-essential
cargo build --release

# install
sudo install -Dm755 target/release/ferromix-daemon /usr/local/bin/
sudo install -Dm755 target/release/ferromix-gui    /usr/local/bin/
install -Dm644 packaging/ferromix.service ~/.config/systemd/user/ferromix.service
systemctl --user daemon-reload
systemctl --user enable --now ferromix

ferromix-gui        # badge reads LIVE when it's talking to the daemon
```

Check it's alive:
```sh
systemctl --user status ferromix
journalctl --user -u ferromix -f      # live log
pw-cli ls Node | grep -i ferromix     # your buses + virtual input
```

Then in each app: point **Discord's input** at `FerroMix B1`, your **softphone's
input** at `FerroMix B2`, and pick a real device for A1 in the GUI.

Arch: swap the deps for `pipewire clang pkgconf`.

## Windows (GUI development only)

Mock backend — fake mics, apps, devices, animated meters, and a SIP call that
drops at 16 s and returns at 24 s so you can watch routes reattach:
```powershell
cargo run -p mixer-gui        # auto-mock off Linux
cargo run -p mixer-gui -- --mock
```

## Config

`~/.config/ferromix/config.toml` — the GUI's `SAVE` writes it back.

```toml
feedback_guard = true

[[buses]]
label = "A1"
kind = "hw"
device = "corsair"       # substring of the output device; omit = default

[[buses]]
label = "B1"
kind = "virtual"         # apps select "FerroMix B1" as their microphone

[[strip]]
input = "discord"        # app name substring, a mic, or "ferromix.vin.0"
assign = ["A1", "B2"]    # buses this strip feeds
```

## Architecture

`mixer-core` (model/engine/config/IPC/mock) · `mixer-pw` (PipeWire: devices,
declarative link reconciler, VU taps, recorder) · `mixer-daemon`
(`ferromix-daemon`, owns the graph, serves GUIs over a Unix socket) ·
`mixer-gui` (`ferromix-gui`, egui console + matrix).

The daemon owns audio; the GUI is disposable. Close it, audio keeps flowing.
See `docs/ARCHITECTURE.md`.

## License

MIT.
