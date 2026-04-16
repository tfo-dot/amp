# AMP - Advanced Media Player

AMP is a highly extensible, high-performance media player built with **Rust**, **Slint**, and **libmpv**. It features a fully pluggable architecture, allowing for easy integration with various media providers (Jellyfin, Plex, Local Files, etc.) through a dynamic plugin system.

## Features

- Loads extensions at runtime
- Uses libmpv for the playback of the actual stream
- Jellyfin built-in

## 🛠️ Architecture

AMP is split into three main components:

1.  **AMP Core**: The main application runner, UI, and MPV integration.
2.  **AMP API (`amp-api`)**: A shared crate defining the traits (`MediaProvider`, `MediaProviderFactory`) that both the core app and plugins use.
3.  **Plugins**: Dynamic libraries (`.so` or `.dll`) located in the `plugins/` directory that implement support for specific media backends.

## 📦 Getting Started

### Prerequisites

- **Rust**: [Install Rust](https://www.rust-lang.org/learn/get-started).
- **libmpv**: Ensure `libmpv` is installed on your system.
    - **Linux**: `sudo apt install libmpv-dev` (or equivalent).
    - **macOS**: `brew install mpv`.
    - **Windows**: Download the dev headers and library from the mpv website.

### Running AMP

1.  Clone the repository:
    ```bash
    git clone https://github.com/your-repo/amp.git
    cd amp
    ```
2.  Build and run:
    ```bash
    cargo run
    ```

## Developing Plugins

For instructions, see the [**Plugin Development Guide (PLUGIN_GUIDE.md)**](PLUGIN_GUIDE.md).