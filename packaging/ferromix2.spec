Name:           ferromix2
Version:        2.7.0
Release:        1%{?dist}
Summary:        Voicemeeter-style virtual audio mixer for PipeWire

License:        MIT
URL:            https://github.com/ZeroDay-Labz/ferromix2
# Generate with: git archive --prefix=%{name}-%{version}/ -o %{name}-%{version}.tar.gz HEAD
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  rust
BuildRequires:  cargo
BuildRequires:  clang-devel
BuildRequires:  pkgconf-pkg-config
BuildRequires:  pipewire-devel
BuildRequires:  systemd-rpm-macros

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

This package installs the daemon (%{name}-daemon, owns the PipeWire graph and
runs as a systemd --user service) and the Iced-based GUI (%{name}, a disposable
IPC client — closing it never interrupts audio). Also ships (but does NOT
apply automatically — see %doc) an optional WirePlumber override that
disables Fedora's role-based loopback routing, which otherwise races
FerroMix for control of PipeWire-pulse clients like Spotify and Firefox.

%prep
%autosetup -n %{name}-%{version}

%build
cargo build --release --locked

%install
install -Dm755 target/release/%{name}-daemon %{buildroot}%{_bindir}/%{name}-daemon
install -Dm755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dm644 packaging/%{name}.service %{buildroot}%{_userunitdir}/%{name}.service
install -Dm644 assets/%{name}.desktop %{buildroot}%{_datadir}/applications/%{name}.desktop
install -Dm644 assets/%{name}.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{name}.svg

# The daemon also self-launches on demand (the GUI spawns it if it isn't
# already running — see crates/mixer-gui-iced/src/link.rs's
# try_launch_daemon), so a user who never enables the systemd --user service
# still gets a working single-click launch from the desktop entry. Enabling
# the service here is still worth doing where the packaging macros support
# it: it means the daemon comes up at login (not just on first GUI launch)
# and survives independently of any one GUI window.
%post
%systemd_user_post %{name}.service

%preun
%systemd_user_preun %{name}.service

%postun
%systemd_user_postun_with_restart %{name}.service

%files
%license LICENSE
%doc packaging/wireplumber/91-ferromix-disable-role-loopbacks.conf
%{_bindir}/%{name}-daemon
%{_bindir}/%{name}
%{_userunitdir}/%{name}.service
%{_datadir}/applications/%{name}.desktop
%{_datadir}/icons/hicolor/scalable/apps/%{name}.svg

%changelog
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
