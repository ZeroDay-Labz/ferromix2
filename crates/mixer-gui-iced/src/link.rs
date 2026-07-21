//! Connection to the FerroMix daemon over the same Unix socket + bincode
//! framing the egui GUI used. The Iced GUI is a pure client: it never touches
//! PipeWire, it only sends `Command`s and polls `MixerState`. That's what lets
//! us rebuild the whole face without risking the audio engine.

use mixer_core::engine::Command;
use mixer_core::ipc::{self, Request, Response};
use mixer_core::model::MixerState;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

/// A blocking client run on a worker thread; the UI talks to it via channels.
pub struct Link {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Link {
    pub fn connect() -> Result<Self, String> {
        let path = ipc::socket_path();
        let stream = UnixStream::connect(&path)
            .map_err(|e| format!("daemon not reachable at {}: {e}", path.display()))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|e| e.to_string())?;
        let reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        Ok(Self { stream, reader })
    }

    pub fn poll_state(&mut self) -> Result<MixerState, String> {
        ipc::write_frame(&mut self.stream, &Request::GetState)?;
        match ipc::read_frame::<_, Response>(&mut self.reader)? {
            Response::State(s) => Ok(s),
            Response::Err(e) => Err(e),
            other => Err(format!("unexpected: {other:?}")),
        }
    }

    pub fn send(&mut self, cmd: Command) -> Result<(), String> {
        ipc::write_frame(&mut self.stream, &Request::Cmd(cmd))?;
        // Drain the ack so it doesn't desync the next state read.
        let _ = ipc::read_frame::<_, Response>(&mut self.reader);
        Ok(())
    }
}

/// Messages from the UI thread to the link worker.
pub enum ToLink {
    Cmd(Command),
    Stop,
}

/// Messages from the link worker back to the UI.
#[derive(Debug, Clone)]
pub enum FromLink {
    Connected,
    State(Box<MixerState>),
    Disconnected(String),
}

/// Spawn the blocking link worker. Returns a sender for commands and a receiver
/// for state snapshots. Reconnects on drop of the daemon.
pub fn spawn() -> (Sender<ToLink>, Receiver<FromLink>) {
    let (to_tx, to_rx) = std::sync::mpsc::channel::<ToLink>();
    let (from_tx, from_rx) = std::sync::mpsc::channel::<FromLink>();

    std::thread::spawn(move || loop {
        let mut link = match Link::connect() {
            Ok(l) => {
                let _ = from_tx.send(FromLink::Connected);
                l
            }
            Err(e) => {
                let _ = from_tx.send(FromLink::Disconnected(e));
                std::thread::sleep(Duration::from_millis(600));
                match to_rx.try_recv() {
                    Ok(ToLink::Stop) => return,
                    _ => continue,
                }
            }
        };

        // Connected: pump commands + poll state at ~30 Hz.
        loop {
            while let Ok(msg) = to_rx.try_recv() {
                match msg {
                    ToLink::Cmd(c) => {
                        if link.send(c).is_err() {
                            let _ = from_tx.send(FromLink::Disconnected("send failed".into()));
                        }
                    }
                    ToLink::Stop => return,
                }
            }
            match link.poll_state() {
                Ok(s) => {
                    if from_tx.send(FromLink::State(Box::new(s))).is_err() {
                        return; // UI gone
                    }
                }
                Err(e) => {
                    let _ = from_tx.send(FromLink::Disconnected(e));
                    break; // reconnect
                }
            }
            // Matches main.rs's subscription poll rate (~60Hz) — snappier
            // meters than the old 33ms/~30Hz.
            std::thread::sleep(Duration::from_millis(16));
        }
    });

    (to_tx, from_rx)
}
