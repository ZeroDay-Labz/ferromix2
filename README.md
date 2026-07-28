# FerroMix

**Voicemeeter for Linux.** A PipeWire mixer that routes any app to any output
or virtual mic, keeps routes alive by name (a VoIP call ending doesn't destroy
your patch), and refuses to build the feedback loops that ruin a mix-minus.

One binary, one launch: open it and it owns PipeWire for as long as it's
running; close it and every FerroMix device/link is torn down, PipeWire goes
straight back to stock behavior. Nothing to enable, nothing left running in
the background.

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

Assignments are keyed by **app name**, never by PipeWire ids. End a call,
close an app, restart FerroMix itself — when it (or the app) comes back, the
reconciler re-links it.
The strip stays visible (marked offline) so your intent is never lost.

## Upgrading from an earlier build — read this

**v3.0.0 merged the daemon into the GUI.** There is no more `ferromix2-daemon`
binary and no more `ferromix2.service`. If you had the old service enabled:

```sh
systemctl --user disable --now ferromix2.service
rm -f ~/.config/systemd/user/ferromix2.service
systemctl --user daemon-reload
```

Your `~/.config/ferromix2/config.toml` (routing, faders, DSP) is untouched
and still applies — just launch `ferromix2` itself from now on.

Separately: versions before 0.5 created their virtual devices with
`object.linger`, so the devices outlived the daemon and every run left
another copy behind (`FerroMix A1`, `FerroMix A1-1`, …). That's long gone —
devices now die with the process (and always did the reconnect-safe way,
sweeping any leftover `ferromix.*` node at startup) — noted here only because
it's the reason process exit was always a safe, complete teardown even before
v3.0.0 made that the *whole* app's behavior, not just the daemon's.

## Fedora

```sh
sudo dnf install rust cargo clang-devel pkgconf-pkg-config pipewire-devel ladspa-swh-plugins
cargo build --release

# install
sudo install -Dm755 target/release/ferromix2 /usr/bin/

ferromix2            # badge reads LIVE the moment it's connected to PipeWire
```

That's it — no service to enable. Check it's alive:
```sh
pw-cli ls Node | grep -i ferromix      # your buses + virtual input
```
(or just watch the header: green dot + LIVE means it's routing; the LOG tab
in the app itself shows the same live log a `journalctl -f` used to.)

Then in each app: point **Discord's input** at `FerroMix B1`, your **softphone's
input** at `FerroMix B2`, and pick a real device for A1 in the GUI.

Ubuntu/Debian: swap the deps for `libpipewire-0.3-dev clang pkg-config build-essential`.
Arch: swap the deps for `pipewire clang pkgconf`, or use `packaging/PKGBUILD`.

The gate/compressor DSP on each strip needs `ladspa-swh-plugins` (or your
distro's equivalent) installed at runtime for the compressor stage; the gate
is a PipeWire builtin and needs nothing extra.

### Recommended: disable role-based loopback routing

Fedora's default WirePlumber config routes PipeWire-pulse clients (Spotify,
Firefox, most desktop apps) through role-based loopback sinks before FerroMix
can claim them — FerroMix detects and redirects this after the fact (you'll
see `REDIRECT` in the log), but it's strictly better to remove the race
entirely:
```sh
mkdir -p ~/.config/wireplumber/wireplumber.conf.d
cp packaging/wireplumber/91-ferromix-disable-role-loopbacks.conf \
   ~/.config/wireplumber/wireplumber.conf.d/
systemctl --user restart wireplumber wireplumber-pipewire pipewire pipewire-pulse
```
This is optional and system-wide — it also turns off Fedora's role-based
ducking (e.g. notification sounds ducking music) for every app, not just
FerroMix. See the comment in that file for the full trade-off. FerroMix
routes correctly either way; this just makes it proactive instead of reactive.

## Troubleshooting

**Window crashes on launch with `wgpu`/`EGL`/dmabuf errors** (seen on hybrid-
GPU laptops — an NVIDIA discrete + Intel integrated GPU under native
Wayland): fixed as of v3.0.2 — FerroMix now detects this at startup and
automatically relaunches itself once under XWayland, no action needed. If
you're still on an older build, launch it with the two Wayland-selecting
variables cleared so it falls back to XWayland on its own:
```sh
WAYLAND_DISPLAY= WAYLAND_SOCKET= ferromix2
```
(`WINIT_UNIX_BACKEND=x11`, the traditional fix for this class of issue,
does **not** work on the windowing library version this app currently
uses — it has no env-var override anymore, only this Wayland-variable
fallback.)

## Config

`~/.config/ferromix2/config.toml` — the header's SAVE indicator writes it back
(autosaves ~1.5s after the last change, or click SAVE directly).

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

`mixer-core` (model/engine/config/mock) · `mixer-pw` (PipeWire: devices,
declarative link reconciler, VU taps, recorder, per-strip DSP) ·
`mixer-gui-iced` (`ferromix2`, the single binary — Iced console + matrix +
settings + log, owning the PipeWire worker thread directly).

One process. Launch it and it owns PipeWire; close it and every FerroMix
device/link is torn down, no lingering state. See `docs/ARCHITECTURE.md`.

## License

MIT.
