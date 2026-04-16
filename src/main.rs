// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod jellyfin;
mod plugin_manager;

use amp_api::{DynProvider, LoginCache};
use jellyfin::JellyfinFactory;
use libmpv_sys::*;
use plugin_manager::PluginManager;
use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer};
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::{Arc, Mutex};

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

    let mut plugin_manager = PluginManager::new();
    plugin_manager.register_builtin(Arc::new(JellyfinFactory));

    let plugins_dir = directories::ProjectDirs::from("com", "amp", "AMP")
        .map(|proj_dirs| proj_dirs.config_dir().join("plugins"))
        .expect("Shouldn't be empty");

    if plugins_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(plugins_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .map_or(false, |ext| ext == "so" || ext == "dll")
                {
                    unsafe {
                        if let Err(e) = plugin_manager.load_plugin(&path) {
                            eprintln!("Failed to load plugin {:?}: {}", path, e);
                        }
                    }
                }
            }
        }
    }

    let factories = plugin_manager.get_factories();
    let provider_list: Vec<ProviderMetadata> = factories
        .iter()
        .map(|f| ProviderMetadata {
            id: f.id().into(),
            name: f.display_name().into(),
        })
        .collect();

    ui.set_available_providers(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(provider_list),
    )));

    let plugin_manager = Arc::new(Mutex::new(plugin_manager));
    let provider: Arc<Mutex<Option<DynProvider>>> = Arc::new(Mutex::new(None));
    let current_series_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let current_episode_ids = Arc::new(Mutex::new(Vec::<String>::new()));

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
        mpv_set_property(
            handle,
            c_osd.as_ptr(),
            mpv_format_MPV_FORMAT_INT64,
            &mut zero as *mut _ as *mut c_void,
        );

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
    let provider_login = provider.clone();
    let s_ids_login = current_series_ids.clone();
    let plugin_manager_login = plugin_manager.clone();

    async fn populate_series_ui(
        client: DynProvider,
        provider_id: String,
        ui_weak: slint::Weak<PlayerWindow>,
        provider_arc: Arc<Mutex<Option<DynProvider>>>,
        s_ids_arc: Arc<Mutex<Vec<String>>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let series = client.get_series().await?;

        // Save to cache
        let cache = LoginCache {
            provider_id,
            config: client.get_persistable_config(),
        };
        let _ = cache.save();

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

        let mut provider_lock = provider_arc.lock().unwrap();
        *provider_lock = Some(client);

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

    let ui_prov = ui.as_weak();
    let pm_prov = plugin_manager.clone();
    ui.on_select_provider(move |id| {
        if let Some(ui) = ui_prov.upgrade() {
            ui.set_selected_provider_id(id.clone());

            let fields = {
                let pm = pm_prov.lock().unwrap();
                pm.get_factory(&id)
                    .map(|f| f.config_fields())
                    .unwrap_or_default()
            };

            let slint_fields: Vec<ConfigFieldMetadata> = fields
                .into_iter()
                .map(|f| ConfigFieldMetadata {
                    key: f.key.into(),
                    label: f.label.into(),
                    is_password: f.is_password,
                    value: f.default_value.into(),
                })
                .collect();

            ui.set_config_fields(slint::ModelRc::from(std::rc::Rc::new(
                slint::VecModel::from(slint_fields),
            )));
            ui.set_current_screen("login".into());
        }
    });

    ui.on_login(move |provider_id, fields| {
        if let Some(ui) = ui_login.upgrade() {
            let provider_arc = provider_login.clone();
            let s_ids_arc = s_ids_login.clone();
            let pm_arc = plugin_manager_login.clone();
            let provider_id = provider_id.to_string();

            let mut config = std::collections::HashMap::new();
            for i in 0..fields.row_count() {
                if let Some(field) = fields.row_data(i) {
                    config.insert(field.key.to_string(), field.value.to_string());
                }
            }

            ui.set_is_loading(true);
            ui.set_error_message("".into());

            let ui_weak = ui_login.clone();
            tokio::spawn(async move {
                let factory = {
                    let pm = pm_arc.lock().unwrap();
                    pm.get_factory(&provider_id)
                };

                if let Some(factory) = factory {
                    // Since create_provider might block or do heavy work
                    let client_res =
                        tokio::task::spawn_blocking(move || factory.create_provider(config))
                            .await
                            .unwrap();

                    match client_res {
                        Ok(client) => {
                            if let Err(e) = populate_series_ui(
                                client,
                                provider_id,
                                ui_weak.clone(),
                                provider_arc,
                                s_ids_arc,
                            )
                            .await
                            {
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
                } else {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.set_error_message("Provider not found".into());
                            ui.set_is_loading(false);
                        }
                    });
                }
            });
        }
    });

    // --- Navigation Callbacks ---
    let ui_nav = ui.as_weak();
    let provider_nav = provider.clone();
    let series_ids_nav = current_series_ids.clone();
    let episode_ids_nav = current_episode_ids.clone();

    ui.on_select_series(move |index| {
        let provider_opt = provider_nav.lock().unwrap().clone();
        if let Some(provider) = provider_opt {
            let s_ids = series_ids_nav.clone();
            let e_ids = episode_ids_nav.clone();
            let ui_weak = ui_nav.clone();

            tokio::spawn(async move {
                let series_id = {
                    let ids = s_ids.lock().unwrap();
                    ids.get(index as usize).cloned()
                };

                if let Some(sid) = series_id {
                    if let Ok(episodes) = provider.get_episodes(&sid).await {
                        // Extract primitive data only
                        let ep_data: Vec<(String, String, String)> = episodes
                            .iter()
                            .map(|e| {
                                (
                                    e.name.clone(),
                                    ticks_to_duration(e.duration_ticks),
                                    e.id.clone(),
                                )
                            })
                            .collect();

                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                let ep_list: Vec<Episode> = ep_data
                                    .iter()
                                    .map(|(name, dur, _id)| Episode {
                                        title: name.clone().into(),
                                        duration: dur.clone().into(),
                                    })
                                    .collect();

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
    let provider_play = provider.clone();
    let ep_ids_play = current_episode_ids.clone();
    let mpv_play = mpv;

    ui.on_select_episode(move |index| {
        let ui = ui_nav.upgrade().unwrap();
        let provider_opt = provider_play.lock().unwrap().clone();
        if let Some(provider) = provider_opt {
            let e_ids = ep_ids_play.clone();
            let mpv_h = mpv_play;

            let episode_id = {
                let ids = e_ids.lock().unwrap();
                ids.get(index as usize).cloned()
            };

            if let Some(eid) = episode_id {
                let stream_url = provider.get_stream_url(&eid);
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
        unsafe {
            mpv_set_property_string(mpv_track.0, name.as_ptr(), val.as_ptr());
        }
    });

    let _ui_track = ui.as_weak();
    let mpv_track = mpv;
    ui.on_select_subtitle_track(move |id| {
        let name = CString::new("sid").unwrap();
        let val = CString::new(id.to_string()).unwrap();
        unsafe {
            mpv_set_property_string(mpv_track.0, name.as_ptr(), val.as_ptr());
        }
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

    render_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(16),
        move || {
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
                if (flags & 1) != 0 {
                    // 1 = MPV_RENDER_UPDATE_FRAME
                    // Use logical pixels for the buffer. Slint will handle the scaling to physical pixels.
                    // This avoids "upper left slice" issues if Slint treats the buffer as logical units.
                    let width = ui.get_video_width() as i32;
                    let height = ui.get_video_height() as i32;

                    if width <= 0 || height <= 0 {
                        return;
                    }

                    // Debug: Uncomment to see dimensions
                    // println!("Rendering at: {}x{} (scale: {})", width, height, ui.window().scale_factor());

                    let mut pixel_buffer =
                        SharedPixelBuffer::<Rgba8Pixel>::new(width as u32, height as u32);
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
                if mpv_get_property(
                    mpv_handle.0,
                    c_pos.as_ptr(),
                    mpv_format_MPV_FORMAT_DOUBLE,
                    &mut pos as *mut _ as *mut c_void,
                ) >= 0
                {
                    ui.set_progress(pos as f32);
                }

                let c_time = CString::new("time-pos").unwrap();
                let mut time: f64 = 0.0;
                if mpv_get_property(
                    mpv_handle.0,
                    c_time.as_ptr(),
                    mpv_format_MPV_FORMAT_DOUBLE,
                    &mut time as *mut _ as *mut c_void,
                ) >= 0
                {
                    ui.set_time_pos(format_time(time).into());
                }

                let c_dur = CString::new("duration").unwrap();
                let mut dur: f64 = 0.0;
                if mpv_get_property(
                    mpv_handle.0,
                    c_dur.as_ptr(),
                    mpv_format_MPV_FORMAT_DOUBLE,
                    &mut dur as *mut _ as *mut c_void,
                ) >= 0
                {
                    ui.set_duration(format_time(dur).into());
                }

                let c_pause = CString::new("pause").unwrap();
                let mut paused: c_int = 0;
                if mpv_get_property(
                    mpv_handle.0,
                    c_pause.as_ptr(),
                    mpv_format_MPV_FORMAT_FLAG,
                    &mut paused as *mut _ as *mut c_void,
                ) >= 0
                {
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
                                let title = t["title"]
                                    .as_str()
                                    .or(t["lang"].as_str())
                                    .unwrap_or("Unknown")
                                    .to_string();
                                let codec = t["codec"].as_str().unwrap_or("");
                                let name = if codec != "" {
                                    format!("{} ({})", title, codec)
                                } else {
                                    title
                                };

                                let info = TrackInfo {
                                    id,
                                    name: name.clone().into(),
                                    active,
                                };
                                if t_type == "audio" {
                                    audio_list.push(info);
                                    if active {
                                        current_audio = name;
                                    }
                                } else if t_type == "sub" {
                                    sub_list.push(info);
                                    if active {
                                        current_sub = name;
                                    }
                                }
                            }

                            ui.set_audio_tracks(slint::ModelRc::from(std::rc::Rc::new(
                                slint::VecModel::from(audio_list),
                            )));
                            ui.set_subtitle_tracks(slint::ModelRc::from(std::rc::Rc::new(
                                slint::VecModel::from(sub_list),
                            )));
                            ui.set_audio_track_name(current_audio.into());
                            ui.set_subtitle_track_name(current_sub.into());
                        }
                    }
                    mpv_free(tracks_ptr as *mut c_void);
                }
            }
        },
    );

    let ui_logout = ui.as_weak();
    let provider_logout = provider.clone();
    ui.on_logout(move || {
        if let Some(ui) = ui_logout.upgrade() {
            let mut prov_lock = provider_logout.lock().unwrap();
            *prov_lock = None;
            LoginCache::delete();
            ui.set_current_screen("provider_select".into());
        }
    });

    let pm_auto = plugin_manager.clone();
    if let Some(cache) = LoginCache::load() {
        let provider_arc = provider.clone();
        let s_ids_arc = current_series_ids.clone();
        let ui_weak = ui.as_weak();

        ui.set_is_loading(true);
        tokio::spawn(async move {
            let factory = {
                let pm = pm_auto.lock().unwrap();
                pm.get_factory(&cache.provider_id)
            };

            if let Some(factory) = factory {
                let provider_id = cache.provider_id.clone();
                let client_res =
                    tokio::task::spawn_blocking(move || factory.create_provider(cache.config))
                        .await
                        .unwrap();

                match client_res {
                    Ok(client) => {
                        if let Err(_) = populate_series_ui(
                            client,
                            provider_id.clone(),
                            ui_weak.clone(),
                            provider_arc,
                            s_ids_arc,
                        )
                        .await
                        {
                            LoginCache::delete();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = ui_weak.upgrade() {
                                    ui.set_is_loading(false);
                                }
                            });
                        } else {
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = ui_weak.upgrade() {
                                    ui.set_selected_provider_id(provider_id.into());
                                }
                            });
                        }
                    }
                    Err(_) => {
                        LoginCache::delete();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.set_is_loading(false);
                            }
                        });
                    }
                }
            } else {
                LoginCache::delete();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_is_loading(false);
                    }
                });
            }
        });
    }

    let result = ui.run();

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
