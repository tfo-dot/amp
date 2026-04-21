use async_trait::async_trait;
use std::error::Error;
use std::sync::Arc;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum MediaItemType {
    Folder,
    Playable,
}

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
    async fn get_root(&self) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>>;
    
    /// Returns children for a given container item
    async fn get_children(&self, parent_id: &str) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>>;
    
    /// Returns "Next Up" or "Continue Watching" items
    async fn get_next_up(&self) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>>;

    fn get_stream_url(&self, item_id: &str) -> String;
    async fn get_item_image_buffer(&self, item_id: &str) -> Result<RawImage, Box<dyn Error + Send + Sync>>;
    fn get_persistable_config(&self) -> HashMap<String, String>;

    async fn get_resume_position(&self, item_id: &str) -> Result<Option<i64>, Box<dyn Error + Send + Sync>>;
    async fn report_playback_start(&self, item_id: &str) -> Result<(), Box<dyn Error + Send + Sync>>;
    async fn report_playback_progress(&self, item_id: &str, position_secs: i64, is_paused: bool) -> Result<(), Box<dyn Error + Send + Sync>>;
    async fn report_playback_stopped(&self, item_id: &str, position_secs: i64) -> Result<(), Box<dyn Error + Send + Sync>>;
    async fn mark_as_played(&self, item_id: &str, played: bool) -> Result<(), Box<dyn Error + Send + Sync>>;
}

pub type DynProvider = Arc<dyn MediaProvider>;

#[derive(Clone, Debug)]
pub struct ConfigField {
    pub key: String,
    pub label: String,
    pub is_password: bool,
    pub default_value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaybackInfo {
    pub title: String,
    pub artist: String,
    pub is_paused: bool,
    pub position_secs: i64,
    pub duration_secs: i64,
}

pub trait PlaybackExtension: Send + Sync {
    fn on_playback_update(&self, info: PlaybackInfo);
    fn on_playback_stop(&self);
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PluginCapability {
    MediaProvider,
    PlaybackExtension,
}

#[async_trait]
pub trait AmpPlugin: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn capabilities(&self) -> Vec<PluginCapability>;

    // MediaProvider specific
    fn config_fields(&self) -> Vec<ConfigField> {
        vec![]
    }
    async fn create_provider(
        &self,
        _config: HashMap<String, String>,
    ) -> Result<DynProvider, Box<dyn Error + Send + Sync>> {
        Err("MediaProvider capability not implemented".into())
    }

    // PlaybackExtension specific
    fn create_extension(&self) -> Result<Arc<dyn PlaybackExtension>, Box<dyn Error + Send + Sync>> {
        Err("PlaybackExtension capability not implemented".into())
    }
}
