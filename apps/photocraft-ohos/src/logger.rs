#[cfg(target_env = "ohos")]
unsafe extern "C" {
    fn craft_log(level: i32, message: *const std::ffi::c_char);
}

pub fn info(message: &str) {
    #[cfg(target_env = "ohos")]
    if let Ok(message) = std::ffi::CString::new(message.replace('\0', " ")) {
        // SAFETY: Native logger only reads this CString synchronously and does not call Rust.
        unsafe { craft_log(0, message.as_ptr()) };
    }
    #[cfg(not(target_env = "ohos"))]
    eprintln!("PhotoCraft: {message}");
}
