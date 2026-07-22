//! The `AudioBackend` trait — the seam between the engine and audio.

use crate::model::{BusKind, Device, Level, LevelKey, RecTarget, SourceInfo, StripDsp};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum BackendEvent {
    SourcesChanged(Vec<SourceInfo>),
    DevicesChanged(Vec<Device>),
    /// (bus idx, apps capturing from it) — who is actually listening to a B bus.
    BusListeners(Vec<(usize, Vec<String>)>),
    /// (strip idx, apps capturing from it) — same as `BusListeners`, for strips.
    StripListeners(Vec<(usize, Vec<String>)>),
    /// Apps with a live capture (microphone) stream — assignable to a B bus.
    CaptureAppsChanged(Vec<SourceInfo>),
    BusReady { idx: usize, id: u32 },
    StripReady { idx: usize, id: u32 },
    /// Stereo peak for a meter.
    Level(LevelKey, Level),
    /// (strip_idx, bus_idx) routes refused because they would close a loop.
    Feedback(Vec<(usize, usize)>),
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

    /// Apply gate/compressor settings to strip `idx`'s live audio path. Loads
    /// the strip's filter-chain module on first touch; later calls reload it
    /// with the new values.
    fn set_strip_dsp(&mut self, idx: usize, dsp: StripDsp) -> BackendResult;

    /// See `Strip.force_mono`'s doc comment — forces the input link to fan
    /// the source's first port into every destination channel evenly,
    /// regardless of how many output ports the source really has.
    fn set_strip_force_mono(&mut self, idx: usize, on: bool) -> BackendResult;

    fn ensure_bus(&mut self, idx: usize, label: &str, kind: BusKind) -> BackendResult;
    fn set_bus_device(&mut self, idx: usize, device: Option<String>) -> BackendResult;
    fn set_bus_volume(&mut self, bus_idx: usize, volume: f32) -> BackendResult;
    fn set_bus_mute(&mut self, bus_idx: usize, mute: bool) -> BackendResult;

    fn set_feedback_guard(&mut self, on: bool) -> BackendResult;

    /// Send a bus into a hardware out, so you can monitor what the far end hears.
    fn set_bus_monitor(&mut self, bus_idx: usize, a_bus_idx: usize, on: bool) -> BackendResult;

    /// Route bus `from`'s output additionally into bus `to`'s input, alongside
    /// whatever strips send to it. Global bus indices on both sides.
    fn set_bus_feed(&mut self, from: usize, to: usize, on: bool) -> BackendResult;

    /// Route bus `bus`'s output additionally into strip `strip`'s device —
    /// the reverse direction of `set_strip_assign`. A strip's meter stays
    /// pre-fader/source-only regardless (see `sync_prefader_tap`), so audio
    /// arriving this way never makes a strip's meter move — only its own
    /// assigned `input` does.
    fn set_bus_strip_feed(&mut self, bus: usize, strip: usize, on: bool) -> BackendResult;

    /// Pick which source bus `idx`'s METER tracks — pre-fader, source-only.
    /// `None` clears it (silent meter). Unlike `set_strip_input`, this is
    /// metering-only: it does NOT link the source's audio into the bus's mix
    /// (see `Bus.input`'s doc comment in `model.rs` for why that would be
    /// actively harmful for a bus that's also a `set_bus_listener` target).
    fn set_bus_input(&mut self, idx: usize, source_key: Option<String>) -> BackendResult;

    /// Point an app's MICROPHONE at bus `bus_idx` — i.e. make Discord listen to
    /// B1 without opening Discord's settings. `None` releases the app.
    fn set_bus_listener(&mut self, bus_idx: usize, app_key: Option<String>) -> BackendResult;

    /// Same as `set_bus_listener`, for a strip — any strip can also be an
    /// app's microphone source, not just B-buses (full strip/bus symmetry:
    /// both can receive from an app via input, and send to an app via
    /// listener, so audio can be routed app-to-app like a hardware mixer
    /// routes across devices).
    fn set_strip_listener(&mut self, idx: usize, app_key: Option<String>) -> BackendResult;

    /// Record any strip or bus to its own WAV.
    fn start_record(&mut self, target: RecTarget, path: PathBuf) -> BackendResult;
    fn stop_record(&mut self, target: RecTarget) -> BackendResult;
}
