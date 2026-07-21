Name:           ferromix2
Version:        2.6.0
Release:        1%{?dist}
Summary:        Voicemeeter-style virtual audio mixer for PipeWire

License:        MIT
URL:            https://github.com/yourname/ferromix
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

%files
%license LICENSE
%doc packaging/wireplumber/91-ferromix-disable-role-loopbacks.conf
%{_bindir}/%{name}-daemon
%{_bindir}/%{name}
%{_userunitdir}/%{name}.service
%{_datadir}/applications/%{name}.desktop
%{_datadir}/icons/hicolor/scalable/apps/%{name}.svg

%changelog
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
