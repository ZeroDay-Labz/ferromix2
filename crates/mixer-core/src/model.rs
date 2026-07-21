//! Shared data model. Voicemeeter-style: STRIPS with a selectable INPUT,
//! assigned to BUSES (A = hardware outputs, B = virtual mics).
//!
//! The primary input for a strip is a **virtual input device** ("FerroMix
//! Input 1".."5"). You point an app's output at one — exactly like setting an
//! app to "Voicemeeter Input" — and that strip becomes that app. This is what
//! makes routing bulletproof: it doesn't depend on detecting the app's stream,
//! so it survives silence, restarts and dropped calls.

use serde::{Deserialize, Serialize};

pub type NodeId = u32;

/// Every strip owns a virtual sink device. Apps can point their output straight
/// at it ("FerroMix Input N", exactly like Voicemeeter Input), and/or you can
/// select a mic or an app to be linked into it. Either way the audio passes
/// through ONE node we own — which is what makes the fader and meter work for
/// every kind of input.
pub fn strip_device_label(idx: usize) -> String {
    format!("FerroMix Input {}", idx + 1)
}

/// Fader law: ±20 dB, with 0.0 dB dead-centre. Linear in dB, so the number on
/// the cap is the gain you actually get. The fader is a *trim*, not a kill —
/// MUTE is the silence control (a fader that has to bottom out at -∞ wastes
/// most of its travel on gain you never use).
pub const DB_MIN: f32 = -20.0;
pub const DB_MAX: f32 = 20.0;
/// Fader position that means 0.0 dB (unity).
pub const UNITY_POS: f32 = (0.0 - DB_MIN) / (DB_MAX - DB_MIN);

/// Fader position → dB. Below the very bottom it's silence.
pub fn pos_to_db(pos: f32) -> f32 {
    DB_MIN + (DB_MAX - DB_MIN) * pos.clamp(0.0, 1.0)
}

/// dB → fader position.
pub fn db_to_pos(db: f32) -> f32 {
    ((db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0)
}

/// Fader position → linear gain for the audio backend.
pub fn pos_to_gain(pos: f32) -> f32 {
    if pos <= 0.001 {
        0.0
    } else {
        10f32.powf(pos_to_db(pos) / 20.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BusKind {
    HwOutput,
    VirtualMic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    /// A hardware capture device (a microphone / line-in).
    HwInput,
    /// An application that is currently playing audio.
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LevelKey {
    Strip(usize),
    Bus(usize),
}

/// Stereo peak pair.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Level {
    pub l: f32,
    pub r: f32,
}

impl Level {
    pub fn max_with(&mut self, o: Level) {
        self.l = self.l.max(o.l);
        self.r = self.r.max(o.r);
    }
    pub fn decay(&mut self, f: f32) {
        self.l *= f;
        self.r *= f;
        if self.l < 1e-4 {
            self.l = 0.0;
        }
        if self.r < 1e-4 {
            self.r = 0.0;
        }
    }
    pub fn peak(&self) -> f32 {
        self.l.max(self.r)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputOption {
    pub key: String,
    pub label: String,
    pub kind: SourceKind,
    pub live: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Strip {
    /// User-given name ("Mic", "Discord", "SIP") — right-click the header to set.
    /// Empty = fall back to the input's own label.
    #[serde(default)]
    pub name: String,
    pub input: Option<String>,
    pub input_label: String,
    pub input_live: bool,
    pub kind: Option<SourceKind>,
    pub volume: f32,
    pub mute: bool,
    pub level: Level,
    pub assign: Vec<bool>,
    #[serde(default)]
    pub recording: bool,
    /// Per-strip DSP. Off by default; each strip owns its own gate + compressor.
    #[serde(default)]
    pub dsp: StripDsp,
}

/// Per-strip signal processing. A downward noise gate followed by a soft-knee
/// compressor, each with a single "amount" knob mapped to sensible internal
/// parameters — so the UI stays two dials, not twenty.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct StripDsp {
    pub gate_on: bool,
    /// 0.0 = fully open (no gating), 1.0 = aggressive. Maps to threshold.
    pub gate: f32,
    pub comp_on: bool,
    /// 0.0 = none, 1.0 = heavy. Maps to threshold + ratio together.
    pub comp: f32,
}

impl Default for StripDsp {
    fn default() -> Self {
        StripDsp { gate_on: false, gate: 0.3, comp_on: false, comp: 0.4 }
    }
}

impl StripDsp {
    /// Gate open threshold in dB, from the knob (0..1 → -60..-20 dB).
    pub fn gate_threshold_db(&self) -> f32 {
        -60.0 + self.gate.clamp(0.0, 1.0) * 40.0
    }
    /// Compressor threshold in dB (0..1 → -6..-30 dB, more knob = lower).
    pub fn comp_threshold_db(&self) -> f32 {
        -6.0 - self.comp.clamp(0.0, 1.0) * 24.0
    }
    /// Compressor ratio (0..1 → 1.5:1 .. 8:1).
    pub fn comp_ratio(&self) -> f32 {
        1.5 + self.comp.clamp(0.0, 1.0) * 6.5
    }
}

impl Strip {
    /// What to show in the header: the user's name if they set one.
    pub fn display_name(&self, idx: usize) -> String {
        if !self.name.trim().is_empty() {
            self.name.clone()
        } else if self.input.is_some() {
            self.input_label.clone()
        } else {
            format!("Input {}", idx + 1)
        }
    }

    pub fn empty(n_buses: usize) -> Self {
        Strip {
            name: String::new(),
            input: None,
            input_label: "—".into(),
            input_live: false,
            kind: None,
            volume: UNITY_POS,
            mute: false,
            level: Level::default(),
            assign: vec![false; n_buses],
            recording: false,
            dsp: StripDsp::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bus {
    pub label: String,
    /// User-given name ("Discord Mic", "Caller"). Empty = just the label.
    #[serde(default)]
    pub name: String,
    /// monitor[a_bus_idx] — send this virtual mic to a hardware out so you can
    /// hear exactly what the far end hears.
    #[serde(default)]
    pub monitor: Vec<bool>,
    pub kind: BusKind,
    pub device: Option<String>,
    /// Apps currently capturing from this virtual mic — who is actually listening.
    #[serde(default)]
    pub listeners: Vec<String>,
    /// The app we have *assigned* to listen here. FerroMix points that app's
    /// microphone at this bus via PipeWire metadata, so you never have to go
    /// hunting through Discord's settings.
    #[serde(default)]
    pub listener: Option<String>,
    pub volume: f32,
    pub mute: bool,
    pub level: Level,
    pub recording: bool,
    pub node_id: Option<NodeId>,
}

impl Bus {
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            self.label.clone()
        } else {
            format!("{} ({})", self.name, self.label)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MixerState {
    pub strips: Vec<Strip>,
    pub buses: Vec<Bus>,
    pub inputs: Vec<InputOption>,
    pub devices: Vec<Device>,
    /// Apps that have a microphone/capture stream open — assignable to a B bus.
    pub capture_apps: Vec<InputOption>,
    pub feedback: Vec<(usize, usize)>,
    /// Where bus recordings are written.
    pub recordings_dir: String,
    pub feedback_guard: bool,
    /// Strip index that is currently the system default output (if any).
    pub default_output: Option<usize>,
    /// Bus index that is currently the system default input (if any).
    pub default_input: Option<usize>,
    /// UI scale factor. 0.0 = auto (follow the monitor's DPI), else applied
    /// directly as the window's scale factor.
    pub ui_scale: f32,
    pub log: Vec<String>,
}

impl MixerState {
    pub fn push_log(&mut self, line: String) {
        self.log.push(line);
        let overflow = self.log.len().saturating_sub(200);
        if overflow > 0 {
            self.log.drain(0..overflow);
        }
    }
    pub fn is_feedback(&self, strip: usize, bus: usize) -> bool {
        self.feedback.iter().any(|&(s, b)| s == strip && b == bus)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub key: String,
    pub label: String,
    pub kind: SourceKind,
}

/// Anything with a fader can be recorded: a strip, or a bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecTarget {
    Strip(usize),
    Bus(usize),
}
