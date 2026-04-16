use amp_api::{ConfigField, MediaItem, MediaProvider, MediaProviderFactory};
use async_trait::async_trait;
use serde::Deserialize;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Deserialize, Debug)]
pub struct JellyfinItemsResponse {
    #[serde(rename = "Items")]
    pub items: Vec<JellyfinItem>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AuthenticateResponse {
    #[serde(rename = "AccessToken")]
    pub access_token: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct JellyfinItem {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "RunTimeTicks")]
    pub run_time_ticks: Option<i64>,
}

#[derive(Clone)]
pub struct JellyfinClient {
    url: String,
    api_key: String,
    client: reqwest::Client,
}

impl JellyfinClient {
    pub async fn authenticate(
        url: String,
        user: String,
        pass: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::Client::new();
        let auth_url = format!("{}/Users/AuthenticateByName", url);
        let auth_header = "MediaBrowser Client=\"AMP\", Device=\"Linux\", DeviceId=\"AMP-CLI\", Version=\"0.1.0\"";

        let mut body = std::collections::HashMap::new();
        body.insert("Username", user);
        body.insert("Pw", pass);

        let resp = client
            .post(auth_url)
            .header("X-Emby-Authorization", auth_header)
            .json(&body)
            .send()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
            .json::<AuthenticateResponse>()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        Ok(Self {
            url: url.clone(),
            api_key: resp.access_token.clone(),
            client,
        })
    }

    pub async fn get_series_internal(&self) -> Result<Vec<JellyfinItem>, reqwest::Error> {
        let url = format!(
            "{}/Items?IncludeItemTypes=Series&Recursive=true&Fields=PrimaryImageAspectRatio",
            self.url
        );
        let resp = self
            .client
            .get(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .send()
            .await?
            .json::<JellyfinItemsResponse>()
            .await?;
        Ok(resp.items)
    }

    pub async fn get_episodes_internal(&self, series_id: &str) -> Result<Vec<JellyfinItem>, reqwest::Error> {
        let url = format!("{}/Shows/{}/Episodes", self.url, series_id);
        let resp = self
            .client
            .get(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .send()
            .await?
            .json::<JellyfinItemsResponse>()
            .await?;
        Ok(resp.items)
    }

    pub fn get_stream_url_internal(&self, item_id: &str) -> String {
        format!(
            "{}/Videos/{}/stream?api_key={}&Static=true",
            self.url, item_id, self.api_key
        )
    }

    fn get_auth_header(&self) -> String {
        format!("MediaBrowser Client=\"AMP\", Device=\"Linux\", DeviceId=\"AMP-CLI\", Version=\"0.1.0\", Token=\"{}\"", self.api_key)
    }

    pub async fn get_item_image_buffer_internal(
        &self,
        item_id: &str,
    ) -> Result<SharedPixelBuffer<Rgba8Pixel>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/Items/{}/Images/Primary?maxWidth=200", self.url, item_id);
        let resp = self
            .client
            .get(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .send()
            .await?
            .bytes()
            .await?;

        let img = image::load_from_memory(&resp)?;
        let rgba = img.to_rgba8();
        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
        );
        Ok(buffer)
    }
}

#[async_trait]
impl MediaProvider for JellyfinClient {
    async fn get_series(&self) -> Result<Vec<MediaItem>, Box<dyn std::error::Error + Send + Sync>> {
        let items = self.get_series_internal().await?;
        Ok(items.into_iter().map(|i| MediaItem {
            id: i.id,
            name: i.name,
            duration_ticks: i.run_time_ticks,
        }).collect())
    }

    async fn get_episodes(&self, series_id: &str) -> Result<Vec<MediaItem>, Box<dyn std::error::Error + Send + Sync>> {
        let items = self.get_episodes_internal(series_id).await?;
        Ok(items.into_iter().map(|i| MediaItem {
            id: i.id,
            name: i.name,
            duration_ticks: i.run_time_ticks,
        }).collect())
    }

    fn get_stream_url(&self, item_id: &str) -> String {
        self.get_stream_url_internal(item_id)
    }

    async fn get_item_image_buffer(&self, item_id: &str) -> Result<SharedPixelBuffer<Rgba8Pixel>, Box<dyn std::error::Error + Send + Sync>> {
        self.get_item_image_buffer_internal(item_id).await
    }

    fn get_persistable_config(&self) -> HashMap<String, String> {
        let mut config = HashMap::new();
        config.insert("url".to_string(), self.url.clone());
        config.insert("api_key".to_string(), self.api_key.clone());
        config
    }
}

pub struct JellyfinFactory;

impl MediaProviderFactory for JellyfinFactory {
    fn id(&self) -> &'static str { "jellyfin" }
    fn display_name(&self) -> &'static str { "Jellyfin" }
    
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

    fn create_provider(&self, config: HashMap<String, String>) -> Result<amp_api::DynProvider, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(api_key) = config.get("api_key") {
            // Recreating from cache
            let url = config.get("url").cloned().ok_or("Missing url in cache")?;
            return Ok(Arc::new(JellyfinClient {
                url,
                api_key: api_key.clone(),
                client: reqwest::Client::new(),
            }));
        }

        let url = config.get("url").cloned().ok_or("Missing url")?;
        let username = config.get("username").cloned().ok_or("Missing username")?;
        let password = config.get("password").cloned().ok_or("Missing password")?;
        
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = rt.block_on(JellyfinClient::authenticate(url, username, password))?;
        Ok(Arc::new(client))
    }
}
