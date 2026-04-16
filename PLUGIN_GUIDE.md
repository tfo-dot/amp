# Plugin Development Guide

This guide explains how to create new media provider plugins for the AMP.

## 1. Project Setup

AMP plugins are Rust dynamic libraries (`.so` on Linux, `.dll` on Windows). Create a new library project:

```bash
cargo new amp-my-provider --lib
```

Update your `Cargo.toml` to specify the crate type and include necessary dependencies:

```toml
[package]
name = "amp-my-provider"
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

## 2. Implementing the Traits

You must implement two primary traits: `MediaProvider` and `MediaProviderFactory`.

### MediaProvider
This trait handles the actual communication with your media source.

```rust
use async_trait::async_trait;
use std::collections::HashMap;
use std::error::Error;

pub struct MyProvider {
    // Your internal state (API keys, base URL, etc.)
}

#[async_trait]
impl MediaProvider for MyProvider {
    async fn get_series(&self) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
        // Return list of series
    }

    async fn get_episodes(&self, series_id: &str) -> Result<Vec<MediaItem>, Box<dyn Error + Send + Sync>> {
        // Return list of episodes for a series
    }

    fn get_stream_url(&self, item_id: &str) -> String {
        // Return the direct streaming URL for an item
    }

    async fn get_item_image_buffer(&self, item_id: &str) -> Result<SharedPixelBuffer<Rgba8Pixel>, Box<dyn Error + Send + Sync>> {
        // Return the thumbnail image buffer
    }

    fn get_persistable_config(&self) -> HashMap<String, String> {
        // Return data needed to recreate this session later (for autologin)
        let mut config = HashMap::new();
        config.insert("token".to_string(), "abc-123".to_string());
        config
    }
}
```

### MediaProviderFactory
This trait defines how your plugin describes itself to the UI and how it instantiates the provider.

```rust
pub struct MyFactory;

impl MediaProviderFactory for MyFactory {
    fn id(&self) -> &'static str { "my-unique-id" }
    fn display_name(&self) -> &'static str { "My Custom Provider" }

    fn config_fields(&self) -> Vec<ConfigField> {
        vec![
            ConfigField {
                key: "url".to_string(),
                label: "Server URL".to_string(),
                is_password: false,
                default_value: "https://".to_string(),
            },
            ConfigField {
                key: "token".to_string(),
                label: "API Token".to_string(),
                is_password: true,
                default_value: "".to_string(),
            },
        ]
    }

    fn create_provider(&self, config: HashMap<String, String>) -> Result<DynProvider, Box<dyn Error + Send + Sync>> {
        // Logic to create your provider from UI input or Cache
        Ok(Arc::new(MyProvider { ... }))
    }
}
```

## 3. Exporting the Plugin

To allow AMP to load your plugin at runtime, you must export the `get_factory` function:

```rust
#[no_mangle]
pub extern "C" fn get_factory() -> *mut dyn MediaProviderFactory {
    let factory = MyFactory;
    Box::into_raw(Box::new(factory))
}
```

## 4. Building and Deployment

1. **Build**: Build the library in release mode.
   ```bash
   cargo build --release
   ```
2. **Copy**: Copy the resulting shared library to the AMP `plugins/` directory.
   - Linux: `target/release/libamp_my_provider.so` -> `plugins/`
   - Windows: `target/release/amp_my_provider.dll` -> `plugins/`
3. **Run**: Launch AMP. Your provider will appear in the selection list.

## 5. Important Notes

- **Threading**: The `MediaProvider` methods are called from an async context (Tokio). Ensure your implementation is `Send + Sync`.
- **Slint Compatibility**: Ensure your plugin uses the exact same version of Slint as the AMP host to avoid ABI mismatches in `SharedPixelBuffer`.
- **Autologin**: AMP calls `create_provider` with the `HashMap` returned by `get_persistable_config` when restarting. Ensure your factory handles these keys.
