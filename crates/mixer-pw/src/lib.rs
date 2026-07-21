//! mixer-pw — the real PipeWire backend (Linux only).
//!
//! All PipeWire objects live on one loop thread. `PwBackend` implements
//! `AudioBackend` by sending `PwCmd`s onto that thread; graph events flow back
//! over an mpsc channel. The worker is declarative: commands update a desired
//! model and a reconciler converges real links on every registry change, so
//! routes survive app restarts and call drops, and feedback loops are caught
//! before their closing link is ever made.

#![cfg(target_os = "linux")]

mod links;
mod recorder;
mod registry;
mod tap;
mod virtual_dev;
mod dsp;
mod worker;

use mixer_core::backend::{AudioBackend, BackendEvent, BackendResult};
use mixer_core::model::{BusKind, RecTarget, StripDsp};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

#[derive(Debug)]
pub(crate) enum PwCmd {
    EnsureStrip { idx: usize, label: String },
    SetStripInput { idx: usize, source_key: Option<String> },
    SetStripVolume { idx: usize, volume: f32 },
    SetStripMute { idx: usize, mute: bool },
    SetStripAssign { idx: usize, bus: usize, on: bool },
    SetStripDsp { idx: usize, dsp: StripDsp },
    SetFeedbackGuard { on: bool },
    SetDefaultOutput { idx: usize },
    SetDefaultInput { idx: usize },
    EnsureBus { idx: usize, label: String, kind: BusKind },
    SetBusDevice { idx: usize, device: Option<String> },
    SetBusVolume { idx: usize, volume: f32 },
    SetBusMute { idx: usize, mute: bool },
    SetBusMonitor { bus: usize, a_bus: usize, on: bool },
    SetBusListener { bus: usize, app_key: Option<String> },
    StartRecord { target: RecTarget, path: PathBuf },
    StopRecord { target: RecTarget },
}

pub struct PwBackend {
    tx: pipewire::channel::Sender<PwCmd>,
}

impl PwBackend {
    pub fn new() -> Result<(Self, Receiver<BackendEvent>), String> {
        let (ev_tx, ev_rx) = std::sync::mpsc::channel::<BackendEvent>();
        let (cmd_tx, cmd_rx) = pipewire::channel::channel::<PwCmd>();
        std::thread::Builder::new()
            .name("ferromix-pw".into())
            .spawn(move || {
                if let Err(e) = worker::run(cmd_rx, ev_tx.clone()) {
                    log::error!("pipewire worker died: {e}");
                    let _ = ev_tx.send(BackendEvent::Log(format!("pipewire worker died: {e}")));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok((PwBackend { tx: cmd_tx }, ev_rx))
    }

    fn send(&self, cmd: PwCmd) -> BackendResult {
        self.tx.send(cmd).map_err(|_| "pipewire thread gone".to_string())
    }
}

impl AudioBackend for PwBackend {
    fn ensure_strip(&mut self, idx: usize, label: &str) -> BackendResult {
        self.send(PwCmd::EnsureStrip { idx, label: label.to_string() })
    }
    fn set_strip_input(&mut self, idx: usize, source_key: Option<String>) -> BackendResult {
        self.send(PwCmd::SetStripInput { idx, source_key })
    }
    fn set_strip_volume(&mut self, idx: usize, volume: f32) -> BackendResult {
        self.send(PwCmd::SetStripVolume { idx, volume })
    }
    fn set_strip_mute(&mut self, idx: usize, mute: bool) -> BackendResult {
        self.send(PwCmd::SetStripMute { idx, mute })
    }
    fn set_strip_assign(&mut self, idx: usize, bus_idx: usize, on: bool) -> BackendResult {
        self.send(PwCmd::SetStripAssign { idx, bus: bus_idx, on })
    }
    fn set_strip_dsp(&mut self, idx: usize, dsp: StripDsp) -> BackendResult {
        self.send(PwCmd::SetStripDsp { idx, dsp })
    }
    fn set_feedback_guard(&mut self, on: bool) -> BackendResult {
        self.send(PwCmd::SetFeedbackGuard { on })
    }
    fn set_default_output_strip(&mut self, idx: usize) -> BackendResult {
        self.send(PwCmd::SetDefaultOutput { idx })
    }
    fn set_default_input_bus(&mut self, idx: usize) -> BackendResult {
        self.send(PwCmd::SetDefaultInput { idx })
    }
    fn ensure_bus(&mut self, idx: usize, label: &str, kind: BusKind) -> BackendResult {
        self.send(PwCmd::EnsureBus { idx, label: label.to_string(), kind })
    }
    fn set_bus_device(&mut self, idx: usize, device: Option<String>) -> BackendResult {
        self.send(PwCmd::SetBusDevice { idx, device })
    }
    fn set_bus_volume(&mut self, bus_idx: usize, volume: f32) -> BackendResult {
        self.send(PwCmd::SetBusVolume { idx: bus_idx, volume })
    }
    fn set_bus_mute(&mut self, bus_idx: usize, mute: bool) -> BackendResult {
        self.send(PwCmd::SetBusMute { idx: bus_idx, mute })
    }
    fn set_bus_monitor(&mut self, bus_idx: usize, a_bus_idx: usize, on: bool) -> BackendResult {
        self.send(PwCmd::SetBusMonitor { bus: bus_idx, a_bus: a_bus_idx, on })
    }
    fn set_bus_listener(&mut self, bus_idx: usize, app_key: Option<String>) -> BackendResult {
        self.send(PwCmd::SetBusListener { bus: bus_idx, app_key })
    }
    fn start_record(&mut self, target: RecTarget, path: PathBuf) -> BackendResult {
        self.send(PwCmd::StartRecord { target, path })
    }
    fn stop_record(&mut self, target: RecTarget) -> BackendResult {
        self.send(PwCmd::StopRecord { target })
    }
}
