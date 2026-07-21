//! MockBackend: a fake world for GUI development without PipeWire.
//!
//! Mirrors the real architecture: every strip is a device. A strip makes sound
//! if you link a source into it OR if a (fake) app has "pointed its output" at
//! its device. Two mics, several apps, a Linphone call that drops at 16 s and
//! returns at 24 s, and a feedback guard on B-buses.

use crate::backend::{AudioBackend, BackendEvent, BackendResult};
use crate::model::{BusKind, Device, Level, LevelKey, RecTarget, SourceInfo, SourceKind};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Shared {
    bus_listener: HashMap<usize, String>,
    n_strips: usize,
    n_buses: usize,
    /// strip -> chosen source key
    strip_input: HashMap<usize, String>,
    strip_faders: HashMap<usize, (f32, bool)>,
    bus_faders: HashMap<usize, (f32, bool)>,
    /// (strip, bus) sends that are on.
    assigns: HashSet<(usize, usize)>,
    bus_kind: HashMap<usize, BusKind>,
    guard: bool,
}

pub struct MockBackend {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<BackendEvent>,
}

impl MockBackend {
    pub fn new() -> (Self, Receiver<BackendEvent>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(Mutex::new(Shared { guard: true, ..Default::default() }));
        let backend = MockBackend { shared: Arc::clone(&shared), tx: tx.clone() };
        std::thread::Builder::new()
            .name("ferromix-mock".into())
            .spawn(move || fake_world(tx, shared))
            .expect("spawn mock");
        (backend, rx)
    }
}

struct FakeApp {
    #[allow(dead_code)]
    key: &'static str,
    label: &'static str,
    /// Also opens a mic input (Discord, softphones) → can create a loop.
    #[allow(dead_code)]
    listens: bool,
}

fn source_list(apps: &[FakeApp]) -> Vec<SourceInfo> {
    let mut v = vec![
        SourceInfo {
            key: "corsair-virtuoso-mic".into(),
            label: "Corsair VIRTUOSO Mic".into(),
            kind: SourceKind::HwInput,
        },
        SourceInfo { key: "blue-yeti".into(), label: "Blue Yeti".into(), kind: SourceKind::HwInput },
    ];
    for a in apps {
        v.push(SourceInfo { key: a.key.into(), label: a.label.into(), kind: SourceKind::App });
    }
    v
}

fn activity(t: f32, seed: u32) -> f32 {
    let f = 1.6 + (seed as f32 % 3.0) * 0.7;
    let beat = ((t * f).sin().abs()).powf(3.0);
    let swell = 0.55 + 0.45 * (t * 0.23 + seed as f32).sin();
    (beat * swell).clamp(0.0, 1.0)
}

fn speech(t: f32, seed: u32) -> f32 {
    let phrase = ((t * 0.35 + seed as f32).sin() > -0.2) as u32 as f32;
    let syll = ((t * 6.0 + seed as f32 * 2.0).sin().abs()).powf(2.0);
    (phrase * (0.35 + 0.6 * syll)).clamp(0.0, 1.0)
}

/// Raw level a source produces, and whether it also listens.
fn source_signal(key: &str, t: f32) -> (f32, bool) {
    match key {
        "corsair-virtuoso-mic" => (speech(t, 7), false),
        "blue-yeti" => (0.05 + 0.03 * (t * 1.3).sin().abs(), false),
        "discord" => (activity(t, 21), true),
        "linphone" => (speech(t, 11), true),
        "firefox" => (activity(t, 22), false),
        "spotify" => (activity(t, 3), false),
        _ => (0.0, false),
    }
}

fn fake_world(tx: Sender<BackendEvent>, shared: Arc<Mutex<Shared>>) {
    let _ = tx.send(BackendEvent::Log("mock backend online (no real audio)".into()));
    let _ = tx.send(BackendEvent::DevicesChanged(vec![
        Device { key: "corsair-virtuoso-headset".into(), label: "Corsair VIRTUOSO Headset".into() },
        Device { key: "hd-audio-speakers".into(), label: "Speakers (HD Audio)".into() },
        Device { key: "hdmi-monitor".into(), label: "HDMI / Monitor".into() },
    ]));

    let mut apps: Vec<FakeApp> = vec![
        FakeApp { key: "firefox", label: "Firefox", listens: false },
        FakeApp { key: "discord", label: "Discord", listens: true },
        FakeApp { key: "linphone", label: "Linphone (SIP)", listens: true },
    ];
    let _ = tx.send(BackendEvent::SourcesChanged(source_list(&apps)));

    let start = Instant::now();
    let (mut spotify_in, mut dropped, mut back) = (false, false, false);

    loop {
        std::thread::sleep(Duration::from_millis(33));
        let t = start.elapsed().as_secs_f32();

        if !spotify_in && t > 8.0 {
            spotify_in = true;
            apps.push(FakeApp { key: "spotify", label: "Spotify", listens: false });
            let _ = tx.send(BackendEvent::SourcesChanged(source_list(&apps)));
            let _ = tx.send(BackendEvent::Log("app appeared: Spotify".into()));
        }
        if !dropped && t > 16.0 {
            dropped = true;
            apps.retain(|a| a.key != "linphone");
            let _ = tx.send(BackendEvent::SourcesChanged(source_list(&apps)));
            let _ = tx.send(BackendEvent::Log("call ended — strip keeps the route".into()));
        }
        if dropped && !back && t > 24.0 {
            back = true;
            apps.push(FakeApp { key: "linphone", label: "Linphone (SIP)", listens: true });
            let _ = tx.send(BackendEvent::SourcesChanged(source_list(&apps)));
            let _ = tx.send(BackendEvent::Log("call started — routes reattached".into()));
        }

        let sh = shared.lock().unwrap();
        let (n_strips, n_buses) = (sh.n_strips, sh.n_buses);
        let live_keys: HashSet<&str> = apps.iter().map(|a| a.key).collect();

        let pan = |lvl: f32, seed: f32| -> Level {
            let w = 0.75 + 0.25 * (t * 0.9 + seed).sin();
            Level { l: (lvl * w).clamp(0.0, 1.0), r: (lvl * (1.75 - w)).clamp(0.0, 1.0) }
        };

        // Strip levels: whatever source is linked into the strip device.
        let mut strip_out = vec![0.0f32; n_strips];
        let mut strip_listens = vec![false; n_strips];
        for i in 0..n_strips {
            let Some(key) = sh.strip_input.get(&i) else { continue };
            let is_hw = key == "corsair-virtuoso-mic" || key == "blue-yeti";
            if !is_hw && !live_keys.contains(key.as_str()) {
                continue; // app not running
            }
            let (raw, listens) = source_signal(key, t);
            strip_listens[i] = listens;
            let (vol, mute) = sh.strip_faders.get(&i).copied().unwrap_or((1.0, false));
            let lvl = (raw * if mute { 0.0 } else { vol }).clamp(0.0, 1.0);
            strip_out[i] = lvl;
            if lvl > 0.003 {
                let _ = tx.send(BackendEvent::Level(LevelKey::Strip(i), pan(lvl, i as f32)));
            }
        }

        let mut fb: Vec<(usize, usize)> = Vec::new();
        let mut bus_sum = vec![0.0f32; n_buses];
        for b in 0..n_buses {
            let is_b = sh.bus_kind.get(&b) == Some(&BusKind::VirtualMic);
            for s in 0..n_strips {
                if !sh.assigns.contains(&(s, b)) {
                    continue;
                }
                // The app on a strip that also listens to this virtual mic would
                // hear itself — refuse the send.
                if sh.guard && is_b && strip_listens[s] {
                    fb.push((s, b));
                    continue;
                }
                bus_sum[b] += strip_out[s];
            }
            let (vol, mute) = sh.bus_faders.get(&b).copied().unwrap_or((1.0, false));
            let out = (bus_sum[b] * if mute { 0.0 } else { vol }).clamp(0.0, 1.0);
            if out > 0.003 {
                let _ = tx.send(BackendEvent::Level(LevelKey::Bus(b), pan(out, 50.0 + b as f32)));
            }
        }
        drop(sh);
        let _ = tx.send(BackendEvent::Feedback(fb));
        // Apps that have a mic open — these are assignable to a B bus.
        let cap_apps: Vec<SourceInfo> = apps
            .iter()
            .filter(|a| a.listens)
            .map(|a| SourceInfo { key: a.key.into(), label: a.label.into(), kind: SourceKind::App })
            .collect();
        let _ = tx.send(BackendEvent::CaptureAppsChanged(cap_apps));

        // Whoever we assigned is now listening.
        let sh2 = shared.lock().unwrap();
        let listeners: Vec<(usize, Vec<String>)> = (0..n_buses)
            .map(|b| {
                let who = sh2
                    .bus_listener
                    .get(&b)
                    .map(|k| {
                        vec![apps
                            .iter()
                            .find(|a| a.key == k)
                            .map(|a| a.label.to_string())
                            .unwrap_or_else(|| k.clone())]
                    })
                    .unwrap_or_default();
                (b, who)
            })
            .collect();
        drop(sh2);
        let _ = tx.send(BackendEvent::BusListeners(listeners));
    }
}

impl AudioBackend for MockBackend {
    fn ensure_strip(&mut self, idx: usize, label: &str) -> BackendResult {
        let mut sh = self.shared.lock().unwrap();
        sh.n_strips = sh.n_strips.max(idx + 1);
        drop(sh);
        let _ = self.tx.send(BackendEvent::StripReady { idx, id: 1000 + idx as u32 });
        let _ = self.tx.send(BackendEvent::Log(format!("strip device ready: {label}")));
        Ok(())
    }
    fn set_strip_input(&mut self, idx: usize, source_key: Option<String>) -> BackendResult {
        let mut sh = self.shared.lock().unwrap();
        match source_key {
            Some(k) => {
                sh.strip_input.insert(idx, k);
            }
            None => {
                sh.strip_input.remove(&idx);
            }
        }
        Ok(())
    }
    fn set_strip_volume(&mut self, idx: usize, volume: f32) -> BackendResult {
        self.shared.lock().unwrap().strip_faders.entry(idx).or_insert((1.0, false)).0 = volume;
        Ok(())
    }
    fn set_strip_mute(&mut self, idx: usize, mute: bool) -> BackendResult {
        self.shared.lock().unwrap().strip_faders.entry(idx).or_insert((1.0, false)).1 = mute;
        Ok(())
    }
    fn set_strip_assign(&mut self, idx: usize, bus_idx: usize, on: bool) -> BackendResult {
        let mut sh = self.shared.lock().unwrap();
        if on {
            sh.assigns.insert((idx, bus_idx));
        } else {
            sh.assigns.remove(&(idx, bus_idx));
        }
        Ok(())
    }
    fn set_strip_dsp(&mut self, _idx: usize, _dsp: crate::model::StripDsp) -> BackendResult {
        // No live audio graph to splice DSP into in mock mode.
        Ok(())
    }
    fn ensure_bus(&mut self, idx: usize, label: &str, kind: BusKind) -> BackendResult {
        let mut sh = self.shared.lock().unwrap();
        sh.n_buses = sh.n_buses.max(idx + 1);
        sh.bus_kind.insert(idx, kind);
        drop(sh);
        let _ = self.tx.send(BackendEvent::BusReady { idx, id: 2000 + idx as u32 });
        let k = match kind {
            BusKind::HwOutput => "hw out",
            BusKind::VirtualMic => "virtual mic",
        };
        let _ = self.tx.send(BackendEvent::Log(format!("bus ready: {label} ({k})")));
        Ok(())
    }
    fn set_bus_device(&mut self, idx: usize, device: Option<String>) -> BackendResult {
        let _ = self.tx.send(BackendEvent::Log(format!(
            "bus {} device: {}",
            idx + 1,
            device.as_deref().unwrap_or("<default>")
        )));
        Ok(())
    }
    fn set_bus_volume(&mut self, bus_idx: usize, volume: f32) -> BackendResult {
        self.shared.lock().unwrap().bus_faders.entry(bus_idx).or_insert((1.0, false)).0 = volume;
        Ok(())
    }
    fn set_bus_mute(&mut self, bus_idx: usize, mute: bool) -> BackendResult {
        self.shared.lock().unwrap().bus_faders.entry(bus_idx).or_insert((1.0, false)).1 = mute;
        Ok(())
    }
    fn set_feedback_guard(&mut self, on: bool) -> BackendResult {
        self.shared.lock().unwrap().guard = on;
        Ok(())
    }
    fn set_default_output_strip(&mut self, idx: usize) -> BackendResult {
        let _ = self.tx.send(BackendEvent::DefaultOutput(Some(idx)));
        let _ = self
            .tx
            .send(BackendEvent::Log(format!("mock: system default output → Input {}", idx + 1)));
        Ok(())
    }
    fn set_default_input_bus(&mut self, idx: usize) -> BackendResult {
        let _ = self.tx.send(BackendEvent::DefaultInput(Some(idx)));
        Ok(())
    }
    fn set_bus_monitor(&mut self, _bus_idx: usize, _a_bus_idx: usize, _on: bool) -> BackendResult {
        Ok(())
    }
    fn set_bus_listener(&mut self, bus_idx: usize, app_key: Option<String>) -> BackendResult {
        let mut sh = self.shared.lock().unwrap();
        match app_key {
            Some(k) => {
                sh.bus_listener.insert(bus_idx, k);
            }
            None => {
                sh.bus_listener.remove(&bus_idx);
            }
        }
        Ok(())
    }
    fn start_record(&mut self, target: RecTarget, path: PathBuf) -> BackendResult {
        let _ = target;
        let _ = self
            .tx
            .send(BackendEvent::Log(format!("mock: 'recording' → {}", path.display())));
        Ok(())
    }
    fn stop_record(&mut self, target: RecTarget) -> BackendResult {
        let _ = self.tx.send(BackendEvent::RecordStopped(target));
        Ok(())
    }
}
