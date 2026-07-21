//! Config: `~/.config/ferromix/config.toml` — fixed strips (each with a chosen
//! input + bus assignments), buses, feedback guard.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusCfg {
    pub label: String,
    #[serde(default)]
    pub name: String,
    /// Which hardware outs this bus is monitored on.
    #[serde(default)]
    pub monitor: Vec<String>,
    /// Other buses (by label) this bus additionally feeds into.
    #[serde(default)]
    pub feeds: Vec<String>,
    /// A directly-assigned source (app/hw key) feeding this bus, same meaning
    /// as `StripCfg.input`.
    #[serde(default)]
    pub input: Option<String>,
    /// App whose microphone we point at this bus.
    #[serde(default)]
    pub listener: Option<String>,
    #[serde(default = "default_hw")]
    pub kind: String,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub mute: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StripCfg {
    #[serde(default)]
    pub name: String,
    /// Chosen input source key (app name substring or hw input key). Empty slot
    /// if absent.
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub mute: bool,
    /// Bus labels this strip feeds, e.g. ["A1", "B1"].
    #[serde(default)]
    pub assign: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Bumped when the meaning of stored values changes, so an old file can be
    /// corrected instead of silently misbehaving.
    #[serde(default)]
    pub version: u32,
    #[serde(default = "default_true")]
    pub feedback_guard: bool,
    #[serde(default)]
    pub recordings_dir: Option<PathBuf>,
    #[serde(default = "default_strip_count")]
    pub strip_count: usize,
    /// UI scale. 0.0 = auto (follow the monitor's DPI). 1.0 = 1080p native,
    /// 1.5–2.0 for 4K. Saved so the window comes back the size you left it.
    #[serde(default)]
    pub ui_scale: f32,
    #[serde(default)]
    pub buses: Vec<BusCfg>,
    #[serde(default, rename = "strip")]
    pub strips: Vec<StripCfg>,
}

/// v2 = faders are ±20 dB positions, not the old cubic 0..1.
/// v3 = one-time fader reset: some configs saved bus faders at +20 dB (max)
/// due to an earlier default-value bug. Bumping the version resets every fader
/// to 0.0 dB once, self-healing the stuck value.
pub const CONFIG_VERSION: u32 = 3;

fn default_volume() -> f32 {
    crate::model::UNITY_POS // 0.0 dB
}
fn default_true() -> bool {
    true
}
fn default_hw() -> String {
    "hw".into()
}
fn default_strip_count() -> usize {
    5
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: CONFIG_VERSION,
            feedback_guard: true,
            recordings_dir: None,
            strip_count: 5,
            ui_scale: 0.0, // auto
            buses: vec![
                BusCfg { name: String::new(), monitor: Vec::new(), feeds: Vec::new(), input: None, listener: None, label: "A1".into(), kind: "hw".into(), device: None, volume: crate::model::UNITY_POS, mute: false },
                BusCfg { name: String::new(), monitor: Vec::new(), feeds: Vec::new(), input: None, listener: None, label: "A2".into(), kind: "hw".into(), device: None, volume: crate::model::UNITY_POS, mute: false },
                BusCfg { name: String::new(), monitor: Vec::new(), feeds: Vec::new(), input: None, listener: None, label: "A3".into(), kind: "hw".into(), device: None, volume: crate::model::UNITY_POS, mute: false },
                BusCfg { name: String::new(), monitor: Vec::new(), feeds: Vec::new(), input: None, listener: None, label: "B1".into(), kind: "virtual".into(), device: None, volume: crate::model::UNITY_POS, mute: false },
                BusCfg { name: String::new(), monitor: Vec::new(), feeds: Vec::new(), input: None, listener: None, label: "B2".into(), kind: "virtual".into(), device: None, volume: crate::model::UNITY_POS, mute: false },
                BusCfg { name: String::new(), monitor: Vec::new(), feeds: Vec::new(), input: None, listener: None, label: "B3".into(), kind: "virtual".into(), device: None, volume: crate::model::UNITY_POS, mute: false },
            ],
            // A sensible starting patch, mostly pre-filled so it's not empty.
            // Each strip starts wired to its own virtual input and to A1, so
            // pointing an app at "FerroMix Input 2" makes it audible instantly.
            // Strips start as bare devices routed to A1: point an app's output
            // at "FerroMix Input N" and you hear it immediately.
            strips: (0..5)
                .map(|_| StripCfg { name: String::new(), input: None, volume: crate::model::UNITY_POS, mute: false, assign: vec!["A1".into()] })
                .collect(),
        }
    }
}

impl BusCfg {
    pub fn bus_kind(&self) -> crate::model::BusKind {
        match self.kind.as_str() {
            "virtual" | "mic" | "b" => crate::model::BusKind::VirtualMic,
            _ => crate::model::BusKind::HwOutput,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("ferromix2").join("config.toml")
    }
    pub fn recordings_dir(&self) -> PathBuf {
        self.recordings_dir.clone().unwrap_or_else(|| {
            dirs::audio_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from(".")).join("ferromix2")
        })
    }
    /// Make sure every bus we ship exists, even in a config written by an older
    /// build. Users should never have to delete a config file to get a feature.
    fn migrate(&mut self) {
        if self.version < CONFIG_VERSION {
            // The fader law changed. An old "1.0" meant unity; under the new
            // law it means +20 dB, which would blast the user. Reset to 0.0 dB.
            for s in self.strips.iter_mut() {
                s.volume = crate::model::UNITY_POS;
            }
            for b in self.buses.iter_mut() {
                b.volume = crate::model::UNITY_POS;
            }
            self.version = CONFIG_VERSION;
        }
        let wanted: [(&str, &str); 6] = [
            ("A1", "hw"),
            ("A2", "hw"),
            ("A3", "hw"),
            ("B1", "virtual"),
            ("B2", "virtual"),
            ("B3", "virtual"),
        ];
        for (label, kind) in wanted {
            if !self.buses.iter().any(|b| b.label.eq_ignore_ascii_case(label)) {
                self.buses.push(BusCfg {
                    listener: None,
                    label: label.into(),
                    name: String::new(),
                    monitor: Vec::new(),
                    feeds: Vec::new(),
                    input: None,
                    kind: kind.into(),
                    device: None,
                    volume: crate::model::UNITY_POS,
                    mute: false,
                });
            }
        }
        // Keep A buses before B buses, in label order.
        self.buses.sort_by(|a, b| a.label.cmp(&b.label));
        if self.strip_count < 5 {
            self.strip_count = 5;
        }
        while self.strips.len() < self.strip_count {
            self.strips.push(StripCfg {
                name: String::new(),
                input: None,
                volume: crate::model::UNITY_POS,
                mute: false,
                assign: vec!["A1".into()],
            });
        }
    }

    pub fn load_or_create() -> Config {
        let mut cfg = match std::fs::read_to_string(Self::path()) {
            Ok(t) => toml::from_str(&t).unwrap_or_else(|e| {
                log::error!("config parse error: {e} — defaults");
                Config::default()
            }),
            Err(_) => Config::default(),
        };
        cfg.migrate();
        let _ = cfg.save();
        cfg
    }
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, toml::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
}
