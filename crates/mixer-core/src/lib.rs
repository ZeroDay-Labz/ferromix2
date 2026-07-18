//! mixer-core — the platform-agnostic brain of FerroMix.
//!
//! Patchbay model: SOURCES (apps + hardware inputs) are assigned to BUSES
//! (A = hardware outputs with device binding, B = virtual mics). Everything
//! compiles on any OS; `mock::MockBackend` fakes audio for GUI dev on Windows,
//! `mixer-pw::PwBackend` drives real PipeWire on Linux.

pub mod backend;
pub mod config;
pub mod engine;
pub mod ipc;
pub mod mock;
pub mod model;

pub use backend::{AudioBackend, BackendEvent};
pub use config::Config;
pub use engine::{Command, Engine, EngineHandle};
pub use model::{Bus, BusKind, Device, InputOption, LevelKey, MixerState, SourceInfo, SourceKind, Strip};
