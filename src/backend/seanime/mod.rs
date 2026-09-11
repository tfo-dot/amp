#![allow(unused_imports, dead_code)]
pub mod client;
pub mod ws;

pub use client::SeanimeClient;
pub use ws::{SeanimeWsEvent, SeanimeWsListener, SeanimeWsSender};

use super::{MediaMetadata, MediaProvider as AppMediaProvider};
use crate::api::{AmpError, MediaItem, MediaItemType, MediaProvider, RawImage};
use crate::player::SessionError;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

pub struct SeanimeProvider {
    client: Arc<SeanimeClient>,
    raw_file_paths: Arc<RwLock<HashMap<String, String>>>,
    stream_cache: Arc<RwLock<HashMap<String, String>>>,
    image_cache: Arc<RwLock<HashMap<String, String>>>,
    durations: Arc<RwLock<HashMap<String, f64>>>,
    total_episodes: Arc<RwLock<HashMap<i32, i32>>>,
    ws_sender: Arc<RwLock<Option<SeanimeWsSender>>>,
}

impl SeanimeProvider {
    pub fn new(client: Arc<SeanimeClient>) -> Self {
        Self {
            client,
            raw_file_paths: Arc::new(RwLock::new(HashMap::new())),
            stream_cache: Arc::new(RwLock::new(HashMap::new())),
            image_cache: Arc::new(RwLock::new(HashMap::new())),
            durations: Arc::new(RwLock::new(HashMap::new())),
            total_episodes: Arc::new(RwLock::new(HashMap::new())),
            ws_sender: Arc::new(RwLock::new(None)),
        }
    }

    pub fn default_local() -> Self {
        Self::new(Arc::new(SeanimeClient::default_local()))
    }

    pub fn client(&self) -> &Arc<SeanimeClient> {
        &self.client
    }

    pub fn set_ws_sender(&self, sender: SeanimeWsSender) {
        if let Ok(mut guard) = self.ws_sender.write() {
            *guard = Some(sender);
        }
    }

    pub fn set_item_duration(&self, item_id: &str, duration_secs: f64) {
        if duration_secs > 0.0
            && let Ok(mut guard) = self.durations.write()
        {
            guard.insert(item_id.to_string(), duration_secs);
        }
    }

    fn cache_raw_path(&self, item_id: String, path: String) {
        if let Ok(mut guard) = self.raw_file_paths.write() {
            guard.insert(item_id, path);
        }
    }

    fn cache_stream(&self, item_id: String, path: String) {
        if let Ok(mut guard) = self.stream_cache.write() {
            guard.insert(item_id, path);
        }
    }

    fn cache_image(&self, item_id: String, url: String) {
        if let Ok(mut guard) = self.image_cache.write() {
            guard.insert(item_id, url);
        }
    }

    pub fn resolve_raw_path_to_playable(&self, raw_path: &str) -> String {
        if Path::new(raw_path).exists() {
            return raw_path.to_string();
        }

        let encoded_path = urlencoding::encode(raw_path);
        let url = format!(
            "{}/api/v1/mediastream/file?path={}",
            self.client.base_url(),
            encoded_path
        );
        eprintln!("[SeanimeProvider] Stream URL for remote path: {}", url);
        url
    }
}

#[async_trait]
impl AppMediaProvider for SeanimeProvider {
    fn id(&self) -> &'static str {
        "seanime"
    }

    async fn resolve_stream(&self, uri: &str) -> Result<String, SessionError> {
        if let Some(path) = uri.strip_prefix("seanime://file/") {
            let playable = self.resolve_raw_path_to_playable(path);
            return Ok(playable);
        }

        if let Some(rest) = uri.strip_prefix("seanime://ep/") {
            let parts: Vec<&str> = rest.split('/').collect();
            if parts.len() >= 2
                && let (Ok(media_id), Ok(ep_num)) =
                    (parts[0].parse::<i32>(), parts[1].parse::<i32>())
                && let Ok(entry) = self.client.get_anime_entry(media_id).await
                && let Some(ep) = entry.episodes.iter().find(|e| e.episode_number == ep_num)
                && let Some(file_path) = ep.file_path()
            {
                let playable = self.resolve_raw_path_to_playable(&file_path);
                self.cache_stream(uri.to_string(), playable.clone());
                return Ok(playable);
            }
        }

        if let Ok(guard) = self.stream_cache.read()
            && let Some(path) = guard.get(uri)
        {
            return Ok(path.clone());
        }

        Ok(uri.to_string())
    }

    async fn fetch_metadata(&self, uri: &str) -> Result<MediaMetadata, SessionError> {
        if let Some(rest) = uri.strip_prefix("seanime://ep/") {
            let parts: Vec<&str> = rest.split('/').collect();
            if parts.len() >= 2
                && let (Ok(media_id), Ok(ep_num)) =
                    (parts[0].parse::<i32>(), parts[1].parse::<i32>())
                && let Ok(entry) = self.client.get_anime_entry(media_id).await
            {
                let series_title = entry
                    .media
                    .as_ref()
                    .and_then(|m| m.title.as_ref().map(|t| t.display_title()))
                    .unwrap_or_else(|| "Anime".to_string());

                if let Some(ep) = entry.episodes.iter().find(|e| e.episode_number == ep_num) {
                    return Ok(MediaMetadata {
                        title: ep.title_string(),
                        artist: series_title,
                        duration_ms: 0,
                    });
                }
            }
        }

        Ok(MediaMetadata {
            title: uri.to_string(),
            artist: "Seanime".to_string(),
            duration_ms: 0,
        })
    }
}

#[async_trait]
impl MediaProvider for SeanimeProvider {
    async fn get_root(&self) -> Result<Vec<MediaItem>, AmpError> {
        let collection = self
            .client
            .get_library_collection()
            .await
            .map_err(|e| AmpError::Provider(e.to_string()))?;

        let mut items = Vec::new();

        // 1. Continue watching items as Playable
        for ep in &collection.continue_watching_list {
            let media_title = ep
                .base_anime
                .as_ref()
                .and_then(|b| b.title.as_ref().map(|t| t.display_title()));
            let media_id = ep.base_anime.as_ref().map(|b| b.id).unwrap_or(0);
            let item_id = format!("ep_{}_{}", media_id, ep.episode_number);

            if let Some(b) = &ep.base_anime {
                if let Some(eps) = b.episodes
                    && let Ok(mut guard) = self.total_episodes.write()
                {
                    guard.insert(media_id, eps);
                }
                if let Some(dur_mins) = b.duration {
                    self.set_item_duration(&item_id, (dur_mins * 60) as f64);
                }
            }

            if let Some(path) = ep.file_path() {
                self.cache_raw_path(item_id.clone(), path.clone());
                let playable = self.resolve_raw_path_to_playable(&path);
                self.cache_stream(item_id.clone(), playable);
            }
            if let Some(img) = ep.thumbnail_url() {
                self.cache_image(item_id.clone(), img);
            }

            items.push(MediaItem {
                id: item_id,
                name: ep.title_string(),
                item_type: MediaItemType::Playable,
                duration_secs: None,
                index: Some(ep.episode_number),
                resume_position_secs: None,
                series_name: media_title,
                season_index: None,
            });
        }

        // 2. Series from lists as Folders
        for list in &collection.lists {
            for entry in &list.entries {
                if let Some(media) = &entry.media {
                    let title = media
                        .title
                        .as_ref()
                        .map(|t| t.display_title())
                        .unwrap_or_else(|| format!("Anime {}", entry.media_id));

                    if let Some(eps) = media.episodes
                        && let Ok(mut guard) = self.total_episodes.write()
                    {
                        guard.insert(entry.media_id, eps);
                    }

                    let folder_id = format!("media_{}", entry.media_id);
                    if let Some(img) = media.cover_image.as_ref().and_then(|c| c.best_url()) {
                        self.cache_image(folder_id.clone(), img);
                    }

                    items.push(MediaItem {
                        id: folder_id,
                        name: title,
                        item_type: MediaItemType::Folder,
                        duration_secs: None,
                        index: None,
                        resume_position_secs: None,
                        series_name: None,
                        season_index: None,
                    });
                }
            }
        }

        Ok(items)
    }

    async fn get_children(&self, parent_id: &str) -> Result<Vec<MediaItem>, AmpError> {
        if let Some(id_str) = parent_id.strip_prefix("media_")
            && let Ok(media_id) = id_str.parse::<i32>()
        {
            let entry = self
                .client
                .get_anime_entry(media_id)
                .await
                .map_err(|e| AmpError::Provider(e.to_string()))?;

            let series_title = entry
                .media
                .as_ref()
                .and_then(|m| m.title.as_ref().map(|t| t.display_title()));

            if let Some(m) = &entry.media
                && let Some(eps) = m.episodes
                && let Ok(mut guard) = self.total_episodes.write()
            {
                guard.insert(media_id, eps);
            }

            let mut items = Vec::new();
            for ep in entry.episodes {
                let item_id = format!("ep_{}_{}", media_id, ep.episode_number);
                if let Some(path) = ep.file_path() {
                    self.cache_raw_path(item_id.clone(), path.clone());
                    let playable = self.resolve_raw_path_to_playable(&path);
                    self.cache_stream(item_id.clone(), playable);
                }
                if let Some(img) = ep.thumbnail_url() {
                    self.cache_image(item_id.clone(), img);
                }

                items.push(MediaItem {
                    id: item_id,
                    name: ep.title_string(),
                    item_type: MediaItemType::Playable,
                    duration_secs: None,
                    index: Some(ep.episode_number),
                    resume_position_secs: None,
                    series_name: series_title.clone(),
                    season_index: None,
                });
            }

            return Ok(items);
        }

        Ok(Vec::new())
    }

    async fn get_next_up(&self) -> Result<Vec<MediaItem>, AmpError> {
        let collection = self
            .client
            .get_library_collection()
            .await
            .map_err(|e| AmpError::Provider(e.to_string()))?;

        let mut items = Vec::new();
        for ep in collection.continue_watching_list {
            let media_title = ep
                .base_anime
                .as_ref()
                .and_then(|b| b.title.as_ref().map(|t| t.display_title()));
            let media_id = ep.base_anime.as_ref().map(|b| b.id).unwrap_or(0);
            let item_id = format!("ep_{}_{}", media_id, ep.episode_number);

            if let Some(b) = &ep.base_anime {
                if let Some(eps) = b.episodes
                    && let Ok(mut guard) = self.total_episodes.write()
                {
                    guard.insert(media_id, eps);
                }
                if let Some(dur_mins) = b.duration {
                    self.set_item_duration(&item_id, (dur_mins * 60) as f64);
                }
            }

            if let Some(path) = ep.file_path() {
                self.cache_raw_path(item_id.clone(), path.clone());
                let playable = self.resolve_raw_path_to_playable(&path);
                self.cache_stream(item_id.clone(), playable);
            }
            if let Some(img) = ep.thumbnail_url() {
                self.cache_image(item_id.clone(), img);
            }

            items.push(MediaItem {
                id: item_id,
                name: ep.title_string(),
                item_type: MediaItemType::Playable,
                duration_secs: None,
                index: Some(ep.episode_number),
                resume_position_secs: None,
                series_name: media_title,
                season_index: None,
            });
        }

        Ok(items)
    }

    async fn search(&self, query: &str) -> Result<Vec<MediaItem>, AmpError> {
        let collection = self
            .client
            .get_library_collection()
            .await
            .map_err(|e| AmpError::Provider(e.to_string()))?;

        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for list in collection.lists {
            for entry in list.entries {
                if let Some(media) = entry.media {
                    let display_title = media
                        .title
                        .as_ref()
                        .map(|t| t.display_title())
                        .unwrap_or_default();

                    if display_title.to_lowercase().contains(&query_lower) {
                        let folder_id = format!("media_{}", entry.media_id);
                        if let Some(img) = media.cover_image.as_ref().and_then(|c| c.best_url()) {
                            self.cache_image(folder_id.clone(), img);
                        }

                        results.push(MediaItem {
                            id: folder_id,
                            name: display_title,
                            item_type: MediaItemType::Folder,
                            duration_secs: None,
                            index: None,
                            resume_position_secs: None,
                            series_name: None,
                            season_index: None,
                        });
                    }
                }
            }
        }

        Ok(results)
    }

    fn get_stream_url(&self, item_id: &str) -> String {
        if let Ok(guard) = self.stream_cache.read()
            && let Some(path) = guard.get(item_id)
        {
            return path.clone();
        }

        if let Ok(guard) = self.raw_file_paths.read()
            && let Some(raw) = guard.get(item_id)
        {
            let playable = self.resolve_raw_path_to_playable(raw);
            self.cache_stream(item_id.to_string(), playable.clone());
            return playable;
        }

        if let Some(rest) = item_id.strip_prefix("ep_") {
            let parts: Vec<&str> = rest.split('_').collect();
            if parts.len() >= 2
                && let (Ok(media_id), Ok(ep_num)) =
                    (parts[0].parse::<i32>(), parts[1].parse::<i32>())
            {
                let client = self.client.clone();
                if let Ok(entry) = futures::executor::block_on(client.get_anime_entry(media_id))
                    && let Some(ep) = entry.episodes.iter().find(|e| e.episode_number == ep_num)
                    && let Some(fp) = ep.file_path()
                {
                    let playable = self.resolve_raw_path_to_playable(&fp);
                    self.cache_stream(item_id.to_string(), playable.clone());
                    return playable;
                }
            }
        }

        item_id.to_string()
    }

    async fn get_item_image_buffer(&self, item_id: &str) -> Result<RawImage, AmpError> {
        let cached_url = self
            .image_cache
            .read()
            .ok()
            .and_then(|guard| guard.get(item_id).cloned());

        if let Some(url) = cached_url {
            let client = reqwest::Client::new();
            let bytes = client
                .get(&url)
                .send()
                .await
                .map_err(|e| AmpError::Provider(e.to_string()))?
                .bytes()
                .await
                .map_err(|e| AmpError::Provider(e.to_string()))?;

            let img = image::load_from_memory(&bytes)
                .map_err(|e| AmpError::Provider(e.to_string()))?
                .to_rgba8();

            return Ok(RawImage {
                width: img.width(),
                height: img.height(),
                rgba8: img.into_raw(),
            });
        }

        Err(AmpError::Provider("No thumbnail available".into()))
    }

    fn get_persistable_config(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("base_url".to_string(), self.client.base_url().to_string());
        map
    }

    async fn get_resume_position(&self, item_id: &str) -> Result<Option<i64>, AmpError> {
        if let Some(rest) = item_id.strip_prefix("ep_") {
            let parts: Vec<&str> = rest.split('_').collect();
            if parts.len() >= 2
                && let (Ok(media_id), Ok(ep_num)) =
                    (parts[0].parse::<i32>(), parts[1].parse::<i32>())
                && let Ok(Some(item)) = self.client.get_watch_history_item(media_id).await
                && item.episode_number == ep_num
                && item.current_time > 3.0
                && item.current_time < (item.duration * 0.9)
            {
                eprintln!(
                    "[SeanimeProvider] Found resume position for Ep {}: {}s",
                    ep_num, item.current_time as i64
                );
                return Ok(Some(item.current_time as i64));
            }
        }
        Ok(None)
    }

    async fn report_playback_start(&self, item_id: &str) -> Result<(), AmpError> {
        if let Some(rest) = item_id.strip_prefix("ep_") {
            let parts: Vec<&str> = rest.split('_').collect();
            if parts.len() >= 2
                && let (Ok(media_id), Ok(ep_num)) =
                    (parts[0].parse::<i32>(), parts[1].parse::<i32>())
            {
                let duration = self
                    .durations
                    .read()
                    .ok()
                    .and_then(|g| g.get(item_id).cloned())
                    .unwrap_or(1440.0);

                if let Ok(guard) = self.ws_sender.read()
                    && let Some(sender) = guard.as_ref()
                {
                    sender.send_video_loaded(item_id, media_id, ep_num, duration);
                }
            }
        }
        Ok(())
    }

    async fn report_playback_progress(
        &self,
        item_id: &str,
        position_secs: i64,
        is_paused: bool,
    ) -> Result<(), AmpError> {
        if let Some(rest) = item_id.strip_prefix("ep_") {
            let parts: Vec<&str> = rest.split('_').collect();
            if parts.len() >= 2
                && let (Ok(media_id), Ok(ep_num)) =
                    (parts[0].parse::<i32>(), parts[1].parse::<i32>())
            {
                let duration = self
                    .durations
                    .read()
                    .ok()
                    .and_then(|g| g.get(item_id).cloned())
                    .unwrap_or(1440.0);

                // 1. Sync over WebSocket to Seanime
                if let Ok(guard) = self.ws_sender.read()
                    && let Some(sender) = guard.as_ref()
                {
                    sender.send_video_status(item_id, position_secs as f64, duration, is_paused);
                }

                // 2. Sync watch history over HTTP to Seanime
                let _ = self
                    .client
                    .update_watch_history(media_id, ep_num, position_secs as f64, duration)
                    .await;

                // 3. Only update AniList episode progress if watched >= 85% of actual media duration
                let total_eps = self
                    .total_episodes
                    .read()
                    .ok()
                    .and_then(|g| g.get(&media_id).cloned())
                    .unwrap_or(24);

                if duration > 0.0 && (position_secs as f64) >= (duration * 0.85) {
                    eprintln!(
                        "[SeanimeProvider] Watched >= 85% ({:.1}s / {:.1}s) - marking Ep {} completed",
                        position_secs as f64, duration, ep_num
                    );
                    let _ = self
                        .client
                        .update_progress(media_id, ep_num, total_eps, None)
                        .await;
                }
            }
        }
        Ok(())
    }

    async fn report_playback_stopped(
        &self,
        item_id: &str,
        position_secs: i64,
    ) -> Result<(), AmpError> {
        if let Some(rest) = item_id.strip_prefix("ep_") {
            let parts: Vec<&str> = rest.split('_').collect();
            if parts.len() >= 2
                && let (Ok(media_id), Ok(ep_num)) =
                    (parts[0].parse::<i32>(), parts[1].parse::<i32>())
            {
                let duration = self
                    .durations
                    .read()
                    .ok()
                    .and_then(|g| g.get(item_id).cloned())
                    .unwrap_or(1440.0);

                // 1. Send WebSocket stopped & terminated event
                if let Ok(guard) = self.ws_sender.read()
                    && let Some(sender) = guard.as_ref()
                {
                    sender.send_video_status(item_id, position_secs as f64, duration, true);
                    sender.send_video_terminated(item_id);
                }

                // 2. Update watch history over HTTP
                let _ = self
                    .client
                    .update_watch_history(media_id, ep_num, position_secs as f64, duration)
                    .await;

                // 3. Only mark completed on AniList if watched >= 85% of actual media duration
                let total_eps = self
                    .total_episodes
                    .read()
                    .ok()
                    .and_then(|g| g.get(&media_id).cloned())
                    .unwrap_or(24);

                if duration > 0.0 && (position_secs as f64) >= (duration * 0.85) {
                    eprintln!(
                        "[SeanimeProvider] Stopped at >= 85% ({:.1}s / {:.1}s) - marking Ep {} completed",
                        position_secs as f64, duration, ep_num
                    );
                    let _ = self
                        .client
                        .update_progress(media_id, ep_num, total_eps, None)
                        .await;
                }
            }
        }
        Ok(())
    }

    async fn mark_as_played(&self, item_id: &str, _played: bool) -> Result<(), AmpError> {
        if let Some(rest) = item_id.strip_prefix("ep_") {
            let parts: Vec<&str> = rest.split('_').collect();
            if parts.len() >= 2
                && let (Ok(media_id), Ok(ep_num)) =
                    (parts[0].parse::<i32>(), parts[1].parse::<i32>())
            {
                let total_eps = self
                    .total_episodes
                    .read()
                    .ok()
                    .and_then(|g| g.get(&media_id).cloned())
                    .unwrap_or(24);
                let _ = self
                    .client
                    .update_progress(media_id, ep_num, total_eps, None)
                    .await;
            }
        }
        Ok(())
    }
}