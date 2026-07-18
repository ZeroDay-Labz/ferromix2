//! ferromix-daemon: the always-on half of FerroMix.
//! Owns the PipeWire graph via mixer-pw, runs the engine, and serves state
//! and commands to any number of GUIs over a Unix socket.

#[cfg(target_os = "linux")]
mod ipc;

#[cfg(target_os = "linux")]
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = mixer_core::Config::load_or_create();
    log::info!("config: {}", mixer_core::Config::path().display());

    let (backend, events) = match mixer_pw::PwBackend::new() {
        Ok(pair) => pair,
        Err(e) => {
            log::error!("could not start PipeWire backend: {e}");
            std::process::exit(1);
        }
    };

    let handle = mixer_core::Engine::spawn(Box::new(backend), events, config);
    ipc::serve(handle); // never returns
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("ferromix-daemon is Linux-only (it drives PipeWire).");
    eprintln!("On Windows, develop the GUI against the mock backend instead:");
    eprintln!("    cargo run -p mixer-gui");
}
