/// Process camouflage — set a custom process name to avoid detection in `ps`/`top`.

/// Apply process name camouflage.
/// Linux: prctl to set kernel thread name + /proc/self/comm.
/// macOS: pthread_setname_np to set thread name visible in Activity Monitor.
pub fn apply_camouflage(name: &str) {
    #[cfg(target_os = "linux")]
    {
        let truncated: String = name.chars().take(15).collect();
        if let Ok(name_cstring) = std::ffi::CString::new(truncated.as_str()) {
            unsafe {
                libc::prctl(libc::PR_SET_NAME, name_cstring.as_ptr(), 0, 0, 0);
            }
        }
        let _ = std::fs::write("/proc/self/comm", format!("{}\n", truncated));
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(name_cstring) = std::ffi::CString::new(name) {
            unsafe {
                libc::pthread_setname_np(name_cstring.as_ptr());
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = name;
    }
}
