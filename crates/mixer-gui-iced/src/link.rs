//! The seam between the UI and the audio engine. Used to be a Unix-socket
//! client talking to a separate `ferromix2-daemon` process; FerroMix is now a
//! single process, so this just boots `mixer-core::Engine` directly on top of
//! the real PipeWire backend and hands back the same `EngineHandle` a daemon
//! would have — the UI still never touches PipeWire itself, it's just an
//! in-process call instead of a socket round-trip.

use mixer_core::engine::EngineHandle;
use mixer_core::Config;

/// Load config, stand up the PipeWire backend, and spawn the engine. Returns
/// an error string (instead of exiting the process, as the old standalone
/// daemon's `main()` did) so the GUI can show it in the header and still let
/// the window come up.
#[cfg(target_os = "linux")]
pub fn start() -> Result<EngineHandle, String> {
    let config = Config::load_or_create();
    log::info!("config: {}", Config::path().display());

    let (backend, events) = mixer_pw::PwBackend::new()?;
    Ok(mixer_core::Engine::spawn(Box::new(backend), events, config))
}

#[cfg(not(target_os = "linux"))]
pub fn start() -> Result<EngineHandle, String> {
    Err("FerroMix drives PipeWire directly and only runs on Linux.".into())
}
