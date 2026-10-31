//! mpv C API FFI bindings and safe wrapper
//! Supports dynamic symbol resolution in-process on Windows so no link-time mpv.lib is required.

use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::io::Write;
use std::ptr;

pub fn log_plugin(msg: &str) {
    eprintln!("[twitch_chat_rust] {}", msg);
    let log_path = std::env::temp_dir().join("mpv_twitch_chat_rust.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{}] {}", timestamp, msg);
    }
}

pub type MpvHandleRaw = *mut c_void;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MpvFormat {
    None = 0,
    String = 1,
    OsdString = 2,
    Flag = 3,
    Int64 = 4,
    Double = 5,
    Node = 6,
    NodeArray = 7,
    NodeMap = 8,
    ByteArray = 9,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MpvEventId {
    None = 0,
    Shutdown = 1,
    LogMessage = 2,
    GetPropertyReply = 3,
    SetPropertyReply = 4,
    CommandReply = 5,
    StartFile = 6,
    EndFile = 7,
    FileLoaded = 8,
    Idle = 11,
    Tick = 14,
    ClientMessage = 16,
    VideoReconfig = 17,
    AudioReconfig = 18,
    Seek = 20,
    PlaybackRestart = 21,
    PropertyChange = 22,
    QueueOverflow = 24,
    Hook = 25,
}

#[repr(C)]
pub struct MpvEventProperty {
    pub name: *const c_char,
    pub format: MpvFormat,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct MpvEvent {
    pub event_id: MpvEventId,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct MpvNode {
    pub u: MpvNodeUnion,
    pub format: MpvFormat,
}

#[repr(C)]
pub union MpvNodeUnion {
    pub string: *mut c_char,
    pub flag: c_int,
    pub int64: i64,
    pub double_: c_double,
    pub list: *mut MpvNodeList,
    pub ba: *mut c_void,
}

#[repr(C)]
pub struct MpvNodeList {
    pub num: c_int,
    pub values: *mut MpvNode,
    pub keys: *mut *mut c_char,
}

// Function pointer signatures for mpv C API
pub type FnClientName = unsafe extern "C" fn(ctx: MpvHandleRaw) -> *const c_char;
pub type FnWaitEvent = unsafe extern "C" fn(ctx: MpvHandleRaw, timeout: c_double) -> *mut MpvEvent;
pub type FnCommand = unsafe extern "C" fn(ctx: MpvHandleRaw, args: *mut *const c_char) -> c_int;
pub type FnCommandString = unsafe extern "C" fn(ctx: MpvHandleRaw, args: *const c_char) -> c_int;
pub type FnObserveProperty = unsafe extern "C" fn(ctx: MpvHandleRaw, reply_userdata: u64, name: *const c_char, format: MpvFormat) -> c_int;
pub type FnSetProperty = unsafe extern "C" fn(ctx: MpvHandleRaw, name: *const c_char, format: MpvFormat, data: *mut c_void) -> c_int;
pub type FnSetPropertyString = unsafe extern "C" fn(ctx: MpvHandleRaw, name: *const c_char, data: *const c_char) -> c_int;
pub type FnGetProperty = unsafe extern "C" fn(ctx: MpvHandleRaw, name: *const c_char, format: MpvFormat, data: *mut c_void) -> c_int;
pub type FnGetPropertyString = unsafe extern "C" fn(ctx: MpvHandleRaw, name: *const c_char) -> *mut c_char;
pub type FnFree = unsafe extern "C" fn(data: *mut c_void);

// Exported symbols matching mpv's MPV_CPLUGIN_DYNAMIC_SYM ABI
// mpv's Windows C-plugin loader uses GetProcAddress(dll, "pfn_mpv_...") and populates these before calling mpv_open_cplugin.
#[no_mangle] pub static mut pfn_mpv_client_name: Option<FnClientName> = None;
#[no_mangle] pub static mut pfn_mpv_wait_event: Option<FnWaitEvent> = None;
#[no_mangle] pub static mut pfn_mpv_command: Option<FnCommand> = None;
#[no_mangle] pub static mut pfn_mpv_command_string: Option<FnCommandString> = None;
#[no_mangle] pub static mut pfn_mpv_observe_property: Option<FnObserveProperty> = None;
#[no_mangle] pub static mut pfn_mpv_set_property: Option<FnSetProperty> = None;
#[no_mangle] pub static mut pfn_mpv_set_property_string: Option<FnSetPropertyString> = None;
#[no_mangle] pub static mut pfn_mpv_get_property: Option<FnGetProperty> = None;
#[no_mangle] pub static mut pfn_mpv_get_property_string: Option<FnGetPropertyString> = None;
#[no_mangle] pub static mut pfn_mpv_free: Option<FnFree> = None;

// Additional exported placeholder pointers that mpv may attempt to populate
#[no_mangle] pub static mut pfn_mpv_client_api_version: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_error_string: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_create: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_initialize: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_destroy: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_terminate_destroy: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_create_client: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_create_weak_client: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_load_config_file: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_get_time_us: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_request_event: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_request_log_messages: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_wakeup: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_set_wakeup_callback: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_wait_async_requests: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_hook_add: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_hook_continue: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_get_property_osd_string: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_set_property_async: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_get_property_async: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_unobserve_property: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_command_slice: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_command_async: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_command_node: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_command_node_async: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_abort_async_command: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_set_option: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_set_option_string: *mut c_void = ptr::null_mut();
#[no_mangle] pub static mut pfn_mpv_free_node_contents: *mut c_void = ptr::null_mut();

struct MpvVTable {
    client_name: FnClientName,
    wait_event: FnWaitEvent,
    command: FnCommand,
    command_string: FnCommandString,
    observe_property: FnObserveProperty,
    set_property: FnSetProperty,
    set_property_string: FnSetPropertyString,
    get_property: FnGetProperty,
    get_property_string: FnGetPropertyString,
    free: FnFree,
}

static mut VTABLE: Option<MpvVTable> = None;

#[cfg(target_os = "windows")]
unsafe fn resolve_symbol<T>(symbol_name: &str) -> Option<T> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(lpModuleName: *const u16) -> *mut c_void;
        fn GetProcAddress(hModule: *mut c_void, lpProcName: *const c_char) -> *mut c_void;
        fn GetCurrentProcess() -> *mut c_void;
        fn K32EnumProcessModules(hProcess: *mut c_void, lphModule: *mut *mut c_void, cb: u32, lpcbNeeded: *mut u32) -> i32;
    }

    let c_sym = CString::new(symbol_name).ok()?;

    // 1. Search in current process (.exe) first
    let h_exe = GetModuleHandleW(ptr::null());
    if !h_exe.is_null() {
        let p = GetProcAddress(h_exe, c_sym.as_ptr());
        if !p.is_null() {
            return Some(std::mem::transmute_copy(&p));
        }
    }

    // 2. Try common mpv dll names if hosted as a library
    for module_name in &[
        "mpv-2.dll", "libmpv-2.dll", "mpv-1.dll", "libmpv-1.dll", 
        "mpv.dll", "libmpv.dll", "mpvnet.exe", "mpv.exe"
    ] {
        let wide: Vec<u16> = OsStr::new(module_name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let h_mod = GetModuleHandleW(wide.as_ptr());
        if !h_mod.is_null() {
            let p = GetProcAddress(h_mod, c_sym.as_ptr());
            if !p.is_null() {
                return Some(std::mem::transmute_copy(&p));
            }
        }
    }

    // 3. Fallback: enumerate all loaded modules in this process
    let mut modules = [ptr::null_mut::<c_void>(); 256];
    let mut cb_needed: u32 = 0;
    if K32EnumProcessModules(
        GetCurrentProcess(),
        modules.as_mut_ptr(),
        (modules.len() * std::mem::size_of::<*mut c_void>()) as u32,
        &mut cb_needed,
    ) != 0 {
        let count = (cb_needed as usize / std::mem::size_of::<*mut c_void>()).min(modules.len());
        for &h_mod in &modules[..count] {
            if !h_mod.is_null() {
                let p = GetProcAddress(h_mod, c_sym.as_ptr());
                if !p.is_null() {
                    return Some(std::mem::transmute_copy(&p));
                }
            }
        }
    }

    None
}

#[cfg(not(target_os = "windows"))]
unsafe fn resolve_symbol<T>(_symbol_name: &str) -> Option<T> {
    None
}

pub unsafe fn init_vtable() -> bool {
    if VTABLE.is_some() {
        return true;
    }

    let client_name = match pfn_mpv_client_name.or_else(|| resolve_symbol::<FnClientName>("mpv_client_name")) {
        Some(f) => f,
        None => {
            log_plugin("init_vtable: failed to resolve mpv_client_name (neither dynamic sym nor GetProcAddress)");
            return false;
        }
    };
    let wait_event = match pfn_mpv_wait_event.or_else(|| resolve_symbol::<FnWaitEvent>("mpv_wait_event")) {
        Some(f) => f,
        None => {
            log_plugin("init_vtable: failed to resolve mpv_wait_event");
            return false;
        }
    };
    let command = match pfn_mpv_command.or_else(|| resolve_symbol::<FnCommand>("mpv_command")) {
        Some(f) => f,
        None => {
            log_plugin("init_vtable: failed to resolve mpv_command");
            return false;
        }
    };
    let command_string = match pfn_mpv_command_string.or_else(|| resolve_symbol::<FnCommandString>("mpv_command_string")) {
        Some(f) => f,
        None => {
            log_plugin("init_vtable: failed to resolve mpv_command_string");
            return false;
        }
    };
    let observe_property = match pfn_mpv_observe_property.or_else(|| resolve_symbol::<FnObserveProperty>("mpv_observe_property")) {
        Some(f) => f,
        None => {
            log_plugin("init_vtable: failed to resolve mpv_observe_property");
            return false;
        }
    };
    let set_property = match pfn_mpv_set_property.or_else(|| resolve_symbol::<FnSetProperty>("mpv_set_property")) {
        Some(f) => f,
        None => {
            log_plugin("init_vtable: failed to resolve mpv_set_property");
            return false;
        }
    };
    let set_property_string = match pfn_mpv_set_property_string.or_else(|| resolve_symbol::<FnSetPropertyString>("mpv_set_property_string")) {
        Some(f) => f,
        None => {
            log_plugin("init_vtable: failed to resolve mpv_set_property_string");
            return false;
        }
    };
    let get_property = match pfn_mpv_get_property.or_else(|| resolve_symbol::<FnGetProperty>("mpv_get_property")) {
        Some(f) => f,
        None => {
            log_plugin("init_vtable: failed to resolve mpv_get_property");
            return false;
        }
    };
    let get_property_string = match pfn_mpv_get_property_string.or_else(|| resolve_symbol::<FnGetPropertyString>("mpv_get_property_string")) {
        Some(f) => f,
        None => {
            log_plugin("init_vtable: failed to resolve mpv_get_property_string");
            return false;
        }
    };
    let free = match pfn_mpv_free.or_else(|| resolve_symbol::<FnFree>("mpv_free")) {
        Some(f) => f,
        None => {
            log_plugin("init_vtable: failed to resolve mpv_free");
            return false;
        }
    };

    log_plugin("init_vtable: all mpv API symbols successfully bound!");
    VTABLE = Some(MpvVTable {
        client_name,
        wait_event,
        command,
        command_string,
        observe_property,
        set_property,
        set_property_string,
        get_property,
        get_property_string,
        free,
    });

    true
}

pub struct MpvHandle {
    raw: MpvHandleRaw,
}

impl MpvHandle {
    pub fn from_raw(raw: MpvHandleRaw) -> Option<Self> {
        if raw.is_null() {
            None
        } else {
            unsafe {
                if !init_vtable() {
                    return None;
                }
            }
            Some(Self { raw })
        }
    }

    pub fn client_name(&self) -> String {
        unsafe {
            let vt = VTABLE.as_ref().unwrap();
            let c_str = (vt.client_name)(self.raw);
            if c_str.is_null() {
                "unknown".to_string()
            } else {
                CStr::from_ptr(c_str).to_string_lossy().into_owned()
            }
        }
    }

    pub fn wait_event(&self, timeout: f64) -> *mut MpvEvent {
        unsafe {
            let vt = VTABLE.as_ref().unwrap();
            (vt.wait_event)(self.raw, timeout)
        }
    }

    pub fn observe_property(&self, reply_userdata: u64, name: &str, format: MpvFormat) -> i32 {
        unsafe {
            let vt = VTABLE.as_ref().unwrap();
            let c_name = CString::new(name).unwrap();
            (vt.observe_property)(self.raw, reply_userdata, c_name.as_ptr(), format)
        }
    }

    pub fn set_property_string(&self, name: &str, val: &str) -> i32 {
        unsafe {
            let vt = VTABLE.as_ref().unwrap();
            let c_name = CString::new(name).unwrap();
            let c_val = CString::new(val).unwrap();
            (vt.set_property_string)(self.raw, c_name.as_ptr(), c_val.as_ptr())
        }
    }

    pub fn set_property_double(&self, name: &str, val: f64) -> i32 {
        unsafe {
            let vt = VTABLE.as_ref().unwrap();
            let c_name = CString::new(name).unwrap();
            let mut d = val;
            (vt.set_property)(
                self.raw,
                c_name.as_ptr(),
                MpvFormat::Double,
                &mut d as *mut f64 as *mut c_void,
            )
        }
    }

    pub fn get_property_string(&self, name: &str) -> Option<String> {
        unsafe {
            let vt = VTABLE.as_ref().unwrap();
            let c_name = CString::new(name).unwrap();
            let ptr = (vt.get_property_string)(self.raw, c_name.as_ptr());
            if ptr.is_null() {
                None
            } else {
                let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
                (vt.free)(ptr as *mut c_void);
                Some(s)
            }
        }
    }

    pub fn get_property_double(&self, name: &str) -> Option<f64> {
        unsafe {
            let vt = VTABLE.as_ref().unwrap();
            let c_name = CString::new(name).unwrap();
            let mut val: f64 = 0.0;
            let res = (vt.get_property)(
                self.raw,
                c_name.as_ptr(),
                MpvFormat::Double,
                &mut val as *mut f64 as *mut c_void,
            );
            if res >= 0 {
                Some(val)
            } else {
                None
            }
        }
    }

    pub fn command(&self, args: &[&str]) -> i32 {
        unsafe {
            let vt = VTABLE.as_ref().unwrap();
            let c_strings: Vec<CString> = args.iter().map(|s| CString::new(*s).unwrap()).collect();
            let mut ptrs: Vec<*const c_char> = c_strings.iter().map(|cs| cs.as_ptr()).collect();
            ptrs.push(ptr::null());
            (vt.command)(self.raw, ptrs.as_mut_ptr())
        }
    }

    pub fn command_string(&self, cmd: &str) -> i32 {
        unsafe {
            let vt = VTABLE.as_ref().unwrap();
            let c_cmd = CString::new(cmd).unwrap();
            (vt.command_string)(self.raw, c_cmd.as_ptr())
        }
    }

    /// Update overlay bitmap directly via mpv overlay-add using high-speed temp file
    pub fn overlay_add_bgra(
        &self,
        id: u32,
        x: i32,
        y: i32,
        bgra_data: &[u8],
        width: u32,
        height: u32,
        stride: u32,
    ) -> i32 {
        let temp_file = std::env::temp_dir().join(format!("mpv_twitch_overlay_{}.raw", id));
        if let Err(e) = std::fs::write(&temp_file, bgra_data) {
            log_plugin(&format!("Failed to write overlay raw file: {}", e));
            return -1;
        }

        let temp_str = temp_file.to_string_lossy().replace('\\', "/");
        let id_str = id.to_string();
        let x_str = x.to_string();
        let y_str = y.to_string();
        let w_str = width.to_string();
        let h_str = height.to_string();
        let stride_str = stride.to_string();

        let ret = self.command(&[
            "overlay-add",
            &id_str,
            &x_str,
            &y_str,
            &temp_str,
            "0",
            "bgra",
            &w_str,
            &h_str,
            &stride_str,
        ]);
        if ret < 0 {
            log_plugin(&format!("mpv overlay-add returned error code: {}", ret));
        }
        ret
    }

    pub fn overlay_remove(&self, id: u32) -> i32 {
        let id_str = id.to_string();
        self.command(&["overlay-remove", &id_str])
    }

    pub fn seek_absolute(&self, seconds: f64) {
        let sec_str = format!("{:.3}", seconds);
        self.command(&["seek", &sec_str, "absolute+exact"]);
    }
}
