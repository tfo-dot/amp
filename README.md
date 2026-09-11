# AMP

AMP is a highly extensible, blazingly fast media player built with Rust and Slint. It features a modular, pluggable architecture supporting built-in media backends and dynamic extensions powered by the Parts scripting language.

## Features

- **Parts Scripting Extensions**: Write custom extensions and media providers in the **Parts** scripting language (`.pts`) with live, zero-restart hot-reloading.
- **High-Performance Video Playback**: Hardware-accelerated rendering powered by `libmpv` with a custom OpenGL FBO rendering pipeline.
- **Modern Reactive UI**: Built with Slint for a smooth, fluid, and responsive user experience.
- **Rich Metadata & Tracking**: Support for episode information, posters, resume positions, and watch history synchronization.
- **Built-in Integrations**:
  - **Seanime**: Full support for anime browsing, episode tracking, watch status synchronization, and streaming via Seanime REST client and WebSocket integration.
  - **Local Media**: Direct browsing and playback of media files from local directories.
  - **Discord RPC**: Real-time Discord Rich Presence displaying current playback info, titles, and timestamps.

## Architecture

AMP is organized into modular subsystems:

1. **AMP Core & UI**: Application state management, navigation, and reactive desktop interface built with Slint.
2. **Video Pipeline**: High-performance `libmpv` integration using OpenGL framebuffer objects (FBO) for seamless rendering.
3. **Parts Extension Engine**: Embedded Parts scripting runtime with background workers, native API bindings (HTTP, JSON, Player control, Shared state), and a live file watcher (`ExtensionWatcher`) for real-time script hot-reloading.
4. **Provider System**: Unified `MediaProvider` abstraction powering both native backends (Seanime, Local Files) and dynamic `.pts` script extensions.

## Getting Started

### Prerequisites

- **Rust**: Install the latest stable version of Rust via [rustup](https://rustup.rs/).
- **libmpv**: Ensure `libmpv` is installed on your system.
  - **Linux**: Install `libmpv-dev` (e.g., `sudo apt install libmpv-dev` or `sudo pacman -S mpv`).
  - **Windows**: Place `mpv-1.dll` in your PATH or the project root.
  - **macOS**: Install `mpv` via Homebrew (`brew install mpv`).

### Running

Clone the repository and run:

```bash
cargo run --release
```

## Configuration & Extensions

AMP stores its extensions in standard configuration paths:

- **Parts Scripts Directory**:
  - **Linux / macOS**: `~/.config/amp/plugins/parts/`
  - **Custom Location**: Configure the `script_path` field in AMP under the `Parts Extensions` provider settings.

Any `.pts` file placed in the directory is automatically discovered and loaded. Saving changes to a script triggers instant live hot-reloading.

## Developing Plugins & Extensions

AMP extensions are written in the **Parts** scripting language (`.pts` files). Scripts evaluate to an export object implementing:

- **Playback Extensions**: React to playback updates (`on_playback_update`), track progress, scrobble to external services, or control the player via native functions.
- **Media Providers**: Expose custom media libraries (`get_root`, `get_children`, `search`), resolve stream URLs (`get_stream_url`), and report watch progress.

For full API specifications, native functions, and code examples, see the [**Plugin & Extension Guide (PLUGIN_GUIDE.md)**](PLUGIN_GUIDE.md).
