#![allow(unused_imports, dead_code)]
pub mod bridge;
pub mod watcher;

use crate::PluginEvent;
use crate::api::{
    AmpError, ConfigField, MediaProvider, PlaybackController, PlaybackExtension, Plugin,
};
use crate::extensions::watcher::ExtensionWatcher;
use bridge::{ExtensionBridge, PartsExtensionBridge};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExtensionError {
    #[error("VM error: {0}")]
    VmError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Script not found: {0}")]
    NotFound(String),
    #[error("Invalid command: {0}")]
    InvalidCommand(i64),
}

#[derive(Clone, Default)]
pub struct PartsPlugin {
    peb: Arc<std::sync::Mutex<Option<Arc<PartsExtensionBridge>>>>,
}

impl PartsPlugin {
    pub fn new() -> Self {
        Self {
            peb: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn get_or_create_bridge(&self, config: HashMap<String, String>) -> Arc<PartsExtensionBridge> {
        let mut guard = self.peb.lock().unwrap();
        if let Some(peb) = &*guard {
            return peb.clone();
        }

        let peb = Arc::new(PartsExtensionBridge::with_config(config));
        let _ = peb.load_scripts();

        // Hot-reload file watcher for .pts scripts
        let mut watcher = ExtensionWatcher::new();
        let mut watch_dirs = Vec::new();
        if let Some(parts_dir) = bridge::get_parts_dir() {
            watch_dirs.push(parts_dir);
        }
        let local_plugins = std::path::PathBuf::from("plugins/parts");
        if local_plugins.is_dir() {
            watch_dirs.push(local_plugins);
        }
        let local_plugins_root = std::path::PathBuf::from("plugins");
        if local_plugins_root.is_dir() {
            watch_dirs.push(local_plugins_root);
        }

        if !watch_dirs.is_empty() {
            let bridge_clone = peb.clone();
            watcher.start(watch_dirs, move |script_path| {
                let _ = bridge_clone.load_scripts();
                eprintln!("[AMP] Reloaded parts extensions: {}", script_path);
            });
        }

        *guard = Some(peb.clone());
        peb
    }
}

impl Plugin for PartsPlugin {
    fn id(&self) -> String {
        "partsplugin".to_string()
    }

    fn visible_name(&self) -> String {
        "Parts Extensions".to_string()
    }

    fn create_provider(
        &self,
        config: HashMap<String, String>,
        _channel: UnboundedSender<PluginEvent>,
    ) -> Option<Result<Arc<dyn MediaProvider + 'static>, AmpError>> {
        let peb = self.get_or_create_bridge(config);
        Some(Ok(peb))
    }

    fn get_config_fields(&self) -> Vec<ConfigField> {
        vec![ConfigField {
            key: "base_url".to_string(),
            label: "Seanime server address".to_string(),
            is_password: false,
            default_value: "http://localhost:43211".to_string(),
        }]
    }

    fn create_extension(
        &self,
        config: HashMap<String, String>,
        _channel: UnboundedSender<PluginEvent>,
    ) -> Option<Result<Arc<dyn PlaybackExtension + 'static>, AmpError>> {
        let peb = self.get_or_create_bridge(config);
        Some(Ok(peb))
    }
}
