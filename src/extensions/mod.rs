#![allow(unused_imports, dead_code)]
pub mod bridge;
pub mod watcher;

pub use bridge::{ExtensionBridge, PartsExtensionBridge};
pub use crate::api::PlaybackController;
pub use watcher::ExtensionWatcher;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExtensionError {
    #[error("VM error: {0}")]
    VmError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Script not found: {0}")]
    NotFound(String),
    #[error("Invalid command: {0}")]
    InvalidCommand(i64),
}
