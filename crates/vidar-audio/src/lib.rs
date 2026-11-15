//! Vidar's real-time engine: WASAPI capture and render, the processing chain, the voice
//! test and microphone selection.

pub mod devices;
pub mod drift;
pub mod explain;
pub mod processor;
pub mod shared;

#[cfg(windows)]
pub mod engine;
#[cfg(windows)]
pub mod voice_test;
#[cfg(windows)]
pub mod wasapi;

#[cfg(windows)]
pub use engine::{Engine, EngineConfig, Health, StreamInfo};
pub use explain::Explained;
pub use shared::{Controls, MeterSnapshot, Meters};
