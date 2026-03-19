/// Process camouflage — set a custom process name to avoid detection in `ps`/`top`.

/// Apply process name camouflage via prctl (Linux only).
/// Sets both the kernel thread name (15-char limit) and /proc/self/comm.
pub fn apply_camouflage(name: &str) {
    #[cfg(target_os = "linux")]
    {
        // Truncate to 15 chars (kernel limit for PR_SET_NAME)
        let truncated: String = name.chars().take(15).collect();
        if let Ok(name_cstring) = std::ffi::CString::new(truncated.as_str()) {
            unsafe {
                libc::prctl(libc::PR_SET_NAME, name_cstring.as_ptr(), 0, 0, 0);
            }
        }
        // Also update /proc/self/comm for tools that read it directly
        let _ = std::fs::write("/proc/self/comm", format!("{}\n", truncated));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = name;
    }
}
