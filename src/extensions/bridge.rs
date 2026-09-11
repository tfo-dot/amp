#![allow(unused_imports, dead_code)]
use super::ExtensionError;
use crate::api::{
    AmpError, MediaItem, MediaItemType, MediaProvider, PlaybackController, PlaybackInfo, RawImage,
};
use async_trait::async_trait;
use parts::engine::Engine;
use parts::value::{FromValue, IntoValue, Value, parts_native};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex, RwLock};

// Global state shared across scripts for set_state / get_state
static GLOBAL_STATE: LazyLock<RwLock<HashMap<String, serde_json::Value>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
// Global playback controller
static PLAYBACK_CONTROLLER: LazyLock<Mutex<Option<Arc<dyn PlaybackController>>>> =
    LazyLock::new(|| Mutex::new(None));
// Global UI slot mutation dispatcher
static UI_SLOT_DISPATCHER: LazyLock<Mutex<Option<Arc<dyn Fn(i64, i64) + Send + Sync>>>> =
    LazyLock::new(|| Mutex::new(None));
// Global VM command dispatcher
static VM_COMMAND_DISPATCHER: LazyLock<
    Mutex<Option<Arc<dyn Fn(i64, i64) -> Result<(), String> + Send + Sync>>>,
> = LazyLock::new(|| Mutex::new(None));
thread_local! {
    static CONFIG: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

pub trait ExtensionBridge: Send + Sync {
    /// Pass time updates as milliseconds (i64) to avoid float allocations in the VM.
    fn notify_time_update(&self, time_ms: i64);

    /// 0 = Paused, 1 = Playing, 2 = Buffering, 3 = Ended
    fn notify_state_changed(&self, state: i64);

    /// Executed by the `.pts` script to mutate playback (e.g., cmd: 1 = Seek, arg: 5000)
    fn vm_command(&mut self, cmd: i64, arg: i64) -> Result<(), ExtensionError>;

    /// Allows the script to inject properties or layout changes into Slint extension slots.
    fn mutate_ui_slot(&self, slot_id: i64, data_ptr: i64);

    /// Gracefully reload the VM instance when inotify detects a `.pts` write.
    fn hot_reload(&mut self, script_path: &str) -> Result<(), ExtensionError>;
}

fn get_controller() -> Option<Arc<dyn PlaybackController>> {
    PLAYBACK_CONTROLLER.lock().unwrap().clone()
}

pub fn set_global_controller(controller: Arc<dyn PlaybackController>) {
    let mut guard = PLAYBACK_CONTROLLER.lock().unwrap();
    *guard = Some(controller);
}

pub fn set_ui_slot_handler<F>(handler: F)
where
    F: Fn(i64, i64) + Send + Sync + 'static,
{
    let mut guard = UI_SLOT_DISPATCHER.lock().unwrap();
    *guard = Some(Arc::new(handler));
}

pub fn set_vm_command_handler<F>(handler: F)
where
    F: Fn(i64, i64) -> Result<(), String> + Send + Sync + 'static,
{
    let mut guard = VM_COMMAND_DISPATCHER.lock().unwrap();
    *guard = Some(Arc::new(handler));
}

// Data translation helpers
pub fn parts_to_raw_image(val: &Value) -> Result<RawImage, AmpError> {
    if let Value::Object(obj_ref) = val {
        let obj = obj_ref.borrow();

        let get_val = |key: &str| {
            let hash = Value::String(key.to_string().into()).get_hash();
            obj.get(&hash).cloned().unwrap_or(Value::Bool(false))
        };

        let width = u32::from_value(&get_val("width")).map_err(AmpError::Provider)?;
        let height = u32::from_value(&get_val("height")).map_err(AmpError::Provider)?;
        let rgba8 = Vec::<u8>::from_value(&get_val("rgba8")).map_err(AmpError::Provider)?;

        Ok(RawImage {
            width,
            height,
            rgba8,
        })
    } else {
        Err(AmpError::Provider("Expected object for RawImage".into()))
    }
}

pub fn parts_to_media_item(val: &Value) -> Result<MediaItem, String> {
    if let Value::Object(obj_ref) = val {
        let obj = obj_ref.borrow();
        let get_val = |key: &str| {
            let hash = Value::String(key.to_string().into()).get_hash();
            obj.get(&hash).cloned().unwrap_or(Value::Bool(false))
        };

        let item_type_str = String::from_value(&get_val("item_type"))?;
        let item_type = match item_type_str.as_str() {
            "Folder" => MediaItemType::Folder,
            "Playable" => MediaItemType::Playable,
            _ => return Err(format!("Invalid item_type: {}", item_type_str)),
        };

        Ok(MediaItem {
            id: String::from_value(&get_val("id"))?,
            name: String::from_value(&get_val("name"))?,
            item_type,
            duration_secs: Option::<i64>::from_value(&get_val("duration_secs"))?,
            index: Option::<i32>::from_value(&get_val("index"))?,
            resume_position_secs: Option::<i64>::from_value(&get_val("resume_position_secs"))?,
            series_name: Option::<String>::from_value(&get_val("series_name"))?,
            season_index: Option::<i32>::from_value(&get_val("season_index"))?,
        })
    } else {
        Err("Expected object for MediaItem".to_string())
    }
}

pub fn parts_to_media_items(val: &Value) -> Result<Vec<MediaItem>, AmpError> {
    if let Value::Object(obj_ref) = val {
        let obj = obj_ref.borrow();
        let len_key = Value::String("len".to_string().into()).get_hash();
        let mut items = Vec::new();
        if let Some(Value::Int(l)) = obj.get(&len_key) {
            for i in 0..*l {
                let int_key = Value::Int(i).get_hash();
                let str_key = Value::String(i.to_string().into()).get_hash();
                if let Some(item_val) = obj.get(&int_key).or_else(|| obj.get(&str_key)) {
                    items.push(parts_to_media_item(item_val).map_err(AmpError::Provider)?);
                }
            }
            return Ok(items);
        } else {
            let mut i = 0;
            loop {
                let int_key = Value::Int(i).get_hash();
                let str_key = Value::String(i.to_string().into()).get_hash();
                if let Some(item_val) = obj.get(&int_key).or_else(|| obj.get(&str_key)) {
                    items.push(parts_to_media_item(item_val).map_err(AmpError::Provider)?);
                    i += 1;
                } else {
                    break;
                }
            }
            if !items.is_empty() {
                return Ok(items);
            }
        }
    }

    if let Ok(vec) = Vec::<Value>::from_value(val) {
        return vec
            .iter()
            .map(parts_to_media_item)
            .collect::<Result<Vec<_>, _>>()
            .map_err(AmpError::Provider);
    }

    Ok(Vec::new())
}

pub fn playback_info_to_parts(info: PlaybackInfo) -> Value {
    let mut map = FxHashMap::default();
    let mut insert_field = |key: &str, val: Value| {
        map.insert(Value::String(key.to_string().into()).get_hash(), val);
    };

    insert_field("title", info.title.clone().into_value());
    insert_field("artist", info.artist.clone().into_value());
    insert_field("series_name", info.series_name.clone().into_value());
    insert_field("season_index", info.season_index.into_value());
    insert_field("episode_index", info.episode_index.into_value());
    insert_field("is_paused", info.is_paused.into_value());
    insert_field("position_secs", info.position_secs.into_value());
    insert_field("duration_secs", info.duration_secs.into_value());

    Value::Object(Rc::new(RefCell::new(map)))
}

pub fn json_to_parts(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Bool(false),
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Double(f)
            } else {
                Value::Int(0)
            }
        }
        serde_json::Value::String(s) => Value::String(s.clone().into()),
        serde_json::Value::Array(arr) => {
            let mut map = FxHashMap::default();
            map.insert(
                Value::String("len".to_string().into()).get_hash(),
                Value::Int(arr.len() as i64),
            );
            for (i, val) in arr.iter().enumerate() {
                map.insert(Value::Int(i as i64).get_hash(), json_to_parts(val));
            }
            Value::Object(Rc::new(RefCell::new(map)))
        }
        serde_json::Value::Object(obj) => {
            let mut map = FxHashMap::default();
            for (k, val) in obj {
                let hash_key = if let Some(stripped) = k.strip_prefix("h_") {
                    stripped
                        .parse::<u64>()
                        .unwrap_or_else(|_| Value::String(k.clone().into()).get_hash())
                } else {
                    Value::String(k.clone().into()).get_hash()
                };
                map.insert(hash_key, json_to_parts(val));
            }
            Value::Object(Rc::new(RefCell::new(map)))
        }
    }
}

pub fn parts_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Int(i) => serde_json::Value::Number((*i).into()),
        Value::Double(d) => {
            if let Some(n) = serde_json::value::Number::from_f64(*d) {
                serde_json::Value::Number(n)
            } else {
                serde_json::Value::Null
            }
        }
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::String(s) | Value::Ref(s) => serde_json::Value::String((**s).clone()),
        Value::Object(obj_ref) => {
            let obj = obj_ref.borrow();
            let len_key = Value::String("len".to_string().into()).get_hash();
            if let Some(Value::Int(l)) = obj.get(&len_key) {
                let mut arr = Vec::new();
                for i in 0..*l {
                    let int_key = Value::Int(i).get_hash();
                    let str_key = Value::String(i.to_string().into()).get_hash();
                    if let Some(item) = obj.get(&int_key).or_else(|| obj.get(&str_key)) {
                        arr.push(parts_to_json(item));
                    } else {
                        arr.push(serde_json::Value::Null);
                    }
                }
                serde_json::Value::Array(arr)
            } else {
                let mut map = serde_json::Map::new();
                for (k, val) in obj.iter() {
                    map.insert(format!("h_{}", k), parts_to_json(val));
                }
                serde_json::Value::Object(map)
            }
        }
        _ => serde_json::Value::Null,
    }
}

// Native functions registered in the parts VM
#[parts_native]
fn parts_log(msg: Value) -> Result<bool, String> {
    eprintln!("[PartsVM] {}", msg);
    Ok(true)
}

#[parts_native]
fn parts_get_state(key: String) -> Result<Value, String> {
    let state = GLOBAL_STATE.read().unwrap();

    if let Some(json_val) = state.get(&key) {
        Ok(json_to_parts(json_val))
    } else {
        Ok(Value::Bool(false))
    }
}

#[parts_native]
fn parts_set_state(key: String, val: Value) -> Result<bool, String> {
    let json_val = parts_to_json(&val);
    let mut state = GLOBAL_STATE.write().unwrap();

    state.insert(key, json_val);
    Ok(true)
}

#[parts_native]
fn parts_player_play() -> Result<bool, String> {
    if let Some(ctrl) = get_controller() {
        ctrl.play();
        Ok(true)
    } else {
        Ok(false)
    }
}

#[parts_native]
fn parts_player_pause() -> Result<bool, String> {
    if let Some(ctrl) = get_controller() {
        ctrl.pause();
        Ok(true)
    } else {
        Ok(false)
    }
}

#[parts_native]
fn parts_player_toggle_pause() -> Result<bool, String> {
    if let Some(ctrl) = get_controller() {
        ctrl.toggle_pause();
        Ok(true)
    } else {
        Ok(false)
    }
}

#[parts_native]
fn parts_player_next() -> Result<bool, String> {
    if let Some(ctrl) = get_controller() {
        ctrl.next();
        Ok(true)
    } else {
        Ok(false)
    }
}

#[parts_native]
fn parts_player_previous() -> Result<bool, String> {
    if let Some(ctrl) = get_controller() {
        ctrl.previous();
        Ok(true)
    } else {
        Ok(false)
    }
}

#[parts_native]
fn parts_player_stop() -> Result<bool, String> {
    if let Some(ctrl) = get_controller() {
        ctrl.stop();
        Ok(true)
    } else {
        Ok(false)
    }
}

#[parts_native]
fn parts_player_seek(secs: i64) -> Result<bool, String> {
    if let Some(ctrl) = get_controller() {
        ctrl.seek(secs);
        Ok(true)
    } else {
        Ok(false)
    }
}

#[parts_native]
fn parts_http_get(url: String) -> Result<String, String> {
    if url.is_empty() {
        return Err("http_get requires a URL".to_string());
    }
    let client = reqwest::blocking::Client::new();
    let resp = client.get(&url).send().map_err(|e| e.to_string())?;
    resp.text().map_err(|e| e.to_string())
}

#[parts_native]
fn parts_http_post(url: String, body: String) -> Result<String, String> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(&url)
        .body(body)
        .send()
        .map_err(|e| e.to_string())?;
    resp.text().map_err(|e| e.to_string())
}

#[parts_native]
fn parts_http_request(
    method: String,
    url: String,
    headers_val: Value,
    body: String,
) -> Result<String, String> {
    let client = reqwest::blocking::Client::new();
    let method_parsed = match method.to_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        "PATCH" => reqwest::Method::PATCH,
        _ => return Err(format!("Unsupported method: {}", method)),
    };

    let mut req = client.request(method_parsed, url);
    if !body.is_empty() {
        req = req.body(body);
    }

    let headers_vec = Vec::<Value>::from_value(&headers_val).unwrap_or_default();
    for item in headers_vec {
        if let Value::Object(item_obj_ref) = item {
            let obj = item_obj_ref.borrow();
            let get_val = |key: &str| {
                let hash = Value::String(key.to_string().into()).get_hash();
                obj.get(&hash).cloned().unwrap_or(Value::Bool(false))
            };

            if let (Ok(k), Ok(v)) = (
                String::from_value(&get_val("key")),
                String::from_value(&get_val("value")),
            ) {
                req = req.header(k.as_str(), v.as_str());
            }
        }
    }

    let resp = req.send().map_err(|e| e.to_string())?;
    resp.text().map_err(|e| e.to_string())
}

#[parts_native]
fn parts_json_parse(json_str: String) -> Result<Value, String> {
    let v: serde_json::Value = serde_json::from_str(&json_str).map_err(|e| e.to_string())?;
    Ok(json_to_parts(&v))
}

#[parts_native]
fn parts_to_string(val: Value) -> Result<String, String> {
    Ok(format!("{}", val))
}

#[parts_native]
fn parts_get_config(key: String) -> Result<String, String> {
    Ok(CONFIG.with(|c| c.borrow().get(&key).cloned().unwrap_or_default()))
}

#[parts_native]
fn parts_vm_command(cmd: i64, arg: i64) -> Result<bool, String> {
    if let Ok(guard) = VM_COMMAND_DISPATCHER.lock()
        && let Some(handler) = guard.as_ref()
    {
        handler(cmd, arg)?;
        return Ok(true);
    }
    // Fallback directly to controller commands
    match cmd {
        1 => {
            // Play
            if let Some(ctrl) = get_controller() {
                ctrl.play();
                return Ok(true);
            }
        }
        2 => {
            // Pause
            if let Some(ctrl) = get_controller() {
                ctrl.pause();
                return Ok(true);
            }
        }
        3 => {
            // Toggle pause
            if let Some(ctrl) = get_controller() {
                ctrl.toggle_pause();
                return Ok(true);
            }
        }
        4 => {
            // Seek (arg in seconds or ms)
            if let Some(ctrl) = get_controller() {
                ctrl.seek(arg);
                return Ok(true);
            }
        }
        5 => {
            // Stop
            if let Some(ctrl) = get_controller() {
                ctrl.stop();
                return Ok(true);
            }
        }
        6 => {
            // Next
            if let Some(ctrl) = get_controller() {
                ctrl.next();
                return Ok(true);
            }
        }
        7 => {
            // Previous
            if let Some(ctrl) = get_controller() {
                ctrl.previous();
                return Ok(true);
            }
        }
        _ => return Err(format!("Unknown command {}", cmd)),
    }
    Ok(false)
}

#[parts_native]
fn parts_mutate_ui_slot(slot_id: i64, data_ptr: i64) -> Result<bool, String> {
    if let Ok(guard) = UI_SLOT_DISPATCHER.lock()
        && let Some(handler) = guard.as_ref()
    {
        handler(slot_id, data_ptr);
        return Ok(true);
    }
    Ok(false)
}

pub fn init_parts_natives() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        parts::std::register_extra_native("log", 1, parts_log);
        parts::std::register_extra_native("get_state", 1, parts_get_state);
        parts::std::register_extra_native("set_state", 2, parts_set_state);
        parts::std::register_extra_native("player_play", 0, parts_player_play);
        parts::std::register_extra_native("player_pause", 0, parts_player_pause);
        parts::std::register_extra_native("player_toggle_pause", 0, parts_player_toggle_pause);
        parts::std::register_extra_native("player_next", 0, parts_player_next);
        parts::std::register_extra_native("player_previous", 0, parts_player_previous);
        parts::std::register_extra_native("player_stop", 0, parts_player_stop);
        parts::std::register_extra_native("player_seek", 1, parts_player_seek);
        parts::std::register_extra_native("http_get", 1, parts_http_get);
        parts::std::register_extra_native("http_post", 2, parts_http_post);
        parts::std::register_extra_native("http_request", 4, parts_http_request);
        parts::std::register_extra_native("json_parse", 1, parts_json_parse);
        parts::std::register_extra_native("to_string", 1, parts_to_string);
        parts::std::register_extra_native("get_config", 1, parts_get_config);
        parts::std::register_extra_native("vm_command", 2, parts_vm_command);
        parts::std::register_extra_native("mutate_ui_slot", 2, parts_mutate_ui_slot);
    });
}

fn has_method(obj: &Value, method_name: &str) -> bool {
    if let Value::Object(obj_ref) = obj {
        let map = obj_ref.borrow();
        let method_hash = Value::String(method_name.to_string().into()).get_hash();
        map.contains_key(&method_hash)
    } else {
        false
    }
}

fn call_parts_method(
    obj: &Value,
    method_name: &str,
    args: Vec<Value>,
    constants: &[Value],
) -> Result<Value, String> {
    if let Value::Object(obj_ref) = obj {
        let map = obj_ref.borrow();
        let method_hash = Value::String(method_name.to_string().into()).get_hash();
        if let Some(method_val) = map.get(&method_hash) {
            match method_val.call(args, constants.to_vec()) {
                Ok(Some(res)) => Ok(res),
                Ok(None) => Ok(Value::Bool(false)),
                Err(e) => Err(format!("Error calling {}: {}", method_name, e)),
            }
        } else {
            Err(format!("Method {} not found on object", method_name))
        }
    } else {
        Err("Target is not an object".to_string())
    }
}

pub enum ScriptRequest {
    // Fast path zero-allocation updates
    TimeUpdate(i64),
    StateChanged(i64),
    VmCommand {
        cmd: i64,
        arg: i64,
        resp_tx: std::sync::mpsc::Sender<Result<(), String>>,
    },

    // Playback events
    OnPlaybackUpdate(PlaybackInfo),
    OnPlaybackStop,

    // Provider requests
    GetRoot {
        resp_tx: std::sync::mpsc::Sender<Result<Vec<MediaItem>, AmpError>>,
    },
    GetChildren {
        parent_id: String,
        resp_tx: std::sync::mpsc::Sender<Result<Vec<MediaItem>, AmpError>>,
    },
    GetNextUp {
        resp_tx: std::sync::mpsc::Sender<Result<Vec<MediaItem>, AmpError>>,
    },
    Search {
        query: String,
        resp_tx: std::sync::mpsc::Sender<Result<Vec<MediaItem>, AmpError>>,
    },
    GetStreamUrl {
        item_id: String,
        resp_tx: std::sync::mpsc::Sender<String>,
    },
    GetItemImageBuffer {
        item_id: String,
        resp_tx: std::sync::mpsc::Sender<Result<RawImage, AmpError>>,
    },
    GetResumePosition {
        item_id: String,
        resp_tx: std::sync::mpsc::Sender<Result<Option<i64>, AmpError>>,
    },
    ReportPlaybackStart {
        item_id: String,
        resp_tx: std::sync::mpsc::Sender<Result<(), AmpError>>,
    },
    ReportPlaybackProgress {
        item_id: String,
        position_secs: i64,
        is_paused: bool,
        resp_tx: std::sync::mpsc::Sender<Result<(), AmpError>>,
    },
    ReportPlaybackStopped {
        item_id: String,
        position_secs: i64,
        resp_tx: std::sync::mpsc::Sender<Result<(), AmpError>>,
    },
    MarkAsPlayed {
        item_id: String,
        played: bool,
        resp_tx: std::sync::mpsc::Sender<Result<(), AmpError>>,
    },
}

#[derive(Clone)]
pub struct LoadedScript {
    pub name: String,
    pub path: PathBuf,
    pub tx: std::sync::mpsc::Sender<ScriptRequest>,
}

fn spawn_script_worker(
    name: String,
    path: PathBuf,
    config: HashMap<String, String>,
    init_tx: std::sync::mpsc::Sender<Result<(), AmpError>>,
) -> std::sync::mpsc::Sender<ScriptRequest> {
    let (tx, rx) = std::sync::mpsc::channel::<ScriptRequest>();

    let mut script_config = HashMap::new();
    for (k, v) in &config {
        if k == "script_path" {
            script_config.insert(k.clone(), v.clone());
        } else if k.starts_with(&format!("{}::", name)) {
            let clean_key = &k[name.len() + 2..];
            script_config.insert(clean_key.to_string(), v.clone());
        } else if !k.contains("::") && name == "fallback" {
            script_config.insert(k.clone(), v.clone());
        }
    }

    CONFIG.with(|c| *c.borrow_mut() = script_config.clone());

    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            let _ = init_tx.send(Err(AmpError::Io(e)));
            return tx;
        }
    };

    let import_path = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    let init_tx_clone = init_tx.clone();
    std::thread::spawn(move || {
        CONFIG.with(|c| *c.borrow_mut() = script_config);
        init_parts_natives();

        let engine = Engine::with_import_path(import_path);

        let run_res = match engine.run(&source) {
            Ok(res) => res,
            Err(e) => {
                let _ =
                    init_tx_clone.send(Err(AmpError::Plugin(format!("Script run error: {}", e))));
                return;
            }
        };

        let script_obj = match run_res.value {
            Some(val) => val,
            None => {
                let _ =
                    init_tx_clone.send(Err(AmpError::Plugin("Script returned no value".into())));
                return;
            }
        };

        let constants = run_res.constants;

        if let Value::Object(ref_cell) = &script_obj {
            let obj = ref_cell.borrow();

            let get_val = |key: &str| {
                let hash = Value::String(key.to_string().into()).get_hash();
                obj.get(&hash).cloned()
            };

            if let Some(err_type_val) = get_val("error_type") {
                let err_msg = match get_val("message") {
                    Some(Value::String(s)) | Some(Value::Ref(s)) => (*s).clone(),
                    _ => "Unknown script error".to_string(),
                };
                let err_type = match err_type_val {
                    Value::String(s) | Value::Ref(s) => s.to_string(),
                    _ => "".to_string(),
                };
                let amp_err = match err_type.as_str() {
                    "Auth" => AmpError::Auth(err_msg),
                    "Plugin" => AmpError::Plugin(err_msg),
                    _ => AmpError::Provider(err_msg),
                };
                let _ = init_tx_clone.send(Err(amp_err));
                return;
            }
        }

        if init_tx_clone.send(Ok(())).is_err() {
            return;
        }

        macro_rules! handle_req {
            ($method:expr, $args:expr, $resp_tx:ident, $mapper:expr, $fallback:expr) => {
                let res = if has_method(&script_obj, $method) {
                    call_parts_method(&script_obj, $method, $args, &constants)
                        .map_err(AmpError::Provider)
                        .and_then($mapper)
                } else {
                    $fallback
                };
                let _ = $resp_tx.send(res);
            };
        }

        macro_rules! handle_infallible {
            ($method:expr, $args:expr, $resp_tx:ident, $mapper:expr, $fallback:expr) => {
                let res = if has_method(&script_obj, $method) {
                    match call_parts_method(&script_obj, $method, $args, &constants) {
                        Ok(val) => $mapper(val),
                        Err(_) => $fallback,
                    }
                } else {
                    $fallback
                };
                let _ = $resp_tx.send(res);
            };
        }

        while let Ok(req) = rx.recv() {
            match req {
                ScriptRequest::TimeUpdate(time_ms) => {
                    if has_method(&script_obj, "on_time_update") {
                        let _ = call_parts_method(
                            &script_obj,
                            "on_time_update",
                            vec![Value::Int(time_ms)],
                            &constants,
                        );
                    }
                }
                ScriptRequest::StateChanged(state) => {
                    if has_method(&script_obj, "on_state_changed") {
                        let _ = call_parts_method(
                            &script_obj,
                            "on_state_changed",
                            vec![Value::Int(state)],
                            &constants,
                        );
                    }
                    if state == 1 && has_method(&script_obj, "on_play") {
                        let _ = call_parts_method(&script_obj, "on_play", vec![], &constants);
                    } else if state == 0 && has_method(&script_obj, "on_pause") {
                        let _ = call_parts_method(&script_obj, "on_pause", vec![], &constants);
                    } else if state == 3 && has_method(&script_obj, "on_stop") {
                        let _ = call_parts_method(&script_obj, "on_stop", vec![], &constants);
                    }
                }
                ScriptRequest::VmCommand { cmd, arg, resp_tx } => {
                    let res = if has_method(&script_obj, "on_vm_command") {
                        call_parts_method(
                            &script_obj,
                            "on_vm_command",
                            vec![Value::Int(cmd), Value::Int(arg)],
                            &constants,
                        )
                        .map(|_| ())
                    } else {
                        Ok(())
                    };
                    let _ = resp_tx.send(res);
                }
                ScriptRequest::OnPlaybackUpdate(info) => {
                    if has_method(&script_obj, "on_playback_update") {
                        let _ = call_parts_method(
                            &script_obj,
                            "on_playback_update",
                            vec![playback_info_to_parts(info)],
                            &constants,
                        );
                    }
                }
                ScriptRequest::OnPlaybackStop => {
                    if has_method(&script_obj, "on_playback_stop") {
                        let _ =
                            call_parts_method(&script_obj, "on_playback_stop", vec![], &constants);
                    }
                }
                ScriptRequest::GetRoot { resp_tx } => {
                    handle_req!(
                        "get_root",
                        vec![],
                        resp_tx,
                        |val| parts_to_media_items(&val),
                        Ok(vec![])
                    );
                }
                ScriptRequest::GetChildren { parent_id, resp_tx } => {
                    handle_req!(
                        "get_children",
                        vec![parent_id.into_value()],
                        resp_tx,
                        |val| parts_to_media_items(&val),
                        Ok(vec![])
                    );
                }
                ScriptRequest::GetNextUp { resp_tx } => {
                    handle_req!(
                        "get_next_up",
                        vec![],
                        resp_tx,
                        |val| parts_to_media_items(&val),
                        Ok(vec![])
                    );
                }
                ScriptRequest::Search { query, resp_tx } => {
                    handle_req!(
                        "search",
                        vec![query.into_value()],
                        resp_tx,
                        |val| parts_to_media_items(&val),
                        Ok(vec![])
                    );
                }
                ScriptRequest::GetStreamUrl { item_id, resp_tx } => {
                    handle_infallible!(
                        "get_stream_url",
                        vec![item_id.into_value()],
                        resp_tx,
                        |val| {
                            match val {
                                Value::String(s) | Value::Ref(s) => (*s).clone(),
                                _ => String::new(),
                            }
                        },
                        String::new()
                    );
                }
                ScriptRequest::GetItemImageBuffer { item_id, resp_tx } => {
                    let res = if has_method(&script_obj, "get_item_image_buffer") {
                        call_parts_method(
                            &script_obj,
                            "get_item_image_buffer",
                            vec![Value::String(item_id.into())],
                            &constants,
                        )
                        .map_err(AmpError::Provider)
                        .and_then(|val| parts_to_raw_image(&val))
                    } else {
                        Err(AmpError::Provider("Not supported".into()))
                    };
                    let _ = resp_tx.send(res);
                }
                ScriptRequest::GetResumePosition { item_id, resp_tx } => {
                    handle_req!(
                        "get_resume_position",
                        vec![item_id.into_value()],
                        resp_tx,
                        |val| {
                            match val {
                                Value::Int(i) => Ok(Some(i)),
                                Value::Double(d) => Ok(Some(d as i64)),
                                _ => Ok(None),
                            }
                        },
                        Ok(None)
                    );
                }
                ScriptRequest::ReportPlaybackStart { item_id, resp_tx } => {
                    handle_req!(
                        "report_playback_start",
                        vec![item_id.into_value()],
                        resp_tx,
                        |_| Ok(()),
                        Ok(())
                    );
                }
                ScriptRequest::ReportPlaybackProgress {
                    item_id,
                    position_secs,
                    is_paused,
                    resp_tx,
                } => {
                    handle_req!(
                        "report_playback_progress",
                        vec![
                            item_id.into_value(),
                            position_secs.into_value(),
                            is_paused.into_value()
                        ],
                        resp_tx,
                        |_| Ok(()),
                        Ok(())
                    );
                }
                ScriptRequest::ReportPlaybackStopped {
                    item_id,
                    position_secs,
                    resp_tx,
                } => {
                    handle_req!(
                        "report_playback_stopped",
                        vec![item_id.into_value(), position_secs.into_value()],
                        resp_tx,
                        |_| Ok(()),
                        Ok(())
                    );
                }
                ScriptRequest::MarkAsPlayed {
                    item_id,
                    played,
                    resp_tx,
                } => {
                    handle_req!(
                        "mark_as_played",
                        vec![item_id.into_value(), played.into_value()],
                        resp_tx,
                        |_| Ok(()),
                        Ok(())
                    );
                }
            }
        }
    });

    tx
}

pub fn get_parts_dir() -> Option<PathBuf> {
    if std::env::var("TESTING").ok() == Some("true".to_string()) {
        return None;
    }
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home)
        .join(".config")
        .join("amp")
        .join("plugins")
        .join("parts");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

pub fn discover_parts_scripts() -> Vec<(String, PathBuf)> {
    let mut scripts = Vec::new();
    if let Some(dir) = get_parts_dir()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().and_then(|s| s.to_str()) == Some("pts")
                && let Some(name) = path.file_stem().and_then(|s| s.to_str())
            {
                scripts.push((name.to_string(), path));
            }
        }
    }
    scripts
}

pub struct PartsExtensionBridge {
    scripts: Arc<RwLock<Vec<LoadedScript>>>,
    config: HashMap<String, String>,
}

impl PartsExtensionBridge {
    pub fn new() -> Self {
        init_parts_natives();
        Self {
            scripts: Arc::new(RwLock::new(Vec::new())),
            config: HashMap::new(),
        }
    }

    pub fn with_config(config: HashMap<String, String>) -> Self {
        init_parts_natives();
        Self {
            scripts: Arc::new(RwLock::new(Vec::new())),
            config,
        }
    }

    pub fn load_scripts(&self) -> Result<(), ExtensionError> {
        let mut scripts_to_load = Vec::new();

        let script_path = self.config.get("script_path").cloned().unwrap_or_default();
        if !script_path.is_empty() {
            scripts_to_load.push(("fallback".to_string(), PathBuf::from(script_path)));
        }

        for (name, path) in discover_parts_scripts() {
            scripts_to_load.push((name, path));
        }

        let mut loaded = Vec::new();
        for (name, path) in scripts_to_load {
            let (init_tx, init_rx) = std::sync::mpsc::channel();
            let tx = spawn_script_worker(name.clone(), path.clone(), self.config.clone(), init_tx);
            if let Ok(Ok(())) = init_rx.recv() {
                loaded.push(LoadedScript { name, path, tx });
            }
        }

        let mut guard = self.scripts.write().unwrap();
        *guard = loaded;
        Ok(())
    }

    pub fn load_single_script(&self, name: &str, path: PathBuf) -> Result<(), ExtensionError> {
        let (init_tx, init_rx) = std::sync::mpsc::channel();
        let tx = spawn_script_worker(name.to_string(), path.clone(), self.config.clone(), init_tx);
        match init_rx.recv() {
            Ok(Ok(())) => {
                let mut guard = self.scripts.write().unwrap();
                guard.retain(|s| s.name != name && s.path != path);
                guard.push(LoadedScript {
                    name: name.to_string(),
                    path,
                    tx,
                });
                Ok(())
            }
            Ok(Err(e)) => Err(ExtensionError::VmError(format!("{:?}", e))),
            Err(_) => Err(ExtensionError::VmError("Worker thread died".into())),
        }
    }

    pub fn notify_playback_update(&self, info: PlaybackInfo) {
        let scripts = self.scripts.read().unwrap().clone();
        for script in scripts {
            let _ = script
                .tx
                .send(ScriptRequest::OnPlaybackUpdate(info.clone()));
        }
    }

    pub fn notify_playback_stop(&self) {
        let scripts = self.scripts.read().unwrap().clone();
        for script in scripts {
            let _ = script.tx.send(ScriptRequest::OnPlaybackStop);
        }
    }
}

impl Default for PartsExtensionBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionBridge for PartsExtensionBridge {
    fn notify_time_update(&self, time_ms: i64) {
        let scripts = self.scripts.read().unwrap().clone();
        for script in scripts {
            let _ = script.tx.send(ScriptRequest::TimeUpdate(time_ms));
        }
    }

    fn notify_state_changed(&self, state: i64) {
        let scripts = self.scripts.read().unwrap().clone();
        for script in scripts {
            let _ = script.tx.send(ScriptRequest::StateChanged(state));
        }
    }

    fn vm_command(&mut self, cmd: i64, arg: i64) -> Result<(), ExtensionError> {
        let scripts = self.scripts.read().unwrap().clone();
        for script in scripts {
            let (resp_tx, resp_rx) = std::sync::mpsc::channel();
            if script
                .tx
                .send(ScriptRequest::VmCommand { cmd, arg, resp_tx })
                .is_ok()
                && let Ok(Err(e)) = resp_rx.recv()
            {
                return Err(ExtensionError::VmError(e));
            }
        }
        Ok(())
    }

    fn mutate_ui_slot(&self, slot_id: i64, data_ptr: i64) {
        if let Ok(guard) = UI_SLOT_DISPATCHER.lock()
            && let Some(handler) = guard.as_ref()
        {
            handler(slot_id, data_ptr);
        }
    }

    fn hot_reload(&mut self, script_path: &str) -> Result<(), ExtensionError> {
        let path = PathBuf::from(script_path);
        if !path.exists() {
            return Err(ExtensionError::NotFound(script_path.to_string()));
        }

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("script")
            .to_string();

        eprintln!(
            "[PartsBridge] Hot-reloading extension script: {} ({})",
            name, script_path
        );
        self.load_single_script(&name, path)
    }
}

// Implement MediaProvider for PartsExtensionBridge so .pts scripts can serve as media sources
#[async_trait]
impl MediaProvider for PartsExtensionBridge {
    async fn get_root(&self) -> Result<Vec<MediaItem>, AmpError> {
        let scripts = self.scripts.read().unwrap().clone();
        let mut all_items = Vec::new();
        for script in scripts {
            let (resp_tx, resp_rx) = std::sync::mpsc::channel();
            if script.tx.send(ScriptRequest::GetRoot { resp_tx }).is_ok()
                && let Ok(Ok(items)) = resp_rx.recv()
            {
                for mut item in items {
                    if script.name != "fallback" {
                        item.id = format!("{}::{}", script.name, item.id);
                    }
                    all_items.push(item);
                }
            }
        }
        Ok(all_items)
    }

    async fn get_children(&self, parent_id: &str) -> Result<Vec<MediaItem>, AmpError> {
        let scripts = self.scripts.read().unwrap().clone();
        for script in &scripts {
            let mut id = parent_id.to_string();
            if let Some(idx) = parent_id.find("::") {
                let name = &parent_id[..idx];
                if script.name == name {
                    id = parent_id[idx + 2..].to_string();
                } else {
                    continue;
                }
            }

            let (resp_tx, resp_rx) = std::sync::mpsc::channel();
            if script
                .tx
                .send(ScriptRequest::GetChildren {
                    parent_id: id,
                    resp_tx,
                })
                .is_ok()
                && let Ok(res) = resp_rx.recv()
            {
                return res.map(|items| {
                    items
                        .into_iter()
                        .map(|mut item| {
                            if script.name != "fallback" {
                                item.id = format!("{}::{}", script.name, item.id);
                            }
                            item
                        })
                        .collect()
                });
            }
        }
        Ok(Vec::new())
    }

    async fn get_next_up(&self) -> Result<Vec<MediaItem>, AmpError> {
        let scripts = self.scripts.read().unwrap().clone();
        let mut all_items = Vec::new();
        for script in scripts {
            let (resp_tx, resp_rx) = std::sync::mpsc::channel();
            if script.tx.send(ScriptRequest::GetNextUp { resp_tx }).is_ok()
                && let Ok(Ok(items)) = resp_rx.recv()
            {
                for mut item in items {
                    if script.name != "fallback" {
                        item.id = format!("{}::{}", script.name, item.id);
                    }
                    all_items.push(item);
                }
            }
        }
        Ok(all_items)
    }

    async fn search(&self, query: &str) -> Result<Vec<MediaItem>, AmpError> {
        let scripts = self.scripts.read().unwrap().clone();
        let mut all_items = Vec::new();
        for script in scripts {
            let (resp_tx, resp_rx) = std::sync::mpsc::channel();
            if script
                .tx
                .send(ScriptRequest::Search {
                    query: query.to_string(),
                    resp_tx,
                })
                .is_ok()
                && let Ok(Ok(items)) = resp_rx.recv()
            {
                for mut item in items {
                    if script.name != "fallback" {
                        item.id = format!("{}::{}", script.name, item.id);
                    }
                    all_items.push(item);
                }
            }
        }
        Ok(all_items)
    }

    fn get_stream_url(&self, item_id: &str) -> String {
        let scripts = self.scripts.read().unwrap().clone();
        for script in &scripts {
            let mut id = item_id.to_string();
            if let Some(idx) = item_id.find("::") {
                let name = &item_id[..idx];
                if script.name == name {
                    id = item_id[idx + 2..].to_string();
                } else {
                    continue;
                }
            }

            let (resp_tx, resp_rx) = std::sync::mpsc::channel();
            if script
                .tx
                .send(ScriptRequest::GetStreamUrl {
                    item_id: id,
                    resp_tx,
                })
                .is_ok()
                && let Ok(url) = resp_rx.recv()
                && !url.is_empty()
            {
                return url;
            }
        }
        String::new()
    }

    async fn get_item_image_buffer(&self, item_id: &str) -> Result<RawImage, AmpError> {
        let scripts = self.scripts.read().unwrap().clone();
        for script in &scripts {
            let mut id = item_id.to_string();
            if let Some(idx) = item_id.find("::") {
                let name = &item_id[..idx];
                if script.name == name {
                    id = item_id[idx + 2..].to_string();
                } else {
                    continue;
                }
            }

            let (resp_tx, resp_rx) = std::sync::mpsc::channel();
            if script
                .tx
                .send(ScriptRequest::GetItemImageBuffer {
                    item_id: id,
                    resp_tx,
                })
                .is_ok()
                && let Ok(res) = resp_rx.recv()
            {
                return res;
            }
        }
        Err(AmpError::Provider("Image buffer not available".into()))
    }

    fn get_persistable_config(&self) -> HashMap<String, String> {
        self.config.clone()
    }

    async fn get_resume_position(&self, item_id: &str) -> Result<Option<i64>, AmpError> {
        let scripts = self.scripts.read().unwrap().clone();
        for script in &scripts {
            let mut id = item_id.to_string();
            if let Some(idx) = item_id.find("::") {
                let name = &item_id[..idx];
                if script.name == name {
                    id = item_id[idx + 2..].to_string();
                } else {
                    continue;
                }
            }

            let (resp_tx, resp_rx) = std::sync::mpsc::channel();
            if script
                .tx
                .send(ScriptRequest::GetResumePosition {
                    item_id: id,
                    resp_tx,
                })
                .is_ok()
                && let Ok(res) = resp_rx.recv()
            {
                return res;
            }
        }
        Ok(None)
    }

    async fn report_playback_start(&self, item_id: &str) -> Result<(), AmpError> {
        let scripts = self.scripts.read().unwrap().clone();
        for script in &scripts {
            let mut id = item_id.to_string();
            if let Some(idx) = item_id.find("::") {
                let name = &item_id[..idx];
                if script.name == name {
                    id = item_id[idx + 2..].to_string();
                } else {
                    continue;
                }
            }

            let (resp_tx, resp_rx) = std::sync::mpsc::channel();
            if script
                .tx
                .send(ScriptRequest::ReportPlaybackStart {
                    item_id: id,
                    resp_tx,
                })
                .is_ok()
            {
                let _ = resp_rx.recv();
            }
        }
        Ok(())
    }

    async fn report_playback_progress(
        &self,
        item_id: &str,
        position_secs: i64,
        is_paused: bool,
    ) -> Result<(), AmpError> {
        let scripts = self.scripts.read().unwrap().clone();
        for script in &scripts {
            let mut id = item_id.to_string();
            if let Some(idx) = item_id.find("::") {
                let name = &item_id[..idx];
                if script.name == name {
                    id = item_id[idx + 2..].to_string();
                } else {
                    continue;
                }
            }

            let (resp_tx, resp_rx) = std::sync::mpsc::channel();
            if script
                .tx
                .send(ScriptRequest::ReportPlaybackProgress {
                    item_id: id,
                    position_secs,
                    is_paused,
                    resp_tx,
                })
                .is_ok()
            {
                let _ = resp_rx.recv();
            }
        }
        Ok(())
    }

    async fn report_playback_stopped(
        &self,
        item_id: &str,
        position_secs: i64,
    ) -> Result<(), AmpError> {
        let scripts = self.scripts.read().unwrap().clone();
        for script in &scripts {
            let mut id = item_id.to_string();
            if let Some(idx) = item_id.find("::") {
                let name = &item_id[..idx];
                if script.name == name {
                    id = item_id[idx + 2..].to_string();
                } else {
                    continue;
                }
            }

            let (resp_tx, resp_rx) = std::sync::mpsc::channel();
            if script
                .tx
                .send(ScriptRequest::ReportPlaybackStopped {
                    item_id: id,
                    position_secs,
                    resp_tx,
                })
                .is_ok()
            {
                let _ = resp_rx.recv();
            }
        }
        Ok(())
    }

    async fn mark_as_played(&self, item_id: &str, played: bool) -> Result<(), AmpError> {
        let scripts = self.scripts.read().unwrap().clone();
        for script in &scripts {
            let mut id = item_id.to_string();
            if let Some(idx) = item_id.find("::") {
                let name = &item_id[..idx];
                if script.name == name {
                    id = item_id[idx + 2..].to_string();
                } else {
                    continue;
                }
            }

            let (resp_tx, resp_rx) = std::sync::mpsc::channel();
            if script
                .tx
                .send(ScriptRequest::MarkAsPlayed {
                    item_id: id,
                    played,
                    resp_tx,
                })
                .is_ok()
            {
                let _ = resp_rx.recv();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    struct DummyController {
        played: AtomicI64,
        paused: AtomicI64,
        seek_val: AtomicI64,
    }

    impl PlaybackController for DummyController {
        fn play(&self) {
            self.played.fetch_add(1, Ordering::SeqCst);
        }
        fn pause(&self) {
            self.paused.fetch_add(1, Ordering::SeqCst);
        }
        fn toggle_pause(&self) {}
        fn next(&self) {}
        fn previous(&self) {}
        fn stop(&self) {}
        fn seek(&self, position_secs: i64) {
            self.seek_val.store(position_secs, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_extension_bridge_lifecycle_and_commands() {
        let dir = std::env::temp_dir();
        let script_file = dir.join("test_extension_bridge.pts");

        let script_content = r#"
            return |>
                on_time_update: fun(time_ms) {
                    set_state("last_time_ms", time_ms);
                },
                on_state_changed: fun(state) {
                    set_state("last_state", state);
                },
                on_play: fun() {
                    set_state("play_hook_called", true);
                },
                on_vm_command: fun(cmd, arg) {
                    set_state("last_vm_cmd", cmd);
                    set_state("last_vm_arg", arg);
                },
                get_root: fun() {
                    return |>
                        0: |>
                            id: "item_1",
                            name: "Test Stream",
                            item_type: "Playable",
                            duration_secs: 300
                        <|,
                        len: 1
                    <|;
                }
            <|;
        "#;

        std::fs::write(&script_file, script_content).unwrap();

        let mut bridge = PartsExtensionBridge::new();
        let load_res = bridge.load_single_script("test_bridge", script_file.clone());
        assert!(
            load_res.is_ok(),
            "Failed to load script: {:?}",
            load_res.err()
        );

        // 1. Test fast time updates (zero-allocation hot path)
        bridge.notify_time_update(12345);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let time_val =
            parts_get_state(&[Value::String("last_time_ms".to_string().into())]).unwrap();
        assert_eq!(time_val, Value::Int(12345));

        // 2. Test state changes
        bridge.notify_state_changed(1);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let state_val =
            parts_get_state(&[Value::String("last_state".to_string().into())]).unwrap();
        assert_eq!(state_val, Value::Int(1));

        let play_hook =
            parts_get_state(&[Value::String("play_hook_called".to_string().into())]).unwrap();
        assert_eq!(play_hook, Value::Bool(true));
        // 3. Test VM command dispatch
        let cmd_res = bridge.vm_command(42, 9999);
        assert!(cmd_res.is_ok());
        std::thread::sleep(std::time::Duration::from_millis(50));

        let last_cmd =
            parts_get_state(&[Value::String("last_vm_cmd".to_string().into())]).unwrap();
        let last_arg =
            parts_get_state(&[Value::String("last_vm_arg".to_string().into())]).unwrap();
        assert_eq!(last_cmd, Value::Int(42));
        assert_eq!(last_arg, Value::Int(9999));

        // 4. Test UI slot mutation
        let slot_received = Arc::new(AtomicI64::new(0));
        let slot_received_clone = slot_received.clone();
        set_ui_slot_handler(move |slot_id, data_ptr| {
            slot_received_clone.store(slot_id + data_ptr, Ordering::SeqCst);
        });
        bridge.mutate_ui_slot(100, 25);
        assert_eq!(slot_received.load(Ordering::SeqCst), 125);

        // 5. Test MediaProvider get_root items
        let root_items = futures::executor::block_on(bridge.get_root()).unwrap();
        assert_eq!(root_items.len(), 1);
        assert_eq!(root_items[0].name, "Test Stream");

        // 6. Test Hot-Reload
        let new_content = r#"
            return |>
                on_time_update: fun(time_ms) {
                    set_state("last_time_ms", time_ms * 2);
                }
            <|;
        "#;
        std::fs::write(&script_file, new_content).unwrap();
        let reload_res = bridge.hot_reload(&script_file.to_string_lossy());
        assert!(
            reload_res.is_ok(),
            "Hot reload failed: {:?}",
            reload_res.err()
        );

        bridge.notify_time_update(100);
        std::thread::sleep(std::time::Duration::from_millis(50));

        let reloaded_time =
            parts_get_state(&[Value::String("last_time_ms".to_string().into())]).unwrap();
        assert_eq!(reloaded_time, Value::Int(200));

        let _ = std::fs::remove_file(script_file);
    }
}
