use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

pub mod error;
pub use error::AmpError;

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
    fn set_controller(&self, _controller: Arc<dyn PlaybackController>) {}
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

#[async_trait]
pub trait LibraryManager: Send + Sync {
    async fn search_and_add_series(&self, title: &str) -> Result<(), AmpError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PluginCapability {
    MediaProvider,
    PlaybackExtension,
    LibraryManager,
}

#[async_trait]
pub trait AmpPlugin: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn capabilities(&self) -> Vec<PluginCapability>;

    // Config fields for MediaProvider
    fn config_fields(&self) -> Vec<ConfigField> {
        vec![]
    }

    // Config fields for Extensions (AniList, Sonarr, etc)
    fn extension_config_fields(&self) -> Vec<ConfigField> {
        vec![]
    }

    async fn create_provider(
        &self,
        _config: HashMap<String, String>,
    ) -> Result<DynProvider, AmpError> {
        Err(AmpError::Plugin(
            "MediaProvider capability not implemented".into(),
        ))
    }

    async fn create_extension(
        &self,
        _config: HashMap<String, String>,
    ) -> Result<Arc<dyn PlaybackExtension>, AmpError> {
        Err(AmpError::Plugin(
            "PlaybackExtension capability not implemented".into(),
        ))
    }

    async fn create_library_manager(
        &self,
        _config: HashMap<String, String>,
    ) -> Result<Arc<dyn LibraryManager>, AmpError> {
        Err(AmpError::Plugin(
            "LibraryManager capability not implemented".into(),
        ))
    }
}
