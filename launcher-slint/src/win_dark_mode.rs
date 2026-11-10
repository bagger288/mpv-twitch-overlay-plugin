//! Win32 DWM dark mode and titlebar color styling for Windows 10/11.

#[cfg(target_os = "windows")]
mod imp {
    use std::ffi::c_void;

    type HWND = *mut c_void;
    type BOOL = i32;
    type DWORD = u32;
    type HRESULT = i32;
    type COLORREF = u32;

    const DWMWA_USE_IMMERSIVE_DARK_MODE_BEFORE_20H1: DWORD = 19;
    const DWMWA_USE_IMMERSIVE_DARK_MODE: DWORD = 20;
    const DWMWA_WINDOW_CORNER_PREFERENCE: DWORD = 33;
    const DWMWA_CAPTION_COLOR: DWORD = 35;
    const DWMWA_TEXT_COLOR: DWORD = 36;

    const DWMWCP_ROUND: DWORD = 2;

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: HWND,
            dwAttribute: DWORD,
            pvAttribute: *const c_void,
            cbAttribute: DWORD,
        ) -> HRESULT;
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetCurrentProcessId() -> DWORD;
        fn GetWindowThreadProcessId(hWnd: HWND, lpdwProcessId: *mut DWORD) -> DWORD;
        fn EnumWindows(
            lpEnumFunc: unsafe extern "system" fn(HWND, isize) -> BOOL,
            lParam: isize,
        ) -> BOOL;
        fn IsWindowVisible(hWnd: HWND) -> BOOL;
    }

    struct EnumData {
        target_pid: DWORD,
        hwnd: HWND,
    }

    unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: isize) -> BOOL {
        let data = &mut *(lparam as *mut EnumData);
        let mut pid: DWORD = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == data.target_pid && IsWindowVisible(hwnd) != 0 {
            data.hwnd = hwnd;
            return 0; // Found main window, stop enumeration
        }
        1 // Continue
    }

    pub fn apply() {
        unsafe {
            let my_pid = GetCurrentProcessId();
            let mut data = EnumData {
                target_pid: my_pid,
                hwnd: std::ptr::null_mut(),
            };
            EnumWindows(enum_windows_proc, &mut data as *mut _ as isize);

            if data.hwnd.is_null() {
                return;
            }

            let hwnd = data.hwnd;

            // 1. Immersive dark mode (Windows 10 20H1+ and Windows 11)
            let dark_mode: BOOL = 1;
            let hr = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark_mode as *const BOOL as *const c_void,
                std::mem::size_of::<BOOL>() as DWORD,
            );
            if hr != 0 {
                // Fallback for older Windows 10 (1809-1909)
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_USE_IMMERSIVE_DARK_MODE_BEFORE_20H1,
                    &dark_mode as *const BOOL as *const c_void,
                    std::mem::size_of::<BOOL>() as DWORD,
                );
            }

            // 2. Custom caption background color matching #18181b (0x00BBGGRR -> 0x001B1818)
            let caption_color: COLORREF = 0x001B1818;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_CAPTION_COLOR,
                &caption_color as *const COLORREF as *const c_void,
                std::mem::size_of::<COLORREF>() as DWORD,
            );

            // 3. Custom caption text color: white (0x00FFFFFF)
            let text_color: COLORREF = 0x00FFFFFF;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_TEXT_COLOR,
                &text_color as *const COLORREF as *const c_void,
                std::mem::size_of::<COLORREF>() as DWORD,
            );

            // 4. Windows 11 rounded corners
            let corner_pref: DWORD = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner_pref as *const DWORD as *const c_void,
                std::mem::size_of::<DWORD>() as DWORD,
            );
        }
    }
}

pub fn apply_dark_titlebar() {
    #[cfg(target_os = "windows")]
    imp::apply();
}
