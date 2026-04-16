use amp_api::MediaProviderFactory;
use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

pub struct PluginManager {
    factories: HashMap<String, Arc<dyn MediaProviderFactory>>,
    #[allow(dead_code)]
    libraries: Vec<Library>, // Keep libraries in memory
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
            libraries: Vec::new(),
        }
    }

    pub fn register_builtin(&mut self, factory: Arc<dyn MediaProviderFactory>) {
        self.factories.insert(factory.id().to_string(), factory);
    }

    pub unsafe fn load_plugin<P: AsRef<std::ffi::OsStr>>(&mut self, path: P) -> Result<(), Box<dyn Error>> {
        let lib = Library::new(path)?;
        
        // Plugins should export a function: pub extern "C" fn get_factory() -> *mut dyn MediaProviderFactory
        let constructor: Symbol<unsafe extern "C" fn() -> *mut dyn MediaProviderFactory> = 
            lib.get(b"get_factory\0")?;
        
        let factory_ptr = constructor();
        let factory = Arc::from_raw(factory_ptr);
        
        self.factories.insert(factory.id().to_string(), factory);
        self.libraries.push(lib);
        
        Ok(())
    }

    pub fn get_factories(&self) -> Vec<Arc<dyn MediaProviderFactory>> {
        self.factories.values().cloned().collect()
    }

    pub fn get_factory(&self, id: &str) -> Option<Arc<dyn MediaProviderFactory>> {
        self.factories.get(id).cloned()
    }
}
