//! FrameCover core: everything except the GUI.
//!
//! Services are plain structs implementing small traits so the app can swap
//! them for fakes in tests (mirroring the Python `Protocol`s).

pub mod atomic_write;
pub mod color;
pub mod converter;
pub mod error;
pub mod ffmpeg;
pub mod frame_stream;
pub mod frames;
pub mod imaging;
pub mod loader;
pub mod models;
pub mod mp4_tags;
pub mod platform;
pub mod probe;
mod sample_entries;
pub mod save_copy;
pub mod thumbnail_writer;
pub mod update_checker;

pub use error::{Error, Result};
pub use models::*;

/// Application version (from the workspace `Cargo.toml`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
