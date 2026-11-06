use crate::api::DynExtension;
use crate::api::DynPlugin;
use crate::api::DynProvider;
use crate::api::Plugin;
use crate::api::PluginEvent;
use crate::backend::local::LocalPlugin;
use crate::backend::seanime::SeanimePlugin;
use crate::discord::DiscordPlugin;
use crate::extensions::PartsPlugin;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct PlaylistItem {
    pub p_id: String,
    pub item_id: String,
    pub name: String,
    pub series_name: String,
    pub index: i32,
    pub season_index: i32,
}

pub struct AppState {
    pub nav_stack: Vec<(String, String)>,         // (id, name)
    pub current_items_ids: Vec<(String, String)>, // (provider_id, item_id)
    pub next_up_ids: Vec<(String, String)>,
    pub active_playlist: Option<(Vec<PlaylistItem>, usize)>,
    pub search_results_ids: Vec<(String, String)>,
    pub current_item_id: Option<(String, String)>,
    pub current_title: String,
    pub current_artist: String,
    pub current_series_name: Option<String>,
    pub current_season_index: Option<i32>,
    pub current_episode_index: Option<i32>,
    pub active_providers: PluginManager,
}

impl AppState {
    pub fn new(channel: UnboundedSender<PluginEvent>) -> Self {
        Self {
            nav_stack: Vec::new(),
            current_items_ids: Vec::new(),
            next_up_ids: Vec::new(),
            active_playlist: None,
            search_results_ids: Vec::new(),
            current_item_id: None,
            current_title: String::new(),
            current_artist: String::new(),
            current_series_name: None,
            current_season_index: None,
            current_episode_index: None,
            active_providers: PluginManager::new(channel),
        }
    }
}

use crate::api::AmpError;
use tokio::sync::mpsc::UnboundedSender;

pub struct PluginManager {
    providers: HashMap<String, DynProvider>,
    extensions: HashMap<String, DynExtension>,
    plugins: HashMap<String, DynPlugin>,
    config: HashMap<String, HashMap<String, String>>,
    p_sender: UnboundedSender<PluginEvent>,
}

impl PluginManager {
    pub fn new(channel: UnboundedSender<PluginEvent>) -> Self {
        let mut pm = Self {
            plugins: HashMap::new(),
            extensions: HashMap::new(),
            providers: HashMap::new(),
            config: HashMap::new(),
            p_sender: channel,
        };

        let _ = pm.load_config();

        {
            let sp = SeanimePlugin {};
            pm.register_builtin_plugin(sp.id(), Arc::new(sp));
        }

        {
            let pp = PartsPlugin::new();
            pm.register_builtin_plugin(pp.id(), Arc::new(pp));
        }

        {
            let dp = DiscordPlugin {};
            pm.register_builtin_plugin(dp.id(), Arc::new(dp));
        }

        {
            let lp = LocalPlugin {};
            pm.register_builtin_plugin(lp.id(), Arc::new(lp));
        }

        pm
    }

    fn config_file() -> Option<std::path::PathBuf> {
        directories::ProjectDirs::from("com", "amp", "AMP")
            .map(|proj_dirs| proj_dirs.config_dir().join("config.json"))
    }

    pub fn load_config(&mut self) -> Result<(), AmpError> {
        if let Some(file) = Self::config_file()
            && let Ok(data) = std::fs::read_to_string(file)
            && let Ok(config) = serde_json::from_str(&data)
        {
            self.config = config;
            return Ok(());
        }
        Ok(())
    }

    pub fn save_config(&self) -> Result<(), AmpError> {
        let file = Self::config_file()
            .ok_or_else(|| AmpError::Plugin("Could not find config directory".into()))?;
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(&self.config)?;
        std::fs::write(file, data)?;
        Ok(())
    }

    pub fn register_builtin_plugin(&mut self, id: String, plugin: DynPlugin) {
        self.plugins.insert(id.clone(), plugin.clone());

        let config = if self.config.contains_key(&id) {
            self.get_config(&id)
        } else {
            let mut cfg = HashMap::new();

            for field in plugin.get_config_fields() {
                cfg.insert(field.key, field.default_value);
            }

            cfg
        };

        if let Some(provider) = plugin.create_provider(config.clone(), self.p_sender.clone())
            && let Ok(provider) = provider
        {
            self.providers.insert(id.clone(), provider);
        }

        if let Some(extension) = plugin.create_extension(config, self.p_sender.clone())
            && let Ok(extension) = extension
        {
            self.extensions.insert(id, extension);
        }
    }

    pub fn update_config(
        &mut self,
        id: String,
        config: HashMap<String, String>,
    ) -> Result<(), AmpError> {
        self.config.remove(&id);

        self.config.insert(id.clone(), config.clone());

        let _ = self.save_config();

        let plugin = self.get_plugin(&id).unwrap();

        if let Some(provider) = plugin.create_provider(config.clone(), self.p_sender.clone())
            && let Ok(provider) = provider
        {
            self.providers.insert(id.clone(), provider);
        }

        if let Some(extension) = plugin.create_extension(config, self.p_sender.clone())
            && let Ok(extension) = extension
        {
            self.extensions.insert(id, extension);
        }

        Ok(())
    }

    pub fn get_plugin(&self, id: &str) -> Option<DynPlugin> {
        self.plugins.get(id).cloned()
    }

    pub fn get_plugins(&self) -> HashMap<String, DynPlugin> {
        self.plugins.clone()
    }

    pub fn get_provider(&self, id: &str) -> Option<DynProvider> {
        self.providers.get(id).cloned()
    }

    pub fn get_providers(&self) -> HashMap<String, DynProvider> {
        self.providers.clone()
    }

    pub fn get_extensions(&self) -> HashMap<String, DynExtension> {
        self.extensions.clone()
    }

    pub fn get_config(&self, id: &str) -> HashMap<String, String> {
        self.config.get(id).unwrap_or(&HashMap::new()).clone()
    }
}
