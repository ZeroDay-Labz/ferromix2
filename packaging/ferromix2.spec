Name:           ferromix2
Version:        3.0.1
Release:        1%{?dist}
Summary:        Voicemeeter-style virtual audio mixer for PipeWire

License:        MIT
URL:            https://github.com/ZeroDay-Labz/ferromix2
# Generate with: git archive --prefix=%{name}-%{version}/ -o %{name}-%{version}.tar.gz HEAD
Source0:        %{name}-%{version}.tar.gz

# v3.0.0's release shipped ferromix2-debuginfo/-debugsource RPMs nobody
# asked for (rpmbuild's default behavior) — not broken, just clutter next
# to a single self-contained binary with no separate library consumers.
%global debug_package %{nil}

BuildRequires:  rust
BuildRequires:  cargo
BuildRequires:  clang-devel
BuildRequires:  pkgconf-pkg-config
BuildRequires:  pipewire-devel

Requires:       pipewire
Requires:       wireplumber
# The per-strip compressor stage (SC4) — the noise gate is a PipeWire builtin
# and needs nothing extra, but PipeWire's builtin filter-graph has no
# compressor of its own.
Requires:       ladspa-swh-plugins

%description
FerroMix2 is a Voicemeeter-Potato-class virtual mixer for Linux, built on
PipeWire and WirePlumber. It routes any app to any hardware output or virtual
microphone, keeps routes alive by app name (a call ending doesn't destroy your
patch), refuses to build the feedback loops that ruin a mix-minus setup, and
runs per-strip noise-gate/compressor DSP.

This package installs a single binary (%{name}) — launch it and it owns the
PipeWire graph for as long as it's open; close it and every FerroMix device/
link is torn down, PipeWire goes straight back to stock behavior. No service
to enable, nothing left running in the background. Also ships (but does NOT
apply automatically — see %doc) an optional WirePlumber override that
disables Fedora's role-based loopback routing, which otherwise races
FerroMix for control of PipeWire-pulse clients like Spotify and Firefox.

%prep
%autosetup -n %{name}-%{version}

%build
cargo build --release --locked

%install
install -Dm755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dm644 assets/%{name}.desktop %{buildroot}%{_datadir}/applications/%{name}.desktop
install -Dm644 assets/%{name}.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{name}.svg

%files
%license LICENSE
%doc packaging/wireplumber/91-ferromix-disable-role-loopbacks.conf
%{_bindir}/%{name}
%{_datadir}/applications/%{name}.desktop
%{_datadir}/icons/hicolor/scalable/apps/%{name}.svg

%changelog
* Tue Jul 28 2026 FerroMix contributors <noreply@example.com> - 3.0.1-1
- FIX: native Wayland windowing could crash outright on hybrid-GPU laptops
  (confirmed live: NVIDIA discrete + Intel integrated, Fedora 44) — wgpu
  picked an adapter, then panicked importing a dmabuf for the window
  surface. The GUI now catches this at startup and automatically
  relaunches itself once under XWayland instead, no user action needed.
- Stopped shipping %{name}-debuginfo/%{name}-debugsource RPMs — clutter
  next to a single self-contained binary.
* Tue Jul 28 2026 FerroMix contributors <noreply@example.com> - 3.0.0-1
- BREAKING: merged the daemon and GUI into a single process. There is no
  more `%{name}-daemon` binary and no more systemd --user service — launch
  %{name} and it owns PipeWire directly for as long as it's open; close the
  window and every FerroMix node/link is torn down, no lingering state, no
  background service. If you have the old %{name}.service enabled, disable
  it after upgrading (`systemctl --user disable --now %{name}.service`) —
  it's no longer installed by this package and referencing a stale unit
  file will otherwise just fail quietly on next login.
- Fixed a real bug the old split surfaced: a PipeWire restart (RESET AUDIO,
  a sample-rate change, or an external `systemctl restart pipewire`) used
  to leave the daemon's connection dead with no way to notice — confirmed
  live, it reported healthy for over two hours while every fader/mute/
  route silently did nothing. The single process now detects this and
  reconnects itself automatically, visible as a brief "reconnecting…" in
  the header.
* Wed Jul 22 2026 FerroMix contributors <noreply@example.com> - 2.7.0-1
- GUI now launches the daemon itself if it isn't already running (checks
  with pgrep first to avoid a double-launch race), so the desktop entry is
  a genuine single-click launch instead of needing the systemd --user
  service enabled or a second terminal command.
- Enable/start the systemd --user service properly on install/upgrade/
  removal (%post/%preun/%postun scriptlets were missing entirely before —
  the unit shipped but was never actually enabled).
- Added a master ON/OFF toggle in the header: OFF releases every app
  FerroMix has redirected and stops reconciling (system behaves like stock
  PipeWire); ON re-applies the existing routing config instantly. Persisted
  across restarts.
- Added a Settings sample-rate picker (44100/48000/96000) that forces
  PipeWire's graph clock system-wide, plus pinned resample.quality on every
  FerroMix adapter node to fix cascaded-resampling audio quality issues.
- Responsive console layout: strip/bus cards now wrap onto additional rows
  instead of overflowing the window at any size, with corrected width math
  (previous version only accounted for strip count, not strip+bus count,
  when sizing the row).
- Visual pass: gradient card/button surfaces, brighter meter housing,
  consolidated type scale and spacing tokens, dividers between card
  sections, SVG icons for the highest-traffic glyphs.
* Tue Jul 21 2026 FerroMix contributors <noreply@example.com> - 2.6.0-1
- Fixed B-bus -> app-mic routing never being exclusive (the app's real
  default microphone stayed linked alongside the B-bus, mixing both
  permanently) — same bug class as the earlier playback-side fix, now
  covering both directions and hardened against silent recurrence.
- Fixed A-bus mute not actually cutting the hardware-output link.
- Ship an optional WirePlumber override (see %doc) that proactively removes
  the role-based-loopback race for playback instead of only reacting to it.
* Mon Jul 20 2026 FerroMix contributors <noreply@example.com> - 2.4.0-1
- Initial RPM packaging: daemon + Iced GUI, systemd --user service, desktop
  entry and icon, LADSPA compressor dependency.
