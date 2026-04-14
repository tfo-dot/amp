// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

use std::ffi::{CString};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::os::raw::{c_char, c_int, c_void};
use libmpv_sys::*;
use slint::{Image, SharedPixelBuffer, Rgba8Pixel};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use directories::ProjectDirs;

// Jellyfin Models
#[derive(Deserialize, Debug, Clone)]
struct JellyfinItem {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "RunTimeTicks")]
    run_time_ticks: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct LoginCache {
    url: String,
    access_token: String,
}

impl LoginCache {
    fn config_file() -> Option<PathBuf> {
        ProjectDirs::from("com", "amp", "AMP")
            .map(|proj_dirs| proj_dirs.config_dir().join("login.json"))
    }

    fn load() -> Option<Self> {
        let file = Self::config_file()?;
        let data = std::fs::read_to_string(file).ok()?;
        serde_json::from_str(&data).ok()
    }

    fn save(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let file = Self::config_file().ok_or("Could not find config directory")?;
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string(self)?;
        std::fs::write(file, data)?;
        Ok(())
    }

    fn delete() {
        if let Some(file) = Self::config_file() {
            let _ = std::fs::remove_file(file);
        }
    }
}

#[derive(Deserialize, Debug)]
struct JellyfinItemsResponse {
    #[serde(rename = "Items")]
    items: Vec<JellyfinItem>,
}

#[derive(Deserialize, Debug, Clone)]
struct AuthenticateResponse {
    #[serde(rename = "AccessToken")]
    access_token: String,
}

#[derive(Deserialize, Debug, Clone)]
struct DiscoveryResult {
    #[serde(rename = "Address")]
    address: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Id")]
    id: String,
}

#[derive(Deserialize, Debug, Clone)]
struct PublicUserResponse {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "HasPassword")]
    has_password: bool,
    #[serde(rename = "PrimaryImageTag")]
    primary_image_tag: Option<String>,
}

#[derive(Clone)]
struct JellyfinClient {
    url: String,
    api_key: String, // This will be the AccessToken after login
    client: reqwest::Client,
}

impl JellyfinClient {
    fn from_token(url: String, access_token: String) -> Self {
        Self {
            url,
            api_key: access_token,
            client: reqwest::Client::new(),
        }
    }

    async fn discover() -> Result<Vec<DiscoveryResult>, Box<dyn std::error::Error + Send + Sync>> {
        use tokio::net::UdpSocket;
        use tokio::time::{timeout, Duration};
        use std::net::SocketAddr;

        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.set_broadcast(true)?;
        
        let broadcast_addr: SocketAddr = "255.255.255.255:7359".parse()?;
        let msg = b"who is JellyfinServer?";
        socket.send_to(msg, broadcast_addr).await?;

        let mut results = Vec::new();
        let mut buf = [0u8; 1024];

        // Wait for responses for up to 2 seconds
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            match timeout(Duration::from_millis(500), socket.recv_from(&mut buf)).await {
                Ok(Ok((len, _addr))) => {
                    if let Ok(res) = serde_json::from_slice::<DiscoveryResult>(&buf[..len]) {
                        if !results.iter().any(|r: &DiscoveryResult| r.id == res.id) {
                            results.push(res);
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(results)
    }

    async fn get_public_users(url: &str) -> Result<Vec<PublicUserResponse>, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::Client::new();
        let users_url = format!("{}/Users/Public", url);
        let resp = client.get(users_url).send().await?.json::<Vec<PublicUserResponse>>().await?;
        Ok(resp)
    }

    async fn get_public_image_buffer(url: &str, item_id: &str, image_tag: &str) -> Result<SharedPixelBuffer<Rgba8Pixel>, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::Client::new();
        let image_url = format!("{}/Items/{}/Images/Primary?tag={}&maxWidth=100", url, item_id, image_tag);
        let resp = client.get(image_url).send().await?.bytes().await?;
        
        let img = image::load_from_memory(&resp)?;
        let rgba = img.to_rgba8();
        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height());
        Ok(buffer)
    }

    async fn authenticate(url: String, user: String, pass: String) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::Client::new();
        let auth_url = format!("{}/Users/AuthenticateByName", url);
        let auth_header = "MediaBrowser Client=\"AMP\", Device=\"Linux\", DeviceId=\"AMP-CLI\", Version=\"0.1.0\"";
        
        let mut body = std::collections::HashMap::new();
        body.insert("Username", user);
        body.insert("Pw", pass);

        let resp = client.post(auth_url)
            .header("X-Emby-Authorization", auth_header)
            .json(&body)
            .send().await.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
            .json::<AuthenticateResponse>().await.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        let client = Self {
            url: url.clone(),
            api_key: resp.access_token.clone(),
            client,
        };

        // Save login cache
        let cache = LoginCache {
            url: url.clone(),
            access_token: resp.access_token.clone(),
        };
        let _ = cache.save();

        Ok(client)
    }

    async fn get_series(&self) -> Result<Vec<JellyfinItem>, reqwest::Error> {
        let url = format!("{}/Items?IncludeItemTypes=Series&Recursive=true&Fields=PrimaryImageAspectRatio", self.url);
        let resp = self.client.get(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .send().await?.json::<JellyfinItemsResponse>().await?;
        Ok(resp.items)
    }

    async fn get_episodes(&self, series_id: &str) -> Result<Vec<JellyfinItem>, reqwest::Error> {
        let url = format!("{}/Shows/{}/Episodes", self.url, series_id);
        let resp = self.client.get(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .send().await?.json::<JellyfinItemsResponse>().await?;
        Ok(resp.items)
    }

    fn get_stream_url(&self, item_id: &str) -> String {
        format!("{}/Videos/{}/stream?api_key={}&Static=true", self.url, item_id, self.api_key)
    }

    fn get_auth_header(&self) -> String {
        format!("MediaBrowser Client=\"AMP\", Device=\"Linux\", DeviceId=\"AMP-CLI\", Version=\"0.1.0\", Token=\"{}\"", self.api_key)
    }

    async fn get_item_image_buffer(&self, item_id: &str) -> Result<SharedPixelBuffer<Rgba8Pixel>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/Items/{}/Images/Primary?maxWidth=200", self.url, item_id);
        let resp = self.client.get(url)
            .header("X-Emby-Authorization", self.get_auth_header())
            .send().await?.bytes().await?;
        
        let img = image::load_from_memory(&resp)?;
        let rgba = img.to_rgba8();
        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height());
        Ok(buffer)
    }
}

// Manually define missing SW render param types if they are not in the current libmpv-sys
const MPV_RENDER_PARAM_SW_SIZE: mpv_render_param_type = 17;
const MPV_RENDER_PARAM_SW_FORMAT: mpv_render_param_type = 18;
const MPV_RENDER_PARAM_SW_STRIDE: mpv_render_param_type = 19;
const MPV_RENDER_PARAM_SW_POINTER: mpv_render_param_type = 20;

// MpvHandle wrapper to allow passing the raw pointer to Slint callbacks.
#[derive(Clone, Copy)]
struct MpvHandle(*mut mpv_handle);
unsafe impl Send for MpvHandle {}
unsafe impl Sync for MpvHandle {}

// MpvRenderContext wrapper
#[derive(Clone, Copy)]
struct MpvRenderCtx(*mut mpv_render_context);
unsafe impl Send for MpvRenderCtx {}
unsafe impl Sync for MpvRenderCtx {}

fn format_time(seconds: f64) -> String {
    let total_seconds = seconds as i64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let secs = total_seconds % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{:02}:{:02}", minutes, secs)
    }
}

fn ticks_to_duration(ticks: Option<i64>) -> String {
    match ticks {
        Some(t) => {
            let seconds = t / 10_000_000;
            format_time(seconds as f64)
        }
        None => "--:--".into(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 0. Ensure MPV-compatible locale for numeric parsing
    std::env::set_var("LC_NUMERIC", "C");
    unsafe {
        let c_locale = CString::new("C").unwrap();
        libc::setlocale(libc::LC_ALL, c_locale.as_ptr());
        libc::setlocale(libc::LC_NUMERIC, c_locale.as_ptr());
    }

    let ui = PlayerWindow::new()?;

    // Jellyfin Client (Shared across callbacks)
    let jf_client: Arc<Mutex<Option<JellyfinClient>>> = Arc::new(Mutex::new(None));
    let current_series_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let current_episode_ids = Arc::new(Mutex::new(Vec::<String>::new()));

    // 1. Setup MPV Handle
    let mpv = unsafe {
        let handle = mpv_create();
        if handle.is_null() {
            panic!("Failed to create mpv context");
        }
        
        let c_vo = CString::new("vo").unwrap();
        let c_libmpv = CString::new("libmpv").unwrap();
        mpv_set_property_string(handle, c_vo.as_ptr(), c_libmpv.as_ptr());

        let c_osd = CString::new("osd-level").unwrap();
        let mut zero: i64 = 0;
        mpv_set_property(handle, c_osd.as_ptr(), mpv_format_MPV_FORMAT_INT64, &mut zero as *mut _ as *mut c_void);

        if mpv_initialize(handle) < 0 {
            panic!("Failed to initialize mpv context");
        }
        MpvHandle(handle)
    };
    
    let render_ctx = unsafe {
        let api_type = CString::new("sw").unwrap();
        let mut params = [
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                data: api_type.as_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: 0, // End of list
                data: ptr::null_mut(),
            },
        ];

        let mut ctx: *mut mpv_render_context = ptr::null_mut();
        let res = mpv_render_context_create(&mut ctx, mpv.0, params.as_mut_ptr());
        if res < 0 || ctx.is_null() {
            panic!("Failed to create mpv render context: {}", res);
        }
        MpvRenderCtx(ctx)
    };

    // --- Login Callback ---
    let ui_login = ui.as_weak();
    let jf_login = jf_client.clone();
    let s_ids_login = current_series_ids.clone();

    async fn populate_series_ui(
        client: JellyfinClient,
        ui_weak: slint::Weak<PlayerWindow>,
        jf_arc: Arc<Mutex<Option<JellyfinClient>>>,
        s_ids_arc: Arc<Mutex<Vec<String>>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let series = client.get_series().await?;
        let mut fetch_futures = Vec::new();
        for s in series {
            let client_clone = client.clone();
            let s_id = s.id.clone();
            let s_name = s.name.clone();
            fetch_futures.push(async move {
                let buffer = client_clone.get_item_image_buffer(&s_id).await.ok();
                (s_id, s_name, buffer)
            });
        }
        
        let series_results = futures::future::join_all(fetch_futures).await;
        
        let mut jf_lock = jf_arc.lock().unwrap();
        *jf_lock = Some(client);

        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut series_ids = Vec::new();
                let mut series_list_data = Vec::new();
                for (id, name, buffer) in series_results {
                    series_ids.push(id);
                    series_list_data.push(Series {
                        name: name.into(),
                        thumbnail: buffer.map(Image::from_rgba8).unwrap_or_else(Image::default),
                    });
                }

                let mut s_ids_lock = s_ids_arc.lock().unwrap();
                *s_ids_lock = series_ids;

                let series_model = std::rc::Rc::new(slint::VecModel::from(series_list_data));
                ui.set_series_list(slint::ModelRc::from(series_model));
                ui.set_current_screen("series".into());
                ui.set_is_loading(false);
            }
        });
        Ok(())
    }

    let ui_disc = ui.as_weak();
    ui.on_discover_servers(move || {
        if let Some(ui) = ui_disc.upgrade() {
            ui.set_is_loading(true);
            ui.set_is_loading_discovery(true);
            let ui_weak = ui_disc.clone();
            tokio::spawn(async move {
                match JellyfinClient::discover().await {
                    Ok(servers) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                let disc_servers: Vec<DiscoveredServer> = servers.into_iter().map(|s| DiscoveredServer {
                                    name: s.name.into(),
                                    address: s.address.into(),
                                }).collect();
                                let model = std::rc::Rc::new(slint::VecModel::from(disc_servers));
                                ui.set_discovered_servers(slint::ModelRc::from(model));
                                ui.set_is_loading(false);
            ui.set_is_loading_discovery(false);

                            }
                        });
                    }
                    Err(_) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.set_is_loading(false);
            ui.set_is_loading_discovery(false);

                            }
                        });
                    }
                }
            });
        }
    });

    let ui_public = ui.as_weak();
    ui.on_get_public_users(move |url| {
        if let Some(ui) = ui_public.upgrade() {
            ui.set_is_loading(true);
            ui.set_selected_server(url.clone());
            let ui_weak = ui_public.clone();
            let url_s = url.to_string();
            tokio::spawn(async move {
                match JellyfinClient::get_public_users(&url_s).await {
                    Ok(users) => {
                        let mut user_futures = Vec::new();
                        for user in users {
                            let url_clone = url_s.clone();
                            user_futures.push(async move {
                                let buffer = if let Some(tag) = &user.primary_image_tag {
                                    JellyfinClient::get_public_image_buffer(&url_clone, &user.id, tag).await.ok()
                                } else {
                                    None
                                };
                                (user, buffer)
                            });
                        }
                        
                        let results = futures::future::join_all(user_futures).await;
                        
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                let public_users: Vec<PublicUser> = results.into_iter().map(|(u, b)| PublicUser {
                                    name: u.name.into(),
                                    id: u.id.into(),
                                    has_password: u.has_password,
                                    thumbnail: b.map(Image::from_rgba8).unwrap_or_else(Image::default),
                                }).collect();
                                let model = std::rc::Rc::new(slint::VecModel::from(public_users));
                                ui.set_public_users(slint::ModelRc::from(model));
                                ui.set_is_loading(false);
                            }
                        });
                    }
                    Err(e) => {
                        let msg = format!("Failed to fetch users: {}", e);
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.set_error_message(msg.into());
                                ui.set_is_loading(false);
                            }
                        });
                    }
                }
            });
        }
    });

    ui.on_login(move |url, user, pass| {
        if let Some(ui) = ui_login.upgrade() {
            let jf_arc = jf_login.clone();
            let s_ids_arc = s_ids_login.clone();
            
            let url_s = url.to_string();
            let user_s = user.to_string();
            let pass_s = pass.to_string();

            ui.set_is_loading(true);
            ui.set_error_message("".into());

            let ui_weak = ui_login.clone();
            tokio::spawn(async move {
                match JellyfinClient::authenticate(url_s, user_s, pass_s).await {
                    Ok(client) => {
                        if let Err(e) = populate_series_ui(client, ui_weak.clone(), jf_arc, s_ids_arc).await {
                            let msg = format!("Failed to fetch library: {}", e);
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = ui_weak.upgrade() {
                                    ui.set_error_message(msg.into());
                                    ui.set_is_loading(false);
                                }
                            });
                        }
                    }
                    Err(e) => {
                        let msg = format!("Authentication failed: {}", e);
                        let ui_main = ui_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_main.upgrade() {
                                ui.set_error_message(msg.into());
                                ui.set_is_loading(false);
                            }
                        });
                    }
                }
            });
        }
    });

    // --- Navigation Callbacks ---
    let ui_nav = ui.as_weak();
    let jf_nav = jf_client.clone();
    let series_ids_nav = current_series_ids.clone();
    let episode_ids_nav = current_episode_ids.clone();
    
    ui.on_select_series(move |index| {
        let jf_opt = jf_nav.lock().unwrap().clone();
        if let Some(jf) = jf_opt {
            let s_ids = series_ids_nav.clone();
            let e_ids = episode_ids_nav.clone();
            let ui_weak = ui_nav.clone();
            
            tokio::spawn(async move {
                let series_id = {
                    let ids = s_ids.lock().unwrap();
                    ids.get(index as usize).cloned()
                };

                if let Some(sid) = series_id {
                    if let Ok(episodes) = jf.get_episodes(&sid).await {
                        // Extract primitive data only
                        let ep_data: Vec<(String, String, String)> = episodes.iter().map(|e| (
                            e.name.clone(),
                            ticks_to_duration(e.run_time_ticks),
                            e.id.clone()
                        )).collect();
                        
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                let ep_list: Vec<Episode> = ep_data.iter().map(|(name, dur, _id)| Episode {
                                    title: name.clone().into(),
                                    duration: dur.clone().into(),
                                }).collect();
                                
                                let mut e_ids_lock = e_ids.lock().unwrap();
                                *e_ids_lock = ep_data.into_iter().map(|(_n, _d, id)| id).collect();
                                
                                let ep_model = std::rc::Rc::new(slint::VecModel::from(ep_list));
                                ui.set_episode_list(slint::ModelRc::from(ep_model));
                                ui.set_current_screen("episodes".into());
                            }
                        });
                    }
                }
            });
        }
    });

    let ui_nav = ui.as_weak();
    ui.on_back_to_series(move || {
        if let Some(ui) = ui_nav.upgrade() {
            ui.set_current_screen("series".into());
        }
    });

    let ui_nav = ui.as_weak();
    let jf_play = jf_client.clone();
    let ep_ids_play = current_episode_ids.clone();
    let mpv_play = mpv;

    ui.on_select_episode(move |index| {
        let ui = ui_nav.upgrade().unwrap();
        let jf_opt = jf_play.lock().unwrap().clone();
        if let Some(jf) = jf_opt {
            let e_ids = ep_ids_play.clone();
            let mpv_h = mpv_play;

            let episode_id = {
                let ids = e_ids.lock().unwrap();
                ids.get(index as usize).cloned()
            };

            if let Some(eid) = episode_id {
                let stream_url = jf.get_stream_url(&eid);
                let cmd = CString::new("loadfile").unwrap();
                let url = CString::new(stream_url).unwrap();
                let mut args: [*const c_char; 3] = [cmd.as_ptr(), url.as_ptr(), ptr::null()];
                unsafe {
                    mpv_command(mpv_h.0, args.as_mut_ptr());
                }
                ui.set_current_screen("player".into());
            }
        }
    });

    let _ui_track = ui.as_weak();
    let mpv_track = mpv;
    ui.on_select_audio_track(move |id| {
        let name = CString::new("aid").unwrap();
        let val = CString::new(id.to_string()).unwrap();
        unsafe { mpv_set_property_string(mpv_track.0, name.as_ptr(), val.as_ptr()); }
    });

    let _ui_track = ui.as_weak();
    let mpv_track = mpv;
    ui.on_select_subtitle_track(move |id| {
        let name = CString::new("sid").unwrap();
        let val = CString::new(id.to_string()).unwrap();
        unsafe { mpv_set_property_string(mpv_track.0, name.as_ptr(), val.as_ptr()); }
    });

    let last_activity = Arc::new(Mutex::new(std::time::Instant::now()));
    let last_activity_clone = last_activity.clone();
    ui.on_user_activity(move || {
        if let Ok(mut last) = last_activity_clone.lock() {
            *last = std::time::Instant::now();
        }
    });

    let _ui_handle = ui.as_weak();
    
    let mpv_toggle = mpv;
    ui.on_toggle_pause(move || {
        let c_pause = CString::new("pause").unwrap();
        let mut paused: c_int = 0;
        unsafe {
            mpv_get_property(
                mpv_toggle.0,
                c_pause.as_ptr(),
                mpv_format_MPV_FORMAT_FLAG,
                &mut paused as *mut _ as *mut c_void,
            );
            let new_paused = if paused == 0 { 1 } else { 0 };
            mpv_set_property(
                mpv_toggle.0,
                c_pause.as_ptr(),
                mpv_format_MPV_FORMAT_FLAG,
                &new_paused as *const _ as *mut c_void,
            );
        }
    });

    let mpv_seek = mpv;
    ui.on_seek(move |percent| {
        let cmd = CString::new("seek").unwrap();
        let val = CString::new(percent.to_string()).unwrap();
        let mode = CString::new("absolute-percent").unwrap();
        let mut args: [*const c_char; 4] = [cmd.as_ptr(), val.as_ptr(), mode.as_ptr(), ptr::null()];
        unsafe {
            mpv_command(mpv_seek.0, args.as_mut_ptr());
        }
    });

    // 6. Setup Rendering Timer
    // This polls MPV for new frames and updates the Slint Image property.
    let render_timer = slint::Timer::default();
    let ui_render = ui.as_weak();
    let mpv_render = render_ctx;
    let mpv_handle = mpv;
    let last_activity_timer = last_activity.clone();

    render_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(16), move || {
        let ui = match ui_render.upgrade() {
            Some(ui) => ui,
            None => return,
        };

        // Update controls visibility based on timeout
        if let Ok(last) = last_activity_timer.lock() {
            let elapsed = last.elapsed();
            let is_paused = ui.get_is_paused();
            // Hide after 3 seconds if playing, or keep visible if paused
            ui.set_controls_visible(elapsed < std::time::Duration::from_secs(3) || is_paused);
        }

        unsafe {
            // Poll for events to keep the engine moving
            loop {
                let event = mpv_wait_event(mpv_handle.0, 0.0);
                if (*event).event_id == mpv_event_id_MPV_EVENT_NONE {
                    break;
                }
            }

            // Check if there's a new frame to render
            let flags = mpv_render_context_update(mpv_render.0);
            if (flags & 1) != 0 { // 1 = MPV_RENDER_UPDATE_FRAME
                // Use logical pixels for the buffer. Slint will handle the scaling to physical pixels.
                // This avoids "upper left slice" issues if Slint treats the buffer as logical units.
                let width = ui.get_video_width() as i32;
                let height = ui.get_video_height() as i32;

                if width <= 0 || height <= 0 { return; }

                // Debug: Uncomment to see dimensions
                // println!("Rendering at: {}x{} (scale: {})", width, height, ui.window().scale_factor());

                let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width as u32, height as u32);
                let stride = width * 4;
                let format = CString::new("rgba").unwrap();
                
                let mut size_arr: [c_int; 2] = [width, height];
                let mut stride_val: usize = stride as usize;
                let buffer_ptr = pixel_buffer.make_mut_bytes().as_mut_ptr();

                let mut params = [
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_SW_SIZE,
                        data: size_arr.as_mut_ptr() as *mut c_void,
                    },
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_SW_FORMAT,
                        data: format.as_ptr() as *mut c_void,
                    },
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_SW_STRIDE,
                        data: &mut stride_val as *mut _ as *mut c_void,
                    },
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_SW_POINTER,
                        data: buffer_ptr as *mut c_void,
                    },
                    mpv_render_param {
                        type_: 0,
                        data: ptr::null_mut(),
                    },
                ];

                let res = mpv_render_context_render(mpv_render.0, params.as_mut_ptr());
                if res >= 0 {
                    ui.set_video_frame(Image::from_rgba8(pixel_buffer));
                }
            }

            // Sync properties like progress and pause state
            let c_pos = CString::new("percent-pos").unwrap();
            let mut pos: f64 = 0.0;
            if mpv_get_property(mpv_handle.0, c_pos.as_ptr(), mpv_format_MPV_FORMAT_DOUBLE, &mut pos as *mut _ as *mut c_void) >= 0 {
                ui.set_progress(pos as f32);
            }

            let c_time = CString::new("time-pos").unwrap();
            let mut time: f64 = 0.0;
            if mpv_get_property(mpv_handle.0, c_time.as_ptr(), mpv_format_MPV_FORMAT_DOUBLE, &mut time as *mut _ as *mut c_void) >= 0 {
                ui.set_time_pos(format_time(time).into());
            }

            let c_dur = CString::new("duration").unwrap();
            let mut dur: f64 = 0.0;
            if mpv_get_property(mpv_handle.0, c_dur.as_ptr(), mpv_format_MPV_FORMAT_DOUBLE, &mut dur as *mut _ as *mut c_void) >= 0 {
                ui.set_duration(format_time(dur).into());
            }

            let c_pause = CString::new("pause").unwrap();
            let mut paused: c_int = 0;
            if mpv_get_property(mpv_handle.0, c_pause.as_ptr(), mpv_format_MPV_FORMAT_FLAG, &mut paused as *mut _ as *mut c_void) >= 0 {
                ui.set_is_paused(paused != 0);
            }

            // Sync track names and lists
            let c_tracks = CString::new("track-list").unwrap();
            let tracks_ptr = mpv_get_property_string(mpv_handle.0, c_tracks.as_ptr());
            if !tracks_ptr.is_null() {
                let json_str = std::ffi::CStr::from_ptr(tracks_ptr).to_string_lossy();
                if let Ok(tracks_data) = serde_json::from_str::<serde_json::Value>(&json_str) {
                    if let Some(list) = tracks_data.as_array() {
                        let mut audio_list = Vec::new();
                        let mut sub_list = Vec::new();
                        let mut current_audio = "None".to_string();
                        let mut current_sub = "None".to_string();

                        for t in list {
                            let id = t["id"].as_i64().unwrap_or(0) as i32;
                            let t_type = t["type"].as_str().unwrap_or("");
                            let active = t["selected"].as_bool().unwrap_or(false);
                            let title = t["title"].as_str().or(t["lang"].as_str()).unwrap_or("Unknown").to_string();
                            let codec = t["codec"].as_str().unwrap_or("");
                            let name = if codec != "" { format!("{} ({})", title, codec) } else { title };

                            let info = TrackInfo { id, name: name.clone().into(), active };
                            if t_type == "audio" {
                                audio_list.push(info);
                                if active { current_audio = name; }
                            } else if t_type == "sub" {
                                sub_list.push(info);
                                if active { current_sub = name; }
                            }
                        }

                        ui.set_audio_tracks(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(audio_list))));
                        ui.set_subtitle_tracks(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(sub_list))));
                        ui.set_audio_track_name(current_audio.into());
                        ui.set_subtitle_track_name(current_sub.into());
                    }
                }
                mpv_free(tracks_ptr as *mut c_void);
            }
        }
    });

    // --- Auto Login ---
    if let Some(cache) = LoginCache::load() {
        let jf_arc = jf_client.clone();
        let s_ids_arc = current_series_ids.clone();
        let ui_weak = ui.as_weak();
        
        ui.set_is_loading(true);
        tokio::spawn(async move {
            let client = JellyfinClient::from_token(cache.url, cache.access_token);
            if let Err(_) = populate_series_ui(client, ui_weak.clone(), jf_arc, s_ids_arc).await {
                // Invalid token or server unreachable, clear cache and stop loading
                LoginCache::delete();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_is_loading(false);
                    }
                });
            }
        });
    }

    // 7. Run the Event Loop
    let result = ui.run();

    // Explicitly stop the timer and drop the UI before cleaning up MPV
    drop(render_timer);
    drop(ui);

    unsafe {
        let stop_cmd = CString::new("stop").unwrap();
        let mut args: [*const c_char; 2] = [stop_cmd.as_ptr(), ptr::null()];
        mpv_command(mpv.0, args.as_mut_ptr());

        mpv_render_context_free(render_ctx.0);
        mpv_terminate_destroy(mpv.0);
    }

    Ok(result?)
}
