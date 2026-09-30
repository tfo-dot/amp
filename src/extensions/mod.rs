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

pub struct PartsPlugin {}

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
        let peb = Arc::new(PartsExtensionBridge::with_config(config));

        // // Hot-reload file watcher for .pts scripts
        let mut watcher = ExtensionWatcher::new();
        if let Some(parts_dir) = bridge::get_parts_dir() {
            let bridge_clone = peb.clone();
            watcher.start(vec![parts_dir], move |script_path| {
                let _ = bridge_clone.load_scripts();
                eprintln!("[AMP] Reloaded parts extensions: {}", script_path);
            });
        }

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
        _: HashMap<String, String>,
        _: UnboundedSender<PluginEvent>,
    ) -> Option<Result<Arc<dyn PlaybackExtension + 'static>, AmpError>> {
        None
    }
}
