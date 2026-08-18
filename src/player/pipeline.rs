pub fn configure_hardware_acceleration(mpv: *mut libmpv_sys::mpv_handle) {
    let hwdec = std::ffi::CString::new("hwdec").unwrap();
    // Disable hardware decoding to guarantee clean, artifact-free frame rendering into OpenGL FBO
    let hwdec_val = std::ffi::CString::new("no").unwrap();
    unsafe {
        libmpv_sys::mpv_set_property_string(mpv, hwdec.as_ptr(), hwdec_val.as_ptr());
    }
}
