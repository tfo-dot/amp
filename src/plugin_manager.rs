use amp_api::{AmpError, AmpPlugin, PluginCapability};
use libloading::{Library, Symbol};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct AppConfig {
    pub provider_configs: HashMap<String, HashMap<String, String>>,
    pub extension_configs: HashMap<String, HashMap<String, String>>,
}

pub struct PluginManager {
    plugins: HashMap<String, Arc<dyn AmpPlugin>>,
    #[allow(dead_code)]
    libraries: Vec<Library>, // Keep libraries in memory
    pub config: AppConfig,
}

impl PluginManager {
    pub fn new() -> Self {
        let mut pm = Self {
            plugins: HashMap::new(),
            libraries: Vec::new(),
            config: AppConfig::default(),
        };
        let _ = pm.load_config();
        pm
    }

    fn config_file() -> Option<std::path::PathBuf> {
        directories::ProjectDirs::from("com", "amp", "AMP")
            .map(|proj_dirs| proj_dirs.config_dir().join("config.json"))
    }

    pub fn load_config(&mut self) -> Result<(), AmpError> {
        if let Some(file) = Self::config_file() {
            if let Ok(data) = std::fs::read_to_string(file) {
                if let Ok(config) = serde_json::from_str(&data) {
                    self.config = config;
                    return Ok(());
                }
            }
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

    pub fn register_builtin_plugin(&mut self, plugin: Arc<dyn AmpPlugin>) {
        self.plugins.insert(plugin.id().to_string(), plugin);
    }

    pub unsafe fn load_plugin<P: AsRef<std::ffi::OsStr> + libloading::AsFilename>(
        &mut self,
        path: P,
    ) -> Result<(), AmpError> {
        let lib = Library::new(path).map_err(|e| AmpError::Plugin(e.to_string()))?;

        if let Ok(init) =
            lib.get::<Symbol<unsafe extern "C" fn() -> *mut dyn AmpPlugin>>(b"get_plugin\0")
        {
            let plugin_ptr = init();
            let plugin = Arc::from_raw(plugin_ptr);
            self.plugins.insert(plugin.id().to_string(), plugin);
            self.libraries.push(lib);
            Ok(())
        } else {
            Err(AmpError::Plugin(
                "Plugin does not export get_plugin symbol".into(),
            ))
        }
    }

    pub fn with_capability(&self, capability: PluginCapability) -> Vec<Arc<dyn AmpPlugin>> {
        self.plugins
            .values()
            .filter(|p| p.capabilities().contains(&capability))
            .cloned()
            .collect()
    }

    pub fn get_plugin(&self, id: &str) -> Option<Arc<dyn AmpPlugin>> {
        self.plugins.get(id).cloned()
    }

    pub fn load_plugins(&mut self) {
        let plugins_dir = directories::ProjectDirs::from("com", "amp", "AMP")
            .map(|proj_dirs| proj_dirs.config_dir().join("plugins"))
            .expect("Shouldn't be empty");

        eprintln!("[AMP] Scanning for plugins in {:?}", plugins_dir);
        if let Ok(entries) = std::fs::read_dir(plugins_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .map_or(false, |ext| ext == "so" || ext == "dll")
                {
                    eprintln!("[AMP] Loading plugin: {:?}", path);
                    unsafe {
                        if let Err(e) = self.load_plugin(&path) {
                            eprintln!("Failed to load plugin {:?}: {}", path, e);
                        }
                    }
                }
            }
        }
    }

    pub fn get_extensions(&self) -> Vec<Arc<dyn amp_api::PlaybackExtension>> {
        let mut exts = Vec::new();

        for p in self.with_capability(PluginCapability::PlaybackExtension) {
            let config = self
                .config
                .extension_configs
                .get(p.id())
                .cloned()
                .unwrap_or_default();

            if let Ok(ext) = futures::executor::block_on(p.create_extension(config)) {
                exts.push(ext);
            }
        }

        exts
    }
}
