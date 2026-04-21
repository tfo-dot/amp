# AMP - Advanced Media Player

AMP is a highly extensible, blazingly fast media player built. It features a fully pluggable architecture, allowing for easy integration with various media providers (Jellyfin, Plex, Local Files, etc.) through a dynamic plugin system.

## Features

- Loads extensions at runtime
- Uses libmpv for the playback of the actual stream
- Jellyfin built-in
- Discord RPC ready

## Architecture

AMP is split into three main components:

1.  **AMP Core**: The main application runner, UI, and MPV integration.
2.  **AMP API (`amp-api`)**: A shared crate defining the traits, that both the core app and plugins use.
3.  **Plugins**: Dynamic libraries (`.so` or `.dll`) located in the `plugins/` directory that implement extensions.

## Getting Started

- Install rust (with cargo)
- Install libmpv

Clone and `cargo run` the project.

## Developing Plugins

For instructions, see the [**Plugin Development Guide (PLUGIN_GUIDE.md)**](PLUGIN_GUIDE.md).