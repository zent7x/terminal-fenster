//! Display refresh probing for default paint-rate selection.

/// Preferred max paint rate when the user does not pass `--fps` / `TERMINAL_FENSTER_FPS`.
pub fn default_fps() -> u32 {
    let hz = probe_refresh_hz();
    if hz >= 100 {
        hz.min(240)
    } else {
        60
    }
}

fn probe_refresh_hz() -> u32 {
    #[cfg(target_os = "macos")]
    {
        if let Some(hz) = macos_main_display_hz() {
            return hz;
        }
    }
    120
}

#[cfg(target_os = "macos")]
fn macos_main_display_hz() -> Option<u32> {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayCopyDisplayMode(display: u32) -> *const std::ffi::c_void;
        fn CGDisplayModeGetRefreshRate(mode: *const std::ffi::c_void) -> f64;
        fn CGDisplayModeRelease(mode: *const std::ffi::c_void);
    }

    unsafe {
        let mode = CGDisplayCopyDisplayMode(CGMainDisplayID());
        if mode.is_null() {
            return None;
        }
        let hz = CGDisplayModeGetRefreshRate(mode);
        CGDisplayModeRelease(mode);
        if hz.is_finite() && hz >= 30.0 {
            Some(hz.round().max(1.0) as u32)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_fps_is_sane() {
        let fps = default_fps();
        assert!((60..=240).contains(&fps));
    }
}
