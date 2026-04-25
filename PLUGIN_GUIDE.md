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

You must implement the `AmpPlugin` trait, and depending on your plugin's purpose, one or more of the capability traits: `MediaProvider`, `PlaybackExtension`, or `LibraryManager`.

### The Unified Plugin Trait (`AmpPlugin`)

This trait is the entry point for your plugin. It describes what your plugin can do via "capabilities".

```rust
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use amp_api::{AmpPlugin, PluginCapability, DynProvider, ConfigField, PlaybackExtension, LibraryManager, AmpError};

pub struct MyPlugin;

#[async_trait]
impl AmpPlugin for MyPlugin {
    fn id(&self) -> &'static str { "my-unique-id" }
    fn display_name(&self) -> &'static str { "My Custom Plugin" }
    
    fn capabilities(&self) -> Vec<PluginCapability> {
        // A plugin can be a provider, an extension, or both!
        vec![PluginCapability::MediaProvider]
    }

    // Config fields for MediaProvider
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

    // Config fields for Extensions (AniList, Sonarr, etc)
    fn extension_config_fields(&self) -> Vec<ConfigField> {
        vec![]
    }

    async fn create_provider(&self, config: HashMap<String, String>) -> Result<DynProvider, AmpError> {
        Ok(Arc::new(MyProvider::new(config)))
    }

    async fn create_extension(&self, _config: HashMap<String, String>) -> Result<Arc<dyn PlaybackExtension>, AmpError> {
        Err(AmpError::Plugin("PlaybackExtension not implemented".into()))
    }

    async fn create_library_manager(&self, _config: HashMap<String, String>) -> Result<Arc<dyn LibraryManager>, AmpError> {
        Err(AmpError::Plugin("LibraryManager not implemented".into()))
    }
}
```

### MediaProvider Trait

Implement this to provide content (movies, shows, etc.).

Key types:
- `MediaItem`: Represents a folder or a playable file. Includes metadata like `duration_secs`, `series_name`, etc.
- `RawImage`: Used for passing cover art/thumbnails (RGBA8).

Required methods:
- `get_root()`: Return the top-level categories.
- `get_children(parent_id)`: Return items within a folder.
- `get_next_up()`: Return "Continue Watching" or "Next Up" items.
- `search(query)`: Search the provider for items.
- `get_stream_url(item_id)`: Return a URL that MPV can play.
- `get_item_image_buffer(item_id)`: Return raw RGBA8 image data for the item.
- `get_persistable_config()`: Return config that should be saved for this provider.
- `get_resume_position(item_id)`: Return the last watched position in seconds.

Playback Reporting:
- `report_playback_start(item_id)`
- `report_playback_progress(item_id, position_secs, is_paused)`
- `report_playback_stopped(item_id, position_secs)`
- `mark_as_played(item_id, played)`

### PlaybackExtension Trait

Implement this to react to what the user is watching.
- `on_playback_update(info)`: Called whenever playback state changes (position, pause/play, metadata).
- `on_playback_stop()`: Called when playback ends.
- `set_controller(controller)`: Optional. Receives a `PlaybackController` to control the player (play/pause/seek/next/prev).

## Exporting the Plugin

To allow AMP to load your plugin at runtime, you must export the `get_plugin` function. **Crucially**, it must return a raw pointer from an `Arc` to ensure correct memory management across the FFI boundary.

```rust
#[no_mangle]
pub extern "C" fn get_plugin() -> *mut dyn AmpPlugin {
    let plugin: Arc<dyn AmpPlugin> = Arc::new(MyPlugin);
    Arc::into_raw(plugin) as *mut dyn AmpPlugin
}
```

## Building and Deployment

1. **Build**: Build the library in release mode.
   ```bash
   cargo build --release
   ```
2. **Copy**: Create a `plugins` directory in your AMP config folder and copy the resulting shared library (`.so` or `.dll`) there.
   - **Linux**: `~/.config/amp/plugins/` (or `~/.config/AMP/plugins/` depending on OS/ProjectDirs)
   - **Windows**: `%AppData%\amp\AMP\config\plugins\`
3. **Run**: Launch AMP. Your plugin will be loaded automatically.
