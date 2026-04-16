use async_trait::async_trait;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::error::Error;
use std::sync::Arc;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

pub struct MediaItem {
    pub id: String,
    pub name: String,
    pub duration_ticks: Option<i64>,
}

#[async_trait]
pub trait MediaProvider: Send + Sync {
    async fn get_series(&self) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>>;
    async fn get_episodes(&self, series_id: &str) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>>;
    fn get_stream_url(&self, item_id: &str) -> String;
    async fn get_item_image_buffer(&self, item_id: &str) -> Result<SharedPixelBuffer<Rgba8Pixel>, Box<dyn Error + Send + Sync>>;
    fn get_persistable_config(&self) -> HashMap<String, String>;
}

pub type DynProvider = Arc<dyn MediaProvider>;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LoginCache {
    pub provider_id: String,
    pub config: HashMap<String, String>,
}

impl LoginCache {
    fn config_file() -> Option<std::path::PathBuf> {
        directories::ProjectDirs::from("com", "amp", "AMP")
            .map(|proj_dirs| proj_dirs.config_dir().join("login.json"))
    }

    pub fn load() -> Option<Self> {
        let file = Self::config_file()?;
        let data = std::fs::read_to_string(file).ok()?;
        serde_json::from_str(&data).ok()
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let file = Self::config_file().ok_or("Could not find config directory")?;
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string(self)?;
        std::fs::write(file, data)?;
        Ok(())
    }

    pub fn delete() {
        if let Some(file) = Self::config_file() {
            let _ = std::fs::remove_file(file);
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConfigField {
    pub key: String,
    pub label: String,
    pub is_password: bool,
    pub default_value: String,
}

pub trait MediaProviderFactory: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn config_fields(&self) -> Vec<ConfigField>;
    fn create_provider(&self, config: HashMap<String, String>) -> Result<DynProvider, Box<dyn Error + Send + Sync>>;
}
