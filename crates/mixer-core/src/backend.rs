//! The `AudioBackend` trait — the seam between the engine and audio.

use crate::model::{BusKind, Device, Level, LevelKey, RecTarget, SourceInfo};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum BackendEvent {
    SourcesChanged(Vec<SourceInfo>),
    DevicesChanged(Vec<Device>),
    /// (bus idx, apps capturing from it) — who is actually listening to a B bus.
    BusListeners(Vec<(usize, Vec<String>)>),
    /// Apps with a live capture (microphone) stream — assignable to a B bus.
    CaptureAppsChanged(Vec<SourceInfo>),
    BusReady { idx: usize, id: u32 },
    StripReady { idx: usize, id: u32 },
    /// Stereo peak for a meter.
    Level(LevelKey, Level),
    /// (strip_idx, bus_idx) routes refused because they would close a loop.
    Feedback(Vec<(usize, usize)>),
    /// Which strip (if any) is the current system default output.
    DefaultOutput(Option<usize>),
    /// Which bus (if any) is the current system default input.
    DefaultInput(Option<usize>),
    Log(String),
    RecordStopped(RecTarget),
}

pub type BackendResult = Result<(), String>;

pub trait AudioBackend: Send {
    /// Ensure strip `idx` exists as a virtual sink device named `label`
    /// ("FerroMix Input N"). Apps may point their output straight at it.
    /// Everything a strip does (fader, mute, meter, sends) happens on this node.
    fn ensure_strip(&mut self, idx: usize, label: &str) -> BackendResult;

    /// Link a source (a mic, or an app that's playing) INTO strip `idx`.
    /// `None` clears it — the strip still works for apps pointed at its device.
    fn set_strip_input(&mut self, idx: usize, source_key: Option<String>) -> BackendResult;

    fn set_strip_volume(&mut self, idx: usize, volume: f32) -> BackendResult;
    fn set_strip_mute(&mut self, idx: usize, mute: bool) -> BackendResult;
    /// Does strip `idx` feed bus `bus_idx`?
    fn set_strip_assign(&mut self, idx: usize, bus_idx: usize, on: bool) -> BackendResult;

    fn ensure_bus(&mut self, idx: usize, label: &str, kind: BusKind) -> BackendResult;
    fn set_bus_device(&mut self, idx: usize, device: Option<String>) -> BackendResult;
    fn set_bus_volume(&mut self, bus_idx: usize, volume: f32) -> BackendResult;
    fn set_bus_mute(&mut self, bus_idx: usize, mute: bool) -> BackendResult;

    fn set_feedback_guard(&mut self, on: bool) -> BackendResult;

    /// Make strip `idx`'s device the system default OUTPUT. Apps that don't let
    /// you pick a device (Zoiper, plenty of others) follow the system default —
    /// this is how you get them onto a strip.
    fn set_default_output_strip(&mut self, idx: usize) -> BackendResult;
    /// Make bus `idx`'s virtual mic the system default INPUT, for apps that
    /// can't pick their microphone either.
    fn set_default_input_bus(&mut self, idx: usize) -> BackendResult;

    /// Send a bus into a hardware out, so you can monitor what the far end hears.
    fn set_bus_monitor(&mut self, bus_idx: usize, a_bus_idx: usize, on: bool) -> BackendResult;

    /// Point an app's MICROPHONE at bus `bus_idx` — i.e. make Discord listen to
    /// B1 without opening Discord's settings. `None` releases the app.
    fn set_bus_listener(&mut self, bus_idx: usize, app_key: Option<String>) -> BackendResult;

    /// Record any strip or bus to its own WAV.
    fn start_record(&mut self, target: RecTarget, path: PathBuf) -> BackendResult;
    fn stop_record(&mut self, target: RecTarget) -> BackendResult;
}
