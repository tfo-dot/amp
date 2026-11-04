#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
#[no_mangle]
pub static NvOptimusEnablement: u32 = 0x00000000;

#[cfg(target_os = "windows")]
#[no_mangle]
pub static AmdPowerXpressRequestHighPerformance: i32 = 0;

slint::include_modules!();

mod api;
mod app;
mod app_state;
mod backend;
mod discord;
mod extensions;
mod image_cache;
mod navigation;
mod player;

use crate::api::{MediaItemType, PlaybackInfo, PluginEvent};
use app_state::AppState;
use backend::seanime::{SeanimeWsEvent, SeanimeWsListener};
use glow::HasContext;
use image_cache::ImageCache;
use libmpv_sys::*;
use navigation::play_episode_with_series_playlist;
use navigation::{format_time, image_from_raw, load_dashboard, load_folder};
use player::{GLResources, MpvHandle, MpvRenderCtx, open_player};
use serde::Deserialize;
use slint::{
    BorrowedOpenGLTextureBuilder, BorrowedOpenGLTextureOrigin, ComponentHandle, Model, Weak,
};
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

fn format_mpv_key(text: &str, ctrl: bool, alt: bool, shift: bool) -> Option<String> {
    let mut parts = Vec::new();

    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }

    if text.len() == 1 && text.chars().next().unwrap().is_alphanumeric() {
        parts.push(text);
        Some(parts.join("+"))
    } else {
        None
    }
}

fn setup_key_bindings(ui: &AppWindow, mpv_handle: *mut mpv_handle) {
    let handle_ptr = mpv_handle as usize;

    ui.on_key_pressed(move |text, ctrl, alt, shift| {
        if let Some(key_combo) = format_mpv_key(&text, ctrl, alt, shift) {
            let handle = handle_ptr as *mut mpv_handle;

            let cmd = CString::new("keypress").unwrap();
            let arg = CString::new(key_combo).unwrap();

            let mut args: [*const std::os::raw::c_char; 3] =
                [cmd.as_ptr(), arg.as_ptr(), ptr::null()];

            unsafe {
                mpv_command(handle, args.as_mut_ptr());
            }
        }
    });
}

struct AppPlaybackController {
    ui: Weak<AppWindow>,
    mpv: MpvHandle,
}

impl crate::api::PlaybackController for AppPlaybackController {
    fn play(&self) {
        let c_pause = CString::new("pause").unwrap();
        let paused: c_int = 0;
        unsafe {
            mpv_set_property(
                self.mpv.get(),
                c_pause.as_ptr(),
                mpv_format_MPV_FORMAT_FLAG,
                &paused as *const _ as *mut c_void,
            );
        }
    }

    fn pause(&self) {
        let c_pause = CString::new("pause").unwrap();
        let paused: c_int = 1;
        unsafe {
            mpv_set_property(
                self.mpv.get(),
                c_pause.as_ptr(),
                mpv_format_MPV_FORMAT_FLAG,
                &paused as *const _ as *mut c_void,
            );
        }
    }

    fn toggle_pause(&self) {
        let ui_weak = self.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.invoke_toggle_pause();
            }
        });
    }

    fn next(&self) {
        let ui_weak = self.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.invoke_next();
            }
        });
    }

    fn previous(&self) {
        let ui_weak = self.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.invoke_previous();
            }
        });
    }

    fn stop(&self) {
        let ui_weak = self.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.invoke_close_player();
            }
        });
    }

    fn seek(&self, position_secs: i64) {
        let scmd = CString::new("seek").unwrap();
        let sval = CString::new(position_secs.to_string()).unwrap();
        let smode = CString::new("absolute").unwrap();
        let mut sargs = [scmd.as_ptr(), sval.as_ptr(), smode.as_ptr(), ptr::null()];
        unsafe {
            mpv_command(self.mpv.get(), sargs.as_mut_ptr());
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("[AMP] Starting up native media client...");

    unsafe {
        std::env::set_var("LC_NUMERIC", "C");
        let c_locale = CString::new("C").unwrap();
        libc::setlocale(libc::LC_ALL, c_locale.as_ptr());
        libc::setlocale(libc::LC_NUMERIC, c_locale.as_ptr());
    }

    eprintln!("[AMP] Initializing UI...");
    let ui = AppWindow::new()?;

    let (p_tx, mut p_rx) = tokio::sync::mpsc::unbounded_channel::<PluginEvent>();

    let state = Arc::new(Mutex::new(AppState::new(p_tx)));
    let cache = Arc::new(ImageCache::new());

    eprintln!("[AMP] Creating MpvHandle...");
    let mpv = MpvHandle::new();

    let controller = Arc::new(AppPlaybackController {
        ui: ui.as_weak(),
        mpv: mpv.clone(),
    });
    extensions::bridge::set_global_controller(controller.clone());

    ui.set_available_providers(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(
            state
                .lock()
                .unwrap()
                .active_providers
                .get_plugins()
                .iter()
                .map(|e| ProviderMetadata {
                    id: e.1.id().into(),
                    name: e.1.visible_name().into(),
                })
                .collect::<Vec<_>>(),
        ),
    )));

    let mpv_render: Rc<RefCell<Option<MpvRenderCtx>>> = Rc::new(RefCell::new(None));
    let gl_resources: Rc<RefCell<Option<GLResources>>> = Rc::new(RefCell::new(None));
    let last_screen: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let update_ctx_ptr: Rc<RefCell<Option<*mut c_void>>> = Rc::new(RefCell::new(None));

    {
        eprintln!("[AMP] Setting up rendering notifier...");
        let ui_weak = ui.as_weak();
        let mpv_h = mpv.clone();
        let mpv_r = mpv_render.clone();
        let gl_res = gl_resources.clone();
        let last_screen_r = last_screen.clone();
        let update_ctx_r = update_ctx_ptr.clone();

        ui.window()
            .set_rendering_notifier(move |rendering_state, api| match rendering_state {
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
                        let res = unsafe {
                            mpv_render_context_create(&mut ctx, mpv_h.get(), params.as_mut_ptr())
                        };
                        if res >= 0 {
                            *mpv_r.borrow_mut() = Some(MpvRenderCtx(ctx));
                            let cb_ctx = Box::into_raw(Box::new(ui_weak.clone())) as *mut c_void;
                            *update_ctx_r.borrow_mut() = Some(cb_ctx);
                            unsafe {
                                mpv_render_context_set_update_callback(ctx, Some(on_mpv_render_update), cb_ctx);
                            }
                            eprintln!("[AMP] MPV Render Context created successfully");
                        } else {
                            eprintln!("[AMP] Failed to create MPV Render Context: {}", res);
                        }
                    }
                }
                slint::RenderingState::BeforeRendering => {
                    if let Some(render_ctx) = mpv_r.borrow().as_ref()
                        && let Some(ui) = ui_weak.upgrade()
                    {
                        let is_player = ui.get_current_screen() == "player";
                        if !is_player {
                            *last_screen_r.borrow_mut() = ui.get_current_screen().to_string();
                            return;
                        }

                        let flags = unsafe { mpv_render_context_update(render_ctx.get()) };
                        let has_frame = (flags & (mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME as u64)) != 0;

                        let just_entered = *last_screen_r.borrow() != "player";
                        *last_screen_r.borrow_mut() = "player".to_string();

                        let sf = ui.window().scale_factor();
                        let raw_w = (ui.get_video_width() * sf) as u32;
                        let raw_h = (ui.get_video_height() * sf) as u32;

                        // Ensure even dimensions for planar color conversion & alignment
                        let width = ((raw_w + 1) & !1).max(2);
                        let height = ((raw_h + 1) & !1).max(2);

                        if width > 0 && height > 0 {
                            let mut res_lock = gl_res.borrow_mut();
                            let is_new = res_lock
                                .as_ref()
                                .is_none_or(|r| r.width != width || r.height != height);

                            if is_new
                                && let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api
                            {
                                let gl = unsafe {
                                    glow::Context::from_loader_function(|s| match CString::new(s) {
                                        Ok(name) => get_proc_address(&name) as *const _,
                                        _ => std::ptr::null(),
                                    })
                                };
                                let new_res = GLResources::new(gl, width, height);

                                let image = unsafe {
                                    BorrowedOpenGLTextureBuilder::new_gl_2d_rgba_texture(
                                        std::num::NonZeroU32::new(new_res.texture.0.get()).unwrap(),
                                        [width, height].into(),
                                    )
                                    .origin(BorrowedOpenGLTextureOrigin::TopLeft)
                                    .build()
                                };
                                ui.set_video_frame(image);

                                *res_lock = Some(new_res);
                            } else if just_entered && let Some(res) = res_lock.as_ref() {
                                let image = unsafe {
                                    BorrowedOpenGLTextureBuilder::new_gl_2d_rgba_texture(
                                        std::num::NonZeroU32::new(res.texture.0.get()).unwrap(),
                                        [width, height].into(),
                                    )
                                    .origin(BorrowedOpenGLTextureOrigin::TopLeft)
                                    .build()
                                };
                                ui.set_video_frame(image);
                            }

                            if (has_frame || is_new || just_entered) && let Some(res) = res_lock.as_ref() {
                                let mut fbo = mpv_opengl_fbo {
                                    fbo: res.fbo.0.get() as i32,
                                    w: width as i32,
                                    h: height as i32,
                                    internal_format: 0x8058, // GL_RGBA8
                                };

                                let mut params = [
                                    mpv_render_param {
                                        type_: mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                                        data: &mut fbo as *mut _ as *mut c_void,
                                    },
                                    mpv_render_param {
                                        type_:
                                            mpv_render_param_type_MPV_RENDER_PARAM_ADVANCED_CONTROL,
                                        data: &mut 1 as *mut _ as *mut c_void,
                                    },
                                    mpv_render_param {
                                        type_: 0,
                                        data: ptr::null_mut(),
                                    },
                                ];

                                unsafe {
                                    // Reset OpenGL state that Slint's Femtovg renderer may have left behind
                                    res.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(res.fbo));
                                    res.gl.viewport(0, 0, width as i32, height as i32);
                                    res.gl.disable(glow::SCISSOR_TEST);
                                    res.gl.disable(glow::DEPTH_TEST);
                                    res.gl.disable(glow::STENCIL_TEST);
                                    res.gl.disable(glow::CULL_FACE);
                                    res.gl.disable(glow::BLEND);
                                    res.gl.clear_color(0.0, 0.0, 0.0, 1.0);
                                    res.gl.clear(glow::COLOR_BUFFER_BIT);

                                    let res_render = mpv_render_context_render(
                                        render_ctx.get(),
                                        params.as_mut_ptr(),
                                    );
                                    if res_render < 0 {
                                        eprintln!("[AMP] Render error: {}", res_render);
                                    }

                                    res.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                                }
                            }
                        }
                    }
                }
                slint::RenderingState::RenderingTeardown => {
                    if let Some(render_ctx) = mpv_r.borrow_mut().take() {
                        unsafe {
                            mpv_render_context_set_update_callback(render_ctx.get(), None, ptr::null_mut());
                            mpv_render_context_free(render_ctx.get());
                        }
                    }
                    if let Some(ctx_ptr) = update_ctx_r.borrow_mut().take() {
                        unsafe {
                            drop(Box::from_raw(ctx_ptr as *mut Weak<AppWindow>));
                        }
                    }
                    *gl_res.borrow_mut() = None;
                }
                _ => {}
            })
            .unwrap();
    }

    setup_key_bindings(&ui, mpv.get());

    // --- Search Callbacks ---
    let ui_search = ui.as_weak();
    let state_search = state.clone();
    let cache_search = cache.clone();
    ui.on_search(move |query| {
        if let Some(ui) = ui_search.upgrade() {
            ui.set_is_loading(true);
        }

        let ui_weak = ui_search.clone();
        let query_str = query.to_string();
        let state_arc = state_search.clone();
        let cache_arc = cache_search.clone();

        tokio::spawn(async move {
            let prov_map = {
                let s = state_arc.lock().unwrap();
                s.active_providers.get_providers()
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
                if let Some(ui) = ui_weak.upgrade() {
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
                }
            });
        });
    });

    let ui_search_sel = ui.as_weak();
    let state_search_sel = state.clone();
    let cache_search_sel = cache.clone();
    let mpv_search_sel = mpv.clone();
    ui.on_select_search_result(move |index| {
        let (p_id, item_id) = {
            let s = state_search_sel.lock().unwrap();
            s.search_results_ids.get(index as usize).cloned().unwrap()
        };

        let ui_weak = ui_search_sel.clone();
        let Some(ui) = ui_weak.upgrade() else { return };
        let item = ui.get_search_results().row_data(index as usize).unwrap();

        if item.is_folder {
            ui.set_is_loading(true);
            let state_arc = state_search_sel.clone();
            let p_id_clone = p_id.clone();
            let item_id_clone = item_id.clone();

            {
                let mut s = state_arc.lock().unwrap();
                s.nav_stack.push((item_id.clone(), item.name.to_string()));
            }
            let prov = {
                let s = state_arc.lock().unwrap();
                s.active_providers.get_provider(&p_id).unwrap()
            };

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
        } else {
            let mpv_h = mpv_search_sel.clone();
            let state_arc = state_search_sel.clone();
            let item_t = item.clone();
            futures::executor::block_on(async move {
                play_episode_with_series_playlist(p_id, item_id, item_t, ui_weak, state_arc, mpv_h)
                    .await;
            });
        }
    });

    // --- Provider Selection & Configuration ---
    let ui_prov_select = ui.as_weak();
    let state_config = state.clone();
    ui.on_select_provider(move |id| {
        let Some(ui) = ui_prov_select.upgrade() else {
            return;
        };

        ui.set_selected_provider_id(id.clone());
        let pm = &state_config.lock().unwrap().active_providers;
        let fields = pm
            .get_plugin(&id)
            .map(|f| f.get_config_fields())
            .unwrap_or_default();
        let saved_config = pm.get_config(&id);
        let slint_fields: Vec<ConfigFieldMetadata> = fields
            .into_iter()
            .map(|f| {
                let value = saved_config
                    .get(&f.key)
                    .cloned()
                    .unwrap_or_else(|| f.default_value.clone());
                ConfigFieldMetadata {
                    key: f.key.into(),
                    label: f.label.into(),
                    is_password: f.is_password,
                    value: value.into(),
                }
            })
            .collect();
        ui.set_config_fields(slint::ModelRc::from(std::rc::Rc::new(
            slint::VecModel::from(slint_fields),
        )));
        ui.set_current_screen("login".into());
    });

    let ui_login_cb = ui.as_weak();
    let state_login = state.clone();
    let cache_login = cache.clone();
    // let parts_bridge_login = parts_bridge.clone();
    ui.on_login(move |provider_id, fields| {
        let Some(ui) = ui_login_cb.upgrade() else {
            return;
        };

        let mut config = std::collections::HashMap::new();
        for i in 0..fields.row_count() {
            if let Some(field) = fields.row_data(i) {
                config.insert(field.key.to_string(), field.value.to_string());
            }
        }

        ui.set_is_loading(true);
        let ui_weak = ui_login_cb.clone();
        let state_arc = state_login.clone();
        let cache_arc = cache_login.clone();
        let p_id = provider_id.to_string();

        tokio::spawn(async move {
            let mut s = state_arc.lock().unwrap();

            let res = s.active_providers.update_config(p_id, config);

            if let Err(e) = res {
                let msg = format!("Login failed: {}", e);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_error_message(msg.into());
                        ui.set_is_loading(false);
                    }
                });
            } else {
                let ui_w = ui_weak.clone();
                let state_ar = state_arc.clone();

                tokio::spawn(async move {
                    let _ = load_dashboard(state_ar, ui_w, cache_arc).await;
                });

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_current_screen("library".into());
                        ui.set_is_loading(false);
                    }
                });
            }
        });
    });

    // --- Item Selection Callbacks ---
    let ui_nav = ui.as_weak();
    let state_nav = state.clone();
    let mpv_nav = mpv.clone();
    let cache_nav = cache.clone();
    ui.on_select_item(move |index| {
        let Some(ui) = ui_nav.upgrade() else { return };
        let (p_id, item_id) = {
            let s = state_nav.lock().unwrap();
            let Some(pair) = s.current_items_ids.get(index as usize).cloned() else {
                return;
            };
            pair
        };
        let prov = {
            let s = state_nav.lock().unwrap();
            s.active_providers.get_provider(&p_id).unwrap()
        };
        let Some(item) = ui.get_current_items().row_data(index as usize) else {
            return;
        };

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

            return;
        }

        let mpv_h = mpv_nav.clone();
        let ui_weak = ui_nav.clone();
        let state_arc = state_nav.clone();

        futures::executor::block_on(async move {
            play_episode_with_series_playlist(p_id, item_id, item, ui_weak, state_arc, mpv_h).await;
        });
    });

    let ui_nu = ui.as_weak();
    let state_nu = state.clone();
    let mpv_nu = mpv.clone();
    ui.on_select_next_up(move |index| {
        let (p_id, item_id) = {
            let s = state_nu.lock().unwrap();
            let Some(pair) = s.next_up_ids.get(index as usize).cloned() else {
                return;
            };
            pair
        };
        let Some(ui) = ui_nu.upgrade() else { return };
        let Some(item) = ui.get_next_up_list().row_data(index as usize) else {
            return;
        };

        let mpv_h = mpv_nu.clone();
        let ui_weak = ui_nu.clone();
        let state_arc = state_nu.clone();

        futures::executor::block_on(async move {
            play_episode_with_series_playlist(p_id, item_id, item, ui_weak, state_arc, mpv_h).await;
        });
    });

    let ui_back = ui.as_weak();
    let state_back = state.clone();
    let cache_back = cache.clone();
    ui.on_back(move || {
        let Some(ui) = ui_back.upgrade() else { return };
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
            s.active_providers.get_providers().values().next().cloned()
        };

        if let Some(p) = prov {
            let p_id = {
                let s = state_arc.lock().unwrap();
                s.active_providers
                    .get_providers()
                    .keys()
                    .next()
                    .cloned()
                    .unwrap()
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
    ui.on_close_player(move || {
        let Some(ui) = ui_stop.upgrade() else { return };
        let mpv_h = mpv_stop.clone();
        let item_info = {
            let mut s = state_stop.lock().unwrap();
            s.current_item_id.take()
        };

        {
            let s = state_stop.lock().unwrap();
            for (_, ext) in s.active_providers.get_extensions() {
                ext.on_playback_stop();
            }
        }

        if let Some((p_id, id)) = item_info {
            let s = state_stop.lock().unwrap();

            if let Some(prov) = s.active_providers.get_provider(&p_id) {
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
                let p_clone = prov.clone();

                tokio::spawn(async move {
                    let _ = p_clone.report_playback_stopped(&id, time_i64).await;

                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

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
            let Some(pair) = s.current_items_ids.get(index as usize).cloned() else {
                return;
            };
            pair
        };
        let prov = {
            let s = state_mark.lock().unwrap();
            let Some(p) = s.active_providers.get_provider(&p_id) else {
                return;
            };
            p
        };

        tokio::spawn(async move {
            let _ = prov.mark_as_played(&item_id, played).await;
        });
    });

    let mpv_track = mpv.clone();
    let ui_track = ui.as_weak();
    ui.on_select_audio_track(move |id| {
        let name = CString::new("aid").unwrap();
        let val = CString::new(id.to_string()).unwrap();
        unsafe {
            mpv_set_property_string(mpv_track.get(), name.as_ptr(), val.as_ptr());
            if let Some(ui) = ui_track.upgrade() {
                update_tracks(&ui, mpv_track.get());
            }
        }
    });

    let mpv_sub = mpv.clone();
    let ui_sub = ui.as_weak();
    ui.on_select_subtitle_track(move |id| {
        let name = CString::new("sid").unwrap();
        let val = CString::new(id.to_string()).unwrap();
        unsafe {
            mpv_set_property_string(mpv_sub.get(), name.as_ptr(), val.as_ptr());
            if let Some(ui) = ui_sub.upgrade() {
                update_tracks(&ui, mpv_sub.get());
            }
        }
    });

    let mpv_speed = mpv.clone();
    let ui_speed = ui.as_weak();
    ui.on_change_speed(move |speed| {
        let name = CString::new("speed").unwrap();
        let speed_val = speed as f64;
        unsafe {
            mpv_set_property(
                mpv_speed.get(),
                name.as_ptr(),
                mpv_format_MPV_FORMAT_DOUBLE,
                &speed_val as *const _ as *mut c_void,
            );
            if let Some(ui) = ui_speed.upgrade() {
                ui.set_playback_speed(format!("{:.2}x", speed_val).into());
            }
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

    let ui_seek = ui.as_weak();
    let mpv_seek = mpv.clone();
    ui.on_seek(move |perc| {
        let scmd = CString::new("seek").unwrap();
        let sval = CString::new(perc.to_string()).unwrap();
        let smode = CString::new("absolute-percent").unwrap();
        let mut sargs = [scmd.as_ptr(), sval.as_ptr(), smode.as_ptr(), ptr::null()];
        unsafe {
            mpv_command(mpv_seek.get(), sargs.as_mut_ptr());
        }

        if let Some(ui) = ui_seek.upgrade() {
            unsafe {
                update_chapters(&ui, mpv_seek.get());
            }
        }
    });

    let ui_skip = ui.as_weak();

    let mpv_skip = mpv.clone();
    ui.on_skip_by(move |secs| {
        let scmd = CString::new("seek").unwrap();
        let sval = CString::new(secs.to_string()).unwrap();
        let mut sargs = [scmd.as_ptr(), sval.as_ptr(), ptr::null()];
        unsafe {
            mpv_command(mpv_skip.get(), sargs.as_mut_ptr());
        }

        if let Some(ui) = ui_skip.upgrade() {
            unsafe {
                update_chapters(&ui, mpv_skip.get());
            }
        }
    });

    // Direction: true is forward (next), false is backwards (previous)
    fn next_item(
        state: Arc<Mutex<AppState>>,
        direction: bool,
        mpv: MpvHandle,
        ui: slint::Weak<AppWindow>,
    ) {
        {
            let mut s = state.lock().unwrap();
            let Some((items, idx)) = s.active_playlist.as_mut() else {
                return;
            };

            if direction {
                if *idx + 1 >= items.len() {
                    return;
                }
                *idx += 1;
            } else {
                if *idx == 0 {
                    return;
                }
                *idx -= 1;
            }

            let item = items[*idx].clone();
            s.current_title = item.name.clone();
            s.current_artist = item.series_name.clone();
            s.current_series_name = Some(item.series_name.to_string());
            s.current_season_index = Some(item.season_index);
            s.current_episode_index = Some(item.index);
            s.current_item_id = Some((item.p_id, item.item_id));
        }

        let mpv_h = mpv.clone();
        let ui_weak = ui.clone();
        let state_arc = state.clone();
        tokio::spawn(async move {
            open_player(ui_weak, state_arc, mpv_h).await;
        });
    }
    let ui_next = ui.as_weak();
    let mpv_next = mpv.clone();
    let state_next = state.clone();

    ui.on_next(move || {
        next_item(state_next.clone(), true, mpv_next.clone(), ui_next.clone());
    });

    let ui_prev = ui.as_weak();
    let mpv_prev = mpv.clone();
    let state_prev = state.clone();
    ui.on_previous(move || {
        next_item(state_prev.clone(), false, mpv_prev.clone(), ui_prev.clone());
    });

    let ui_render = ui.as_weak();
    let mpv_h = mpv.clone();
    let state_timer = state.clone();
    let last_report = Arc::new(Mutex::new(std::time::Instant::now()));
    let last_ext_update = Arc::new(Mutex::new(std::time::Instant::now()));

    let c_time = CString::new("time-pos").unwrap();
    let c_dur = CString::new("duration").unwrap();
    let c_perc = CString::new("percent-pos").unwrap();
    let c_pause = CString::new("pause").unwrap();
    let c_speed = CString::new("speed").unwrap();

    let render_timer = slint::Timer::default();
    let mut last_sec: i64 = -1;
    let mut last_dur: i64 = -1;
    let mut last_paused: i32 = -1;
    let mut last_speed_str = String::new();
    let mut last_controls_visible = false;
    let mut last_progress: f32 = -1.0;

    render_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(100),
        {
            let state_arc = state_timer.clone();
            move || {
                let ui = match ui_render.upgrade() {
                    Some(ui) => ui,
                    None => return,
                };

                unsafe {
                    loop {
                        let ev = mpv_wait_event(mpv_h.get(), 0.0);
                        if (*ev).event_id == mpv_event_id_MPV_EVENT_NONE {
                            break;
                        }

                        if (*ev).event_id == mpv_event_id_MPV_EVENT_TRACKS_CHANGED
                            || (*ev).event_id == mpv_event_id_MPV_EVENT_FILE_LOADED
                        {
                            update_tracks(&ui, mpv_h.get());
                        }

                        if (*ev).event_id == mpv_event_id_MPV_EVENT_FILE_LOADED {
                            update_chapters(&ui, mpv_h.get());
                        }
                    }

                    let is_in_player = ui.get_current_screen() == "player";
                    if !is_in_player {
                        if let Ok(mut last) = last_ext_update.lock()
                            && last.elapsed() >= std::time::Duration::from_secs(1)
                        {
                            *last = std::time::Instant::now();
                            let s = state_arc.lock().unwrap();

                            for (_, ext) in s.active_providers.get_extensions() {
                                ext.on_playback_stop();
                            }
                        }
                        return;
                    }

                    if let Ok(last) = last_activity.lock() {
                        let is_p = ui.get_is_paused();
                        let show = last.elapsed() < std::time::Duration::from_secs(3) || is_p;
                        if show != last_controls_visible {
                            last_controls_visible = show;
                            ui.set_controls_visible(show);
                        }
                    }

                    let mut time: f64 = 0.0;
                    let mut dur: i64 = 0;
                    let mut perc: f64 = 0.0;
                    let mut paused: c_int = 0;

                    if mpv_get_property(
                        mpv_h.get(),
                        c_perc.as_ptr(),
                        mpv_format_MPV_FORMAT_DOUBLE,
                        &mut perc as *mut _ as *mut c_void,
                    ) >= 0
                    {
                        let p = perc as f32;
                        if (p - last_progress).abs() > 0.05 {
                            last_progress = p;
                            ui.set_progress(p);
                        }
                    }

                    let got_time = mpv_get_property(
                        mpv_h.get(),
                        c_time.as_ptr(),
                        mpv_format_MPV_FORMAT_DOUBLE,
                        &mut time as *mut _ as *mut c_void,
                    ) >= 0;

                    let got_dur = mpv_get_property(
                        mpv_h.get(),
                        c_dur.as_ptr(),
                        mpv_format_MPV_FORMAT_INT64,
                        &mut dur as *mut _ as *mut c_void,
                    ) >= 0;

                    let cur_sec = if got_time { time as i64 } else { -1 };
                    if cur_sec != last_sec || dur != last_dur {
                        last_sec = cur_sec;
                        last_dur = dur;

                        if got_time {
                            ui.set_time_pos(format_time(cur_sec).into());
                        }

                        if got_dur && dur > 0 {
                            ui.set_duration(format_time(dur).into());
                        }

                        if got_time && got_dur {
                            let remaining_secs = dur - cur_sec;
                            if remaining_secs >= 0 {
                                ui.set_remaining_time(
                                    format!("-{}", format_time(remaining_secs)).into(),
                                );
                                let current_time = chrono::Local::now();
                                if let Some(delta) = chrono::Duration::try_seconds(remaining_secs) {
                                    let end_time = current_time + delta;
                                    ui.set_ends_at(
                                        format!("ends at {}", end_time.format("%-I:%M %p")).into(),
                                    );
                                } else {
                                    ui.set_ends_at("".into());
                                }
                            } else {
                                ui.set_remaining_time("-00:00".into());
                                ui.set_ends_at("".into());
                            }
                        } else {
                            ui.set_remaining_time("-00:00".into());
                            ui.set_ends_at("".into());
                        }
                    }

                    if mpv_get_property(
                        mpv_h.get(),
                        c_pause.as_ptr(),
                        mpv_format_MPV_FORMAT_FLAG,
                        &mut paused as *mut _ as *mut c_void,
                    ) >= 0
                    {
                        if paused != last_paused {
                            last_paused = paused;
                            ui.set_is_paused(paused != 0);
                        }
                    }

                    let mut speed: f64 = 1.0;
                    if mpv_get_property(
                        mpv_h.get(),
                        c_speed.as_ptr(),
                        mpv_format_MPV_FORMAT_DOUBLE,
                        &mut speed as *mut _ as *mut c_void,
                    ) >= 0
                    {
                        let sp_str = format!("{:.2}x", speed);
                        if sp_str != last_speed_str {
                            last_speed_str = sp_str.clone();
                            ui.set_playback_speed(sp_str.into());
                        }
                    }

                    if let Ok(mut last) = last_ext_update.lock()
                        && last.elapsed() >= std::time::Duration::from_millis(500)
                    {
                        *last = std::time::Instant::now();

                        let s = state_arc.lock().unwrap();
                        let info = PlaybackInfo {
                            title: s.current_title.clone(),
                            artist: s.current_artist.clone(),
                            series_name: s.current_series_name.clone(),
                            season_index: s.current_season_index,
                            episode_index: s.current_episode_index,
                            is_paused: paused != 0,
                            position_secs: time as i64,
                            duration_secs: dur,
                        };

                        for (_, ext) in s.active_providers.get_extensions() {
                            ext.on_playback_update(info.clone());
                        }
                    }

                    if got_time
                        && time > 0.0
                        && let Ok(mut last) = last_report.lock()
                        && last.elapsed() >= std::time::Duration::from_secs(1)
                        && paused == 0
                    {
                        *last = std::time::Instant::now();

                        let report_data = {
                            let s = state_arc.lock().unwrap();
                            s.current_item_id.as_ref().and_then(|(p_id, id)| {
                                s.active_providers
                                    .get_provider(p_id)
                                    .map(|p| (p.clone(), id.clone(), p_id.clone()))
                            })
                        };

                        if let Some((provider, item_id, provider_id)) = report_data {
                            eprintln!("[AMP] Syncing progress to: {}", provider_id);

                            let time_i64 = time as i64;
                            let is_paused = paused != 0;

                            tokio::spawn(async move {
                                let _ = provider
                                    .report_playback_progress(&item_id, time_i64, is_paused)
                                    .await;
                            });
                        }
                    }
                }
            }
        },
    );

    ui.set_is_loading(true);
    ui.set_current_screen("library".into());

    let ui_init = ui.as_weak();
    let state_init = state.clone();
    let cache_init = cache.clone();
    tokio::spawn(async move {
        let _ = load_dashboard(state_init, ui_init.clone(), cache_init).await;
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_init.upgrade() {
                ui.set_is_loading(false);
            }
        });
    });

    eprintln!("[AMP] Entering Plugin event loop...");
    let ui_weak = ui.as_weak();
    let state_ws = state.clone();
    let cache_ws = cache.clone();

    tokio::spawn(async move {
        while let Some(ev) = p_rx.recv().await {
            match ev {
                PluginEvent::ReloadDashboard => {
                    let ui_weak_for_event = ui_weak.clone();
                    let state_arc = state_ws.clone();
                    let cache_arc = cache_ws.clone();

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak_for_event.upgrade()
                            && ui.get_current_screen() == "library"
                        {
                            let ui_weak_for_task = ui_weak_for_event.clone();
                            let state_arc = state_arc.clone();
                            let cache_arc = cache_arc.clone();

                            tokio::spawn(async move {
                                let _ =
                                    load_dashboard(state_arc, ui_weak_for_task, cache_arc).await;
                            });
                        }
                    });
                }
            }
        }
    });

    eprintln!("[AMP] Entering Slint event loop...");
    let result = ui.run();
    eprintln!("[AMP] Event loop exited.");

    MpvHandle::stop(&mpv);
    Ok(result?)
}

unsafe fn update_tracks(ui: &AppWindow, mpv_h: *mut mpv_handle) {
    let c_tracks = CString::new("track-list").unwrap();
    let tptr = unsafe { mpv_get_property_string(mpv_h, c_tracks.as_ptr()) };
    if !tptr.is_null() {
        let js = unsafe { CStr::from_ptr(tptr) }.to_string_lossy();
        if let Ok(tracks) = serde_json::from_str::<Vec<Track>>(&js) {
            let mut alist = Vec::new();
            let mut slist = Vec::new();
            let mut active_audio_name = "Default".to_string();
            let mut active_sub_name = "None".to_string();

            for t in &tracks {
                let track_info = t.as_track_info();
                if t.active {
                    match t.track_type {
                        TrackType::Audio => active_audio_name = track_info.name.to_string(),
                        TrackType::Sub => active_sub_name = track_info.name.to_string(),
                        _ => (),
                    }
                }
                match t.track_type {
                    TrackType::Audio => alist.push(track_info),
                    TrackType::Sub => slist.push(track_info),
                    _ => (),
                }
            }

            ui.set_audio_tracks(slint::ModelRc::from(std::rc::Rc::new(
                slint::VecModel::from(alist),
            )));
            ui.set_subtitle_tracks(slint::ModelRc::from(std::rc::Rc::new(
                slint::VecModel::from(slist),
            )));
            ui.set_audio_track_name(active_audio_name.into());
            ui.set_subtitle_track_name(active_sub_name.into());
        }
        unsafe { mpv_free(tptr as *mut c_void) };
    }
}

unsafe fn update_chapters(ui: &AppWindow, mpv_h: *mut mpv_handle) {
    let c_chapters = CString::new("chapter-list").unwrap();
    let c_current_chapter = CString::new("chapter").unwrap();

    let cptr = unsafe { mpv_get_property_string(mpv_h, c_chapters.as_ptr()) };
    if cptr.is_null() {
        return;
    }

    let js = unsafe { CStr::from_ptr(cptr) }.to_string_lossy();

    let mut current_idx: i64 = -1;
    unsafe {
        mpv_get_property(
            mpv_h,
            c_current_chapter.as_ptr(),
            mpv_format_MPV_FORMAT_INT64,
            &mut current_idx as *mut i64 as *mut c_void,
        );
    }

    let c_dur = CString::new("duration").unwrap();

    let mut dur: i64 = 0;

    let got_dur = unsafe {
        mpv_get_property(
            mpv_h,
            c_dur.as_ptr(),
            mpv_format_MPV_FORMAT_INT64,
            &mut dur as *mut _ as *mut c_void,
        ) >= 0
    };

    if got_dur && dur > 0 {
        ui.set_duration(format_time(dur).into());
    }

    if let Ok(chapters) = serde_json::from_str::<Vec<Chapter>>(&js) {
        let mut chapter_items = Vec::new();
        let mut active_name = "None".to_string();

        for (idx, ch) in chapters.iter().enumerate() {
            let is_active = idx as i64 == current_idx;

            let display_title = if ch.title.is_empty() {
                format!("Chapter {}", idx + 1)
            } else {
                ch.title.clone()
            };

            if is_active {
                active_name = display_title.clone();
            }

            chapter_items.push(ChapterInfo {
                // index: idx as i32,
                title: display_title.into(),
                time_str: ch.formatted_time().into(),
                start_percent: (ch.time as f32 / dur as f32) * 100.0,
            });
        }

        ui.set_chapters(slint::ModelRc::from(std::rc::Rc::new(
            slint::VecModel::from(chapter_items),
        )));
        ui.set_current_chapter_title(active_name.into());
    }

    unsafe { mpv_free(cptr as *mut c_void) };
}

#[derive(Debug, Deserialize, Clone)]
struct Chapter {
    #[serde(default)]
    title: String,
    time: f64, // czas startu w sekundach
}

impl Chapter {
    // Pomocnicze formatowanie sekund do "MM:SS" lub "HH:MM:SS" pod Slinta
    fn formatted_time(&self) -> String {
        let total_secs = self.time.round() as u64;
        let hours = total_secs / 3600;
        let minutes = (total_secs % 3600) / 60;
        let seconds = total_secs % 60;

        if hours > 0 {
            format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
        } else {
            format!("{:02}:{:02}", minutes, seconds)
        }
    }
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
    codec: Option<String>,
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
            name: match &self.codec {
                Some(c) if !c.is_empty() => format!("{} ({})", name, c).into(),
                _ => name.into(),
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
    #[serde(other)]
    Other,
}

unsafe extern "C" fn get_proc_address_mpv(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    unsafe {
        let get_proc_address = &*(ctx as *const &dyn Fn(&CStr) -> *const c_void);
        let name = CStr::from_ptr(name);
        get_proc_address(name) as *mut c_void
    }
}

unsafe extern "C" fn on_mpv_render_update(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let ui_weak = unsafe { &*(ctx as *const Weak<AppWindow>) };
    let ui_weak = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui_weak.upgrade() {
            if ui.get_current_screen() == "player" {
                ui.window().request_redraw();
            }
        }
    });
}
