#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod jellyfin;
mod plugin_manager;
mod fbo;
mod discord;
mod login_cache;

use discord::DiscordExtensionFactory;
use amp_api::{DynProvider, MediaItemType, PlaybackExtension, PlaybackInfo, PluginCapability, RawImage};
use login_cache::LoginCache;
use glow::HasContext;
use jellyfin::JellyfinFactory;
use plugin_manager::PluginManager;
use fbo::GLResources;
use libmpv_sys::*;
use serde::Deserialize;
use slint::{BorrowedOpenGLTextureBuilder, BorrowedOpenGLTextureOrigin, Image, Model, Weak};
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct MpvHandle(*mut mpv_handle);

impl MpvHandle {
    fn new() -> Self {
        unsafe {
            let handle = mpv_create();

            if handle.is_null() {
                panic!("Failed to create mpv context");
            }

            let cache_opts = [
                ("vo", "libmpv"),
                ("gpu-api", "opengl"),
                ("hwdec", "no"),
                ("cache", "yes"),
                ("demuxer-max-bytes", "150M"),
                ("demuxer-max-back-bytes", "75M"),
                ("vd-lavc-threads", "0"),
            ];

            for (opt, val) in cache_opts {
                let c_opt = CString::new(opt).unwrap();
                let c_val = CString::new(val).unwrap();
                mpv_set_property_string(handle, c_opt.as_ptr(), c_val.as_ptr());
            }

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

            // Request log messages
            let c_terminal = CString::new("terminal").unwrap();
            let mut yes: i64 = 1;
            mpv_set_property(handle, c_terminal.as_ptr(), mpv_format_MPV_FORMAT_FLAG, &mut yes as *mut _ as *mut c_void);
            let c_info = CString::new("info").unwrap();
            mpv_request_log_messages(handle, c_info.as_ptr());

            MpvHandle(handle)
        }
    }

    fn get(&self) -> *mut mpv_handle {
        self.0
    }

    fn stop(&self) {
        let scmd = CString::new("stop").unwrap();
        let mut sargs = [scmd.as_ptr(), ptr::null()];
        unsafe {
            mpv_command(self.get(), sargs.as_mut_ptr());
            mpv_terminate_destroy(self.get());
        }
    }
}

unsafe impl Send for MpvHandle {}
unsafe impl Sync for MpvHandle {}

// MpvRenderContext wrapper
#[derive(Clone)]
struct MpvRenderCtx(*mut mpv_render_context);

impl MpvRenderCtx {
    fn get(&self) -> *mut mpv_render_context {
        self.0
    }
}

unsafe impl Send for MpvRenderCtx {}
unsafe impl Sync for MpvRenderCtx {}

unsafe extern "C" fn get_proc_address_mpv(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    let get_proc_address = &*(ctx as *const &dyn Fn(&CStr) -> *const c_void);
    let name = CStr::from_ptr(name);
    get_proc_address(name) as *mut c_void
}

fn format_time(seconds: i64) -> String {
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("[AMP] Starting up...");
    std::env::set_var("LC_NUMERIC", "C");

    unsafe {
        let c_locale = CString::new("C").unwrap();
        libc::setlocale(libc::LC_ALL, c_locale.as_ptr());
        libc::setlocale(libc::LC_NUMERIC, c_locale.as_ptr());
    }

    eprintln!("[AMP] Initializing UI...");
    let ui = PlayerWindow::new()?;

    eprintln!("[AMP] Initializing Plugin Manager...");

    let mut plugin_manager = PluginManager::new();
    plugin_manager.register_builtin_plugin(Arc::new(JellyfinFactory));
    plugin_manager.register_builtin_plugin(Arc::new(DiscordExtensionFactory));

    let plugins_dir = directories::ProjectDirs::from("com", "amp", "AMP")
        .map(|proj_dirs| proj_dirs.config_dir().join("plugins"))
        .expect("Shouldn't be empty");

    if plugins_dir.exists() {
        eprintln!("[AMP] Scanning for plugins in {:?}", plugins_dir);
        if let Ok(entries) = std::fs::read_dir(plugins_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .map_or(false, |ext| ext == "so" || ext == "dll")
                {
                    eprintln!("[AMP] Loading plugin: {:?}", path);
                    unsafe {
                        if let Err(e) = plugin_manager.load_plugin(&path) {
                            eprintln!("Failed to load plugin {:?}: {}", path, e);
                        }
                    }
                }
            }
        }
    }

    let extensions: Vec<Arc<dyn PlaybackExtension>> = plugin_manager.with_capability(PluginCapability::PlaybackExtension)
        .iter()
        .filter_map(|p| p.create_extension().ok())
        .collect();
    let extensions = Arc::new(extensions);

    let provider_list: Vec<ProviderMetadata> = plugin_manager.with_capability(PluginCapability::MediaProvider)
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
    
    let nav_stack = Arc::new(Mutex::new(Vec::<(String, String)>::new())); // (id, name)
    let current_items_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let next_up_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let current_item_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let current_title = Arc::new(Mutex::new(String::new()));
    let current_artist = Arc::new(Mutex::new(String::new()));

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
                        unsafe {
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
                            let res = mpv_render_context_create(&mut ctx, mpv_h.get(), params.as_mut_ptr());
                            if res >= 0 {
                                *mpv_r.borrow_mut() = Some(MpvRenderCtx(ctx));
                                eprintln!("[AMP] MPV Render Context created successfully");
                            } else {
                                eprintln!("[AMP] Failed to create MPV Render Context: {}", res);
                            }
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
                                                if let Ok(name) = CString::new(s) {
                                                    get_proc_address(&name) as *const _
                                                } else {
                                                    std::ptr::null()
                                                }
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
                                            // Handle error
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

    // --- Navigation Logic ---

    async fn load_folder(
        client: DynProvider,
        folder_id: Option<String>,
        folder_name: String,
        ui_weak: slint::Weak<PlayerWindow>,
        current_items_ids: Arc<Mutex<Vec<String>>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let items_res = if let Some(ref id) = folder_id {
            client.get_children(id).await
        } else {
            client.get_root().await
        };

        let items = match items_res {
            Ok(items) => items,
            Err(e) => {
                set_loading(ui_weak, false);
                return Err(e);
            }
        };

        let mut fetch_futures = Vec::new();
        for i in items {
            let client_clone = client.clone();
            fetch_futures.push(async move {
                let buffer = client_clone.get_item_image_buffer(&i.id).await.ok();
                (i, buffer)
            });
        }

        let results = futures::future::join_all(fetch_futures).await;

        let ui_weak_clone = ui_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak_clone.upgrade() {
                let mut ids = Vec::new();
                let mut grid_items = Vec::new();

                for (item, buffer) in results {
                    ids.push(item.id.clone());

                    grid_items.push(GridItem {
                        id: item.id.into(),
                        name: item.name.into(),
                        thumbnail: buffer.map(|r| image_from_raw(r)).unwrap_or_default(),
                        description: if item.item_type == MediaItemType::Playable {
                            item.duration_secs.map(|s| format_time(s)).unwrap_or_default().into()
                        } else {
                            "".into()
                        },
                        meta: if item.item_type == MediaItemType::Playable {
                            item.resume_position_secs.map(|s| format_time(s)).unwrap_or_default().into()
                        } else {
                            "".into()
                        },
                        series_name: item.series_name.unwrap_or_default().into(),
                        is_folder: item.item_type == MediaItemType::Folder,
                        index: item.index.unwrap_or(0)
                    });
                }

                *current_items_ids.lock().unwrap() = ids;
                ui.set_current_items(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(grid_items))));
                ui.set_current_folder_name(folder_name.clone().into());
                eprintln!("[AMP] Switching to library screen: {}", folder_name);
                ui.set_current_screen("library".into());
                ui.set_is_loading(false);
            }
        });
        Ok(())
    }

    async fn populate_home(
        client: DynProvider,
        ui_weak: slint::Weak<PlayerWindow>,
        provider_arc: Arc<Mutex<Option<DynProvider>>>,
        current_items_ids: Arc<Mutex<Vec<String>>>,
        nu_ids_arc: Arc<Mutex<Vec<String>>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let next_up = client.get_next_up().await.unwrap_or_default();

        let mut next_up_futures = Vec::new();
        for s in next_up {
            let client_clone = client.clone();
            next_up_futures.push(async move {
                let buffer = client_clone.get_item_image_buffer(&s.id).await.ok();
                (s, buffer)
            });
        }

        let next_up_results = futures::future::join_all(next_up_futures).await;

        {
            let mut provider_lock = provider_arc.lock().unwrap();
            *provider_lock = Some(client.clone());
        }

        let ui_weak_clone = ui_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak_clone.upgrade() {
                let mut nu_ids = Vec::new();
                let mut nu_list = Vec::new();

                for (item, buffer) in next_up_results {
                    nu_ids.push(item.id.clone());
                    nu_list.push(GridItem {
                        id: item.id.into(),
                        name: item.name.into(),
                        thumbnail: buffer.map(|r| image_from_raw(r)).unwrap_or_default(),
                        description: format!("S{}E{}", item.season_index.unwrap_or(0), item.index.unwrap_or(0)).into(),
                        meta: "".into(), // We use series_name field now
                        series_name: item.series_name.unwrap_or_default().into(),
                        is_folder: false,
                        index: item.index.unwrap_or(0)
                    });
                }

                *nu_ids_arc.lock().unwrap() = nu_ids;
                ui.set_next_up_list(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(nu_list))));
            }
        });

        load_folder(client, None, "Library".into(), ui_weak, current_items_ids).await
    }

    // --- Callbacks ---

    let ui_prov_select = ui.as_weak();
    let pm_prov_select = plugin_manager.clone();

    ui.on_select_provider(move |id| {
        if let Some(ui) = ui_prov_select.upgrade() {
            ui.set_selected_provider_id(id.clone());
            let pm = pm_prov_select.lock().unwrap();
            let fields = pm.get_plugin(&id).map(|f| f.config_fields()).unwrap_or_default();
            let slint_fields: Vec<ConfigFieldMetadata> = fields.into_iter().map(|f| ConfigFieldMetadata {
                key: f.key.into(), label: f.label.into(), is_password: f.is_password, value: f.default_value.into(),
            }).collect();
            ui.set_config_fields(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(slint_fields))));
            ui.set_current_screen("login".into());
        }
    });

    let ui_login_cb = ui.as_weak();
    let pm_cb = plugin_manager.clone();
    let prov_cb = provider.clone();
    let items_cb = current_items_ids.clone();
    let nu_cb = next_up_ids.clone();
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
        let prov_arc = prov_cb.clone();
        let items_arc = items_cb.clone();
        let nu_arc = nu_cb.clone();
        let provider_id = provider_id.to_string();

        tokio::spawn(async move {
            let factory = pm.lock().unwrap().get_plugin(&provider_id);
            if let Some(factory) = factory {
                let client_res = factory.create_provider(config).await;
                match client_res {
                    Ok(client) => {
                        let cache = LoginCache {
                            provider_id: provider_id.clone(),
                            config: client.get_persistable_config(),
                        };

                        let _ = cache.save();

                        if let Err(e) = populate_home(client, ui_weak.clone(), prov_arc, items_arc, nu_arc).await {
                            let msg = format!("Failed to load library: {}", e);
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = ui_weak.upgrade() {
                                    ui.set_error_message(msg.into());
                                    ui.set_is_loading(false);
                                }
                            });
                        }
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
    let prov_nav = provider.clone();
    let items_nav = current_items_ids.clone();
    let stack_nav = nav_stack.clone();
    let mpv_nav = mpv.clone();
    let item_id_arc = current_item_id.clone();
    let title_arc = current_title.clone();
    let artist_arc = current_artist.clone();

    ui.on_select_item(move |index| {
        let ui = ui_nav.upgrade().unwrap();
        let prov_opt = prov_nav.lock().unwrap().clone();
        if let Some(prov) = prov_opt {
            let id = items_nav.lock().unwrap().get(index as usize).cloned().unwrap();
            let item = ui.get_current_items().row_data(index as usize).unwrap();
            
            if item.is_folder {
                ui.set_is_loading(true);
                let ui_weak = ui_nav.clone();
                let items_arc = items_nav.clone();
                let id_clone = id.clone();
                stack_nav.lock().unwrap().push((id.clone(), item.name.to_string()));
                tokio::spawn(async move { let _ = load_folder(prov, Some(id_clone), item.name.to_string(), ui_weak, items_arc).await; });
            } else {
                let mpv_h = mpv_nav.clone();
                let item_id_p = item_id_arc.clone();
                let ui_weak = ui_nav.clone();

                *title_arc.lock().unwrap() = item.name.to_string();
                *artist_arc.lock().unwrap() = item.series_name.to_string();

                tokio::spawn(async move { open_player(prov, item_id_p, mpv_h, ui_weak, id).await; });
            }
        }
    });

    let ui_nu = ui.as_weak();
    let prov_nu = provider.clone();
    let nu_ids_arc = next_up_ids.clone();
    let mpv_nu = mpv.clone();
    let item_id_nu = current_item_id.clone();
    let title_nu = current_title.clone();
    let artist_nu = current_artist.clone();
    ui.on_select_next_up(move |index| {
        let prov_opt = prov_nu.lock().unwrap().clone();
        if let Some(prov) = prov_opt {
            let id = nu_ids_arc.lock().unwrap().get(index as usize).cloned().unwrap();
            let item = ui_nu.upgrade().unwrap().get_next_up_list().row_data(index as usize).unwrap();
            let title_val = item.name.to_string();
            let artist_val = item.series_name.to_string();
            *title_nu.lock().unwrap() = title_val;
            *artist_nu.lock().unwrap() = artist_val;

            let mpv_h = mpv_nu.clone();
            let item_id_p = item_id_nu.clone();
            let ui_weak = ui_nu.clone();

            tokio::spawn(async move { open_player(prov, item_id_p, mpv_h, ui_weak, id).await });
        }
    });

    async fn open_player(provider: DynProvider, item_id_p: Arc<Mutex<Option<String>>>, mpv: MpvHandle, ui_weak: Weak<PlayerWindow>, item_id: String) {
        let resume_pos = provider.get_resume_position(&item_id).await.unwrap_or(None);
        let _ = provider.report_playback_start(&item_id).await;
        let stream_url = provider.get_stream_url(&item_id);
        
        *item_id_p.lock().unwrap() = Some(item_id);
        
        let _ = slint::invoke_from_event_loop(move || {
                if let Some(pos) = resume_pos {
                let c_start = CString::new("start").unwrap();
                let c_pos = if pos > 0 { CString::new(pos.to_string()) } else { CString::new("0") };

                unsafe { mpv_set_property_string(mpv.get(), c_start.as_ptr(), c_pos.unwrap().as_ptr()); }
            }

            let cmd = CString::new("loadfile").unwrap();
            let url = CString::new(stream_url).unwrap();
            let mut args = [cmd.as_ptr(), url.as_ptr(), ptr::null()];
            
            unsafe {
                mpv_command(mpv.get(), args.as_mut_ptr());
            }
            if let Some(ui) = ui_weak.upgrade() { ui.set_current_screen("player".into()); }
        });
    }

    let ui_back = ui.as_weak();
    let prov_back = provider.clone();
    let stack_back = nav_stack.clone();
    let items_back = current_items_ids.clone();
    ui.on_back(move || {
        let ui = ui_back.upgrade().unwrap();
        let mut stack = stack_back.lock().unwrap();
        stack.pop();
        let parent = stack.last().cloned();
        
        ui.set_is_loading(true);
        let prov = prov_back.lock().unwrap().as_ref().unwrap().clone();
        let ui_weak = ui_back.clone();
        let items_arc = items_back.clone();
        tokio::spawn(async move {
            let (pid, pname) = parent.map(|(id, name)| (Some(id), name)).unwrap_or((None, "Library".into()));
            let _ = load_folder(prov, pid, pname, ui_weak, items_arc).await;
        });
    });

    let ui_stop = ui.as_weak();
    let mpv_stop = mpv.clone();
    let _prov_stop = provider.clone();
    let item_id_stop = current_item_id.clone();
    let prov_close = provider.clone();
    let items_close = current_items_ids.clone();
    let nu_close = next_up_ids.clone();
    let stack_close = nav_stack.clone();

    let extensions_close = extensions.clone();
    ui.on_close_player(move || {
        let ui = ui_stop.upgrade().unwrap();
        let mpv_h = mpv_stop.clone();
        let prov_opt = prov_close.lock().unwrap().clone();
        let item_id = item_id_stop.lock().unwrap().take();
        
        for ext in extensions_close.iter() {
            ext.on_playback_stop();
        }
        
        if let (Some(prov), Some(id)) = (prov_opt, item_id) {
            unsafe {
                let mut time: f64 = 0.0;
                let c_time = CString::new("time-pos").unwrap();
                let res = mpv_get_property(mpv_h.get(), c_time.as_ptr(), mpv_format_MPV_FORMAT_DOUBLE, &mut time as *mut _ as *mut c_void);
                
                let time_i64 = if res >= 0 { time as i64 } else { 0 };
                eprintln!("[AMP] Closing player for {}. Current time: {}s (res: {})", id, time_i64, res);

                let scmd = CString::new("stop").unwrap();
                let mut sargs = [scmd.as_ptr(), ptr::null()];
                mpv_command(mpv_h.get(), sargs.as_mut_ptr());

                let ui_weak = ui_stop.clone();
                let prov_clone = prov.clone();
                let items_arc = items_close.clone();
                let nu_arc = nu_close.clone();
                let prov_arc = prov_close.clone();
                let stack_arc = stack_close.clone();

                tokio::spawn(async move {
                    // Send progress
                    let _ = prov_clone.report_playback_progress(&id, time_i64, false).await;
                    let _ = prov_clone.report_playback_stopped(&id, time_i64).await;
                    
                    // Re-fetch data to update resume positions
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    
                    let (pid, pname) = {
                        let stack = stack_arc.lock().unwrap();
                        stack.last().cloned().map(|(id, name)| (Some(id), name)).unwrap_or((None, "Library".into()))
                    };

                    if pid.is_none() {
                        let _ = populate_home(prov_clone, ui_weak, prov_arc, items_arc, nu_arc).await;
                    } else {
                        let _ = load_folder(prov_clone, pid, pname, ui_weak, items_arc).await;
                    }
                });
            }
        }
        ui.set_current_screen("library".into());
    });

    let prov_mark = provider.clone();
    let items_mark = current_items_ids.clone();
    ui.on_mark_as_played(move |index, played| {
        let prov_opt = prov_mark.lock().unwrap().clone();
        if let Some(prov) = prov_opt {
            let id = items_mark.lock().unwrap().get(index as usize).cloned().unwrap();
            tokio::spawn(async move { let _ = prov.mark_as_played(&id, played).await; });
        }
    });

    let _ui_track = ui.as_weak();
    let mpv_track = mpv.clone();
    ui.on_select_audio_track(move |id| {
        let name = CString::new("aid").unwrap();
        let val = CString::new(id.to_string()).unwrap();
        unsafe { mpv_set_property_string(mpv_track.get(), name.as_ptr(), val.as_ptr()); }
    });

    let _ui_sub = ui.as_weak();
    let mpv_sub = mpv.clone();
    ui.on_select_subtitle_track(move |id| {
        let name = CString::new("sid").unwrap();
        let val = CString::new(id.to_string()).unwrap();
        unsafe { mpv_set_property_string(mpv_sub.get(), name.as_ptr(), val.as_ptr()); }
    });

    let last_activity = Arc::new(Mutex::new(std::time::Instant::now()));
    let la_clone = last_activity.clone();
    ui.on_user_activity(move || { *la_clone.lock().unwrap() = std::time::Instant::now(); });

    let mpv_toggle = mpv.clone();
    ui.on_toggle_pause(move || {
        let c_pause = CString::new("pause").unwrap();
        let mut paused: c_int = 0;
        unsafe {
            mpv_get_property(mpv_toggle.get(), c_pause.as_ptr(), mpv_format_MPV_FORMAT_FLAG, &mut paused as *mut _ as *mut c_void);
            let new_p = if paused == 0 { 1 } else { 0 };
            mpv_set_property(mpv_toggle.get(), c_pause.as_ptr(), mpv_format_MPV_FORMAT_FLAG, &new_p as *const _ as *mut c_void);
        }
    });

    let mpv_seek = mpv.clone();
    ui.on_seek(move |perc| {
        let scmd = CString::new("seek").unwrap();
        let sval = CString::new(perc.to_string()).unwrap();
        let smode = CString::new("absolute-percent").unwrap();
        let mut sargs = [scmd.as_ptr(), sval.as_ptr(), smode.as_ptr(), ptr::null()];
        unsafe { mpv_command(mpv_seek.get(), sargs.as_mut_ptr()); }
    });

    let mpv_skip = mpv.clone();
    ui.on_skip_by(move |secs| {
        let scmd = CString::new("seek").unwrap();
        let sval = CString::new(secs.to_string()).unwrap();
        let mut sargs = [scmd.as_ptr(), sval.as_ptr(), ptr::null()];
        unsafe { mpv_command(mpv_skip.get(), sargs.as_mut_ptr()); }
    });

    let ui_logout = ui.as_weak();
    let prov_logout = provider.clone();
    ui.on_logout(move || {
        if let Some(ui) = ui_logout.upgrade() {
            *prov_logout.lock().unwrap() = None;
            LoginCache::delete();
            ui.set_current_screen("provider_select".into());
        }
    });

    // Rendering Timer
    let render_timer = slint::Timer::default();
    let ui_render = ui.as_weak();
    let mpv_r = mpv_render.clone();
    let mpv_h = mpv.clone();
    let la_timer = last_activity.clone();
    let last_report = Arc::new(Mutex::new(std::time::Instant::now()));
    let prov_timer = provider.clone();
    let item_id_timer = current_item_id.clone();

    let extensions_timer = extensions.clone();
    let last_ext_update = Arc::new(Mutex::new(std::time::Instant::now()));
    let title_timer = current_title.clone();
    let artist_timer = current_artist.clone();

    render_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(16), move || {
        let ui = match ui_render.upgrade() { Some(ui) => ui, None => return };

        if let Ok(last) = la_timer.lock() {
            let is_p = ui.get_is_paused();
            ui.set_controls_visible(last.elapsed() < std::time::Duration::from_secs(3) || is_p);
        }

        unsafe {
            loop {
                let ev = mpv_wait_event(mpv_h.get(), 0.0);
                if (*ev).event_id == mpv_event_id_MPV_EVENT_NONE { break; }
            }

            if let Some(rctx) = mpv_r.borrow().as_ref() {
                if (mpv_render_context_update(rctx.get()) & 1) != 0 { ui.window().request_redraw(); }
            }

            let mut time: f64 = 0.0;
            let mut dur: i64 = 0;
            let mut perc: f64 = 0.0;
            let mut paused: c_int = 0;
            let c_time = CString::new("time-pos").unwrap();
            let c_dur = CString::new("duration").unwrap();
            let c_perc = CString::new("percent-pos").unwrap();
            let c_pause = CString::new("pause").unwrap();
            if mpv_get_property(mpv_h.get(), c_perc.as_ptr(), mpv_format_MPV_FORMAT_DOUBLE, &mut perc as *mut _ as *mut c_void) >= 0 { ui.set_progress(perc as f32); }
            if mpv_get_property(mpv_h.get(), c_time.as_ptr(), mpv_format_MPV_FORMAT_DOUBLE, &mut time as *mut _ as *mut c_void) >= 0 { ui.set_time_pos(format_time(time as i64).into()); }
            if mpv_get_property(mpv_h.get(), c_dur.as_ptr(), mpv_format_MPV_FORMAT_INT64, &mut dur as *mut _ as *mut c_void) >= 0 { ui.set_duration(format_time(dur).into()); }
            if mpv_get_property(mpv_h.get(), c_pause.as_ptr(), mpv_format_MPV_FORMAT_FLAG, &mut paused as *mut _ as *mut c_void) >= 0 { ui.set_is_paused(paused != 0); }

            if ui.get_current_screen() == "player" {
                if last_ext_update.lock().unwrap().elapsed() >= std::time::Duration::from_secs(1) {
                    *last_ext_update.lock().unwrap() = std::time::Instant::now();
                    
                    let info = PlaybackInfo {
                        title: title_timer.lock().unwrap().clone(),
                        artist: artist_timer.lock().unwrap().clone(),
                        is_paused: paused != 0,
                        position_secs: time as i64,
                        duration_secs: dur,
                    };

                    for ext in extensions_timer.iter() {
                        ext.on_playback_update(info.clone());
                    }
                }
            } else {
                if last_ext_update.lock().unwrap().elapsed() >= std::time::Duration::from_secs(1) {
                    *last_ext_update.lock().unwrap() = std::time::Instant::now();
                    for ext in extensions_timer.iter() {
                        ext.on_playback_stop();
                    }
                }
            }

            if let Ok(mut last) = last_report.lock() {
                if last.elapsed() >= std::time::Duration::from_secs(10) {
                    *last = std::time::Instant::now();
                    let prov = prov_timer.lock().unwrap().clone();
                    let iid = item_id_timer.lock().unwrap().clone();
                    if let (Some(p), Some(id)) = (prov, iid) {
                        let time_i64 = time as i64;
                        tokio::spawn(async move { let _ = p.report_playback_progress(&id, time_i64, paused != 0).await; });
                    }
                }
            }

            let c_tracks = CString::new("track-list").unwrap();
            let tptr = mpv_get_property_string(mpv_h.get(), c_tracks.as_ptr());
            if !tptr.is_null() {
                let js = CStr::from_ptr(tptr).to_string_lossy();
                if let Ok(tracks) = serde_json::from_str::<Vec<Track>>(&js) {
                    let mut alist = Vec::new(); let mut slist = Vec::new();
                    for t in &tracks {
                        match t.track_type {
                            TrackType::Audio => alist.push(t.as_track_info()),
                            TrackType::Sub => slist.push(t.as_track_info()),
                            _ => ()
                        }
                    }
                    ui.set_audio_tracks(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(alist))));
                    ui.set_subtitle_tracks(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(slist))));
                }
                mpv_free(tptr as *mut c_void);
            }
        }
    });

    if let Some(cache) = LoginCache::load() {
        ui.set_is_loading(true);
        let ui_weak = ui.as_weak();
        let pm = plugin_manager.clone();
        let prov_arc = provider.clone();
        let items_arc = current_items_ids.clone();
        let nu_arc = next_up_ids.clone();
        tokio::spawn(async move {
            let factory = pm.lock().unwrap().get_plugin(&cache.provider_id);
            if let Some(factory) = factory {
                if let Ok(client) = factory.create_provider(cache.config).await {
                    let _ = populate_home(client, ui_weak, prov_arc, items_arc, nu_arc).await;
                } else {
                    set_loading(ui_weak, false);
                }
            } else {
                set_loading(ui_weak, false);
            }
        });
    }

    eprintln!("[AMP] Entering main loop...");
    
    let result = ui.run();

    eprintln!("[AMP] Main loop exited.");

    mpv.stop();

    Ok(result?)
}

#[derive(Deserialize, Debug)]
struct Track { id: i32, #[serde(rename = "type")] track_type: TrackType, #[serde(rename = "selected", default)] active: bool, #[serde(default)] title: Option<String>, #[serde(default)] lang: Option<String>, #[serde(default)] codec: String }
impl Track {
    fn as_track_info(&self) -> TrackInfo {
        let name = self.title.as_ref().or(self.lang.as_ref()).cloned().unwrap_or_else(|| "Unknown".into());
        TrackInfo { active: self.active, id: self.id, name: if self.codec.is_empty() { name.into() } else { format!("{} ({})", name, self.codec).into() } }
    }
}
#[derive(Deserialize, PartialEq, Debug)] #[serde(rename_all = "lowercase")] enum TrackType { Audio, Video, Sub }

fn image_from_raw(raw: RawImage) -> Image {
    let slint_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &raw.rgba8,
        raw.width,
        raw.height,
    );

    Image::from_rgba8(slint_buf)
}


fn set_loading(ui_weak: Weak<PlayerWindow>, status: bool) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_is_loading(status);
        }
    });
}