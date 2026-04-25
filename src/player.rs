use crate::app_state::AppState;
use libmpv_sys::*;
use slint::Weak;
use std::ffi::CString;
use std::os::raw::c_void;
use std::ptr;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MpvHandle(*mut mpv_handle);

impl MpvHandle {
    pub fn new() -> Self {
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
                ("terminal", "no"),
                ("stop-screensaver", "yes"),
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

            let c_warn = CString::new("warn").unwrap();
            mpv_request_log_messages(handle, c_warn.as_ptr());

            MpvHandle(handle)
        }
    }

    pub fn get(&self) -> *mut mpv_handle {
        self.0
    }

    pub fn stop(&self) {
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

#[derive(Clone)]
pub struct MpvRenderCtx(pub *mut mpv_render_context);

impl MpvRenderCtx {
    pub fn get(&self) -> *mut mpv_render_context {
        self.0
    }
}

unsafe impl Send for MpvRenderCtx {}
unsafe impl Sync for MpvRenderCtx {}

pub async fn open_player(
    ui_weak: Weak<crate::PlayerWindow>,
    state_arc: Arc<Mutex<AppState>>,
    mpv: MpvHandle,
) {
    let (provider, item_id) = {
        let state = state_arc.lock().unwrap();

        if state.current_item_id.is_none() {
            return;
        }

        let (provider_id, i_id) = state.current_item_id.clone().unwrap();

        (state.active_providers.get(&provider_id).cloned(), i_id)
    };

    let provider = match provider {
        Some(p) => p,
        None => return,
    };

    let resume_pos = provider.get_resume_position(&item_id).await.unwrap_or(None);
    let _ = provider.report_playback_start(&item_id).await;
    let stream_url = provider.get_stream_url(&item_id);

    let (has_prev, has_next) = {
        let state = state_arc.lock().unwrap();
        if let Some((items, idx)) = state.active_playlist.as_ref() {
            (*idx > 0, *idx < items.len() - 1)
        } else {
            (false, false)
        }
    };

    let _ = slint::invoke_from_event_loop(move || {
        if let Some(pos) = resume_pos {
            let c_start = CString::new("start").unwrap();
            let c_pos = if pos > 0 {
                CString::new(pos.to_string()).unwrap()
            } else {
                CString::new("0").unwrap()
            };

            unsafe {
                mpv_set_property_string(mpv.get(), c_start.as_ptr(), c_pos.as_ptr());
            }
        }

        let cmd = CString::new("loadfile").unwrap();
        let url = CString::new(stream_url).unwrap();
        let mut args = [cmd.as_ptr(), url.as_ptr(), ptr::null()];
        unsafe {
            mpv_command(mpv.get(), args.as_mut_ptr());
        }

        if let Some(ui) = ui_weak.upgrade() {
            ui.set_current_screen("player".into());
            ui.set_has_next(has_next);
            ui.set_has_previous(has_prev);
            ui.set_is_loading(false);
        }
    });
}
