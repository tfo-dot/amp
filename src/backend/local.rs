#![allow(dead_code)]
use super::{MediaMetadata, MediaProvider as AppMediaProvider};
use crate::player::SessionError;
use amp_api::{AmpError, MediaItem, MediaItemType, MediaProvider, RawImage};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct LocalProvider {
    media_dir: PathBuf,
}

impl LocalProvider {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let videos_dir = PathBuf::from(home).join("Videos");
        Self {
            media_dir: videos_dir,
        }
    }

    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            media_dir: dir.into(),
        }
    }
}

impl Default for LocalProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AppMediaProvider for LocalProvider {
    fn id(&self) -> &'static str {
        "local"
    }

    async fn resolve_stream(&self, uri: &str) -> Result<String, SessionError> {
        if let Some(path) = uri.strip_prefix("file://") {
            return Ok(path.to_string());
        }
        Ok(uri.to_string())
    }

    async fn fetch_metadata(&self, uri: &str) -> Result<MediaMetadata, SessionError> {
        let path = Path::new(uri);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Local Media")
            .to_string();

        Ok(MediaMetadata {
            title: name,
            artist: "Local File".to_string(),
            duration_ms: 0,
        })
    }
}

#[async_trait]
impl MediaProvider for LocalProvider {
    async fn get_root(&self) -> Result<Vec<MediaItem>, AmpError> {
        let mut items = Vec::new();
        if self.media_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&self.media_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();

                    if path.is_dir() {
                        items.push(MediaItem {
                            id: path.to_string_lossy().to_string(),
                            name,
                            item_type: MediaItemType::Folder,
                            duration_secs: None,
                            index: None,
                            resume_position_secs: None,
                            series_name: None,
                            season_index: None,
                        });
                    } else if is_video_file(&path) {
                        items.push(MediaItem {
                            id: path.to_string_lossy().to_string(),
                            name,
                            item_type: MediaItemType::Playable,
                            duration_secs: None,
                            index: None,
                            resume_position_secs: None,
                            series_name: Some("Local Videos".to_string()),
                            season_index: None,
                        });
                    }
                }
            }
        }
        Ok(items)
    }

    async fn get_children(&self, parent_id: &str) -> Result<Vec<MediaItem>, AmpError> {
        let dir = PathBuf::from(parent_id);
        let mut items = Vec::new();
        if dir.exists() && dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();

                    if path.is_dir() {
                        items.push(MediaItem {
                            id: path.to_string_lossy().to_string(),
                            name,
                            item_type: MediaItemType::Folder,
                            duration_secs: None,
                            index: None,
                            resume_position_secs: None,
                            series_name: None,
                            season_index: None,
                        });
                    } else if is_video_file(&path) {
                        items.push(MediaItem {
                            id: path.to_string_lossy().to_string(),
                            name,
                            item_type: MediaItemType::Playable,
                            duration_secs: None,
                            index: None,
                            resume_position_secs: None,
                            series_name: Some(dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string()),
                            season_index: None,
                        });
                    }
                }
            }
        }
        Ok(items)
    }

    async fn get_next_up(&self) -> Result<Vec<MediaItem>, AmpError> {
        Ok(Vec::new())
    }

    async fn search(&self, query: &str) -> Result<Vec<MediaItem>, AmpError> {
        let q_lower = query.to_lowercase();
        let mut items = Vec::new();
        if self.media_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&self.media_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();

                    if name.to_lowercase().contains(&q_lower) && is_video_file(&path) {
                        items.push(MediaItem {
                            id: path.to_string_lossy().to_string(),
                            name,
                            item_type: MediaItemType::Playable,
                            duration_secs: None,
                            index: None,
                            resume_position_secs: None,
                            series_name: Some("Local Videos".to_string()),
                            season_index: None,
                        });
                    }
                }
            }
        }
        Ok(items)
    }

    fn get_stream_url(&self, item_id: &str) -> String {
        item_id.to_string()
    }

    async fn get_item_image_buffer(&self, _item_id: &str) -> Result<RawImage, AmpError> {
        Err(AmpError::Provider("No image".into()))
    }

    fn get_persistable_config(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("media_dir".to_string(), self.media_dir.to_string_lossy().to_string());
        map
    }

    async fn get_resume_position(&self, _item_id: &str) -> Result<Option<i64>, AmpError> {
        Ok(None)
    }

    async fn report_playback_start(&self, _item_id: &str) -> Result<(), AmpError> {
        Ok(())
    }

    async fn report_playback_progress(
        &self,
        _item_id: &str,
        _position_secs: i64,
        _is_paused: bool,
    ) -> Result<(), AmpError> {
        Ok(())
    }

    async fn report_playback_stopped(
        &self,
        _item_id: &str,
        _position_secs: i64,
    ) -> Result<(), AmpError> {
        Ok(())
    }

    async fn mark_as_played(&self, _item_id: &str, _played: bool) -> Result<(), AmpError> {
        Ok(())
    }
}

fn is_video_file(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        matches!(
            ext.to_lowercase().as_str(),
            "mkv" | "mp4" | "avi" | "webm" | "mov" | "flv" | "ts" | "m4v"
        )
    } else {
        false
    }
}
