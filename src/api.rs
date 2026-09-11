use thiserror::Error;

#[derive(Error, Debug)]
pub enum AmpError {
    #[error("Plugin error: {0}")]
    Plugin(String),

    #[error("Reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl From<Box<dyn std::error::Error + Send + Sync>> for AmpError {
    fn from(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        AmpError::Unknown(e.to_string())
    }
}

impl From<String> for AmpError {
    fn from(s: String) -> Self {
        AmpError::Unknown(s)
    }
}

impl From<&str> for AmpError {
    fn from(s: &str) -> Self {
        AmpError::Unknown(s.to_string())
    }
}

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum MediaItemType {
    Folder,
    Playable,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaItem {
    pub id: String,
    pub name: String,
    pub item_type: MediaItemType,
    pub duration_secs: Option<i64>,
    pub index: Option<i32>,
    pub resume_position_secs: Option<i64>,
    pub series_name: Option<String>,
    pub season_index: Option<i32>,
}

pub struct RawImage {
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
}

#[async_trait]
pub trait MediaProvider: Send + Sync {
    /// Returns the root items (e.g., "Library Sections" or "Home")
    async fn get_root(&self) -> Result<Vec<MediaItem>, AmpError>;

    /// Returns children for a given container item
    async fn get_children(&self, parent_id: &str) -> Result<Vec<MediaItem>, AmpError>;

    /// Returns "Next Up" or "Continue Watching" items
    async fn get_next_up(&self) -> Result<Vec<MediaItem>, AmpError>;

    /// Search for items
    async fn search(&self, query: &str) -> Result<Vec<MediaItem>, AmpError>;

    fn get_stream_url(&self, item_id: &str) -> String;
    async fn get_item_image_buffer(&self, item_id: &str) -> Result<RawImage, AmpError>;

    //It's used in /bridge and /extensions modules
    #[allow(dead_code)]
    fn get_persistable_config(&self) -> HashMap<String, String>;

    async fn get_resume_position(&self, item_id: &str) -> Result<Option<i64>, AmpError>;
    async fn report_playback_start(&self, item_id: &str) -> Result<(), AmpError>;
    async fn report_playback_progress(
        &self,
        item_id: &str,
        position_secs: i64,
        is_paused: bool,
    ) -> Result<(), AmpError>;
    async fn report_playback_stopped(
        &self,
        item_id: &str,
        position_secs: i64,
    ) -> Result<(), AmpError>;
    async fn mark_as_played(&self, item_id: &str, played: bool) -> Result<(), AmpError>;
}

pub type DynProvider = Arc<dyn MediaProvider>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaybackInfo {
    pub title: String,
    pub artist: String,
    pub series_name: Option<String>,
    pub season_index: Option<i32>,
    pub episode_index: Option<i32>,
    pub is_paused: bool,
    pub position_secs: i64,
    pub duration_secs: i64,
}

pub trait PlaybackExtension: Send + Sync {
    fn on_playback_update(&self, info: PlaybackInfo);
    fn on_playback_stop(&self);
}

pub trait PlaybackController: Send + Sync {
    fn play(&self);
    fn pause(&self);
    fn toggle_pause(&self);
    fn next(&self);
    fn previous(&self);
    fn stop(&self);
    fn seek(&self, position_secs: i64);
}
