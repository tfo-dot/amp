#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SeanimeError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Server returned error: {0}")]
    Api(String),
    #[error("Item not found: {0}")]
    NotFound(String),
}

// Wrapper for all Seanime JSON responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeanimeResponse<T> {
    pub data: Option<T>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeStatus {
    pub os: Option<String>,
    #[serde(rename = "clientDevice")]
    pub client_device: Option<String>,
    pub user: Option<SeanimeUser>,
    pub settings: Option<SeanimeSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeUser {
    pub viewer: Option<SeanimeViewer>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeViewer {
    pub name: Option<String>,
    pub avatar: Option<SeanimeAvatar>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeAvatar {
    pub large: Option<String>,
    pub medium: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeSettings {
    pub library: Option<SeanimeLibrarySettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeLibrarySettings {
    #[serde(rename = "libraryPath")]
    pub library_path: Option<String>,
    #[serde(rename = "autoUpdateProgress")]
    pub auto_update_progress: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeTitle {
    pub english: Option<String>,
    pub romaji: Option<String>,
    pub native: Option<String>,
    #[serde(rename = "userPreferred")]
    pub user_preferred: Option<String>,
}

impl SeanimeTitle {
    pub fn display_title(&self) -> String {
        self.user_preferred
            .clone()
            .or_else(|| self.english.clone())
            .or_else(|| self.romaji.clone())
            .or_else(|| self.native.clone())
            .unwrap_or_else(|| "Unknown Anime".to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeCoverImage {
    pub color: Option<String>,
    #[serde(rename = "extraLarge")]
    pub extra_large: Option<String>,
    pub large: Option<String>,
    pub medium: Option<String>,
}

impl SeanimeCoverImage {
    pub fn best_url(&self) -> Option<String> {
        self.extra_large
            .clone()
            .or_else(|| self.large.clone())
            .or_else(|| self.medium.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeMedia {
    pub id: i32,
    #[serde(rename = "idMal")]
    pub id_mal: Option<i32>,
    pub title: Option<SeanimeTitle>,
    #[serde(rename = "coverImage")]
    pub cover_image: Option<SeanimeCoverImage>,
    #[serde(rename = "bannerImage")]
    pub banner_image: Option<String>,
    pub episodes: Option<i32>,
    pub status: Option<String>,
    pub format: Option<String>,
    pub duration: Option<i32>,
    pub description: Option<String>,
    pub genres: Option<Vec<String>>,
    #[serde(rename = "meanScore")]
    pub mean_score: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeLibraryData {
    #[serde(rename = "allFilesLocked")]
    pub all_files_locked: Option<bool>,
    #[serde(rename = "sharedPath")]
    pub shared_path: Option<String>,
    #[serde(rename = "unwatchedCount")]
    pub unwatched_count: Option<i32>,
    #[serde(rename = "mainFileCount")]
    pub main_file_count: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeListData {
    pub progress: Option<i32>,
    pub score: Option<f64>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeListEntry {
    #[serde(rename = "mediaId")]
    pub media_id: i32,
    pub media: Option<SeanimeMedia>,
    #[serde(rename = "libraryData")]
    pub library_data: Option<SeanimeLibraryData>,
    #[serde(rename = "listData")]
    pub list_data: Option<SeanimeListData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeList {
    #[serde(rename = "type")]
    pub list_type: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub entries: Vec<SeanimeListEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeCollection {
    #[serde(default)]
    pub lists: Vec<SeanimeList>,
    #[serde(rename = "continueWatchingList", default)]
    pub continue_watching_list: Vec<SeanimeEpisode>,
    #[serde(rename = "unmatchedLocalFiles", default)]
    pub unmatched_local_files: Vec<SeanimeLocalFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeParsedInfo {
    pub title: Option<String>,
    pub episode: Option<String>,
    pub season: Option<String>,
    #[serde(rename = "releaseGroup")]
    pub release_group: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeLocalFile {
    pub path: String,
    pub name: Option<String>,
    #[serde(rename = "parsedInfo")]
    pub parsed_info: Option<SeanimeParsedInfo>,
    pub locked: Option<bool>,
    pub ignored: Option<bool>,
    #[serde(rename = "mediaId")]
    pub media_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeEpisodeMetadata {
    pub image: Option<String>,
    pub title: Option<String>,
    pub overview: Option<String>,
    #[serde(rename = "airDate")]
    pub air_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeEpisode {
    #[serde(rename = "displayTitle")]
    pub display_title: Option<String>,
    #[serde(rename = "episodeTitle")]
    pub episode_title: Option<String>,
    #[serde(rename = "episodeNumber")]
    pub episode_number: i32,
    #[serde(rename = "progressNumber")]
    pub progress_number: Option<i32>,
    #[serde(rename = "isDownloaded")]
    pub is_downloaded: Option<bool>,
    #[serde(rename = "localFile")]
    pub local_file: Option<SeanimeLocalFile>,
    #[serde(rename = "episodeMetadata")]
    pub episode_metadata: Option<SeanimeEpisodeMetadata>,
    #[serde(rename = "baseAnime")]
    pub base_anime: Option<SeanimeMedia>,
}

impl SeanimeEpisode {
    pub fn title_string(&self) -> String {
        if let Some(t) = &self.episode_title {
            if !t.is_empty() {
                return format!("Ep {} - {}", self.episode_number, t);
            }
        }
        if let Some(dt) = &self.display_title {
            return dt.clone();
        }
        format!("Episode {}", self.episode_number)
    }

    pub fn thumbnail_url(&self) -> Option<String> {
        self.episode_metadata
            .as_ref()
            .and_then(|m| m.image.clone())
            .or_else(|| self.base_anime.as_ref().and_then(|b| b.cover_image.as_ref().and_then(|c| c.best_url())))
    }

    pub fn file_path(&self) -> Option<String> {
        self.local_file.as_ref().map(|f| f.path.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeAnimeEntry {
    #[serde(rename = "mediaId")]
    pub media_id: i32,
    pub media: Option<SeanimeMedia>,
    #[serde(default)]
    pub episodes: Vec<SeanimeEpisode>,
    #[serde(rename = "localFiles", default)]
    pub local_files: Vec<SeanimeLocalFile>,
    #[serde(rename = "nextEpisode")]
    pub next_episode: Option<SeanimeEpisode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeanimeEpisodeCollection {
    #[serde(default)]
    pub episodes: Vec<SeanimeEpisode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateProgressPayload {
    #[serde(rename = "mediaId")]
    pub media_id: i32,
    #[serde(rename = "episodeNumber")]
    pub episode_number: i32,
    #[serde(rename = "totalEpisodes")]
    pub total_episodes: i32,
    #[serde(rename = "malId", skip_serializing_if = "Option::is_none")]
    pub mal_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediastreamRequestPayload {
    #[serde(rename = "clientId")]
    pub client_id: String,
    pub path: String,
    #[serde(rename = "audioStreamIndex")]
    pub audio_stream_index: i32,
    #[serde(rename = "streamType")]
    pub stream_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MediastreamContainer {
    #[serde(rename = "streamUrl")]
    pub stream_url: Option<String>,
    #[serde(rename = "filePath")]
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuityOptions {
    #[serde(rename = "mediaId")]
    pub media_id: i32,
    #[serde(rename = "episodeNumber")]
    pub episode_number: i32,
    #[serde(rename = "currentTime")]
    pub current_time: f64,
    pub duration: f64,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateContinuityPayload {
    pub options: ContinuityOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WatchHistoryItem {
    #[serde(rename = "mediaId")]
    pub media_id: i32,
    #[serde(rename = "episodeNumber")]
    pub episode_number: i32,
    #[serde(rename = "currentTime")]
    pub current_time: f64,
    pub duration: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WatchHistoryItemResponse {
    pub found: bool,
    pub item: Option<WatchHistoryItem>,
}

#[derive(Clone)]
pub struct SeanimeClient {
    base_url: String,
    http_client: reqwest::Client,
}

impl SeanimeClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut url = base_url.into();
        if url.ends_with('/') {
            url.pop();
        }
        Self {
            base_url: url,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn default_local() -> Self {
        Self::new("http://192.168.1.250:3211")
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// GET /api/v1/status
    pub async fn get_status(&self) -> Result<SeanimeStatus, SeanimeError> {
        let url = format!("{}/api/v1/status", self.base_url);
        let resp = self.http_client.get(&url).send().await?;
        let res: SeanimeResponse<SeanimeStatus> = resp.json().await?;
        if let Some(err) = res.error {
            return Err(SeanimeError::Api(err));
        }
        Ok(res.data.unwrap_or_default())
    }

    /// GET /api/v1/library/collection
    pub async fn get_library_collection(&self) -> Result<SeanimeCollection, SeanimeError> {
        let url = format!("{}/api/v1/library/collection", self.base_url);
        let resp = self.http_client.get(&url).send().await?;
        let res: SeanimeResponse<SeanimeCollection> = resp.json().await?;
        if let Some(err) = res.error {
            return Err(SeanimeError::Api(err));
        }
        Ok(res.data.unwrap_or_default())
    }

    /// GET /api/v1/library/anime-entry/{id}
    pub async fn get_anime_entry(&self, media_id: i32) -> Result<SeanimeAnimeEntry, SeanimeError> {
        let url = format!("{}/api/v1/library/anime-entry/{}", self.base_url, media_id);
        let resp = self.http_client.get(&url).send().await?;
        let res: SeanimeResponse<SeanimeAnimeEntry> = resp.json().await?;
        if let Some(err) = res.error {
            return Err(SeanimeError::Api(err));
        }
        Ok(res.data.unwrap_or_default())
    }

    /// GET /api/v1/anime/episode-collection/{id}
    pub async fn get_episode_collection(&self, media_id: i32) -> Result<Vec<SeanimeEpisode>, SeanimeError> {
        let url = format!("{}/api/v1/anime/episode-collection/{}", self.base_url, media_id);
        let resp = self.http_client.get(&url).send().await?;
        let res: SeanimeResponse<SeanimeEpisodeCollection> = resp.json().await?;
        if let Some(err) = res.error {
            return Err(SeanimeError::Api(err));
        }
        Ok(res.data.map(|d| d.episodes).unwrap_or_default())
    }

    /// POST /api/v1/library/anime-entry/update-progress
    pub async fn update_progress(
        &self,
        media_id: i32,
        episode_number: i32,
        total_episodes: i32,
        mal_id: Option<i32>,
    ) -> Result<(), SeanimeError> {
        let url = format!("{}/api/v1/library/anime-entry/update-progress", self.base_url);
        let payload = UpdateProgressPayload {
            media_id,
            episode_number,
            total_episodes,
            mal_id,
        };
        let resp = self.http_client.post(&url).json(&payload).send().await?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SeanimeError::Api(format!("Update progress failed: {}", body)));
        }
        Ok(())
    }

    /// POST /api/v1/playback-manager/sync-current-progress
    pub async fn sync_current_progress(&self) -> Result<(), SeanimeError> {
        let url = format!("{}/api/v1/playback-manager/sync-current-progress", self.base_url);
        let resp = self.http_client.post(&url).send().await?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SeanimeError::Api(format!("Sync progress failed: {}", body)));
        }
        Ok(())
    }

    /// POST /api/v1/library/scan
    pub async fn scan_library(&self) -> Result<(), SeanimeError> {
        let url = format!("{}/api/v1/library/scan", self.base_url);
        let resp = self.http_client.post(&url).send().await?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SeanimeError::Api(format!("Library scan failed: {}", body)));
        }
        Ok(())
    }

    /// POST /api/v1/mediastream/request
    pub async fn request_mediastream(&self, file_path: &str) -> Result<String, SeanimeError> {
        let url = format!("{}/api/v1/mediastream/request", self.base_url);
        let payload = MediastreamRequestPayload {
            client_id: "amp-desktop".to_string(),
            path: file_path.to_string(),
            audio_stream_index: 0,
            stream_type: "direct".to_string(),
        };
        let resp = self.http_client.post(&url).json(&payload).send().await?;
        let res: SeanimeResponse<MediastreamContainer> = resp.json().await?;
        if let Some(err) = res.error {
            return Err(SeanimeError::Api(err));
        }
        if let Some(container) = res.data {
            if let Some(stream_path) = container.stream_url {
                if stream_path.starts_with("http://") || stream_path.starts_with("https://") {
                    return Ok(stream_path);
                } else {
                    return Ok(format!("{}{}", self.base_url, stream_path));
                }
            }
        }
        Ok(format!("{}/api/v1/mediastream/direct", self.base_url))
    }

    /// PATCH /api/v1/continuity/item
    pub async fn update_watch_history(
        &self,
        media_id: i32,
        episode_number: i32,
        current_time: f64,
        duration: f64,
    ) -> Result<(), SeanimeError> {
        let url = format!("{}/api/v1/continuity/item", self.base_url);
        let payload = UpdateContinuityPayload {
            options: ContinuityOptions {
                media_id,
                episode_number,
                current_time,
                duration,
                kind: "mediastream".to_string(),
            },
        };
        let resp = self.http_client.patch(&url).json(&payload).send().await?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SeanimeError::Api(format!("Update watch history failed: {}", body)));
        }
        Ok(())
    }

    /// GET /api/v1/continuity/item/{id}
    pub async fn get_watch_history_item(&self, media_id: i32) -> Result<Option<WatchHistoryItem>, SeanimeError> {
        let url = format!("{}/api/v1/continuity/item/{}", self.base_url, media_id);
        let resp = self.http_client.get(&url).send().await?;
        let res: SeanimeResponse<WatchHistoryItemResponse> = resp.json().await?;
        if let Some(data) = res.data {
            if data.found {
                return Ok(data.item);
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_seanime_client_live() {
        let client = SeanimeClient::default_local();
        // 1. Status (check server reachability)
        let status = match client.get_status().await {
            Ok(st) => st,
            Err(e) => {
                eprintln!("[SeanimeClientTest] Server not reachable ({}): {:?}. Skipping live assertions.", client.base_url(), e);
                return;
            }
        };
        assert!(status.user.is_some());
        let user = status.user.unwrap();
        assert_eq!(user.viewer.and_then(|v| v.name), Some("TheForgottenOne".to_string()));
        // 2. Library Collection
        let collection = client.get_library_collection().await;
        assert!(collection.is_ok(), "Failed to get collection: {:?}", collection.err());
        let collection = collection.unwrap();
        assert!(!collection.lists.is_empty() || !collection.continue_watching_list.is_empty());

        // 3. Anime Entry (Link Click Season 3: 191832)
        let entry = client.get_anime_entry(191832).await;
        assert!(entry.is_ok(), "Failed to get anime entry: {:?}", entry.err());
        let entry = entry.unwrap();
        assert_eq!(entry.media_id, 191832);
        assert!(!entry.episodes.is_empty());
        assert!(entry.episodes[0].file_path().is_some());

        // 4. Episode Collection
        let episodes = client.get_episode_collection(191832).await;
        assert!(episodes.is_ok(), "Failed to get episode collection: {:?}", episodes.err());
        let episodes = episodes.unwrap();
        assert!(!episodes.is_empty());
        assert_eq!(episodes[0].episode_number, 1);

        // 5. Continuity Watch History
        let update_res = client.update_watch_history(191832, 1, 45.0, 1307.0).await;
        assert!(update_res.is_ok(), "Failed to update watch history: {:?}", update_res.err());

        let history_item = client.get_watch_history_item(191832).await;
        assert!(history_item.is_ok(), "Failed to get watch history item: {:?}", history_item.err());
    }
}
