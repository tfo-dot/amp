#![allow(unused_imports, dead_code)]
pub mod local;
pub mod seanime;

pub use local::LocalProvider;
pub use seanime::SeanimeProvider;

use crate::player::SessionError;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct MediaMetadata {
    pub title: String,
    pub artist: String,
    pub duration_ms: i64,
}

#[async_trait]
pub trait MediaProvider: Send + Sync {
    fn id(&self) -> &'static str;
    async fn resolve_stream(&self, uri: &str) -> Result<String, SessionError>;
    async fn fetch_metadata(&self, uri: &str) -> Result<MediaMetadata, SessionError>;
}
