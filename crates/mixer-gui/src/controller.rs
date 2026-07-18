//! How the GUI talks to an engine: either in-process (mock, any OS) or over
//! the daemon's Unix socket (Linux). The rest of the GUI only sees this trait.

use mixer_core::engine::{Command, EngineHandle};
use mixer_core::mock::MockBackend;
use mixer_core::model::MixerState;
use mixer_core::{Config, Engine};

pub trait Controller {
    fn snapshot(&mut self) -> MixerState;
    fn send(&mut self, cmd: Command);
    /// Short badge for the top bar: "MOCK", "LIVE", "OFFLINE".
    fn mode(&self) -> &'static str;
}

pub fn make(force_mock: bool) -> Box<dyn Controller> {
    if force_mock || !cfg!(target_os = "linux") {
        return Box::new(Local::new_mock());
    }
    #[cfg(unix)]
    {
        return Box::new(ipc::Ipc::new());
    }
    #[allow(unreachable_code)]
    Box::new(Local::new_mock())
}

/// In-process engine with the mock backend.
struct Local {
    handle: EngineHandle,
}

impl Local {
    fn new_mock() -> Self {
        let config = Config::load_or_create();
        let (backend, events) = MockBackend::new();
        Local { handle: Engine::spawn(Box::new(backend), events, config) }
    }
}

impl Controller for Local {
    fn snapshot(&mut self) -> MixerState {
        self.handle.snapshot()
    }
    fn send(&mut self, cmd: Command) {
        self.handle.send(cmd);
    }
    fn mode(&self) -> &'static str {
        "MOCK"
    }
}

#[cfg(unix)]
mod ipc {
    use super::Controller;
    use mixer_core::engine::Command;
    use mixer_core::ipc::{read_frame, socket_path, write_frame, Request, Response};
    use mixer_core::model::MixerState;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    /// Polls the daemon each frame; auto-reconnects if it goes away.
    pub struct Ipc {
        stream: Option<UnixStream>,
        last: MixerState,
        last_attempt: Instant,
    }

    impl Ipc {
        pub fn new() -> Self {
            let mut me = Ipc {
                stream: None,
                last: MixerState::default(),
                last_attempt: Instant::now() - Duration::from_secs(10),
            };
            me.try_connect();
            me
        }

        fn try_connect(&mut self) {
            if self.stream.is_some() || self.last_attempt.elapsed() < Duration::from_secs(1) {
                return;
            }
            self.last_attempt = Instant::now();
            match UnixStream::connect(socket_path()) {
                Ok(s) => {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                    self.stream = Some(s);
                }
                Err(e) => log::debug!("daemon not reachable: {e}"),
            }
        }

        fn roundtrip(&mut self, req: &Request) -> Option<Response> {
            let s = self.stream.as_mut()?;
            if write_frame(s, req).is_err() {
                self.stream = None;
                return None;
            }
            match read_frame::<_, Response>(s) {
                Ok(r) => Some(r),
                Err(_) => {
                    self.stream = None;
                    None
                }
            }
        }
    }

    impl Controller for Ipc {
        fn snapshot(&mut self) -> MixerState {
            self.try_connect();
            if let Some(Response::State(state)) = self.roundtrip(&Request::GetState) {
                self.last = state;
            }
            self.last.clone()
        }

        fn send(&mut self, cmd: Command) {
            let _ = self.roundtrip(&Request::Cmd(cmd));
        }

        fn mode(&self) -> &'static str {
            if self.stream.is_some() {
                "LIVE"
            } else {
                "OFFLINE"
            }
        }
    }
}
