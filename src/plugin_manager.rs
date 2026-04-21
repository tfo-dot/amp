use amp_api::{AmpPlugin, PluginCapability};
use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

pub struct PluginManager {
    plugins: HashMap<String, Arc<dyn AmpPlugin>>,
    #[allow(dead_code)]
    libraries: Vec<Library>, // Keep libraries in memory
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            libraries: Vec::new(),
        }
    }

    pub fn register_builtin_plugin(&mut self, plugin: Arc<dyn AmpPlugin>) {
        self.plugins.insert(plugin.id().to_string(), plugin);
    }

    pub unsafe fn load_plugin<P: AsRef<std::ffi::OsStr>>(&mut self, path: P) -> Result<(), Box<dyn Error>> {
        let lib = Library::new(path)?;
        
        if let Ok(init) = lib.get::<Symbol<unsafe extern "C" fn() -> *mut dyn AmpPlugin>>(b"get_plugin\0") {
            let plugin_ptr = init();
            let plugin = Arc::from_raw(plugin_ptr);
            self.plugins.insert(plugin.id().to_string(), plugin);
            self.libraries.push(lib);
            Ok(())
        } else {
            Err("Plugin does not export get_plugin symbol".into())
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
}
