//! The engine: owns a backend, manages fixed strips (each with a selectable
//! input), tracks available inputs from the backend, and serves GUI commands.
//! The backend is source-key based; strips are translated to it here.

use crate::backend::{AudioBackend, BackendEvent};
use crate::config::Config;
use crate::model::{self as model, Bus, BusKind, InputOption, Level, LevelKey, MixerState, RecTarget, SourceInfo, Strip};
use serde::{Deserialize, Serialize};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    SetStripInput { strip: usize, input: Option<String> },
    ToggleAssign { strip: usize, bus: usize },
    SetStripVolume { strip: usize, volume: f32 },
    SetStripMute { strip: usize, mute: bool },
    SetStripDsp { strip: usize, dsp: crate::model::StripDsp },
    /// See `Strip.force_mono`'s doc comment.
    SetStripForceMono { strip: usize, on: bool },
    SetBusVolume { bus: usize, volume: f32 },
    SetBusMute { bus: usize, mute: bool },
    SetBusDevice { bus: usize, device: Option<String> },
    /// Which source a bus's METER tracks (pre-fader, source-only) —
    /// metering-only, unlike `SetStripInput`: does not route audio into the
    /// bus's mix, independent of whatever strips are routed into it via
    /// `ToggleAssign`. See `Bus.input`'s doc comment for why.
    SetBusInput { bus: usize, input: Option<String> },
    SetRecordingsDir { path: String },
    SetUiScale { scale: f32 },
    /// Forces PipeWire's own graph clock to `rate` (44100/48000/96000) via
    /// `pw-metadata -n settings 0 clock.force-rate` — system-wide, same as
    /// every app currently running, not just FerroMix's own nodes. This is
    /// the only way to actually stop an app whose native stream rate isn't
    /// the graph's rate from being resampled before its audio ever reaches
    /// FerroMix (see `virtual_dev.rs`'s `resample.quality` pinning for the
    /// complementary fix inside FerroMix's own node chain). Disruptive in
    /// the same way `ResetAudio` (the GUI's "RESET AUDIO TO STOCK PIPEWIRE"
    /// button) is — streams may briefly glitch/reconnect while the graph
    /// renegotiates — that's expected, not a bug to engineer around.
    SetSampleRate { rate: u32 },
    SetStripName { strip: usize, name: String },
    SetBusName { bus: usize, name: String },
    ToggleBusMonitor { bus: usize, a_bus: usize },
    /// Route bus `from`'s output additionally into bus `to`'s input. Refused
    /// (no-op) if `to` already feeds `from` — direct 2-cycles only are
    /// guarded against; longer chains are not, by design (see plan notes).
    ToggleBusFeed { from: usize, to: usize },
    /// Route bus `bus`'s output additionally into strip `strip`'s device —
    /// the reverse direction of `ToggleAssign`. Refused (no-op) if that strip
    /// already sends to this bus (a direct strip→bus→strip cycle).
    ToggleBusStripFeed { bus: usize, strip: usize },
    SetBusListener { bus: usize, app: Option<String> },
    /// Same as `SetBusListener`, for a strip — see `Strip.listener`'s doc
    /// comment for why strips can do this too, not just B-buses.
    SetStripListener { strip: usize, app: Option<String> },
    StartRecordTarget { target: RecTarget },
    StopRecordTarget { target: RecTarget },
    SetFeedbackGuard { on: bool },
    AddStrip,
    /// Removes the LAST strip only — never a specific index. A strip's
    /// index is baked into its real PipeWire node name/description, its DSP
    /// module's node names, and saved `BusCfg.strip_feeds` entries, so
    /// removing from the middle and reindexing everything after it would
    /// mean destroying and recreating every later strip's actual PipeWire
    /// node and re-pointing anything routed to them. Mirroring `AddStrip`'s
    /// append-only symmetry avoids all of that. Refused (no-op) if only one
    /// strip remains.
    RemoveLastStrip,
    /// Master bypass toggle. `on: false` releases every app FerroMix has
    /// currently redirected (clears `target.object` metadata, handing
    /// control back to WirePlumber's own default policy) and stops the
    /// reconciler from doing anything further — your system behaves like
    /// stock PipeWire. Desired routing state (which strip has which input,
    /// which sends are on, etc.) is NOT cleared, just not enforced while
    /// off, so `on: true` snaps everything back exactly where it was —
    /// this is a pause, not a reset. Persisted (`Config.enabled`) so a
    /// daemon restart doesn't silently re-enable it.
    SetEnabled { on: bool },
    Save,
}

#[derive(Clone)]
pub struct EngineHandle {
    pub cmd_tx: Sender<Command>,
    pub state: Arc<Mutex<MixerState>>,
}

impl EngineHandle {
    pub fn snapshot(&self) -> MixerState {
        self.state.lock().expect("state poisoned").clone()
    }
    pub fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }
}

pub struct Engine;

impl Engine {
    pub fn spawn(mut backend: Box<dyn AudioBackend>, events: Receiver<BackendEvent>, config: Config) -> EngineHandle {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Command>();
        let state = Arc::new(Mutex::new(initial_state(&config)));
        let handle = EngineHandle { cmd_tx, state: Arc::clone(&state) };
        std::thread::Builder::new()
            .name("ferromix-engine".into())
            .spawn(move || run(backend.as_mut(), events, cmd_rx, state, config))
            .expect("spawn engine");
        handle
    }
}

fn initial_state(cfg: &Config) -> MixerState {
    let buses: Vec<Bus> = cfg
        .buses
        .iter()
        .map(|b| Bus {
            label: b.label.clone(),
            name: b.name.clone(),
            monitor: Vec::new(),
            feeds: Vec::new(),
            strip_feeds: Vec::new(),
            input: b.input.clone(),
            input_label: b.input.clone().unwrap_or_else(|| "—".into()),
            input_live: false,
            listener: b.listener.clone(),
            kind: b.bus_kind(),
            device: b.device.clone(),
            listeners: Vec::new(),
            volume: b.volume,
            mute: b.mute,
            level: Level::default(),
            recording: false,
            node_id: None,
        })
        .collect();
    let n_a = buses.iter().filter(|b| b.kind == BusKind::HwOutput).count();
    let n_bus = buses.len();
    let n_strips = cfg.strip_count.max(cfg.strips.len());
    let mut buses = buses;
    for b in buses.iter_mut() {
        b.monitor = vec![false; n_a];
        b.feeds = vec![false; n_bus];
        b.strip_feeds = vec![false; n_strips];
    }
    // restore saved monitor flags by A-bus label
    let a_labels: Vec<String> = cfg
        .buses
        .iter()
        .filter(|b| b.bus_kind() == BusKind::HwOutput)
        .map(|b| b.label.clone())
        .collect();
    // restore saved feed flags by (any) bus label
    let all_labels: Vec<String> = cfg.buses.iter().map(|b| b.label.clone()).collect();
    for (bi, bcfg) in cfg.buses.iter().enumerate() {
        if let Some(b) = buses.get_mut(bi) {
            for (ai, al) in a_labels.iter().enumerate() {
                if bcfg.monitor.iter().any(|m| m.eq_ignore_ascii_case(al)) {
                    if let Some(slot) = b.monitor.get_mut(ai) {
                        *slot = true;
                    }
                }
            }
            for (gi, gl) in all_labels.iter().enumerate() {
                if bcfg.feeds.iter().any(|f| f.eq_ignore_ascii_case(gl)) {
                    if let Some(slot) = b.feeds.get_mut(gi) {
                        *slot = true;
                    }
                }
            }
            for &si in bcfg.strip_feeds.iter() {
                if let Some(slot) = b.strip_feeds.get_mut(si) {
                    *slot = true;
                }
            }
        }
    }
    let n = buses.len();
    let mut strips: Vec<Strip> = cfg
        .strips
        .iter()
        .map(|s| {
            let assign = buses.iter().map(|b| s.assign.iter().any(|a| a.eq_ignore_ascii_case(&b.label))).collect();
            Strip {
                name: s.name.clone(),
                input: s.input.clone(),
                input_label: s.input.clone().unwrap_or_else(|| "—".into()),
                input_live: false,
                kind: None,
                volume: s.volume,
                mute: s.mute,
                level: Level::default(),
                assign,
                recording: false,
                dsp: crate::model::StripDsp::default(),
                listener: s.listener.clone(),
                listeners: Vec::new(),
                force_mono: s.force_mono,
            }
        })
        .collect();
    while strips.len() < cfg.strip_count {
        strips.push(Strip::empty(n));
    }
    MixerState {
        strips,
        buses,
        inputs: Vec::new(),
        devices: Vec::new(),
        feedback: Vec::new(),
        capture_apps: Vec::new(),
        recordings_dir: cfg.recordings_dir().display().to_string(),
        feedback_guard: cfg.feedback_guard,
        ui_scale: cfg.ui_scale,
        sample_rate: cfg.sample_rate,
        enabled: cfg.enabled,
        log: Vec::new(),
    }
}

/// Write `clock.allowed-rates` (widened to every rate the Settings picker
/// offers, so switching between them never needs this again) and
/// `clock.force-rate` to PipeWire's "settings" metadata object — the two
/// writes `Command::SetSampleRate`'s handler needs, factored out so both the
/// live command handler and daemon startup (below) can re-assert a
/// persisted non-default rate without duplicating the exact `pw-metadata`
/// invocations. See `Command::SetSampleRate`'s doc comment for why this
/// shells out rather than going through `AudioBackend`.
fn apply_sample_rate_metadata(rate: u32) {
    let _ = std::process::Command::new("pw-metadata")
        .args(["-n", "settings", "0", "clock.allowed-rates", "[ 44100, 48000, 96000 ]"])
        .output();
    let _ = std::process::Command::new("pw-metadata")
        .args(["-n", "settings", "0", "clock.force-rate", &rate.to_string()])
        .output();
}

fn ts() -> String {
    let s = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("[{:02}:{:02}:{:02}]", (s / 3600) % 24, (s / 60) % 60, s % 60)
}

/// Push a strip's whole desired state to the backend.
fn push_strip(backend: &mut dyn AudioBackend, idx: usize, strip: &Strip) {
    let _ = backend.set_strip_input(idx, strip.input.clone());
    for (bi, on) in strip.assign.iter().enumerate() {
        let _ = backend.set_strip_assign(idx, bi, *on);
    }
    let _ = backend.set_strip_volume(idx, strip.volume);
    let _ = backend.set_strip_mute(idx, strip.mute);
    if strip.force_mono {
        let _ = backend.set_strip_force_mono(idx, true);
    }
}

/// Resolve a chosen input key against the live input list.
fn resolve_input<'a>(inputs: &'a [InputOption], key: &str) -> Option<&'a InputOption> {
    inputs
        .iter()
        .find(|i| i.key.eq_ignore_ascii_case(key))
        .or_else(|| inputs.iter().find(|i| i.key.to_lowercase().contains(&key.to_lowercase())))
}

fn refresh_strip_inputs(state: &mut MixerState) {
    let inputs = state.inputs.clone();
    for s in &mut state.strips {
        if let Some(key) = s.input.clone() {
            if let Some(opt) = resolve_input(&inputs, &key) {
                // Canonicalize: config may hold a substring ("corsair"); once a
                // live input matches, adopt its exact key so meters, faders and
                // backend routing all agree.
                s.input = Some(opt.key.clone());
                s.input_label = opt.label.clone();
                s.input_live = opt.live;
                s.kind = Some(opt.kind);
            } else {
                s.input_label = format!("{key} (offline)");
                s.input_live = false;
            }
        } else {
            // No source linked in — the strip is still a live device that apps
            // can point their output at.
            s.input_label = "(device only)".into();
            s.input_live = false;
            s.kind = None;
        }
    }
}

/// Same resolution as `refresh_strip_inputs`, for a bus's directly-assigned
/// input. Applies to every bus regardless of kind (A and B alike).
fn refresh_bus_inputs(state: &mut MixerState) {
    let inputs = state.inputs.clone();
    for b in &mut state.buses {
        if let Some(key) = b.input.clone() {
            if let Some(opt) = resolve_input(&inputs, &key) {
                b.input = Some(opt.key.clone());
                b.input_label = opt.label.clone();
                b.input_live = opt.live;
            } else {
                b.input_label = format!("{key} (offline)");
                b.input_live = false;
            }
        } else {
            b.input_label = "—".into();
            b.input_live = false;
        }
    }
}

fn run(
    backend: &mut dyn AudioBackend,
    events: Receiver<BackendEvent>,
    commands: Receiver<Command>,
    state: Arc<Mutex<MixerState>>,
    mut config: Config,
) {
    {
        let st = state.lock().unwrap();
        let _ = backend.set_feedback_guard(config.feedback_guard);
        // Must land before any push_strip/set_bus_* calls below reach the
        // backend — if starting up disabled, nothing should get actively
        // redirected during the initial sync either.
        let _ = backend.set_enabled(config.enabled);
        // Must land before any ensure_strip/ensure_bus below — those create
        // the actual adapter nodes, which are pinned to whatever rate is
        // current at creation time (see `AudioBackend::set_sample_rate`).
        let _ = backend.set_sample_rate(config.sample_rate);
        // PipeWire's own `clock.force-rate`/`clock.allowed-rates` metadata
        // is NOT itself persisted across a PipeWire restart (confirmed
        // live) — only FerroMix's own `config.toml` remembers the user's
        // choice. Re-assert it here so a persisted non-default rate from a
        // previous session doesn't silently revert to pipewire.conf's stock
        // 48000 default the next time the system boots or PipeWire restarts
        // without FerroMix having been the one to trigger it. A no-op write
        // when `sample_rate` is already 48000 (the default).
        if config.sample_rate != 48_000 {
            apply_sample_rate_metadata(config.sample_rate);
        }
        // Each strip is a virtual sink device apps can point their output at.
        for (i, _s) in st.strips.iter().enumerate() {
            let _ = backend.ensure_strip(i, &model::strip_device_label(i));
        }
        for (i, b) in st.buses.iter().enumerate() {
            let _ = backend.ensure_bus(i, &b.label, b.kind);
            if b.kind == BusKind::HwOutput {
                let _ = backend.set_bus_device(i, b.device.clone());
            }
            let _ = backend.set_bus_volume(i, b.volume);
            let _ = backend.set_bus_mute(i, b.mute);
        }
        for (i, s) in st.strips.iter().enumerate() {
            push_strip(backend, i, s);
            if s.listener.is_some() {
                let _ = backend.set_strip_listener(i, s.listener.clone());
            }
        }
        for (bi, b) in st.buses.iter().enumerate() {
            if b.input.is_some() {
                let _ = backend.set_bus_input(bi, b.input.clone());
            }
            if b.listener.is_some() {
                let _ = backend.set_bus_listener(bi, b.listener.clone());
            }
            for (ai, on) in b.monitor.iter().enumerate() {
                if *on {
                    let _ = backend.set_bus_monitor(bi, ai, true);
                }
            }
            for (gi, on) in b.feeds.iter().enumerate() {
                if *on {
                    let _ = backend.set_bus_feed(bi, gi, true);
                }
            }
            for (si, on) in b.strip_feeds.iter().enumerate() {
                if *on {
                    let _ = backend.set_bus_strip_feed(bi, si, true);
                }
            }
        }
    }
    state.lock().unwrap().push_log(format!("{} FerroMix engine up", ts()));

    let mut last_decay = Instant::now();

    loop {
        let mut worked = false;

        while let Ok(ev) = events.try_recv() {
            worked = true;
            let mut st = state.lock().unwrap();
            match ev {
                BackendEvent::SourcesChanged(list) => {
                    st.inputs = list
                        .into_iter()
                        .map(|s: SourceInfo| InputOption { key: s.key, label: s.label, kind: s.kind, live: true })
                        .collect();
                    refresh_strip_inputs(&mut st);
                    refresh_bus_inputs(&mut st);
                    // Re-link any strip whose chosen source just (re)appeared.
                    let strips = st.strips.clone();
                    for (i, s) in strips.iter().enumerate() {
                        if s.input.is_some() {
                            let _ = backend.set_strip_input(i, s.input.clone());
                        }
                    }
                    // Same for any bus with a direct input assigned.
                    let buses = st.buses.clone();
                    for (i, b) in buses.iter().enumerate() {
                        if b.input.is_some() {
                            let _ = backend.set_bus_input(i, b.input.clone());
                        }
                    }
                }
                BackendEvent::DevicesChanged(devs) => st.devices = devs,
                BackendEvent::CaptureAppsChanged(list) => {
                    st.capture_apps = list
                        .into_iter()
                        .map(|s| InputOption { key: s.key, label: s.label, kind: s.kind, live: true })
                        .collect();
                    // Re-assert any bus/strip listener whose app just (re)appeared.
                    let buses = st.buses.clone();
                    for (i, b) in buses.iter().enumerate() {
                        if b.listener.is_some() {
                            let _ = backend.set_bus_listener(i, b.listener.clone());
                        }
                    }
                    let strips = st.strips.clone();
                    for (i, s) in strips.iter().enumerate() {
                        if s.listener.is_some() {
                            let _ = backend.set_strip_listener(i, s.listener.clone());
                        }
                    }
                }
                BackendEvent::BusListeners(list) => {
                    for b in st.buses.iter_mut() {
                        b.listeners.clear();
                    }
                    for (idx, apps) in list {
                        if let Some(b) = st.buses.get_mut(idx) {
                            b.listeners = apps;
                        }
                    }
                }
                BackendEvent::StripListeners(list) => {
                    for s in st.strips.iter_mut() {
                        s.listeners.clear();
                    }
                    for (idx, apps) in list {
                        if let Some(s) = st.strips.get_mut(idx) {
                            s.listeners = apps;
                        }
                    }
                }
                BackendEvent::BusReady { idx, id } => {
                    if let Some(b) = st.buses.get_mut(idx) {
                        b.node_id = Some(id);
                    }
                }
                BackendEvent::StripReady { idx, id: _ } => {
                    let _ = idx;
                }
                BackendEvent::Level(key, lv) => match key {
                    LevelKey::Bus(i) => {
                        if let Some(b) = st.buses.get_mut(i) {
                            b.level.max_with(lv);
                        }
                    }
                    LevelKey::Strip(i) => {
                        if let Some(s) = st.strips.get_mut(i) {
                            s.level.max_with(lv);
                        }
                    }
                },
                BackendEvent::Feedback(pairs) => {
                    if pairs.len() > st.feedback.len() {
                        st.push_log(format!("{} ⚠ feedback loop blocked", ts()));
                    }
                    st.feedback = pairs;
                }
                BackendEvent::Log(l) => st.push_log(format!("{} {l}", ts())),
                BackendEvent::RecordStopped(t) => match t {
                    RecTarget::Bus(i) => {
                        if let Some(b) = st.buses.get_mut(i) {
                            b.recording = false;
                        }
                    }
                    RecTarget::Strip(i) => {
                        if let Some(s) = st.strips.get_mut(i) {
                            s.recording = false;
                        }
                    }
                },
            }
        }

        while let Ok(cmd) = commands.try_recv() {
            worked = true;
            let mut st = state.lock().unwrap();
            match cmd {
                Command::SetStripInput { strip, input } => {
                    if let Some(s) = st.strips.get_mut(strip) {
                        s.input = input.clone();
                    }
                    refresh_strip_inputs(&mut st);
                    let label = st
                        .strips
                        .get(strip)
                        .map(|s| s.input_label.clone())
                        .unwrap_or_else(|| "—".into());
                    st.push_log(format!("{} strip {:02} input → {}", ts(), strip + 1, label));
                    let _ = backend.set_strip_input(strip, input);
                }
                Command::ToggleAssign { strip, bus } => {
                    let mut on = false;
                    if let Some(s) = st.strips.get_mut(strip) {
                        if let Some(a) = s.assign.get_mut(bus) {
                            *a = !*a;
                            on = *a;
                        }
                    }
                    let bl = st.buses.get(bus).map(|b| b.label.clone()).unwrap_or_default();
                    st.push_log(format!("{} strip {:02} {} {}", ts(), strip + 1, if on { "→" } else { "⇸" }, bl));
                    let _ = backend.set_strip_assign(strip, bus, on);
                }
                Command::SetStripVolume { strip, volume } => {
                    let v = volume.clamp(0.0, 1.0);
                    if let Some(s) = st.strips.get_mut(strip) {
                        s.volume = v;
                    }
                    let _ = backend.set_strip_volume(strip, v);
                }
                Command::SetStripDsp { strip, dsp } => {
                    if let Some(s) = st.strips.get_mut(strip) {
                        s.dsp = dsp;
                    }
                    let _ = backend.set_strip_dsp(strip, dsp);
                }
                Command::SetStripForceMono { strip, on } => {
                    if let Some(s) = st.strips.get_mut(strip) {
                        s.force_mono = on;
                    }
                    let _ = backend.set_strip_force_mono(strip, on);
                }
                Command::SetStripMute { strip, mute } => {
                    if let Some(s) = st.strips.get_mut(strip) {
                        s.mute = mute;
                    }
                    let _ = backend.set_strip_mute(strip, mute);
                }
                Command::SetBusVolume { bus, volume } => {
                    let v = volume.clamp(0.0, 1.0);
                    if let Some(b) = st.buses.get_mut(bus) {
                        b.volume = v;
                    }
                    let _ = backend.set_bus_volume(bus, v);
                }
                Command::SetBusMute { bus, mute } => {
                    if let Some(b) = st.buses.get_mut(bus) {
                        b.mute = mute;
                    }
                    let _ = backend.set_bus_mute(bus, mute);
                }
                Command::SetBusDevice { bus, device } => {
                    let bl = st.buses.get(bus).map(|b| b.label.clone()).unwrap_or_default();
                    if let Some(b) = st.buses.get_mut(bus) {
                        b.device = device.clone();
                    }
                    st.push_log(format!("{} {} → device {}", ts(), bl, device.as_deref().unwrap_or("<default>")));
                    let _ = backend.set_bus_device(bus, device);
                }
                Command::SetBusInput { bus, input } => {
                    if let Some(b) = st.buses.get_mut(bus) {
                        b.input = input.clone();
                    }
                    refresh_bus_inputs(&mut st);
                    let label = st.buses.get(bus).map(|b| b.input_label.clone()).unwrap_or_else(|| "—".into());
                    let bl = st.buses.get(bus).map(|b| b.label.clone()).unwrap_or_default();
                    st.push_log(format!("{} {} input → {}", ts(), bl, label));
                    let _ = backend.set_bus_input(bus, input);
                }
                Command::StartRecordTarget { target } => {
                    let label = match target {
                        RecTarget::Bus(i) => st.buses.get(i).map(|b| b.display_name()).unwrap_or_default(),
                        RecTarget::Strip(i) => {
                            st.strips.get(i).map(|s| s.display_name(i)).unwrap_or_default()
                        }
                    };
                    let dir = config.recordings_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                    let safe: String = label
                        .chars()
                        .map(|c| if c.is_alphanumeric() { c } else { '-' })
                        .collect();
                    let path = dir.join(format!("rec-{safe}-{secs}.wav"));
                    st.push_log(format!("{} ● REC {} → {}", ts(), label, path.display()));
                    match target {
                        RecTarget::Bus(i) => {
                            if let Some(b) = st.buses.get_mut(i) {
                                b.recording = true;
                            }
                        }
                        RecTarget::Strip(i) => {
                            if let Some(s) = st.strips.get_mut(i) {
                                s.recording = true;
                            }
                        }
                    }
                    let _ = backend.start_record(target, path);
                }
                Command::StopRecordTarget { target } => {
                    match target {
                        RecTarget::Bus(i) => {
                            if let Some(b) = st.buses.get_mut(i) {
                                b.recording = false;
                            }
                        }
                        RecTarget::Strip(i) => {
                            if let Some(s) = st.strips.get_mut(i) {
                                s.recording = false;
                            }
                        }
                    }
                    st.push_log(format!("{} ■ recording saved", ts()));
                    let _ = backend.stop_record(target);
                }
                Command::SetStripName { strip, name } => {
                    if let Some(s) = st.strips.get_mut(strip) {
                        s.name = name;
                    }
                }
                Command::SetBusName { bus, name } => {
                    if let Some(b) = st.buses.get_mut(bus) {
                        b.name = name;
                    }
                }
                Command::SetBusListener { bus, app } => {
                    let bl = st.buses.get(bus).map(|b| b.label.clone()).unwrap_or_default();
                    if let Some(b) = st.buses.get_mut(bus) {
                        b.listener = app.clone();
                    }
                    st.push_log(format!(
                        "{} {} → mic of {}",
                        ts(),
                        bl,
                        app.as_deref().unwrap_or("— none —")
                    ));
                    let _ = backend.set_bus_listener(bus, app);
                }
                Command::SetStripListener { strip, app } => {
                    let label = st.strips.get(strip).map(|s| s.display_name(strip)).unwrap_or_default();
                    if let Some(s) = st.strips.get_mut(strip) {
                        s.listener = app.clone();
                    }
                    st.push_log(format!(
                        "{} {} → mic of {}",
                        ts(),
                        label,
                        app.as_deref().unwrap_or("— none —")
                    ));
                    let _ = backend.set_strip_listener(strip, app);
                }
                Command::ToggleBusMonitor { bus, a_bus } => {
                    let mut on = false;
                    if let Some(b) = st.buses.get_mut(bus) {
                        if let Some(m) = b.monitor.get_mut(a_bus) {
                            *m = !*m;
                            on = *m;
                        }
                    }
                    let _ = backend.set_bus_monitor(bus, a_bus, on);
                }
                Command::ToggleBusFeed { from, to } => {
                    // Direct 2-cycle guard only: refuse turning `from → to` on
                    // if `to → from` is already on. Longer chains (B1→B2→B3→B1)
                    // are not detected — a deliberate scope boundary, not a gap
                    // (real Voicemeeter doesn't have bus-to-bus routing at all).
                    let already_reverse = st.buses.get(to).and_then(|b| b.feeds.get(from)).copied().unwrap_or(false);
                    let currently_on = st.buses.get(from).and_then(|b| b.feeds.get(to)).copied().unwrap_or(false);
                    if !currently_on && already_reverse {
                        let fl = st.buses.get(from).map(|b| b.label.clone()).unwrap_or_default();
                        let tl = st.buses.get(to).map(|b| b.label.clone()).unwrap_or_default();
                        st.push_log(format!("{} refused: {} already feeds {} (would 2-cycle)", ts(), tl, fl));
                    } else {
                        let mut on = false;
                        if let Some(b) = st.buses.get_mut(from) {
                            if let Some(f) = b.feeds.get_mut(to) {
                                *f = !*f;
                                on = *f;
                            }
                        }
                        let _ = backend.set_bus_feed(from, to, on);
                    }
                }
                Command::ToggleBusStripFeed { bus, strip } => {
                    // Same direct-2-cycle guard as ToggleBusFeed, mirrored
                    // for the strip→bus→strip case: refuse feeding a strip
                    // that's already sending to this same bus.
                    let strip_sends_here = st.strips.get(strip).and_then(|s| s.assign.get(bus)).copied().unwrap_or(false);
                    let currently_on = st.buses.get(bus).and_then(|b| b.strip_feeds.get(strip)).copied().unwrap_or(false);
                    if !currently_on && strip_sends_here {
                        let bl = st.buses.get(bus).map(|b| b.label.clone()).unwrap_or_default();
                        let sl = st.strips.get(strip).map(|s| s.display_name(strip)).unwrap_or_default();
                        st.push_log(format!("{} refused: {} already sends to {} (would 2-cycle)", ts(), sl, bl));
                    } else {
                        let mut on = false;
                        if let Some(b) = st.buses.get_mut(bus) {
                            if let Some(f) = b.strip_feeds.get_mut(strip) {
                                *f = !*f;
                                on = *f;
                            }
                        }
                        let _ = backend.set_bus_strip_feed(bus, strip, on);
                    }
                }
                Command::SetRecordingsDir { path } => {
                    let p = std::path::PathBuf::from(shellexpand_home(&path));
                    config.recordings_dir = Some(p.clone());
                    st.recordings_dir = p.display().to_string();
                    st.push_log(format!("{} recordings dir → {}", ts(), p.display()));
                    let _ = config.save();
                }
                Command::SetUiScale { scale } => {
                    config.ui_scale = scale.clamp(0.5, 3.0);
                    st.ui_scale = config.ui_scale;
                    let _ = config.save();
                }
                Command::SetSampleRate { rate } => {
                    // Only the three rates the Settings picker offers — a
                    // stray/typo'd value falling back to 48000 (the system's
                    // current default) is safer than forcing an arbitrary
                    // rate PipeWire might refuse outright.
                    let rate = match rate { 44_100 | 48_000 | 96_000 => rate, _ => 48_000 };
                    config.sample_rate = rate;
                    st.sample_rate = rate;
                    let _ = config.save();
                    let _ = backend.set_sample_rate(rate);
                    st.push_log(format!("{} sample rate → {rate} Hz (restarting PipeWire to apply)", ts()));
                    // Confirmed live (this session): setting clock.force-rate
                    // alone is NOT enough to actually switch the running
                    // rate — PipeWire keeps the existing rate if the ALSA
                    // driver node is already active, and the setting itself
                    // isn't picked up cleanly without the graph restarting.
                    // Same class of disruptive action as the GUI's "RESET
                    // AUDIO TO STOCK PIPEWIRE" button (restarts the same
                    // three units), which is why this restarts PipeWire too
                    // rather than trying to hot-swap the rate on a live
                    // graph — then re-applies the metadata shortly after
                    // (spawned on a separate thread so this command handler
                    // doesn't block the engine's command loop for the
                    // ~1.5s the restart needs to settle) so the FRESH graph
                    // comes up already forced to the new rate instead of
                    // silently reverting to pipewire.conf's stock default.
                    apply_sample_rate_metadata(rate);
                    let _ = std::process::Command::new("systemctl")
                        .args(["--user", "restart", "pipewire.socket", "pipewire-pulse.socket"])
                        .spawn();
                    let _ = std::process::Command::new("systemctl")
                        .args(["--user", "restart", "wireplumber.service"])
                        .spawn();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(1500));
                        apply_sample_rate_metadata(rate);
                    });
                }
                Command::SetFeedbackGuard { on } => {
                    config.feedback_guard = on;
                    st.feedback_guard = on;
                    st.push_log(format!("{} feedback guard {}", ts(), if on { "ON" } else { "OFF" }));
                    let _ = backend.set_feedback_guard(on);
                }
                Command::SetEnabled { on } => {
                    config.enabled = on;
                    st.enabled = on;
                    let _ = config.save();
                    st.push_log(format!(
                        "{} FerroMix {} — {}",
                        ts(),
                        if on { "ON" } else { "OFF" },
                        if on { "routing config applied" } else { "released to stock PipeWire" }
                    ));
                    let _ = backend.set_enabled(on);
                }
                Command::AddStrip => {
                    let n = st.buses.len();
                    let new_idx = st.strips.len();
                    let strip = Strip::empty(n);
                    // Previously GUI-state-only: grew `st.strips` but never
                    // told the backend, so a strip added mid-session had no
                    // real PipeWire node until the next restart (config
                    // reload picked up the new count). `ensure_strip` +
                    // `push_strip` mirror exactly what the startup loop does
                    // for every config-loaded strip — same calls, just for
                    // one strip, live.
                    let _ = backend.ensure_strip(new_idx, &model::strip_device_label(new_idx));
                    push_strip(backend, new_idx, &strip);
                    st.strips.push(strip);
                    st.push_log(format!("{} added strip {:02}", ts(), new_idx + 1));
                }
                Command::RemoveLastStrip => {
                    if st.strips.len() <= 1 {
                        st.push_log(format!("{} refused: can't remove the last remaining strip", ts()));
                    } else {
                        let idx = st.strips.len() - 1;
                        st.strips.pop();
                        let _ = backend.remove_strip(idx);
                        st.push_log(format!("{} removed strip {:02}", ts(), idx + 1));
                    }
                }
                Command::Save => {
                    config.strip_count = st.strips.len();
                    let a_labels: Vec<String> = st
                        .buses
                        .iter()
                        .filter(|b| b.kind == BusKind::HwOutput)
                        .map(|b| b.label.clone())
                        .collect();
                    let all_labels: Vec<String> = st.buses.iter().map(|b| b.label.clone()).collect();
                    config.buses = st.buses.iter().map(|b| crate::config::BusCfg {
                        label: b.label.clone(),
                        name: b.name.clone(),
                        listener: b.listener.clone(),
                        input: b.input.clone(),
                        monitor: b
                            .monitor
                            .iter()
                            .enumerate()
                            .filter(|(_, on)| **on)
                            .filter_map(|(ai, _)| a_labels.get(ai).cloned())
                            .collect(),
                        feeds: b
                            .feeds
                            .iter()
                            .enumerate()
                            .filter(|(_, on)| **on)
                            .filter_map(|(gi, _)| all_labels.get(gi).cloned())
                            .collect(),
                        strip_feeds: b
                            .strip_feeds
                            .iter()
                            .enumerate()
                            .filter(|(_, on)| **on)
                            .map(|(si, _)| si)
                            .collect(),
                        kind: match b.kind { BusKind::HwOutput => "hw", BusKind::VirtualMic => "virtual" }.into(),
                        device: b.device.clone(),
                        volume: b.volume,
                        mute: b.mute,
                    }).collect();
                    config.strips = st.strips.iter().map(|s| crate::config::StripCfg {
                        name: s.name.clone(),
                        input: s.input.clone(),
                        listener: s.listener.clone(),
                        volume: s.volume,
                        mute: s.mute,
                        assign: s.assign.iter().enumerate().filter(|(_, on)| **on)
                            .filter_map(|(bi, _)| st.buses.get(bi).map(|b| b.label.clone())).collect(),
                        force_mono: s.force_mono,
                    }).collect();
                    match config.save() {
                        Ok(()) => st.push_log(format!("{} config saved", ts())),
                        Err(e) => st.push_log(format!("{} save FAILED: {e}", ts())),
                    }
                }
            }
        }

        let dt = last_decay.elapsed();
        // 16ms to match the GUI's faster (~60Hz) poll rate — smoother meter
        // decay. The 0.82 constant is normalized per-33ms via dt/0.033 below,
        // so the overall decay RATE per second is unchanged, just applied in
        // smaller, more frequent steps.
        if dt >= Duration::from_millis(16) {
            last_decay = Instant::now();
            let decay = 0.82_f32.powf(dt.as_secs_f32() / 0.033);
            let mut st = state.lock().unwrap();
            for s in &mut st.strips {
                s.level.decay(decay);
            }
            for b in &mut st.buses {
                b.level.decay(decay);
            }
        }

        if !worked {
            std::thread::sleep(Duration::from_millis(4));
        }
    }
}

pub use crate::model::Device as HwDevice;

/// Expand a leading `~` so the settings field accepts "~/Music/ferromix".
fn shellexpand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    p.to_string()
}
