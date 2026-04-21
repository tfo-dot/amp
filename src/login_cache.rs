use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
