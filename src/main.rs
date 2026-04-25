#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod app_state;
mod discord;
mod fbo;
mod image_cache;
mod jellyfin;
mod navigation;
mod player;
mod plugin_manager;

use amp_api::{MediaItemType, PlaybackInfo, PluginCapability};
use app_state::{AppState, PlaylistItem};
use fbo::GLResources;
use glow::HasContext;
use image_cache::ImageCache;
use libmpv_sys::*;
use navigation::{format_time, image_from_raw, load_dashboard, load_folder};
use player::{open_player, MpvHandle, MpvRenderCtx};
use plugin_manager::PluginManager;
use slint::{BorrowedOpenGLTextureBuilder, BorrowedOpenGLTextureOrigin, ComponentHandle, Model};
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("[AMP] Starting up...");
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("LC_NUMERIC", "C") };

    unsafe {
        let c_locale = CString::new("C").unwrap();
        libc::setlocale(libc::LC_ALL, c_locale.as_ptr());
        libc::setlocale(libc::LC_NUMERIC, c_locale.as_ptr());
    }

    eprintln!("[AMP] Initializing UI...");
    let ui = PlayerWindow::new()?;

    eprintln!("[AMP] Initializing Plugin Manager...");

    let mut plugin_manager = PluginManager::new();

    plugin_manager.register_builtin_plugin(Arc::new(jellyfin::JellyfinFactory));
    plugin_manager.register_builtin_plugin(Arc::new(discord::DiscordExtensionFactory));
    plugin_manager.load_plugins();

    let extensions = Arc::new(plugin_manager.get_extensions());

    let provider_list: Vec<ProviderMetadata> = plugin_manager
        .with_capability(PluginCapability::MediaProvider)
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
    let state = Arc::new(Mutex::new(AppState::new()));
    let cache = Arc::new(ImageCache::new());

    eprintln!("[AMP] Creating MpvHandle...");
    let mpv = MpvHandle::new();

    let mpv_render: Rc<RefCell<Option<MpvRenderCtx>>> = Rc::new(RefCell::new(None));
    let gl_resources: Rc<RefCell<Option<GLResources>>> = Rc::new(RefCell::new(None));

    {
        eprintln!("[AMP] Setting up rendering notifier...");
        let ui_weak = ui.as_weak();
        let mpv_h = mpv.clone();
        let mpv_r = mpv_render.clone();
        let gl_res = gl_resources.clone();

        ui.window().set_rendering_notifier(move |state, api| {
            match state {
                slint::RenderingState::RenderingSetup => {
                    if let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api {
                            let api_type = CString::new("opengl").unwrap();
                            let mut init_params = mpv_opengl_init_params {
                                get_proc_address: Some(get_proc_address_mpv),
                                get_proc_address_ctx: get_proc_address as *const _ as *mut c_void,
                                extra_exts: std::ptr::null(),
                            };
                            let mut params = [
                                mpv_render_param {
                                    type_: mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                                    data: api_type.as_ptr() as *mut c_void,
                                },
                                mpv_render_param {
                                    type_: mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                                    data: &mut init_params as *mut _ as *mut c_void,
                                },
                                mpv_render_param {
                                    type_: 0,
                                    data: ptr::null_mut(),
                                },
                            ];

                            let mut ctx: *mut mpv_render_context = ptr::null_mut();
                            let res = unsafe { mpv_render_context_create(&mut ctx, mpv_h.get(), params.as_mut_ptr()) };
                            if res >= 0 {
                                *mpv_r.borrow_mut() = Some(MpvRenderCtx(ctx));
                                eprintln!("[AMP] MPV Render Context created successfully");
                            } else {
                                eprintln!("[AMP] Failed to create MPV Render Context: {}", res);
                            }
                    }
                }
                slint::RenderingState::BeforeRendering => {
                    if let Some(render_ctx) = mpv_r.borrow().as_ref() {
                        if let Some(ui) = ui_weak.upgrade() {
                            let sf = ui.window().scale_factor();
                            let width = (ui.get_video_width() * sf) as u32;
                            let height = (ui.get_video_height() * sf) as u32;

                            if width > 0 && height > 0 {
                                let mut res_lock = gl_res.borrow_mut();
                                if res_lock.as_ref().map_or(true, |r| r.width != width || r.height != height) {
                                    if let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api {
                                        let gl = unsafe {
                                            glow::Context::from_loader_function(|s| {
                                                match CString::new(s) { Ok(name) => {
                                                    get_proc_address(&name) as *const _
                                                } _ => {
                                                    std::ptr::null()
                                                }}
                                            })
                                        };
                                        *res_lock = Some(GLResources::new(gl, width, height));
                                        eprintln!("[AMP] Created GL resources for {}x{} (SF: {})", width, height, sf);
                                    }
                                }

                                if let Some(res) = res_lock.as_ref() {
                                    let mut fbo = mpv_opengl_fbo {
                                        fbo: res.fbo.0.get() as i32,
                                        w: width as i32,
                                        h: height as i32,
                                        internal_format: 0,
                                    };

                                    let mut params = [
                                        mpv_render_param {
                                            type_: mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                                            data: &mut fbo as *mut _ as *mut c_void,
                                        },
                                        mpv_render_param {
                                            type_: mpv_render_param_type_MPV_RENDER_PARAM_ADVANCED_CONTROL,
                                            data: &mut 1 as *mut _ as *mut c_void,
                                        },
                                        mpv_render_param {
                                            type_: 0,
                                            data: ptr::null_mut(),
                                        },
                                    ];

                                    unsafe {
                                        res.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(res.fbo));
                                        res.gl.viewport(0, 0, width as i32, height as i32);
                                        res.gl.clear_color(0.0, 0.0, 0.0, 1.0);
                                        res.gl.clear(glow::COLOR_BUFFER_BIT);
                                        res.gl.bind_framebuffer(glow::FRAMEBUFFER, None);

                                        let res_render = mpv_render_context_render(render_ctx.get(), params.as_mut_ptr());
                                        if res_render < 0 {
                                            eprintln!("[AMP] Couldn't create context for {}x{} (SF: {})", width, height, res_render);
                                        }

                                        let image = BorrowedOpenGLTextureBuilder::new_gl_2d_rgba_texture(
                                            std::num::NonZeroU32::new(res.texture.0.get()).unwrap(),
                                            [width, height].into(),
                                        )
                                        .origin(BorrowedOpenGLTextureOrigin::TopLeft)
                                        .build();
                                        ui.set_video_frame(image);
                                    }
                                }
                            }
                        }
                    }
                }
                slint::RenderingState::RenderingTeardown => {
                    if let Some(render_ctx) = mpv_r.borrow_mut().take() {
                        unsafe {
                            mpv_render_context_free(render_ctx.get());
                        }
                    }
                    *gl_res.borrow_mut() = None;
                }
                _ => {}
            }
        }).unwrap();
    }

    // --- Callbacks ---
    let ui_search = ui.as_weak();
    let state_search = state.clone();
    let cache_search = cache.clone();
    ui.on_search(move |query| {
        ui_search.upgrade().unwrap().set_is_loading(true);

        let ui_weak = ui_search.clone();
        let query_str = query.to_string();
        let state_arc = state_search.clone();
        let cache_arc = cache_search.clone();

        tokio::spawn(async move {
            let prov_map = {
                let s = state_arc.lock().unwrap();
                s.active_providers.clone()
            };

            let mut all_results_futures = Vec::new();

            for (p_id, client) in prov_map {
                if let Ok(results) = client.search(&query_str).await {
                    for r in results {
                        let p_id_c = p_id.clone();
                        let client_c = client.clone();
                        let cache_c = cache_arc.clone();

                        all_results_futures.push(async move {
                            (
                                p_id_c,
                                r.clone(),
                                cache_c.get_or_fetch(&r.id, client_c).await,
                            )
                        });
                    }
                }
            }

            let all_results = futures::future::join_all(all_results_futures).await;

            let _ = slint::invoke_from_event_loop(move || {
                let ui = ui_weak.upgrade().unwrap();

                let mut ids = Vec::new();
                let mut grid_items = Vec::new();

                for (p_id, item, buffer) in all_results {
                    ids.push((p_id, item.id.clone()));

                    grid_items.push(GridItem {
                        id: item.id.into(),
                        name: item.name.into(),
                        thumbnail: buffer.map(image_from_raw).unwrap_or_default(),
                        description: item.series_name.unwrap_or_default().into(),
                        meta: "".into(),
                        series_name: "".into(),
                        is_folder: item.item_type == MediaItemType::Folder,
                        index: item.index.unwrap_or(0),
                        season_index: item.season_index.unwrap_or(0),
                    });
                }

                {
                    let mut s = state_arc.lock().unwrap();
                    s.search_results_ids = ids;
                }

                ui.set_search_results(slint::ModelRc::from(std::rc::Rc::new(
                    slint::VecModel::from(grid_items),
                )));

                ui.set_is_loading(false);
            });
        });
    });

    let ui_search_sel = ui.as_weak();
    let state_search_sel = state.clone();
    let cache_search_sel = cache.clone();
    ui.on_select_search_result(move |index| {
        let (p_id, item_id) = {
            let s = state_search_sel.lock().unwrap();
            s.search_results_ids.get(index as usize).cloned().unwrap()
        };

        let prov = {
            let s = state_search_sel.lock().unwrap();
            s.active_providers.get(&p_id).cloned().unwrap()
        };

        let ui_weak = ui_search_sel.clone();
        let ui = ui_weak.upgrade().unwrap();
        
        let state_arc = state_search_sel.clone();

        let item = ui.get_search_results().row_data(index as usize).unwrap();

        ui.set_is_loading(true);

        let p_id_clone = p_id.clone();
        let item_id_clone = item_id.clone();

        {
            let mut s = state_arc.lock().unwrap();
            s.nav_stack.push((item_id.clone(), item.name.to_string()));
        }

        let cache_search_s = cache_search_sel.clone();
        tokio::spawn(async move {
            let _ = load_folder(
                prov,
                p_id_clone,
                Some(item_id_clone),
                item.name.to_string(),
                ui_weak,
                state_arc,
                cache_search_s,
            )
            .await;
        });
    });

    let ui_prov_select = ui.as_weak();
    let pm_prov_select = plugin_manager.clone();
    ui.on_select_provider(move |id| {
        if let Some(ui) = ui_prov_select.upgrade() {
            ui.set_selected_provider_id(id.clone());
            let pm = pm_prov_select.lock().unwrap();
            let fields = pm
                .get_plugin(&id)
                .map(|f| f.config_fields())
                .unwrap_or_default();
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

    let ui_login_cb = ui.as_weak();
    let pm_cb = plugin_manager.clone();
    let state_login = state.clone();
    let cache_login = cache.clone();
    ui.on_login(move |provider_id, fields| {
        let ui = ui_login_cb.upgrade().unwrap();
        let mut config = std::collections::HashMap::new();
        for i in 0..fields.row_count() {
            if let Some(field) = fields.row_data(i) {
                config.insert(field.key.to_string(), field.value.to_string());
            }
        }
        ui.set_is_loading(true);
        let ui_weak = ui_login_cb.clone();
        let pm = pm_cb.clone();
        let state_arc = state_login.clone();
        let cache_arc = cache_login.clone();
        let provider_id = provider_id.to_string();

        tokio::spawn(async move {
            let factory = pm.lock().unwrap().get_plugin(&provider_id);
            if let Some(factory) = factory {
                let client_res = factory.create_provider(config).await;
                match client_res {
                    Ok(client) => {
                        {
                            let mut pm_lock = pm.lock().unwrap();
                            pm_lock
                                .config
                                .provider_configs
                                .insert(provider_id.clone(), client.get_persistable_config());
                            let _ = pm_lock.save_config();
                        }

                        {
                            let mut s = state_arc.lock().unwrap();
                            s.active_providers
                                .insert(provider_id.clone(), client.clone());
                        }

                        let _ = slint::invoke_from_event_loop({
                            let ui_weak = ui_weak.clone();
                            let state_arc = state_arc.clone();
                            let cache_arc = cache_arc.clone();
                            move || {
                                if let Some(ui) = ui_weak.upgrade() {
                                    ui.set_current_screen("library".into());
                                }
                                tokio::spawn(async move {
                                    let _ = load_dashboard(state_arc, ui_weak, cache_arc).await;
                                });
                            }
                        });
                    }
                    Err(e) => {
                        let msg = format!("Login failed: {}", e);
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
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
    });

    let ui_nav = ui.as_weak();
    let state_nav = state.clone();
    let mpv_nav = mpv.clone();
    let cache_nav = cache.clone();
    ui.on_select_item(move |index| {
        let ui = ui_nav.upgrade().unwrap();
        let (p_id, item_id) = {
            let s = state_nav.lock().unwrap();
            s.current_items_ids.get(index as usize).cloned().unwrap()
        };
        let prov = {
            let s = state_nav.lock().unwrap();
            s.active_providers.get(&p_id).cloned().unwrap()
        };
        let item = ui.get_current_items().row_data(index as usize).unwrap();

        if item.is_folder {
            ui.set_is_loading(true);
            let ui_weak = ui_nav.clone();
            let state_arc = state_nav.clone();
            let cache_arc = cache_nav.clone();
            let p_id_clone = p_id.clone();
            let item_id_clone = item_id.clone();
            {
                let mut s = state_arc.lock().unwrap();
                s.nav_stack.push((item_id.clone(), item.name.to_string()));
            }
            tokio::spawn(async move {
                let _ = load_folder(
                    prov,
                    p_id_clone,
                    Some(item_id_clone),
                    item.name.to_string(),
                    ui_weak,
                    state_arc,
                    cache_arc,
                )
                .await;
            });
        } else {
            let mpv_h = mpv_nav.clone();
            let ui_weak = ui_nav.clone();
            let state_arc = state_nav.clone();

            let mut playlist_items = Vec::new();
            {
                let s = state_arc.lock().unwrap();
                for (i, id_pair) in s.current_items_ids.iter().enumerate() {
                    if let Some(it) = ui.get_current_items().row_data(i) {
                        playlist_items.push(PlaylistItem {
                            p_id: id_pair.0.clone(),
                            item_id: id_pair.1.clone(),
                            name: it.name.to_string(),
                            series_name: it.series_name.to_string(),
                            index: it.index,
                            season_index: it.season_index,
                        });
                    }
                }
            }

            {
                let mut s = state_arc.lock().unwrap();
                s.active_playlist = Some((playlist_items, index as usize));
                s.current_title = item.name.to_string();
                s.current_artist = item.series_name.to_string();
            }

            let series = Some(item.series_name.to_string());
            let season = Some(item.season_index);
            let episode = Some(item.index);

            tokio::spawn(async move {
                open_player(
                    ui_weak, state_arc, mpv_h, p_id, item_id, series, season, episode,
                )
                .await;
            });
        }
    });

    let ui_nu = ui.as_weak();
    let state_nu = state.clone();
    let mpv_nu = mpv.clone();
    ui.on_select_next_up(move |index| {
        let (p_id, item_id) = {
            let s = state_nu.lock().unwrap();
            s.next_up_ids.get(index as usize).cloned().unwrap()
        };
        let ui = ui_nu.upgrade().unwrap();
        let item = ui.get_next_up_list().row_data(index as usize).unwrap();

        {
            let mut s = state_nu.lock().unwrap();
            s.current_title = item.name.to_string();
            s.current_artist = item.series_name.to_string();

            let mut playlist_items = Vec::new();
            for (i, id_pair) in s.next_up_ids.iter().enumerate() {
                if let Some(it) = ui.get_next_up_list().row_data(i) {
                    playlist_items.push(PlaylistItem {
                        p_id: id_pair.0.clone(),
                        item_id: id_pair.1.clone(),
                        name: it.name.to_string(),
                        series_name: it.series_name.to_string(),
                        index: it.index,
                        season_index: it.season_index,
                    });
                }
            }
            s.active_playlist = Some((playlist_items, index as usize));
        }

        let mpv_h = mpv_nu.clone();
        let ui_weak = ui_nu.clone();
        let state_arc = state_nu.clone();

        let series = Some(item.series_name.to_string());
        let season = Some(item.season_index);
        let episode = Some(item.index);

        tokio::spawn(async move {
            open_player(
                ui_weak, state_arc, mpv_h, p_id, item_id, series, season, episode,
            )
            .await
        });
    });

    let ui_back = ui.as_weak();
    let state_back = state.clone();
    let cache_back = cache.clone();
    ui.on_back(move || {
        let ui = ui_back.upgrade().unwrap();
        let parent = {
            let mut s = state_back.lock().unwrap();
            s.nav_stack.pop();
            s.nav_stack.last().cloned()
        };

        ui.set_is_loading(true);
        let ui_weak = ui_back.clone();
        let state_arc = state_back.clone();
        let cache_arc = cache_back.clone();

        let prov = {
            let s = state_arc.lock().unwrap();
            s.active_providers.values().next().cloned()
        };

        if let Some(p) = prov {
            let p_id = {
                let s = state_arc.lock().unwrap();
                s.active_providers.keys().next().cloned().unwrap()
            };
            tokio::spawn(async move {
                let (fid, pname) = parent
                    .map(|(id, name)| (Some(id), name))
                    .unwrap_or((None, "Library".into()));
                let _ = load_folder(p, p_id, fid, pname, ui_weak, state_arc, cache_arc).await;
            });
        }
    });

    let ui_stop = ui.as_weak();
    let mpv_stop = mpv.clone();
    let state_stop = state.clone();
    let cache_stop = cache.clone();
    let extensions_close = extensions.clone();
    ui.on_close_player(move || {
        let ui = ui_stop.upgrade().unwrap();
        let mpv_h = mpv_stop.clone();
        let item_info = {
            let mut s = state_stop.lock().unwrap();
            s.current_item_id.take()
        };

        for ext in extensions_close.iter() {
            ext.on_playback_stop();
        }

        if let Some((p_id, id)) = item_info {
            let prov = {
                let s = state_stop.lock().unwrap();
                s.active_providers.get(&p_id).cloned()
            };

            if let Some(p) = prov {
                let mut time: f64 = 0.0;
                let c_time = CString::new("time-pos").unwrap();
                let res = unsafe {
                    mpv_get_property(
                        mpv_h.get(),
                        c_time.as_ptr(),
                        mpv_format_MPV_FORMAT_DOUBLE,
                        &mut time as *mut _ as *mut c_void,
                    )
                };

                let time_i64 = if res >= 0 { time as i64 } else { 0 };

                let scmd = CString::new("stop").unwrap();
                let mut sargs = [scmd.as_ptr(), ptr::null()];
                unsafe { mpv_command(mpv_h.get(), sargs.as_mut_ptr()) };

                let ui_weak = ui_stop.clone();
                let state_arc = state_stop.clone();
                let cache_arc = cache_stop.clone();
                let p_clone = p.clone();

                tokio::spawn(async move {
                    let _ = p_clone.report_playback_progress(&id, time_i64, false).await;
                    let _ = p_clone.report_playback_stopped(&id, time_i64).await;

                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                    let (fid, pname) = {
                        let s = state_arc.lock().unwrap();
                        s.nav_stack
                            .last()
                            .cloned()
                            .map(|(id, name)| (Some(id), name))
                            .unwrap_or((None, "Library".into()))
                    };

                    if fid.is_none() {
                        let _ = load_dashboard(state_arc, ui_weak, cache_arc).await;
                    } else {
                        let _ =
                            load_folder(p_clone, p_id, fid, pname, ui_weak, state_arc, cache_arc)
                                .await;
                    }
                });
            }
        }
        ui.set_current_screen("library".into());
    });

    let state_mark = state.clone();
    ui.on_mark_as_played(move |index, played| {
        let (p_id, item_id) = {
            let s = state_mark.lock().unwrap();
            s.current_items_ids.get(index as usize).cloned().unwrap()
        };
        let prov = {
            let s = state_mark.lock().unwrap();
            s.active_providers.get(&p_id).cloned().unwrap()
        };
        tokio::spawn(async move {
            let _ = prov.mark_as_played(&item_id, played).await;
        });
    });

    let mpv_track = mpv.clone();
    ui.on_select_audio_track(move |id| {
        let name = CString::new("aid").unwrap();
        let val = CString::new(id.to_string()).unwrap();
        unsafe {
            mpv_set_property_string(mpv_track.get(), name.as_ptr(), val.as_ptr());
        }
    });

    let mpv_sub = mpv.clone();
    ui.on_select_subtitle_track(move |id| {
        let name = CString::new("sid").unwrap();
        let val = CString::new(id.to_string()).unwrap();
        unsafe {
            mpv_set_property_string(mpv_sub.get(), name.as_ptr(), val.as_ptr());
        }
    });

    let last_activity = Arc::new(Mutex::new(std::time::Instant::now()));
    let la_clone = last_activity.clone();
    ui.on_user_activity(move || {
        *la_clone.lock().unwrap() = std::time::Instant::now();
    });

    let mpv_toggle = mpv.clone();
    ui.on_toggle_pause(move || {
        let c_pause = CString::new("pause").unwrap();
        let mut paused: c_int = 0;
        unsafe {
            mpv_get_property(
                mpv_toggle.get(),
                c_pause.as_ptr(),
                mpv_format_MPV_FORMAT_FLAG,
                &mut paused as *mut _ as *mut c_void,
            );
            let new_p = if paused == 0 { 1 } else { 0 };
            mpv_set_property(
                mpv_toggle.get(),
                c_pause.as_ptr(),
                mpv_format_MPV_FORMAT_FLAG,
                &new_p as *const _ as *mut c_void,
            );
        }
    });

    let mpv_seek = mpv.clone();
    ui.on_seek(move |perc| {
        let scmd = CString::new("seek").unwrap();
        let sval = CString::new(perc.to_string()).unwrap();
        let smode = CString::new("absolute-percent").unwrap();
        let mut sargs = [scmd.as_ptr(), sval.as_ptr(), smode.as_ptr(), ptr::null()];
        unsafe {
            mpv_command(mpv_seek.get(), sargs.as_mut_ptr());
        }
    });

    let mpv_skip = mpv.clone();
    ui.on_skip_by(move |secs| {
        let scmd = CString::new("seek").unwrap();
        let sval = CString::new(secs.to_string()).unwrap();
        let mut sargs = [scmd.as_ptr(), sval.as_ptr(), ptr::null()];
        unsafe {
            mpv_command(mpv_skip.get(), sargs.as_mut_ptr());
        }
    });

    let ui_next = ui.as_weak();
    let mpv_next = mpv.clone();
    let state_next = state.clone();
    ui.on_next(move || {
        let (p_id, item_id, series, season, episode) = {
            let mut s = state_next.lock().unwrap();
            if let Some((items, idx)) = s.active_playlist.as_mut() {
                if *idx + 1 < items.len() {
                    *idx += 1;
                    let item = items[*idx].clone();
                    s.current_title = item.name.clone();
                    s.current_artist = item.series_name.clone();
                    (
                        item.p_id,
                        item.item_id,
                        Some(item.series_name),
                        Some(item.season_index),
                        Some(item.index),
                    )
                } else {
                    return;
                }
            } else {
                return;
            }
        };
        let mpv_h = mpv_next.clone();
        let ui_weak = ui_next.clone();
        let state_arc = state_next.clone();
        tokio::spawn(async move {
            open_player(
                ui_weak, state_arc, mpv_h, p_id, item_id, series, season, episode,
            )
            .await;
        });
    });

    let ui_prev = ui.as_weak();
    let mpv_prev = mpv.clone();
    let state_prev = state.clone();
    ui.on_previous(move || {
        let (p_id, item_id, series, season, episode) = {
            let mut s = state_prev.lock().unwrap();
            if let Some((items, idx)) = s.active_playlist.as_mut() {
                if *idx > 0 {
                    *idx -= 1;
                    let item = items[*idx].clone();
                    s.current_title = item.name.clone();
                    s.current_artist = item.series_name.clone();
                    (
                        item.p_id,
                        item.item_id,
                        Some(item.series_name),
                        Some(item.season_index),
                        Some(item.index),
                    )
                } else {
                    return;
                }
            } else {
                return;
            }
        };
        let mpv_h = mpv_prev.clone();
        let ui_weak = ui_prev.clone();
        let state_arc = state_prev.clone();
        tokio::spawn(async move {
            open_player(
                ui_weak, state_arc, mpv_h, p_id, item_id, series, season, episode,
            )
            .await;
        });
    });

    let ui_render = ui.as_weak();
    let mpv_r = mpv_render.clone();
    let mpv_h = mpv.clone();
    let state_timer = state.clone();

    let last_report = Arc::new(Mutex::new(std::time::Instant::now()));
    let last_ext_update = Arc::new(Mutex::new(std::time::Instant::now()));

    let render_timer = slint::Timer::default();
    render_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(16),
        {
            let state_arc = state_timer.clone();
            move || {
                let ui = match ui_render.upgrade() {
                    Some(ui) => ui,
                    None => return,
                };

                if let Ok(last) = last_activity.clone().lock() {
                    let is_p = ui.get_is_paused();
                    ui.set_controls_visible(
                        last.elapsed() < std::time::Duration::from_secs(3) || is_p,
                    );
                }

                unsafe {
                    loop {
                        let ev = mpv_wait_event(mpv_h.get(), 0.0);
                        if (*ev).event_id == mpv_event_id_MPV_EVENT_NONE {
                            break;
                        }
                    }

                    if let Some(rctx) = mpv_r.borrow().as_ref() {
                        if (mpv_render_context_update(rctx.get()) & 1) != 0 {
                            ui.window().request_redraw();
                        }
                    }

                    let mut time: f64 = 0.0;
                    let mut dur: i64 = 0;
                    let mut perc: f64 = 0.0;
                    let mut paused: c_int = 0;
                    let c_time = CString::new("time-pos").unwrap();
                    let c_dur = CString::new("duration").unwrap();
                    let c_perc = CString::new("percent-pos").unwrap();
                    let c_pause = CString::new("pause").unwrap();
                    if mpv_get_property(
                        mpv_h.get(),
                        c_perc.as_ptr(),
                        mpv_format_MPV_FORMAT_DOUBLE,
                        &mut perc as *mut _ as *mut c_void,
                    ) >= 0
                    {
                        ui.set_progress(perc as f32);
                    }
                    if mpv_get_property(
                        mpv_h.get(),
                        c_time.as_ptr(),
                        mpv_format_MPV_FORMAT_DOUBLE,
                        &mut time as *mut _ as *mut c_void,
                    ) >= 0
                    {
                        ui.set_time_pos(format_time(time as i64).into());
                    }
                    if mpv_get_property(
                        mpv_h.get(),
                        c_dur.as_ptr(),
                        mpv_format_MPV_FORMAT_INT64,
                        &mut dur as *mut _ as *mut c_void,
                    ) >= 0
                    {
                        ui.set_duration(format_time(dur).into());
                    }
                    if mpv_get_property(
                        mpv_h.get(),
                        c_pause.as_ptr(),
                        mpv_format_MPV_FORMAT_FLAG,
                        &mut paused as *mut _ as *mut c_void,
                    ) >= 0
                    {
                        ui.set_is_paused(paused != 0);
                    }

                    if ui.get_current_screen() == "player" {
                        if last_ext_update.lock().unwrap().elapsed()
                            >= std::time::Duration::from_secs(1)
                        {
                            *last_ext_update.lock().unwrap() = std::time::Instant::now();

                            let info = {
                                let s = state_arc.lock().unwrap();
                                PlaybackInfo {
                                    title: s.current_title.clone(),
                                    artist: s.current_artist.clone(),
                                    series_name: s.current_series_name.clone(),
                                    season_index: s.current_season_index,
                                    episode_index: s.current_episode_index,
                                    is_paused: paused != 0,
                                    position_secs: time as i64,
                                    duration_secs: dur,
                                }
                            };

                            for ext in extensions.clone().iter() {
                                ext.on_playback_update(info.clone());
                            }
                        }
                    } else {
                        if last_ext_update.lock().unwrap().elapsed()
                            >= std::time::Duration::from_secs(1)
                        {
                            *last_ext_update.lock().unwrap() = std::time::Instant::now();
                            for ext in extensions.clone().iter() {
                                ext.on_playback_stop();
                            }
                        }
                    }

                    if let Ok(mut last) = last_report.lock() {
                        if last.elapsed() >= std::time::Duration::from_secs(10) {
                            *last = std::time::Instant::now();
                            let (prov, id, p_id) = {
                                let s = state_arc.lock().unwrap();
                                let item = s.current_item_id.clone();
                                let p = item
                                    .as_ref()
                                    .and_then(|(p_id, _)| s.active_providers.get(p_id).cloned());
                                (
                                    p,
                                    item.as_ref().map(|(_, id)| id.clone()),
                                    item.as_ref().map(|(p_id, _)| p_id.clone()),
                                )
                            };

                            if let (Some(p), Some(id), Some(p_id)) = (prov, id, p_id) {
                                eprintln!("[AMP] Updating playback for: {}", p_id);
                                let time_i64 = time as i64;
                                tokio::spawn(async move {
                                    let _ = p
                                        .report_playback_progress(&id, time_i64, paused != 0)
                                        .await;
                                });
                            }
                        }
                    }

                    let c_tracks = CString::new("track-list").unwrap();
                    let tptr = mpv_get_property_string(mpv_h.get(), c_tracks.as_ptr());
                    if !tptr.is_null() {
                        let js = CStr::from_ptr(tptr).to_string_lossy();
                        if let Ok(tracks) = serde_json::from_str::<Vec<Track>>(&js) {
                            let mut alist = Vec::new();
                            let mut slist = Vec::new();
                            for t in &tracks {
                                match t.track_type {
                                    TrackType::Audio => alist.push(t.as_track_info()),
                                    TrackType::Sub => slist.push(t.as_track_info()),
                                    _ => (),
                                }
                            }
                            ui.set_audio_tracks(slint::ModelRc::from(std::rc::Rc::new(
                                slint::VecModel::from(alist),
                            )));
                            ui.set_subtitle_tracks(slint::ModelRc::from(std::rc::Rc::new(
                                slint::VecModel::from(slist),
                            )));
                        }
                        mpv_free(tptr as *mut c_void);
                    }
                }
            }
        },
    );

    let saved_configs = plugin_manager
        .lock()
        .unwrap()
        .config
        .provider_configs
        .clone();

    if !saved_configs.is_empty() {
        ui.set_is_loading(true);

        let ui_weak = ui.as_weak();
        let pm = plugin_manager.clone();
        let state_arc = state.clone();

        tokio::spawn(async move {
            for (provider_id, config) in saved_configs {
                let factory = pm.lock().unwrap().get_plugin(&provider_id);

                if let None = factory {
                    eprintln!(
                        "[AMP] Trying to load config for missing provider: {}",
                        provider_id
                    );

                    continue;
                }

                match factory.unwrap().create_provider(config).await {
                    Ok(client) => {
                        let mut s = state_arc.lock().unwrap();

                        s.active_providers
                            .insert(provider_id.clone(), client.clone());

                        eprintln!(
                            "[AMP] Successfully initialized form file for plugin {}",
                            provider_id
                        )
                    }
                    Err(e) => eprintln!(
                        "[AMP] error initializing config from file for plugin {}, {}",
                        provider_id, e
                    ),
                }
            }

            let _ = load_dashboard(state_arc, ui_weak.clone(), cache).await;

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_is_loading(false);
                }
            });
        });
    }

    eprintln!("[AMP] Entering main loop...");

    let result = ui.run();

    eprintln!("[AMP] Main loop exited.");

    mpv.stop();
    Ok(result?)
}

#[derive(serde::Deserialize, Debug)]
struct Track {
    id: i32,
    #[serde(rename = "type")]
    track_type: TrackType,
    #[serde(rename = "selected", default)]
    active: bool,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    codec: String,
}

impl Track {
    fn as_track_info(&self) -> TrackInfo {
        let name = self
            .title
            .as_ref()
            .or(self.lang.as_ref())
            .cloned()
            .unwrap_or_else(|| "Unknown".into());
        TrackInfo {
            active: self.active,
            id: self.id,
            name: if self.codec.is_empty() {
                name.into()
            } else {
                format!("{} ({})", name, self.codec).into()
            },
        }
    }
}

#[derive(serde::Deserialize, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum TrackType {
    Audio,
    Video,
    Sub,
}

unsafe extern "C" fn get_proc_address_mpv(ctx: *mut c_void, name: *const c_char) -> *mut c_void { unsafe {
    let get_proc_address = &*(ctx as *const &dyn Fn(&CStr) -> *const c_void);
    let name = CStr::from_ptr(name);
    get_proc_address(name) as *mut c_void
}}
