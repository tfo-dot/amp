# Plugin Development Guide

This guide explains how to create new plugins for the AMP.

## Project Setup

AMP plugins are Rust dynamic libraries (`.so` on Linux, `.dll` on Windows). Create a new library project:

```bash
cargo new amp-my-ext --lib
```

Update your `Cargo.toml` to specify the crate type and include necessary dependencies:

```toml
[package]
name = "amp-my-ext"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
async-trait = "0.1"
serde = { version = "1.0", features = ["derive"] }
# The shared API crate providing the traits and common types
amp-api = { path = "../amp-api" }
```

## Implementing the Traits

You must implement the `AmpPlugin` trait, and depending on your plugin's purpose, the `MediaProvider` and/or `PlaybackExtension` traits.

### The Unified Plugin Trait (`AmpPlugin`)

This trait is the entry point for your plugin. It describes what your plugin can do via "capabilities".

```rust
use async_trait::async_trait;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use amp_api::{AmpPlugin, PluginCapability, DynProvider, ConfigField, PlaybackExtension};

pub struct MyPlugin;

#[async_trait]
impl AmpPlugin for MyPlugin {
    fn id(&self) -> &'static str { "my-unique-id" }
    fn display_name(&self) -> &'static str { "My Custom Plugin" }
    
    fn capabilities(&self) -> Vec<PluginCapability> {
        // A plugin can be a provider, an extension, or both!
        vec![PluginCapability::MediaProvider]
    }

    // Only needed if you have MediaProvider capability
    fn config_fields(&self) -> Vec<ConfigField> {
        vec![
            ConfigField {
                key: "url".to_string(),
                label: "Server URL".to_string(),
                is_password: false,
                default_value: "https://".to_string(),
            },
        ]
    }

    async fn create_provider(&self, config: HashMap<String, String>) -> Result<DynProvider, Box<dyn Error + Send + Sync>> {
        Ok(Arc::new(MyProvider { ... }))
    }
}
```

## Exporting the Plugin

To allow AMP to load your plugin at runtime, you must export the `get_plugin` function:

```rust
#[no_mangle]
pub extern "C" fn get_plugin() -> *mut dyn AmpPlugin {
    let plugin = MyPlugin;
    Box::into_raw(Box::new(plugin))
}
```

## Building and Deployment

1. **Build**: Build the library in release mode.
   ```bash
   cargo build --release
   ```
2. **Copy**: Copy the resulting shared library to the AMP `plugins/` directory.
3. **Run**: Launch AMP. Your plugin will be loaded automatically.
