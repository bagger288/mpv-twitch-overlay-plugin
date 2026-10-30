//! mpv C API FFI bindings and safe wrapper
//! Supports dynamic symbol resolution in-process on Windows so no link-time mpv.lib is required.

use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::ptr;

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
type FnClientName = unsafe extern "C" fn(ctx: MpvHandleRaw) -> *const c_char;
type FnWaitEvent = unsafe extern "C" fn(ctx: MpvHandleRaw, timeout: c_double) -> *mut MpvEvent;
type FnCommand = unsafe extern "C" fn(ctx: MpvHandleRaw, args: *mut *const c_char) -> c_int;
type FnCommandString = unsafe extern "C" fn(ctx: MpvHandleRaw, args: *const c_char) -> c_int;
type FnObserveProperty = unsafe extern "C" fn(ctx: MpvHandleRaw, reply_userdata: u64, name: *const c_char, format: MpvFormat) -> c_int;
type FnSetProperty = unsafe extern "C" fn(ctx: MpvHandleRaw, name: *const c_char, format: MpvFormat, data: *mut c_void) -> c_int;
type FnSetPropertyString = unsafe extern "C" fn(ctx: MpvHandleRaw, name: *const c_char, data: *const c_char) -> c_int;
type FnGetProperty = unsafe extern "C" fn(ctx: MpvHandleRaw, name: *const c_char, format: MpvFormat, data: *mut c_void) -> c_int;
type FnGetPropertyString = unsafe extern "C" fn(ctx: MpvHandleRaw, name: *const c_char) -> *mut c_char;
type FnFree = unsafe extern "C" fn(data: *mut c_void);

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
        fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    }

    let c_sym = CString::new(symbol_name).ok()?;

    // Search in current process (.exe) first
    let h_exe = GetModuleHandleW(ptr::null());
    if !h_exe.is_null() {
        let p = GetProcAddress(h_exe, c_sym.as_ptr());
        if !p.is_null() {
            return Some(std::mem::transmute_copy(&p));
        }
    }

    // Try common mpv dll names if hosted as a library
    for module_name in &["mpv-2.dll", "mpv-1.dll", "mpv.dll", "libmpv-2.dll"] {
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

    let client_name = match resolve_symbol::<FnClientName>("mpv_client_name") {
        Some(f) => f,
        None => return false,
    };
    let wait_event = match resolve_symbol::<FnWaitEvent>("mpv_wait_event") {
        Some(f) => f,
        None => return false,
    };
    let command = match resolve_symbol::<FnCommand>("mpv_command") {
        Some(f) => f,
        None => return false,
    };
    let command_string = match resolve_symbol::<FnCommandString>("mpv_command_string") {
        Some(f) => f,
        None => return false,
    };
    let observe_property = match resolve_symbol::<FnObserveProperty>("mpv_observe_property") {
        Some(f) => f,
        None => return false,
    };
    let set_property = match resolve_symbol::<FnSetProperty>("mpv_set_property") {
        Some(f) => f,
        None => return false,
    };
    let set_property_string = match resolve_symbol::<FnSetPropertyString>("mpv_set_property_string") {
        Some(f) => f,
        None => return false,
    };
    let get_property = match resolve_symbol::<FnGetProperty>("mpv_get_property") {
        Some(f) => f,
        None => return false,
    };
    let get_property_string = match resolve_symbol::<FnGetPropertyString>("mpv_get_property_string") {
        Some(f) => f,
        None => return false,
    };
    let free = match resolve_symbol::<FnFree>("mpv_free") {
        Some(f) => f,
        None => return false,
    };

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

    /// Update overlay bitmap directly via mpv overlay-add
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
        // mpv overlay-add command: overlay-add <id> <x> <y> "@<memory_address>" 0 "bgra" <w> <h> <stride>
        let addr_str = format!("@{}", bgra_data.as_ptr() as usize);
        let id_str = id.to_string();
        let x_str = x.to_string();
        let y_str = y.to_string();
        let w_str = width.to_string();
        let h_str = height.to_string();
        let stride_str = stride.to_string();

        self.command(&[
            "overlay-add",
            &id_str,
            &x_str,
            &y_str,
            &addr_str,
            "0",
            "bgra",
            &w_str,
            &h_str,
            &stride_str,
        ])
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
