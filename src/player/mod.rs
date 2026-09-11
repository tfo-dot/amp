#![allow(unused_imports, dead_code)]
pub mod fbo;
pub mod mpv;
pub mod pipeline;

pub use fbo::GLResources;
pub use mpv::{MpvHandle, MpvRenderCtx, open_player};
pub use pipeline::configure_hardware_acceleration;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SessionError {
    #[error("Failed to resolve stream")]
    ResolveError,
    #[error("Network error")]
    NetworkError,
}

pub trait MediaSession: Send + Sync {
    fn play(&mut self);
    fn pause(&mut self);
    fn seek(&mut self, time_ms: i64);
    fn teardown(&mut self);
}
