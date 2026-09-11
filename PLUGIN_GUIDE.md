# AMP Plugin & Extension Guide

AMP plugins and extensions are written in the **Parts** scripting language (`.pts` files). Scripts run inside an embedded Parts virtual machine runtime, providing high performance, sandboxed execution, and instant live hot-reloading without compilation.

A single Parts script can serve as:
- **Playback Extension**: React to playback events, track watch progress, integrate with external APIs (scrobbling, Discord RPC, AniList, etc.), or control the player.
- **Media Provider**: Expose media libraries, search content, provide directory trees, and resolve playable stream URLs.
- **Hybrid**: Combine provider and extension functionality in one script.

---

## Directory Setup & Deployment

### Script Location

Place your `.pts` script files in the default AMP Parts directory:

- **Linux / macOS**: `~/.config/amp/plugins/parts/`
- **Custom Location**: Specify a direct script path in AMP under `Settings` -> `Parts Extensions` (`script_path`).

AMP automatically discovers and loads all `.pts` files found in this directory.

### Live Hot-Reloading

AMP includes a built-in file watcher for Parts extensions. When you edit and save any `.pts` file in the plugins directory, AMP automatically reloads the script in real-time without needing to restart the application.

---

## Script Structure & Lifecycle

Every Parts script must evaluate to and return an object (`|> ... <|`) containing functions for the hooks and capabilities it implements.

```parts
return |>
    // Playback Extension Hooks
    on_playback_update: fun(info) {
        log("Playing: " + to_string(info.title));
    },
    on_playback_stop: fun() {
        log("Playback stopped");
    },
    on_time_update: fun(time_ms) {
        // Fast-path time update in milliseconds
    },
    on_state_changed: fun(state) {
        // 1 = Play, 0 = Pause, 3 = Stop
    },

    // Media Provider Hooks (Optional)
    get_root: fun() {
        return |>
            0: |>
                id: "item_1",
                name: "My Video Stream",
                item_type: "Playable",
                duration_secs: 1800
            <|,
            len: 1
        <|;
    },
    get_stream_url: fun(item_id) {
        return "https://example.com/stream.mp4";
    }
<|;
```

### Initialization Error Reporting

If your script fails to initialize (e.g., missing credentials or invalid configuration), you can return an error object instead:

```parts
return |>
    error_type: "Auth", // "Auth", "Plugin", or "Provider"
    message: "Missing or invalid API token"
<|;
```

---

## Built-in Native Functions

AMP registers native functions into the Parts runtime:

### Logging & Utilities
- `log(value)`: Prints a message to stderr (`[PartsVM] <value>`).
- `to_string(value)`: Converts any value into its string representation.
- `get_config(key)`: Retrieves a configuration string passed to AMP for this script.

### Global State Management
- `get_state(key)`: Retrieves a value stored in global cross-script state. Returns `false` if the key does not exist.
- `set_state(key, value)`: Stores a value into global state (persists across script calls and hot-reloads).

### HTTP Networking
- `http_get(url)`: Performs a blocking HTTP GET request and returns the response body as a string.
- `http_post(url, body)`: Performs a blocking HTTP POST request with a string body and returns the response body.
- `http_request(method, url, headers, body)`: Performs custom HTTP requests (`GET`, `POST`, `PUT`, `DELETE`, `PATCH`).
  - `headers`: Array of header objects `[ |> key: "Authorization", value: "Bearer ..." <| ]`.

### JSON Handling
- `json_parse(json_str)`: Parses a JSON string into native Parts objects and arrays.

### Player Controls
- `player_play()`: Resumes media playback.
- `player_pause()`: Pauses media playback.
- `player_toggle_pause()`: Toggles play/pause state.
- `player_next()`: Skips to the next playlist item.
- `player_previous()`: Skips to the previous playlist item.
- `player_stop()`: Stops media playback.
- `player_seek(seconds)`: Seeks to a specific position in seconds.
- `vm_command(cmd, arg)`: Sends low-level player commands:
  - `1`: Play
  - `2`: Pause
  - `3`: Toggle Pause
  - `4`: Seek (arg in seconds)
  - `5`: Stop
  - `6`: Next
  - `7`: Previous
- `mutate_ui_slot(slot_id, data_ptr)`: Dispatches UI slot mutations to the frontend.

---

## Playback Extension Hooks

Implement these callback functions to react to playback events and player status:

| Hook | Arguments | Description |
|------|-----------|-------------|
| `on_playback_update` | `info` | Called when playback starts, metadata changes, or periodic progress updates occur. |
| `on_playback_stop` | None | Called when playback is stopped. |
| `on_time_update` | `time_ms` | Fast-path position update in milliseconds (zero-allocation hot path). |
| `on_state_changed` | `state` | Called on player state transitions (`1` = Playing, `0` = Paused, `3` = Stopped). |
| `on_play` | None | Triggered when playback transitions to playing. |
| `on_pause` | None | Triggered when playback transitions to paused. |
| `on_stop` | None | Triggered when playback transitions to stopped. |
| `on_vm_command` | `cmd, arg` | Called when a custom VM command is dispatched. |

### `PlaybackInfo` Object Format

The `info` object passed to `on_playback_update` contains:

```parts
|>
    title: "Episode Title or File Name",
    artist: "Artist or Uploader",
    series_name: "Series Name",        // optional string or null
    season_index: 1,                   // optional integer
    episode_index: 4,                  // optional integer
    is_paused: false,                  // boolean
    position_secs: 142,                // integer (current position in seconds)
    duration_secs: 1440                // integer (total duration in seconds)
<|
```

---

## Media Provider Implementation

To expose content in AMP, implement the media provider hook methods in your returned object.

### Data Types

#### `MediaItem` Object

```parts
|>
    id: "unique_item_id",              // unique string ID within this script
    name: "Item Title",                // display name
    item_type: "Playable",             // "Playable" or "Folder"
    duration_secs: 1440,               // optional integer
    index: 1,                          // optional episode/track index
    resume_position_secs: 0,           // optional last watched position
    series_name: "My Series",          // optional series name
    season_index: 1                    // optional season number
<|
```

#### Media Item Collections

Lists of `MediaItem` objects can be returned either as an indexed object with a `len` property or as an array:

```parts
return |>
    0: |> id: "item_1", name: "Episode 1", item_type: "Playable", duration_secs: 1400 <|,
    1: |> id: "item_2", name: "Episode 2", item_type: "Playable", duration_secs: 1400 <|,
    len: 2
<|;
```

### Provider Methods

- `get_root()`: Return top-level folders or categories shown in the library view.
- `get_children(parent_id)`: Return items inside a container folder (`item_type: "Folder"`).
- `get_next_up()`: Return "Continue Watching" or "Next Up" items.
- `search(query)`: Search for items matching the query string.
- `get_stream_url(item_id)`: Return a playable media URL (HTTP/HTTPS, local file path, or stream link) for MPV to play.
- `get_resume_position(item_id)`: Return the last watched position in seconds (or `0`).
- `get_item_image_buffer(item_id)`: (Optional) Return raw RGBA8 thumbnail data:
  ```parts
  return |>
      width: 320,
      height: 180,
      rgba8: raw_byte_array
  <|;
  ```

### Playback Reporting Hooks

- `report_playback_start(item_id)`: Called when playback starts for an item.
- `report_playback_progress(item_id, position_secs, is_paused)`: Called periodically during playback.
- `report_playback_stopped(item_id, position_secs)`: Called when the user stops playback.
- `mark_as_played(item_id, played)`: Called when an item is marked as watched or unwatched.

---

## Examples

### 1. Webhook Scrobbler Extension (`webhook_scrobbler.pts`)

This script posts playback status to an external tracking webhook whenever a video plays:

```parts
let WEBHOOK_URL = "https://example.com/api/scrobble";

return |>
    on_playback_update: fun(info) {
        if (!info.is_paused) {
            let payload = "{\"title\":\"" + to_string(info.title) + "\",\"position\":" + to_string(info.position_secs) + "}";
            let headers = |>
                0: |> key: "Content-Type", value: "application/json" <|,
                len: 1
            <|;
            http_request("POST", WEBHOOK_URL, headers, payload);
            log("Scrobbled progress for: " + to_string(info.title));
        }
    },

    on_playback_stop: fun() {
        log("Playback stopped, resetting tracking state");
        set_state("current_playing", false);
    }
<|;
```

### 2. Custom Media Provider (`custom_feed.pts`)

This script fetches items from a JSON API and returns playable streams:

```parts
let FEED_URL = "https://api.example.com/videos.json";

return |>
    get_root: fun() {
        let json_text = http_get(FEED_URL);
        let data = json_parse(json_text);
        
        // Return parsed video items
        return |>
            0: |>
                id: "stream_101",
                name: "Big Buck Bunny",
                item_type: "Playable",
                duration_secs: 596
            <|,
            1: |>
                id: "folder_shows",
                name: "Anime Series",
                item_type: "Folder"
            <|,
            len: 2
        <|;
    },

    get_children: fun(parent_id) {
        if (parent_id == "folder_shows") {
            return |>
                0: |>
                    id: "stream_201",
                    name: "Episode 1",
                    item_type: "Playable",
                    duration_secs: 1440,
                    series_name: "Anime Series",
                    season_index: 1,
                    index: 1
                <|,
                len: 1
            <|;
        }
        return |> len: 0 <|;
    },

    get_stream_url: fun(item_id) {
        if (item_id == "stream_101") {
            return "https://commondatastorage.googleapis.com/gtv-videos-bucket/sample/BigBuckBunny.mp4";
        }
        return "https://example.com/video/" + item_id + ".mp4";
    },

    get_resume_position: fun(item_id) {
        let last_pos = get_state("resume_" + item_id);
        if (last_pos) {
            return last_pos;
        }
        return 0;
    },

    report_playback_progress: fun(item_id, position_secs, is_paused) {
        set_state("resume_" + item_id, position_secs);
    }
<|;
```
