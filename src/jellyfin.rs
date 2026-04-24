use amp_api::{
    AmpError, AmpPlugin, ConfigField, LibraryManager, MediaItem, MediaProvider, PlaybackExtension,
    PluginCapability, RawImage,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde::Deserializer;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Deserialize, Debug, Clone)]
pub struct AuthenticateResponse {
    #[serde(rename = "AccessToken")]
    pub access_token: String,
    #[serde(rename = "User")]
    pub user: JellyfinUser,
}

#[derive(Deserialize, Debug, Clone)]
pub struct JellyfinUser {
    #[serde(rename = "Id")]
    pub id: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct JellyfinItemsResponse {
    #[serde(rename = "Items")]
    #[serde(default)]
    pub items: Vec<JellyfinItem>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct JellyfinItem {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    #[serde(rename = "IsFolder")]
    pub is_folder: bool,
    #[serde(rename = "RunTimeTicks")]
    #[serde(deserialize_with = "ticks_to_seconds")]
    #[serde(default)]
    pub run_time_seconds: Option<i64>,
    #[serde(rename = "UserData")]
    #[serde(default)]
    pub user_data: Option<JellyfinUserData>,
    #[serde(rename = "IndexNumber")]
    #[serde(default)]
    pub index_number: Option<i32>,
    #[serde(rename = "SeriesName")]
    #[serde(default)]
    pub series_name: Option<String>,
    #[serde(rename = "ParentIndexNumber")]
    #[serde(default)]
    pub parent_index_number: Option<i32>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct JellyfinUserData {
    #[serde(rename = "PlaybackPositionTicks")]
    pub playback_position_ticks: i64,
}

fn ticks_to_seconds<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let ticks: Option<i64> = Option::deserialize(deserializer)?;
    // 1 tick = 100ns -> 10,000,000 ticks = 1 second
    Ok(ticks.map(|t| t / 10_000_000))
}

#[derive(Clone)]
pub struct JellyfinClient {
    url: String,
    api_key: String,
    user_id: String,
    client: reqwest::Client,
}

impl JellyfinClient {
    pub async fn authenticate(
        url: String,
        user: String,
        pass: String,
    ) -> Result<Self, AmpError> {
        let client = reqwest::Client::new();
        let auth_url = format!("{}/Users/AuthenticateByName", url);

        let mut body = std::collections::HashMap::new();
        body.insert("Username", user);
        body.insert("Pw", pass);

        let resp = client
            .post(auth_url)
            .header(
                "X-Emby-Authorization",
                format!(
                    "MediaBrowser Client=\"AMP\", Device=\"Linux\", DeviceId=\"AMP-CLI\", Version=\"{}\"",
                    env!("CARGO_PKG_VERSION")
                ),
            )
            .json(&body)
            .send()
            .await
            .map_err(|e| AmpError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(AmpError::Auth(format!("Authentication failed with status: {}", resp.status())));
        }

        let auth_resp = resp
            .json::<AuthenticateResponse>()
            .await
            .map_err(AmpError::from)?;

        Ok(Self {
            url: url.clone(),
            api_key: auth_resp.access_token.clone(),
            user_id: auth_resp.user.id,
            client,
        })
    }

    pub async fn get_items_internal(
        &self,
        parent_id: Option<&str>,
    ) -> Result<Vec<JellyfinItem>, AmpError> {
        let url = if let Some(pid) = parent_id {
            format!(
                "{}/Items?ParentId={}&Fields=PrimaryImageAspectRatio,UserData",
                self.url, pid
            )
        } else {
            format!("{}/UserViews", self.url)
        };

        let resp_text = self
            .client
            .get(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .send()
            .await
            .map_err(|e| AmpError::Network(e.to_string()))?
            .text()
            .await
            .map_err(|e| AmpError::Network(e.to_string()))?;

        match serde_json::from_str::<JellyfinItemsResponse>(&resp_text) {
            Ok(resp) => Ok(resp.items),
            Err(e) => {
                eprintln!("[Jellyfin] Failed to decode items response: {}", e);
                Err(AmpError::Serialization(e))
            }
        }
    }

    pub async fn get_next_up_internal(
        &self,
    ) -> Result<Vec<JellyfinItem>, AmpError> {
        let url = format!("{}/Shows/NextUp?UserId={}", self.url, self.user_id);
        let resp_text = self
            .client
            .get(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .send()
            .await
            .map_err(|e| AmpError::Network(e.to_string()))?
            .text()
            .await
            .map_err(|e| AmpError::Network(e.to_string()))?;

        match serde_json::from_str::<JellyfinItemsResponse>(&resp_text) {
            Ok(resp) => Ok(resp.items),
            Err(e) => {
                eprintln!("[Jellyfin] Failed to decode next up response: {}", e);
                Err(AmpError::Serialization(e))
            }
        }
    }

    pub fn get_stream_url_internal(&self, item_id: &str) -> String {
        format!(
            "{}/Videos/{}/stream?api_key={}&Static=true",
            self.url, item_id, self.api_key
        )
    }

    fn get_auth_header(&self) -> String {
        format!("{}, Token=\"{}\", UserId=\"{}\"", Self::get_header(), self.api_key, self.user_id)
    }

    fn get_header() -> String {
        format!(
            "MediaBrowser Client=\"AMP\", Device=\"Linux\", DeviceId=\"AMP-CLI\", Version=\"{}\"",
            env!("CARGO_PKG_VERSION")
        )
    }

    pub async fn get_item_image_buffer_internal(
        &self,
        item_id: &str,
    ) -> Result<RawImage, AmpError> {
        let url = format!("{}/Items/{}/Images/Primary", self.url, item_id);
        let resp = self.client.get(url).send().await.map_err(|e| AmpError::Network(e.to_string()))?;
        let bytes = resp.bytes().await.map_err(|e| AmpError::Network(e.to_string()))?;
        let img = image::load_from_memory(&bytes).map_err(|e| AmpError::Provider(format!("Image decode error: {}", e)))?;
        let rgba = img.to_rgba8();
        Ok(RawImage {
            width: rgba.width(),
            height: rgba.height(),
            rgba8: rgba.into_raw(),
        })
    }

    fn map_item(i: JellyfinItem) -> amp_api::MediaItem {
        let item_type = if i.is_folder || i.item_type == "Series" || i.item_type == "Season" {
            amp_api::MediaItemType::Folder
        } else {
            amp_api::MediaItemType::Playable
        };

        amp_api::MediaItem {
            id: i.id,
            name: i.name,
            item_type,
            duration_secs: i.run_time_seconds,
            index: i.index_number,
            resume_position_secs: i
                .user_data
                .as_ref()
                .map(|ud| ud.playback_position_ticks / 10_000_000),
            series_name: i.series_name,
            season_index: i.parent_index_number,
        }
    }
}

#[async_trait]
impl MediaProvider for JellyfinClient {
    async fn get_root(&self) -> Result<Vec<MediaItem>, AmpError> {
        let items = self.get_items_internal(None).await?;
        Ok(items.into_iter().map(Self::map_item).collect())
    }

    async fn get_children(
        &self,
        parent_id: &str,
    ) -> Result<Vec<MediaItem>, AmpError> {
        let items = self.get_items_internal(Some(parent_id)).await?;
        Ok(items.into_iter().map(Self::map_item).collect())
    }

    async fn get_next_up(
        &self,
    ) -> Result<Vec<MediaItem>, AmpError> {
        let items = self.get_next_up_internal().await?;
        Ok(items.into_iter().map(Self::map_item).collect())
    }

    async fn search(
        &self,
        query: &str,
    ) -> Result<Vec<MediaItem>, AmpError> {
        let url = format!("{}/Users/{}/Items?searchTerm={}&Recursive=true&IncludeItemTypes=Series", self.url, self.user_id, urlencoding::encode(query));
        let resp_text = self
            .client
            .get(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .send()
            .await
            .map_err(|e| AmpError::Network(e.to_string()))?
            .text()
            .await
            .map_err(|e| AmpError::Network(e.to_string()))?;

        match serde_json::from_str::<JellyfinItemsResponse>(&resp_text) {
            Ok(resp) => Ok(resp.items.into_iter().map(Self::map_item).collect()),
            Err(e) => {
                eprintln!("[Jellyfin] Failed to decode search response: {}", e);
                Err(AmpError::Serialization(e))
            }
        }
    }

    fn get_stream_url(&self, item_id: &str) -> String {
        self.get_stream_url_internal(item_id)
    }

    async fn get_item_image_buffer(
        &self,
        item_id: &str,
    ) -> Result<RawImage, AmpError> {
        self.get_item_image_buffer_internal(item_id).await
    }

    fn get_persistable_config(&self) -> HashMap<String, String> {
        let mut config = HashMap::new();
        config.insert("url".to_string(), self.url.clone());
        config.insert("api_key".to_string(), self.api_key.clone());
        config.insert("user_id".to_string(), self.user_id.clone());
        config
    }

    async fn get_resume_position(
        &self,
        item_id: &str,
    ) -> Result<Option<i64>, AmpError> {
        let url = format!("{}/Users/{}/Items/{}", self.url, self.user_id, item_id);
        let resp = self
            .client
            .get(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .send()
            .await
            .map_err(|e| AmpError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text: String = resp.text().await.unwrap_or_default();

            eprintln!(
                "[Jellyfin] Getting resume position failed: {} - {}",
                status, err_text
            );

            return Err(AmpError::Provider(format!("Failed to get resume position: {}", status)));
        }

        let item = resp.json::<JellyfinItem>().await.map_err(AmpError::from)?;

        Ok(item
            .user_data
            .map(|ud| ud.playback_position_ticks / 10_000_000))
    }

    async fn report_playback_start(
        &self,
        item_id: &str,
    ) -> Result<(), AmpError> {
        let url = format!("{}/Sessions/Playing", self.url);
        let body = serde_json::json!({
            "ItemId": item_id,
        });

        eprintln!("[Jellyfin] Reporting playback start for {}", item_id);
        let resp = self
            .client
            .post(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .json(&body)
            .send()
            .await
            .map_err(|e| AmpError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            eprintln!("[Jellyfin] Start report failed: {} - {}", status, err_text);
        }
        Ok(())
    }

    async fn report_playback_progress(
        &self,
        item_id: &str,
        position_secs: i64,
        is_paused: bool,
    ) -> Result<(), AmpError> {
        let url = format!("{}/Sessions/Playing/Progress", self.url);
        let body = serde_json::json!({
            "ItemId": item_id,
            "PositionTicks": position_secs * 10_000_000,
            "IsPaused": is_paused,
        });

        let resp = self
            .client
            .post(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .json(&body)
            .send()
            .await
            .map_err(|e| AmpError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            eprintln!(
                "[Jellyfin] Progress report failed: {} - {}",
                status, err_text
            );
        } else {
            eprintln!("[Jellyfin] Progress report success: {}", status);
        }
        Ok(())
    }

    async fn report_playback_stopped(
        &self,
        item_id: &str,
        position_secs: i64,
    ) -> Result<(), AmpError> {
        let url = format!("{}/Sessions/Playing/Stopped", self.url);
        let body = serde_json::json!({
            "ItemId": item_id,
            "PositionTicks": position_secs * 10_000_000,
        });

        let resp = self
            .client
            .post(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .json(&body)
            .send()
            .await
            .map_err(|e| AmpError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            eprintln!(
                "[Jellyfin] Stopped report failed: {} - {}",
                status, err_text
            );
        }
        Ok(())
    }

    async fn mark_as_played(
        &self,
        item_id: &str,
        played: bool,
    ) -> Result<(), AmpError> {
        let url = if played {
            format!(
                "{}/Users/{}/PlayedItems/{}",
                self.url, self.user_id, item_id
            )
        } else {
            format!(
                "{}/Users/{}/PlayedItems/{}",
                self.url, self.user_id, item_id
            )
        };

        if played {
            let _ = self
                .client
                .post(url)
                .header("X-Emby-Authorization", self.get_auth_header())
                .send()
                .await
                .map_err(|e| AmpError::Network(e.to_string()))?;
        } else {
            let _ = self
                .client
                .delete(url)
                .header("X-Emby-Authorization", self.get_auth_header())
                .send()
                .await
                .map_err(|e| AmpError::Network(e.to_string()))?;
        }
        Ok(())
    }
}

pub struct JellyfinFactory;

#[async_trait]
impl AmpPlugin for JellyfinFactory {
    fn id(&self) -> &'static str {
        "jellyfin"
    }
    fn display_name(&self) -> &'static str {
        "Jellyfin"
    }
    fn capabilities(&self) -> Vec<PluginCapability> {
        vec![PluginCapability::MediaProvider]
    }

    fn config_fields(&self) -> Vec<ConfigField> {
        vec![
            ConfigField {
                key: "url".to_string(),
                label: "Server URL".to_string(),
                is_password: false,
                default_value: "http://localhost:8096".to_string(),
            },
            ConfigField {
                key: "username".to_string(),
                label: "Username".to_string(),
                is_password: false,
                default_value: "".to_string(),
            },
            ConfigField {
                key: "password".to_string(),
                label: "Password".to_string(),
                is_password: true,
                default_value: "".to_string(),
            },
        ]
    }

    async fn create_provider(
        &self,
        config: HashMap<String, String>,
    ) -> Result<amp_api::DynProvider, AmpError> {
        if let Some(api_key) = config.get("api_key") {
            let url = config.get("url").cloned().ok_or_else(|| AmpError::Plugin("Missing url in cache".into()))?;
            let user_id = config
                .get("user_id")
                .cloned()
                .ok_or_else(|| AmpError::Plugin("Missing user_id in cache".into()))?;

            return Ok(Arc::new(JellyfinClient {
                url,
                api_key: api_key.clone(),
                user_id,
                client: reqwest::Client::new(),
            }));
        }

        let url = config.get("url").cloned().ok_or_else(|| AmpError::Plugin("Missing url".into()))?;
        let username = config.get("username").cloned().ok_or_else(|| AmpError::Plugin("Missing username".into()))?;
        let password = config.get("password").cloned().ok_or_else(|| AmpError::Plugin("Missing password".into()))?;

        let client = JellyfinClient::authenticate(url, username, password).await?;
        Ok(Arc::new(client))
    }

    fn extension_config_fields(&self) -> Vec<ConfigField> {
        vec![]
    }

    async fn create_extension(
        &self,
        _config: HashMap<String, String>,
    ) -> Result<Arc<dyn PlaybackExtension>, AmpError> {
        Err(AmpError::Plugin("Not implemented".into()))
    }

    async fn create_library_manager(
        &self,
        _config: HashMap<String, String>,
    ) -> Result<Arc<dyn LibraryManager>, AmpError> {
        Err(AmpError::Plugin("Not implemented".into()))
    }
}
