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
    SetBusVolume { bus: usize, volume: f32 },
    SetBusMute { bus: usize, mute: bool },
    SetBusDevice { bus: usize, device: Option<String> },
    SetRecordingsDir { path: String },
    SetUiScale { scale: f32 },
    SetStripName { strip: usize, name: String },
    SetBusName { bus: usize, name: String },
    ToggleBusMonitor { bus: usize, a_bus: usize },
    SetBusListener { bus: usize, app: Option<String> },
    StartRecordTarget { target: RecTarget },
    StopRecordTarget { target: RecTarget },
    SetDefaultOutput { strip: usize },
    SetDefaultInput { bus: usize },
    SetFeedbackGuard { on: bool },
    AddStrip,
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
    let mut buses = buses;
    for (i, b) in buses.iter_mut().enumerate() {
        let _ = i;
        b.monitor = vec![false; n_a];
    }
    // restore saved monitor flags by A-bus label
    let a_labels: Vec<String> = cfg
        .buses
        .iter()
        .filter(|b| b.bus_kind() == BusKind::HwOutput)
        .map(|b| b.label.clone())
        .collect();
    for (bi, bcfg) in cfg.buses.iter().enumerate() {
        if let Some(b) = buses.get_mut(bi) {
            for (ai, al) in a_labels.iter().enumerate() {
                if bcfg.monitor.iter().any(|m| m.eq_ignore_ascii_case(al)) {
                    if let Some(slot) = b.monitor.get_mut(ai) {
                        *slot = true;
                    }
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
        default_output: None,
        default_input: None,
        log: Vec::new(),
    }
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
        }
        for (bi, b) in st.buses.iter().enumerate() {
            if b.listener.is_some() {
                let _ = backend.set_bus_listener(bi, b.listener.clone());
            }
            for (ai, on) in b.monitor.iter().enumerate() {
                if *on {
                    let _ = backend.set_bus_monitor(bi, ai, true);
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
                    // Re-link any strip whose chosen source just (re)appeared.
                    let strips = st.strips.clone();
                    for (i, s) in strips.iter().enumerate() {
                        if s.input.is_some() {
                            let _ = backend.set_strip_input(i, s.input.clone());
                        }
                    }
                }
                BackendEvent::DevicesChanged(devs) => st.devices = devs,
                BackendEvent::CaptureAppsChanged(list) => {
                    st.capture_apps = list
                        .into_iter()
                        .map(|s| InputOption { key: s.key, label: s.label, kind: s.kind, live: true })
                        .collect();
                    // Re-assert any bus listener whose app just (re)appeared.
                    let buses = st.buses.clone();
                    for (i, b) in buses.iter().enumerate() {
                        if b.listener.is_some() {
                            let _ = backend.set_bus_listener(i, b.listener.clone());
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
                BackendEvent::DefaultOutput(idx) => st.default_output = idx,
                BackendEvent::DefaultInput(idx) => st.default_input = idx,
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
                Command::SetRecordingsDir { path } => {
                    let p = std::path::PathBuf::from(shellexpand_home(&path));
                    config.recordings_dir = Some(p.clone());
                    st.recordings_dir = p.display().to_string();
                    st.push_log(format!("{} recordings dir → {}", ts(), p.display()));
                    let _ = config.save();
                }
                Command::SetUiScale { scale } => {
                    config.ui_scale = scale.clamp(0.5, 3.0);
                    let _ = config.save();
                }
                Command::SetDefaultOutput { strip } => {
                    st.push_log(format!("{} system default output → Input {}", ts(), strip + 1));
                    let _ = backend.set_default_output_strip(strip);
                }
                Command::SetDefaultInput { bus } => {
                    let bl = st.buses.get(bus).map(|b| b.label.clone()).unwrap_or_default();
                    st.push_log(format!("{} system default input → {}", ts(), bl));
                    let _ = backend.set_default_input_bus(bus);
                }
                Command::SetFeedbackGuard { on } => {
                    config.feedback_guard = on;
                    st.feedback_guard = on;
                    st.push_log(format!("{} feedback guard {}", ts(), if on { "ON" } else { "OFF" }));
                    let _ = backend.set_feedback_guard(on);
                }
                Command::AddStrip => {
                    let n = st.buses.len();
                    st.strips.push(Strip::empty(n));
                    let count = st.strips.len();
                    st.push_log(format!("{} added strip {:02}", ts(), count));
                }
                Command::Save => {
                    config.strip_count = st.strips.len();
                    let a_labels: Vec<String> = st
                        .buses
                        .iter()
                        .filter(|b| b.kind == BusKind::HwOutput)
                        .map(|b| b.label.clone())
                        .collect();
                    config.buses = st.buses.iter().map(|b| crate::config::BusCfg {
                        label: b.label.clone(),
                        name: b.name.clone(),
                        listener: b.listener.clone(),
                        monitor: b
                            .monitor
                            .iter()
                            .enumerate()
                            .filter(|(_, on)| **on)
                            .filter_map(|(ai, _)| a_labels.get(ai).cloned())
                            .collect(),
                        kind: match b.kind { BusKind::HwOutput => "hw", BusKind::VirtualMic => "virtual" }.into(),
                        device: b.device.clone(),
                        volume: b.volume,
                        mute: b.mute,
                    }).collect();
                    config.strips = st.strips.iter().map(|s| crate::config::StripCfg {
                        name: s.name.clone(),
                        input: s.input.clone(),
                        volume: s.volume,
                        mute: s.mute,
                        assign: s.assign.iter().enumerate().filter(|(_, on)| **on)
                            .filter_map(|(bi, _)| st.buses.get(bi).map(|b| b.label.clone())).collect(),
                    }).collect();
                    match config.save() {
                        Ok(()) => st.push_log(format!("{} config saved", ts())),
                        Err(e) => st.push_log(format!("{} save FAILED: {e}", ts())),
                    }
                }
            }
        }

        let dt = last_decay.elapsed();
        if dt >= Duration::from_millis(33) {
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
