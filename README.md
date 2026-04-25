AMP is a highly extensible, blazingly fast media player built with Rust and Slint. It features a fully pluggable architecture, allowing for easy integration with various media providers through a dynamic plugin system.

## Features

- **Runtime Extensions**: Loads dynamic libraries (`.so` / `.dll`) at runtime to extend functionality.
- **Hardware Accelerated Playback**: Uses `libmpv` for high-performance, hardware-accelerated video playback.
- **Modern UI**: Built with Slint for a smooth and responsive user interface.
- **Rich Metadata**: Support for posters, episode information, and watch status synchronization.
- **Built-in Integrations**:
  - **Jellyfin**: Full support for browsing, searching, and streaming from Jellyfin servers.
  - **Discord RPC**: Automatically updates your Discord status with current playback info.

## Architecture

AMP is split into three main components:

1.  **AMP Core**: The main application runner, UI (Slint), and MPV integration.
2.  **AMP API (`amp-api`)**: A shared crate defining the traits and types that both the core app and plugins use.
3.  **Plugins**: Dynamic libraries located in the `plugins/` directory (or built-in) that implement media providers or playback extensions.

## Getting Started

### Prerequisites

- **Rust**: Install the latest stable version of Rust via [rustup](https://rustup.rs/).
- **libmpv**: Ensure `libmpv` is installed on your system.
  - **Linux**: Install `libmpv-dev` (e.g., `sudo apt install libmpv-dev`).
  - **Windows**: Place `mpv-1.dll` in your path or the project root.

### Running

Clone the repository and run:

```bash
cargo run --release
```

## Configuration

AMP stores its configuration and plugins in the standard OS config directory:

- **Linux**: `~/.config/AMP/`
- **Windows**: `%AppData%\amp\AMP\config\`
- **macOS**: `~/Library/Application Support/com.amp.AMP/`

The `config.json` file stores provider and extension settings.

## Developing Plugins

AMP supports three types of plugin capabilities:
- **MediaProvider**: Integrate new content sources (e.g., Plex, Local Files).
- **PlaybackExtension**: React to playback events (e.g., Discord RPC, Scrobbling).
- **LibraryManager**: Manage and organize media libraries.

For detailed instructions, see the [**Plugin Development Guide (PLUGIN_GUIDE.md)**](PLUGIN_GUIDE.md).
