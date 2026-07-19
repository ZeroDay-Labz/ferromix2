//! Daemon <-> GUI protocol: length-prefixed bincode frames over a Unix socket.
//! (Same framing pattern as a tick-locked TCP handshake — u32 LE length, then
//! a bincode payload — so it stays boring and debuggable with `socat`.)

use crate::engine::Command;
use crate::model::MixerState;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{Read, Write};

pub const MAX_FRAME: u32 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Ask for a full state snapshot (the GUI polls this ~30 Hz).
    GetState,
    /// Forward a command to the engine.
    Cmd(Command),
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    State(MixerState),
    Ok,
    Err(String),
    Pong,
}

pub fn socket_path() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return std::path::PathBuf::from(dir).join("ferromix2.sock");
    }
    std::env::temp_dir().join(format!("ferromix2-{}.sock", uid()))
}

fn uid() -> u32 {
    // Portable-enough: on the platforms the daemon runs on, this env var
    // exists under systemd; fall back to 0 for the path suffix otherwise.
    std::env::var("UID").ok().and_then(|s| s.parse().ok()).unwrap_or(0)
}

pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), String> {
    let bytes = bincode::serialize(msg).map_err(|e| e.to_string())?;
    let len = bytes.len() as u32;
    if len > MAX_FRAME {
        return Err(format!("frame too large: {len}"));
    }
    w.write_all(&len.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&bytes).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())
}

pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<T, String> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).map_err(|e| e.to_string())?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME {
        return Err(format!("frame too large: {len}"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).map_err(|e| e.to_string())?;
    bincode::deserialize(&buf).map_err(|e| e.to_string())
}
