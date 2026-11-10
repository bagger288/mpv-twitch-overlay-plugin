//! Twitch Chat Overlay mpv C-Plugin (Rust Native DLL)
//!
//! High-performance native mpv plugin for Windows / Linux.
//! Provides instantaneous emote rendering and rock-solid Direct3D11 / gpu-next compatibility.

#![allow(dead_code)]

pub mod animation;
pub mod chat;
pub mod emotes;
pub mod layout;
pub mod mouse;
pub mod mpv_api;
pub mod renderer;
pub mod session;
pub mod text_effects;
pub mod ui;

use emotes::EmoteCache;
use mouse::{MouseAction, PanelMouse};
use mpv_api::{log_plugin, MpvEventClientMessage, MpvEventId, MpvFormat, MpvHandle, MpvHandleRaw};
use renderer::ChatRenderer;
use session::SessionManager;
use std::os::raw::c_int;
use std::path::PathBuf;
use ui::{clamp_outline_width, max_settings_scroll, search_result_hit, ChatViewport, HeaderFade, HitBox, SearchHit, TextEffect, UiState, UiView};

// mpv overlay ID must be in the range [0..63].
// ID 1 is safe, valid, and avoids thumbfast.lua's default ID 42.
const OVERLAY_ID: u32 = 1;

/// Retrieves cursor coordinates in mpv OSD coordinate space
fn get_cursor_pos(mpv: &MpvHandle) -> Option<(i32, i32)> {
    // 1. mpv native subproperties (available in mpv >= 0.33)
    if let (Some(x), Some(y)) = (
        mpv.get_property_double("mouse-pos/x"),
        mpv.get_property_double("mouse-pos/y"),
    ) {
        return Some((x.round() as i32, y.round() as i32));
    }

    // 2. mpv "mouse-pos" property as JSON string {"x":123,"y":456,"hover":true}
    if let Some(json_str) = mpv.get_property_string("mouse-pos") {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json_str) {
            if let (Some(x), Some(y)) = (
                val.get("x").and_then(|v| v.as_f64()),
                val.get("y").and_then(|v| v.as_f64()),
            ) {
                return Some((x.round() as i32, y.round() as i32));
            }
        }
    }

    // 3. Fallback: OS window cursor converted to OSD coordinate space
    #[cfg(target_os = "windows")]
    {
        if let Some(pos) = get_win32_osd_cursor_pos(mpv) {
            return Some(pos);
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn get_win32_osd_cursor_pos(mpv: &MpvHandle) -> Option<(i32, i32)> {
    use std::os::raw::c_void;

    #[repr(C)]
    #[derive(Default, Copy, Clone)]
    struct WinPoint {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Default, Copy, Clone)]
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetCursorPos(lpPoint: *mut WinPoint) -> i32;
        fn ScreenToClient(hWnd: *mut c_void, lpPoint: *mut WinPoint) -> i32;
        fn GetClientRect(hWnd: *mut c_void, lpRect: *mut WinRect) -> i32;
        fn GetForegroundWindow() -> *mut c_void;
        fn GetCurrentProcessId() -> u32;
        fn GetWindowThreadProcessId(hWnd: *mut c_void, lpdwProcessId: *mut u32) -> u32;
    }

    unsafe {
        let mut pt = WinPoint::default();
        if GetCursorPos(&mut pt) == 0 {
            return None;
        }

        let my_pid = GetCurrentProcessId();
        let mut target_hwnd: *mut c_void = std::ptr::null_mut();

        // 1. Try mpv's own window handle from "wid"
        if let Some(wid_str) = mpv.get_property_string("wid") {
            if let Ok(wid_num) = wid_str.parse::<usize>() {
                if wid_num != 0 {
                    target_hwnd = wid_num as *mut c_void;
                }
            }
        }

        // 2. Fallback: foreground window if it belongs to our mpv process
        if target_hwnd.is_null() {
            let fg = GetForegroundWindow();
            if !fg.is_null() {
                let mut pid: u32 = 0;
                GetWindowThreadProcessId(fg, &mut pid);
                if pid == my_pid {
                    target_hwnd = fg;
                }
            }
        }

        if !target_hwnd.is_null() {
            let mut rc = WinRect::default();
            GetClientRect(target_hwnd, &mut rc);
            if ScreenToClient(target_hwnd, &mut pt) != 0 {
                let client_w = (rc.right - rc.left).max(1) as f64;
                let client_h = (rc.bottom - rc.top).max(1) as f64;

                let osd_w = mpv.get_property_double("osd-width").unwrap_or(client_w);
                let osd_h = mpv.get_property_double("osd-height").unwrap_or(client_h);

                let osd_x = (pt.x as f64 * osd_w / client_w).round() as i32;
                let osd_y = (pt.y as f64 * osd_h / client_h).round() as i32;

                return Some((osd_x, osd_y));
            }
        }

        None
    }
}

/// Builds keybindings string for the search input section in mpv
fn build_search_section_string() -> String {
    let mut s = String::with_capacity(8192);

    // Latin letters a-z and A-Z
    for b in b'a'..=b'z' {
        let ch = b as char;
        let up = ch.to_ascii_uppercase();
        s.push_str(&format!("{ch} script-message twitch_chat_key {ch}\n"));
        s.push_str(&format!("Shift+{ch} script-message twitch_chat_key {up}\n"));
    }

    // Digits 0-9
    for d in b'0'..=b'9' {
        let dh = d as char;
        s.push_str(&format!("{dh} script-message twitch_chat_key {dh}\n"));
    }

    // Printable symbols
    s.push_str("SPACE script-message twitch_chat_space\n");
    s.push_str("- script-message twitch_chat_key -\n");
    s.push_str("_ script-message twitch_chat_key _\n");
    s.push_str(". script-message twitch_chat_key .\n");
    s.push_str(", script-message twitch_chat_key ,\n");
    s.push_str("! script-message twitch_chat_key !\n");
    s.push_str("? script-message twitch_chat_key ?\n");
    s.push_str("/ script-message twitch_chat_key /\n");
    s.push_str("@ script-message twitch_chat_key @\n");
    s.push_str("# script-message twitch_chat_key #\n");
    s.push_str(": script-message twitch_chat_key :\n");
    s.push_str("; script-message twitch_chat_key ;\n");
    s.push_str("+ script-message twitch_chat_key +\n");
    s.push_str("= script-message twitch_chat_key =\n");
    s.push_str("* script-message twitch_chat_key *\n");
    s.push_str("( script-message twitch_chat_key (\n");
    s.push_str(") script-message twitch_chat_key )\n");
    s.push_str("[ script-message twitch_chat_key [\n");
    s.push_str("] script-message twitch_chat_key ]\n");

    // Cyrillic letters (Russian lowercase & uppercase)
    const CYR_LOWER: &str = "абвгдеёжзийклмнопрстуфхцчшщъыьэюя";
    const CYR_UPPER: &str = "АБВГДЕЁЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯ";
    for (low, up) in CYR_LOWER.chars().zip(CYR_UPPER.chars()) {
        s.push_str(&format!("{low} script-message twitch_chat_key {low}\n"));
        s.push_str(&format!("{up} script-message twitch_chat_key {up}\n"));
        s.push_str(&format!("Shift+{low} script-message twitch_chat_key {up}\n"));
    }

    // Control keys
    s.push_str("BS script-message twitch_chat_bs\n");
    s.push_str("ESC script-message twitch_chat_esc\n");
    s.push_str("ENTER script-message twitch_chat_enter\n");

    // Suppress player navigation keys during search so they never seek or trigger actions
    s.push_str("LEFT script-message twitch_chat_ignore\n");
    s.push_str("RIGHT script-message twitch_chat_ignore\n");
    s.push_str("UP script-message twitch_chat_ignore\n");
    s.push_str("DOWN script-message twitch_chat_ignore\n");
    s.push_str("PGUP script-message twitch_chat_ignore\n");
    s.push_str("PGDWN script-message twitch_chat_ignore\n");
    s.push_str("Tab script-message twitch_chat_ignore\n");

    s
}

struct PluginState {
    mpv: MpvHandle,
    session: Option<SessionManager>,
    emotes: Option<EmoteCache>,
    renderer: ChatRenderer,
    ui: UiState,
    current_time_pos: f64,
    current_path: String,
    explicit_session: Option<PathBuf>,
    config_path: Option<PathBuf>,
    pending_search_seek: Option<f64>,
    vo_configured: bool,
    playback_active: bool,
    overlay_active: bool,
    mouse_section_defined: bool,
    mouse_section_enabled: bool,
    search_section_defined: bool,
    search_section_enabled: bool,
    mouse: PanelMouse,
    header_fade: HeaderFade,
    last_header_update: std::time::Instant,
    chat_scene: Option<renderer::ChatScene>,
    animation_due: Option<std::time::Instant>,
    last_animation_render: std::time::Instant,
    last_cache_check: std::time::Instant,
}

impl PluginState {
    fn new(mpv: MpvHandle) -> Self {
        Self {
            mpv,
            session: None,
            emotes: None,
            renderer: ChatRenderer::new(),
            ui: UiState::default(),
            current_time_pos: 0.0,
            current_path: String::new(),
            explicit_session: None,
            config_path: None,
            pending_search_seek: None,
            vo_configured: false,
            playback_active: false,
            overlay_active: false,
            mouse_section_defined: false,
            mouse_section_enabled: false,
            search_section_defined: false,
            search_section_enabled: false,
            mouse: PanelMouse::default(),
            header_fade: HeaderFade::default(),
            last_header_update: std::time::Instant::now(),
            chat_scene: None,
            animation_due: None,
            last_animation_render: std::time::Instant::now(),
            last_cache_check: std::time::Instant::now(),
        }
    }

    fn init_mouse_section(&mut self) {
        if !self.mouse_section_defined {
            let client_name = self.mpv.client_name();
            let bindings = format!(
                "MBTN_LEFT script-binding {client_name}/twitch_chat_mouse\nWHEEL_UP script-message twitch_chat_wheel_up\nWHEEL_DOWN script-message twitch_chat_wheel_down"
            );
            let ret = self.mpv.command(&[
                "define-section",
                "twitch_chat_mouse",
                &bindings,
                "force",
            ]);
            if ret < 0 {
                log_plugin(&format!("Failed to define mouse section: {}", ret));
                return;
            }
            self.mpv.command(&["disable-section", "twitch_chat_mouse"]);
            self.mouse_section_defined = true;
            self.mouse_section_enabled = false;
        }
    }

    fn init_search_section(&mut self) {
        if !self.search_section_defined {
            let section_str = build_search_section_string();
            self.mpv.command(&[
                "define-section",
                "twitch_chat_search",
                &section_str,
                "force",
            ]);
            self.mpv.command(&["disable-section", "twitch_chat_search"]);
            self.search_section_defined = true;
            self.search_section_enabled = false;
        }
    }

    fn update_search_keys_state(&mut self) {
        self.init_search_section();
        let should_enable = self.ui.chat_enabled && !self.ui.minimized && self.ui.current_view == UiView::Search;
        if should_enable && !self.search_section_enabled {
            self.mpv.command(&["enable-section", "twitch_chat_search", "allow-hide-cursor+allow-vo-dragging"]);
            self.mpv.command(&["script-message", "twitch_search_active", "1"]);
            self.search_section_enabled = true;
        } else if !should_enable && self.search_section_enabled {
            self.mpv.command(&["disable-section", "twitch_chat_search"]);
            self.mpv.command(&["script-message", "twitch_search_active", "0"]);
            self.search_section_enabled = false;
        }
    }

    fn resolve_config_path(&self) -> PathBuf {
        if let Some(ref path) = self.config_path {
            return path.clone();
        }
        // 1. Check current working directory
        if let Ok(cwd) = std::env::current_dir() {
            let p = cwd.join("config.json");
            if p.is_file() {
                return p;
            }
        }
        // 2. Check relative to session directory
        if let Some(session) = &self.session {
            let p2 = session.session_dir.join("../../config.json");
            if p2.is_file() {
                return p2;
            }
            let p1 = session.session_dir.join("../config.json");
            if p1.is_file() {
                return p1;
            }
            let p0 = session.session_dir.join("config.json");
            if p0.is_file() {
                return p0;
            }
        }
        // 3. Check relative to explicit_session
        if let Some(ref es) = self.explicit_session {
            let base = if es.is_dir() { es.clone() } else { es.parent().unwrap_or(es).to_path_buf() };
            let p2 = base.join("../../config.json");
            if p2.is_file() {
                return p2;
            }
            let p1 = base.join("../config.json");
            if p1.is_file() {
                return p1;
            }
        }
        // 4. If session is inside a "records" folder, assume its parent is the project root
        if let Some(session) = &self.session {
            if let Some(parent) = session.session_dir.parent() {
                if parent.file_name().and_then(|n| n.to_str()) == Some("records") {
                    if let Some(root) = parent.parent() {
                        return root.join("config.json");
                    }
                }
            }
        }
        // 5. Fallback to cwd
        if let Ok(cwd) = std::env::current_dir() {
            return cwd.join("config.json");
        }
        PathBuf::from("config.json")
    }

    fn apply_overlay_settings(&mut self, val: &serde_json::Value) {
        if let Some(x) = val.get("chat_x").and_then(|v| v.as_i64()) {
            self.ui.panel_x = x as i32;
        }
        if let Some(y) = val.get("chat_y").and_then(|v| v.as_i64()) {
            self.ui.panel_y = y as i32;
        }
        if let Some(w) = val.get("chat_width").and_then(|v| v.as_u64()) {
            self.ui.panel_width = w as u32;
        }
        if let Some(h) = val.get("chat_height").and_then(|v| v.as_u64()) {
            self.ui.panel_height = h as u32;
        }
        if let Some(fs) = val.get("chat_font_size").and_then(|v| v.as_f64()) {
            self.ui.font_size = fs as f32;
        }
        if let Some(op) = val.get("chat_bg_opacity").and_then(|v| v.as_f64()) {
            self.ui.bg_opacity = op as f32;
        }
        if let Some(enabled) = val.get("chat_enabled").or_else(|| val.get("default_chat_enabled")).and_then(|v| v.as_bool()) {
            self.ui.chat_enabled = enabled;
        }
        if let Some(enabled) = val.get("emotes_enabled").or_else(|| val.get("default_emotes_enabled")).and_then(|v| v.as_bool()) {
            self.ui.emotes_enabled = enabled;
        }
        if let Some(enabled) = val.get("badges_enabled").and_then(|v| v.as_bool()) {
            self.ui.badges_enabled = enabled;
        }
        if let Some(effect) = val.get("chat_text_effect").and_then(|v| v.as_str()).and_then(TextEffect::parse) {
            self.ui.text_effect = effect;
        }
        if let Some(position) = val.get("chat_minimized_position").and_then(|v| v.as_array()) {
            if let (Some(x), Some(y)) = (position.get(0).and_then(|v| v.as_i64()), position.get(1).and_then(|v| v.as_i64())) {
                self.ui.minimized_position = Some((x as i32, y as i32));
            }
        }
        if let Some(width) = val.get("chat_outline_width").and_then(|v| v.as_f64()) {
            self.ui.outline_width = clamp_outline_width(width as f32);
        }
    }

    fn load_chat_settings(&mut self) {
        let config_path = self.resolve_config_path();
        log_plugin(&format!("Loading chat overlay settings from global config: {:?}", config_path));

        let mut loaded = false;
        if config_path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if val.get("chat_x").is_some() || val.get("chat_width").is_some() || val.get("chat_font_size").is_some() || val.get("chat_enabled").is_some() {
                        self.apply_overlay_settings(&val);
                        loaded = true;
                    }
                }
            }
        }

        // Migration: if global config did not have overlay settings yet, check if session has legacy chat_settings.json
        if !loaded {
            if let Some(session) = &self.session {
                let legacy_settings = session.session_dir.join("chat_settings.json");
                if legacy_settings.is_file() {
                    if let Ok(content) = std::fs::read_to_string(&legacy_settings) {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                            log_plugin(&format!("Migrating legacy chat_settings.json from {:?} to global config", legacy_settings));
                            self.apply_overlay_settings(&val);
                            self.save_chat_settings();
                        }
                    }
                }
            }
        }
    }

    fn save_chat_settings(&self) {
        let config_path = self.resolve_config_path();
        log_plugin(&format!("Saving chat overlay settings to global config: {:?}", config_path));

        let mut map: serde_json::Map<String, serde_json::Value> = if config_path.is_file() {
            std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
                .and_then(|val| val.as_object().cloned())
                .unwrap_or_default()
        } else {
            serde_json::Map::new()
        };

        map.insert("chat_x".to_string(), serde_json::json!(self.ui.panel_x));
        map.insert("chat_y".to_string(), serde_json::json!(self.ui.panel_y));
        map.insert("chat_width".to_string(), serde_json::json!(self.ui.panel_width));
        map.insert("chat_height".to_string(), serde_json::json!(self.ui.panel_height));
        map.insert("chat_font_size".to_string(), serde_json::json!(self.ui.font_size));
        map.insert("chat_bg_opacity".to_string(), serde_json::json!(self.ui.bg_opacity));
        map.insert("chat_enabled".to_string(), serde_json::json!(self.ui.chat_enabled));
        map.insert("emotes_enabled".to_string(), serde_json::json!(self.ui.emotes_enabled));
        map.insert("badges_enabled".to_string(), serde_json::json!(self.ui.badges_enabled));
        map.insert("chat_text_effect".to_string(), serde_json::json!(self.ui.text_effect));
        map.insert("chat_outline_width".to_string(), serde_json::json!(self.ui.outline_width));
        map.insert("chat_minimized_position".to_string(), serde_json::json!(self.ui.minimized_position));

        if let Ok(json_str) = serde_json::to_string_pretty(&serde_json::Value::Object(map)) {
            let tmp_path = config_path.with_extension("json.tmp");
            if std::fs::write(&tmp_path, &json_str).is_ok() {
                let _ = std::fs::rename(&tmp_path, &config_path);
            } else {
                let _ = std::fs::write(&config_path, json_str);
            }
        }
    }

    fn handle_mouse_action(&mut self, action: MouseAction) {
        match action {
            MouseAction::DragStarted => {
                log_plugin(&format!(
                    "Started dragging chat: panel=({}, {})",
                    self.ui.panel_x, self.ui.panel_y
                ));
            }
            MouseAction::DragFinished => {
                self.save_chat_settings();
                log_plugin(&format!(
                    "Finished dragging chat, saved position=({}, {})",
                    self.ui.panel_x, self.ui.panel_y
                ));
            }
            MouseAction::Click(x, y) => self.on_click_event(Some(x), Some(y)),
            MouseAction::None => {}
        }
    }

    fn on_mouse_binding(&mut self, key_state: &str) {
        if key_state.as_bytes().first() == Some(&b'u')
            && key_state.as_bytes().get(2) != Some(&b'c')
        {
            self.update_drag_position();
        }
        let was_dragging = self.mouse.is_dragging();
        let cursor = get_cursor_pos(&self.mpv);
        let action = self.mouse.handle(key_state, cursor, &self.ui);
        if was_dragging && self.ui.minimized && matches!(action, MouseAction::Click(_, _)) {
            self.save_chat_settings();
        }
        self.handle_mouse_action(action);
    }

    fn cancel_mouse(&mut self) {
        let action = self.mouse.cancel();
        self.handle_mouse_action(action);
    }

    fn update_drag_position(&mut self) {
        if !self.mouse.is_dragging() {
            return;
        }
        if let Some(cursor) = get_cursor_pos(&self.mpv) {
            let osd_w = self.mpv.get_property_double("osd-width").unwrap_or(1920.0) as i32;
            let osd_h = self.mpv.get_property_double("osd-height").unwrap_or(1080.0) as i32;
            if let Some((x, y)) = self.mouse.drag_position(cursor, (osd_w, osd_h)) {
                let (old_x, old_y, _, _) = self.ui.overlay_geometry();
                if x != old_x || y != old_y {
                    self.ui.set_overlay_position(x, y);
                    self.update_frame();
                }
            }
        }
    }

    fn update_mouse_hover(&mut self) {
        if !self.ui.chat_enabled || !self.vo_configured || !self.playback_active {
            self.header_fade.hovered = false;
            self.cancel_mouse();
            if self.mouse_section_enabled {
                self.mpv.command(&["disable-section", "twitch_chat_mouse"]);
                self.mouse_section_enabled = false;
            }
            return;
        }

        self.init_mouse_section();

        let (panel_x, panel_y, panel_width, panel_height) = self.ui.overlay_geometry();
        let is_hovering = if self.mouse.is_pressed() {
            true
        } else if self.mpv.get_property_bool("mouse-pos/hover") == Some(false) {
            false
        } else if let Some((mx, my)) = get_cursor_pos(&self.mpv) {
            mx >= panel_x - 10 && mx <= panel_x + panel_width as i32 + 10
                && my >= panel_y - 10 && my <= panel_y + panel_height as i32 + 10
        } else {
            false
        };

        self.header_fade.hovered = is_hovering && !self.ui.minimized;
        if is_hovering && !self.mouse_section_enabled {
            let ret = self.mpv.command(&["enable-section", "twitch_chat_mouse", "allow-hide-cursor"]);
            self.mouse_section_enabled = ret >= 0;
            if ret < 0 {
                log_plugin(&format!("Failed to enable mouse section: {}", ret));
            }
        } else if !is_hovering && self.mouse_section_enabled && !self.mouse.is_pressed() {
            self.mpv.command(&["disable-section", "twitch_chat_mouse"]);
            self.mouse_section_enabled = false;
        }
    }

    fn update_header_animation(&mut self) {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_header_update).as_secs_f32().min(0.05);
        self.last_header_update = now;
        let opacity = self.header_fade.advance(elapsed);
        if (opacity - self.ui.header_opacity).abs() > 0.001
            || (opacity != self.ui.header_opacity && (opacity == 0.0 || opacity == 1.0))
        {
            self.ui.header_opacity = opacity;
            self.update_frame();
        }
    }

    fn perform_search(&mut self) {
        if let Some(session) = &self.session {
            self.ui.search_results = session.search_for_video(
                &self.ui.search_query,
                &self.current_path,
                self.ui.offset_ms,
                50,
            );
            self.ui.search_scroll_offset = 0;
        }
    }

    fn scroll_chat(&mut self, delta: i32) {
        if let Some(end) = self.ui.chat_scroll_anchor {
            let count = self.session.as_ref().map(|s| s.messages.len()).unwrap_or(0);
            self.ui.chat_scroll_anchor = Some((end as i64 - delta as i64).clamp(0, count as i64) as usize);
            self.ui.scroll_offset = self.ui.scroll_offset.saturating_add(delta);
        } else {
            let max = self.session.as_ref().map(|s| s.messages.len().min(i32::MAX as usize) as i32).unwrap_or(50000);
            self.ui.scroll_offset = self.ui.scroll_offset.saturating_add(delta).clamp(-max, max);
        }
        self.ui.highlighted_message_ts = None;
    }

    fn open_chat_search_result(&mut self, result: &ui::SearchResult) {
        if let Some(session) = &self.session {
            if result.msg_idx < session.messages.len() {
                self.ui.chat_scroll_anchor = Some(result.msg_idx + 1);
                self.ui.highlighted_message_ts = Some(result.timestamp_ms);
                let now = session.video_start_ms(&self.current_path) + self.ui.offset_ms + (self.current_time_pos * 1000.0).round() as i64;
                let current_end = session.messages.partition_point(|message| message.timestamp_ms <= now);
                self.ui.scroll_offset = (current_end as i64 - result.msg_idx as i64 - 1).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
                log_plugin(&format!("Scrolled chat to search message: timestamp_ms={}", result.timestamp_ms));
            }
        }
    }

    fn seek_search_result(&mut self, result: &ui::SearchResult) {
        self.ui.chat_scroll_anchor = None;
        self.ui.highlighted_message_ts = None;
        self.ui.scroll_offset = 0;
        if let Some(path) = result.video_path.as_deref() {
            let target = self.session.as_ref().and_then(|session| session.part_index(path));
            let current = self.session.as_ref().and_then(|session| session.part_index(&self.current_path));
            if target.is_some() && target != current {
                let playlist_index = self.mpv.get_property_double("playlist-pos").unwrap_or(0.0) as i64;
                let destination = playlist_index + target.unwrap() as i64 - current.unwrap_or(0) as i64;
                self.pending_search_seek = Some(result.sec);
                if destination >= 0 && self.mpv.command(&["playlist-play-index", &destination.to_string()]) >= 0 {
                    return;
                }
                if self.mpv.command(&["loadfile", path, "replace"]) < 0 {
                    self.pending_search_seek = None;
                }
                return;
            }
        }
        self.mpv.seek_absolute(result.sec);
    }

    fn on_click_event(&mut self, explicit_x: Option<i32>, explicit_y: Option<i32>) {
        let (mx, my) = if let (Some(x), Some(y)) = (explicit_x, explicit_y) {
            (x, y)
        } else if let Some(pos) = get_cursor_pos(&self.mpv) {
            pos
        } else {
            return;
        };

        if self.ui.minimized {
            let (x, y, width, height) = self.ui.overlay_geometry();
            if mx >= x && mx < x + width as i32 && my >= y && my < y + height as i32 {
                self.ui.restore();
                self.update_search_keys_state();
                self.update_mouse_hover();
                self.update_frame();
                log_plugin(&format!("Restored chat at ({}, {})", self.ui.panel_x, self.ui.panel_y));
            }
            return;
        }
        let rel_x = (mx - self.ui.panel_x) as f32;
        let rel_y = (my - self.ui.panel_y) as f32;

        if rel_x < -15.0
            || rel_x > (self.ui.panel_width as f32 + 15.0)
            || rel_y < -15.0
            || rel_y > (self.ui.panel_height as f32 + 15.0)
        {
            if self.mouse_section_enabled && !self.mouse.is_pressed() {
                self.mpv.command(&["disable-section", "twitch_chat_mouse"]);
                self.mouse_section_enabled = false;
            }
            return;
        }

        let width = self.ui.panel_width as f32;

        // 1. Header Tabs & Buttons
        if ChatRenderer::btn_tab_chat().contains(rel_x, rel_y) {
            self.ui.current_view = UiView::Chat;
            self.update_search_keys_state();
            self.update_frame();
            return;
        }
        if ChatRenderer::btn_tab_search().contains(rel_x, rel_y) {
            self.ui.current_view = UiView::Search;
            self.perform_search();
            self.update_search_keys_state();
            self.update_frame();
            return;
        }
        if ChatRenderer::btn_tab_settings().contains(rel_x, rel_y) {
            self.ui.current_view = UiView::Settings;
            self.update_search_keys_state();
            self.update_frame();
            return;
        }
        if ChatRenderer::btn_close(width).contains(rel_x, rel_y) {
            let osd = (
                self.mpv.get_property_double("osd-width").unwrap_or(1920.0) as i32,
                self.mpv.get_property_double("osd-height").unwrap_or(1080.0) as i32,
            );
            self.ui.minimize(osd);
            self.header_fade.hovered = false;
            self.save_chat_settings();
            self.update_search_keys_state();
            self.update_mouse_hover();
            self.update_frame();
            log_plugin("Minimized chat to Twitch button");
            return;
        }

        if ChatRenderer::is_drag_handle(rel_x, rel_y, width) {
            return;
        }

        // 2. View specific clicks
        match self.ui.current_view {
            UiView::Chat => {
                let height = self.ui.panel_height as f32;
                // If scrolled (into history or future), clicking bottom banner returns to live
                if (self.ui.scroll_offset != 0 || self.ui.chat_scroll_anchor.is_some()) && rel_y >= height - 32.0 {
                    self.ui.scroll_offset = 0;
                    self.ui.chat_scroll_anchor = None;
                    self.ui.highlighted_message_ts = None;
                    self.update_frame();
                    return;
                }

                // Hit-test visible messages to select a synchronization anchor.
                let session = match self.session.as_ref() {
                    Some(s) => s,
                    None => return,
                };

                let current_video_sec = self.current_time_pos;
                let session_start = session.video_start_ms(&self.current_path);
                let current_ms = session_start
                    + self.ui.offset_ms
                    + (current_video_sec * 1000.0).round() as i64;
                let active_msgs = session.visible_messages(current_ms, &self.ui, 80);

                let dummy_cache;
                let emotes_ref = match self.emotes.as_ref() {
                    Some(e) => e,
                    None => {
                        dummy_cache = EmoteCache::new(&session.session_dir);
                        &dummy_cache
                    }
                };

                let layout_messages = self.renderer.layouter().layout_messages_with_icons(
                    active_msgs,
                    &session.emotes_manifest,
                    emotes_ref,
                    self.ui.panel_width as f32,
                    self.ui.font_size,
                    self.ui.emotes_enabled,
                    self.ui.badges_enabled,
                );

                let viewport = ChatViewport::new(&self.ui, layout_messages.last().map(|m| m.bottom_y).unwrap_or(0.0));
                if rel_x < 0.0 || rel_x >= width || !viewport.contains_y(rel_y) {
                    return;
                }

                for msg in &layout_messages {
                    let msg_top = viewport.base_y + msg.top_y;
                    let msg_bottom = viewport.base_y + msg.bottom_y;

                    if rel_y >= msg_top && rel_y <= msg_bottom {
                        if msg.msg_index < active_msgs.len() {
                            let clicked_msg = &active_msgs[msg.msg_index];
                            self.ui.selected_sync_msg = Some(clicked_msg.clone());
                            let preview: String = clicked_msg.text.chars().take(48).collect();
                            self.mpv.osd_message(&format!("Якорь выбран: {}: {}", clicked_msg.display_name, preview), 1500);
                            log_plugin(&format!("Selected sync anchor: timestamp_ms={}", clicked_msg.timestamp_ms));
                            self.update_frame();
                            return;
                        }
                    }
                }
            }
            UiView::Settings => {
                self.handle_settings_click(rel_x, rel_y);
            }
            UiView::Search => {
                self.handle_search_click(rel_x, rel_y);
            }
        }
    }

    fn handle_settings_click(&mut self, rx: f32, ry: f32) {
        if ry < 32.0 || ry >= self.ui.panel_height as f32 {
            return;
        }
        let ry = ry + self.ui.settings_scroll_offset.min(max_settings_scroll(self.ui.panel_height));
        if ChatRenderer::btn_text_effect(self.ui.panel_width as f32).contains(rx, ry) {
            self.ui.text_effect = self.ui.text_effect.next();
            self.save_chat_settings();
            self.update_frame();
            return;
        }
        for (button, delta) in [(ChatRenderer::btn_outline_minus(), -0.5), (ChatRenderer::btn_outline_plus(), 0.5)] {
            if button.contains(rx, ry) {
                self.ui.outline_width = clamp_outline_width(self.ui.outline_width + delta);
                self.save_chat_settings();
                self.update_frame();
                return;
            }
        }
        let width = self.ui.panel_width as f32;
        let mut cur_y = 36.0 + 24.0; // after title

        // Row 1: Reset offset
        if ChatRenderer::btn_offset_reset().contains(rx, ry) {
            self.ui.offset_ms = 0;
            let video_filename = self.current_path.clone();
            if let Some(session) = &mut self.session {
                session.save_offset(&video_filename, 0);
            }
            self.update_frame();
            return;
        }
        cur_y += 26.0;

        // Offset Buttons: [-10s] [-1s] [-0.1s] [+0.1s] [+1s] [+10s]
        let deltas: [i64; 6] = [-10000, -1000, -100, 100, 1000, 10000];
        for (i, &delta) in deltas.iter().enumerate() {
            let hb = ChatRenderer::btn_offset_step(i, cur_y);
            if hb.contains(rx, ry) {
                self.ui.offset_ms += delta;
                let video_filename = self.current_path.clone();
                if let Some(session) = &mut self.session {
                    session.save_offset(&video_filename, self.ui.offset_ms);
                }
                self.update_frame();
                return;
            }
        }
        cur_y += 32.0;

        // Row 2: Font Size [A-] (65, cur_y, 28, 22), [A+] (98, cur_y, 28, 22)
        if HitBox::new(65.0, cur_y, 28.0, 22.0).contains(rx, ry) {
            self.ui.font_size = (self.ui.font_size - 1.0).max(11.0);
            self.save_chat_settings();
            self.update_frame();
            return;
        }
        if HitBox::new(98.0, cur_y, 28.0, 22.0).contains(rx, ry) {
            self.ui.font_size = (self.ui.font_size + 1.0).min(36.0);
            self.save_chat_settings();
            self.update_frame();
            return;
        }

        // Panel Width: [W-] (230, cur_y, 28, 22), [W+] (262, cur_y, 28, 22)
        if HitBox::new(230.0, cur_y, 28.0, 22.0).contains(rx, ry) {
            self.ui.panel_width = (self.ui.panel_width.saturating_sub(40)).max(260);
            self.save_chat_settings();
            self.update_frame();
            return;
        }
        if HitBox::new(262.0, cur_y, 28.0, 22.0).contains(rx, ry) {
            self.ui.panel_width = (self.ui.panel_width + 40).min(1200);
            self.save_chat_settings();
            self.update_frame();
            return;
        }
        cur_y += 30.0;

        // Row 3: Panel Height: [H-] (65, cur_y, 28, 22), [H+] (98, cur_y, 28, 22)
        if HitBox::new(65.0, cur_y, 28.0, 22.0).contains(rx, ry) {
            self.ui.panel_height = (self.ui.panel_height.saturating_sub(40)).max(220);
            self.save_chat_settings();
            self.update_frame();
            return;
        }
        if HitBox::new(98.0, cur_y, 28.0, 22.0).contains(rx, ry) {
            self.ui.panel_height = (self.ui.panel_height + 40).min(1400);
            self.save_chat_settings();
            self.update_frame();
            return;
        }

        // Background Opacity: (190, cur_y, 90, 22)
        if HitBox::new(190.0, cur_y, 90.0, 22.0).contains(rx, ry) {
            const OP_PRESETS: [f32; 7] = [1.0, 0.85, 0.75, 0.60, 0.40, 0.20, 0.0];
            let mut next_idx = 0;
            for (idx, &p) in OP_PRESETS.iter().enumerate() {
                if (self.ui.bg_opacity - p).abs() < 0.06 {
                    next_idx = (idx + 1) % OP_PRESETS.len();
                    break;
                }
            }
            self.ui.bg_opacity = OP_PRESETS[next_idx];
            self.save_chat_settings();
            self.update_frame();
            return;
        }
        cur_y += 34.0;

        if ChatRenderer::btn_emotes().contains(rx, ry) {
            self.ui.emotes_enabled = !self.ui.emotes_enabled;
            self.save_chat_settings();
            self.update_frame();
            return;
        }
        if ChatRenderer::btn_badges().contains(rx, ry) {
            self.ui.badges_enabled = !self.ui.badges_enabled;
            self.save_chat_settings();
            self.update_frame();
            return;
        }
        cur_y += 34.0;

        // Row 5: 1-Click Sync Button
        let sync_btn_hb = HitBox::new(20.0, cur_y + 44.0, width - 40.0, 24.0);
        if sync_btn_hb.contains(rx, ry) {
            let target_msg = if let Some(ref m) = self.ui.selected_sync_msg {
                Some(m.clone())
            } else if let Some(session) = &self.session {
                // If no message explicitly clicked yet, find the message nearest to current playback position
                let session_start = session.video_start_ms(&self.current_path);
                let cur_ts = session_start + (self.current_time_pos * 1000.0).round() as i64 + self.ui.offset_ms;
                let msgs = session.get_messages_up_to(cur_ts, 0, 1);
                msgs.last().cloned()
            } else {
                None
            };

            if let Some(anchor_msg) = target_msg {
                self.ui.selected_sync_msg = Some(anchor_msg.clone());
                let target_ts = anchor_msg.timestamp_ms;
                let current_video_sec = self.current_time_pos;
                let video_filename = self.current_path.clone();

                if let Some(session) = &mut self.session {
                    let success = session.calibrate_anchor(target_ts, current_video_sec, &video_filename);
                    self.ui.offset_ms = 0;

                    let vid_str = session::format_hms(current_video_sec);
                    let author = &anchor_msg.display_name;
                    let text_snip: String = anchor_msg.text.chars().take(28).collect();

                    if success {
                        let fname = session.session_file
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "session.json".to_string());
                        let msg = format!(
                            "⚓ Видео и чат синхронизированы!\nКадр видео: {}\nЯкорь: {}: {}\nsession_start_ms = {}\nСохранено в {}",
                            vid_str,
                            author,
                            text_snip,
                            session.video_start_ms(&self.current_path),
                            fname
                        );
                        self.mpv.osd_message(&msg, 3500);
                        log_plugin(&format!("calibrate_anchor saved to disk: {}", msg));
                    } else {
                        let msg = format!(
                            "⚓ Синхронизировано в памяти (ошибка записи в {:?})\nКадр: {}",
                            session.session_file, vid_str
                        );
                        self.mpv.osd_message(&msg, 3000);
                        log_plugin(&format!("calibrate_anchor write failed: {:?}", session.session_file));
                    }
                    self.update_frame();
                }
            } else {
                self.mpv.osd_message("Сначала кликните по сообщению в чате для выбора якоря!", 2500);
            }
        }
    }

    fn handle_search_click(&mut self, rx: f32, ry: f32) {
        let Some(hit) = search_result_hit(&self.ui, rx, ry) else { return; };
        let result = match hit {
            SearchHit::Message(index) | SearchHit::Timestamp(index) => self.ui.search_results[index].clone(),
        };
        match hit {
            SearchHit::Message(_) => self.open_chat_search_result(&result),
            SearchHit::Timestamp(_) => self.seek_search_result(&result),
        }
        self.ui.current_view = UiView::Chat;
        self.update_search_keys_state();
        self.update_frame();
    }

    fn parse_script_opts(&mut self, opts: &str) {
        log_plugin(&format!("Processing script-opts: {}", opts));
        for item in opts.split(',') {
            if let Some((k, v)) = item.split_once('=') {
                let key = k.trim();
                let val = v.trim();
                match key {
                    "twitch_config" | "config" => {
                        log_plugin(&format!("twitch_config specified: {}", val));
                        self.config_path = Some(PathBuf::from(val));
                    }
                    "twitch_session" => {
                        log_plugin(&format!("twitch_session specified: {}", val));
                        self.explicit_session = Some(PathBuf::from(val));
                        self.on_path_changed(val);
                    }
                    "twitch_chat" => {
                        self.ui.chat_enabled = val.eq_ignore_ascii_case("yes");
                    }
                    "twitch_emotes" => {
                        self.ui.emotes_enabled = val.eq_ignore_ascii_case("yes");
                    }
                    "twitch_font_size" => {
                        if let Ok(fs) = val.parse::<f32>() {
                            self.ui.font_size = fs;
                        }
                    }
                    "twitch_width" => {
                        if let Ok(w) = val.parse::<u32>() {
                            self.ui.panel_width = w;
                        }
                    }
                    "twitch_height" => {
                        if let Ok(h) = val.parse::<u32>() {
                            self.ui.panel_height = h;
                        }
                    }
                    "twitch_x" => {
                        if let Ok(x) = val.parse::<i32>() {
                            self.ui.panel_x = x;
                        }
                    }
                    "twitch_y" => {
                        if let Ok(y) = val.parse::<i32>() {
                            self.ui.panel_y = y;
                        }
                    }
                    "twitch_bg_opacity" => {
                        if let Ok(op) = val.parse::<f32>() {
                            self.ui.bg_opacity = op;
                        }
                    }
                    "twitch_text_effect" => {
                        if let Some(effect) = TextEffect::parse(val) {
                            self.ui.text_effect = effect;
                        }
                    }
                    "twitch_outline_width" => {
                        if let Ok(width) = val.parse::<f32>() {
                            self.ui.outline_width = clamp_outline_width(width);
                        }
                    }
                    "twitch_offset" => {
                        if let Ok(sec) = val.parse::<f64>() {
                            self.ui.offset_ms = (sec * 1000.0).round() as i64;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn on_path_changed(&mut self, new_path: &str) {
        if new_path.is_empty() {
            return;
        }
        log_plugin(&format!("on_path_changed: {}", new_path));
        self.cancel_mouse();
        let path = PathBuf::from(new_path);
        let load_path = self.explicit_session.as_ref().unwrap_or(&path);
        if let Some(mgr) = SessionManager::try_load(load_path) {
            let initial_offset = mgr.video_offset_ms(new_path);
            self.ui.offset_ms = initial_offset;

            log_plugin(&format!(
                "Session successfully loaded from: {:?}, initial offset: {}ms",
                mgr.session_dir, initial_offset
            ));
            log_plugin(&format!(
                "Found {} chat messages, {} emotes, {} badges",
                mgr.messages.len(),
                mgr.emotes_manifest.total_emotes_count(),
                mgr.emotes_manifest.badges.len()
            ));
            self.load_chat_settings();

            let cache_directory = mgr.cache_directory();
            self.emotes = Some(EmoteCache::with_cache_dir(&mgr.session_dir, cache_directory.as_deref()));
            let session_changed = self.session.as_ref().map(|session| session.session_file != mgr.session_file).unwrap_or(true);
            if session_changed {
                self.ui.selected_sync_msg = None;
                self.ui.scroll_offset = 0;
                self.ui.chat_scroll_anchor = None;
                self.ui.highlighted_message_ts = None;
                self.ui.search_results.clear();
                self.ui.search_scroll_offset = 0;
            }
            self.session = Some(mgr);
            self.current_path = new_path.to_string();
            self.update_frame();
        } else {
            log_plugin(&format!("Could not load session from path: {:?}", path));
        }
    }

    fn render_animation_frame(&mut self, force: bool) {
        if !self.ui.chat_enabled || self.ui.minimized || self.ui.current_view != UiView::Chat || !self.vo_configured || !self.playback_active {
            self.animation_due = None;
            return;
        }
        let now = std::time::Instant::now();
        let Some(cache) = self.emotes.as_ref() else { return; };
        let Some(scene) = self.chat_scene.as_mut() else { return; };
        let frame = self.renderer.render_scene(scene, cache, cache.elapsed(), force);
        if let Some(data) = frame.pixels {
            let ret = self.mpv.overlay_add_bgra(OVERLAY_ID, self.ui.panel_x, self.ui.panel_y, &data, self.ui.panel_width, self.ui.panel_height, self.ui.panel_width * 4);
            if ret >= 0 { self.overlay_active = true; }
            self.last_animation_render = now;
        }
        self.animation_due = frame.next_frame.map(|delay| {
            (now + delay).max(self.last_animation_render + std::time::Duration::from_nanos(16_666_667))
        });
    }

    fn update_animations(&mut self) {
        let now = std::time::Instant::now();
        if self.last_cache_check.elapsed() >= std::time::Duration::from_secs(1) {
            self.last_cache_check = now;
            if self.emotes.as_ref().map(|cache| cache.invalidate_changed()).unwrap_or(false) {
                self.update_frame();
                return;
            }
        }
        if self.animation_due.map(|due| due <= now).unwrap_or(false) {
            self.render_animation_frame(false);
        }
    }

    fn update_frame(&mut self) {
        self.chat_scene = None;
        self.animation_due = None;
        if !self.ui.chat_enabled {
            if self.overlay_active {
                self.mpv.overlay_remove(OVERLAY_ID);
                self.overlay_active = false;
            }
            return;
        }

        // Strictly guard against calling overlay-add before vo-configured is true
        if !self.vo_configured || !self.playback_active {
            return;
        }

        if self.ui.minimized {
            let cache = match self.emotes.as_ref() { Some(cache) => cache, None => return };
            let Some(bitmap) = self.renderer.render_frame(&[], cache, &self.ui, "") else { return; };
            let (x, y, width, height) = self.ui.overlay_geometry();
            let ret = self.mpv.overlay_add_bgra(OVERLAY_ID, x, y, &bitmap, width, height, width * 4);
            if ret >= 0 { self.overlay_active = true; }
            return;
        }
        let session = match self.session.as_mut() {
            Some(s) => s,
            None => return,
        };
        let emotes = match self.emotes.as_mut() {
            Some(e) => e,
            None => return,
        };

        let current_video_sec = self.current_time_pos;
        let session_start = session.video_start_ms(&self.current_path);
        let current_ms =
            session_start + self.ui.offset_ms + (current_video_sec * 1000.0).round() as i64;

        // Fetch recent messages with scroll offset applied
        let active_msgs = session.visible_messages(current_ms, &self.ui, 80);

        // Layout messages with badges and inline emotes
        let layout_messages = self.renderer.layouter().layout_messages_with_icons(
            active_msgs,
            &session.emotes_manifest,
            emotes,
            self.ui.panel_width as f32,
            self.ui.font_size,
            self.ui.emotes_enabled,
            self.ui.badges_enabled,
        );

        let channel = self
            .session
            .as_ref()
            .map(|s| s.session_data.channel.as_str())
            .unwrap_or("");
        if self.ui.current_view == UiView::Chat {
            self.chat_scene = self.renderer.prepare_chat_scene(&layout_messages, emotes, &self.ui, channel);
            self.render_animation_frame(true);
            return;
        }
        let bgra_data =
            match self
                .renderer
                .render_frame(&layout_messages, emotes, &self.ui, channel)
            {
                Some(d) => d,
                None => return,
            };

        let stride = self.ui.panel_width * 4;
        let ret = self.mpv.overlay_add_bgra(
            OVERLAY_ID,
            self.ui.panel_x,
            self.ui.panel_y,
            &bgra_data,
            self.ui.panel_width,
            self.ui.panel_height,
            stride,
        );

        if ret >= 0 {
            self.overlay_active = true;
        }
    }
}

fn is_vo_configured(mpv: &MpvHandle) -> bool {
    mpv.get_property_string("vo-configured")
        .map(|s| s.eq_ignore_ascii_case("yes") || s == "1" || s.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Native mpv C-plugin entry point exported from DLL
#[no_mangle]
pub unsafe extern "C" fn mpv_open_cplugin(ctx: MpvHandleRaw) -> c_int {
    log_plugin("=== mpv_open_cplugin entered ===");
    let mpv = match MpvHandle::from_raw(ctx) {
        Some(h) => {
            log_plugin(&format!(
                "MpvHandle initialized. Client name: {}",
                h.client_name()
            ));
            h
        }
        None => {
            log_plugin("FATAL: Failed to initialize MpvHandle / vtable!");
            return -1;
        }
    };

    // Observe essential playback properties ONLY
    mpv.observe_property(1, "time-pos", MpvFormat::Double);
    mpv.observe_property(2, "path", MpvFormat::String);
    mpv.observe_property(3, "pause", MpvFormat::Flag);
    mpv.observe_property(4, "vo-configured", MpvFormat::Flag);
    mpv.observe_property(5, "mouse-pos", MpvFormat::None);

    let mut state = PluginState::new(mpv);

    // Read initial script-opts
    if let Some(opts) = state.mpv.get_property_string("script-opts") {
        state.parse_script_opts(&opts);
    }
    if let Some(opts) = state.mpv.get_property_string("options/script-opts") {
        state.parse_script_opts(&opts);
    }

    state.load_chat_settings();

    // Main event loop (runs in mpv's dedicated plugin thread)
    loop {
        // Continuously update position if dragging
        state.update_drag_position();

        // Dynamically update hover state so mouse section is enabled ONLY when cursor is over the panel
        state.update_mouse_hover();
        state.update_header_animation();
        state.update_search_keys_state();
        state.update_animations();

        let mut wait_timeout: f64 = if state.mouse.is_dragging() { 0.01 } else if state.header_fade.is_animating() { 0.016 } else { 0.04 };
        if let Some(due) = state.animation_due {
            wait_timeout = wait_timeout.min(due.saturating_duration_since(std::time::Instant::now()).as_secs_f64());
        }
        let event_ptr = state.mpv.wait_event(wait_timeout);
        if event_ptr.is_null() {
            // Even on timeout, if in Search view, refresh overlay for cursor blink animation
            if state.ui.current_view == UiView::Search && state.vo_configured && state.playback_active {
                state.update_frame();
            }
            continue;
        }

        let event = &*event_ptr;
        match event.event_id {
            MpvEventId::Shutdown => {
                state.cancel_mouse();
                log_plugin("Shutdown event received, exiting loop");
                break;
            }
            MpvEventId::PropertyChange => {
                let prop_reply = event.reply_userdata;
                match prop_reply {
                    1 => {
                        // time-pos changed
                        if let Some(tp) = state.mpv.get_property_double("time-pos") {
                            state.current_time_pos = tp;
                            state.playback_active = true;
                            state.update_frame();
                        }
                    }
                    2 => {
                        // path changed
                        if let Some(p) = state.mpv.get_property_string("path") {
                            state.on_path_changed(&p);
                            state.update_frame();
                        }
                    }
                    4 => {
                        // vo-configured changed
                        let configured = is_vo_configured(&state.mpv);
                        state.vo_configured = configured;
                        if !configured {
                            if state.overlay_active {
                                state.mpv.overlay_remove(OVERLAY_ID);
                                state.overlay_active = false;
                            }
                        } else {
                            state.update_frame();
                        }
                    }
                    5 => {
                        // mouse-pos changed: instantly update dragging and hover detection
                        state.update_drag_position();
                        state.update_mouse_hover();
                    }
                    _ => {}
                }
            }
            MpvEventId::PlaybackRestart => {
                state.playback_active = true;
                state.vo_configured = is_vo_configured(&state.mpv);
                if let Some(tp) = state.mpv.get_property_double("time-pos") {
                    state.current_time_pos = tp;
                }
                if state.session.is_none() {
                    if let Some(p) = state.mpv.get_property_string("path") {
                        state.on_path_changed(&p);
                    }
                }
                state.update_frame();
            }
            MpvEventId::FileLoaded => {
                state.playback_active = true;
                state.vo_configured = is_vo_configured(&state.mpv);
                if let Some(p) = state.mpv.get_property_string("path") {
                    state.on_path_changed(&p);
                }
                if let Some(seconds) = state.pending_search_seek.take() {
                    state.mpv.seek_absolute(seconds);
                }
            }
            MpvEventId::EndFile => {
                state.cancel_mouse();
                state.playback_active = false;
                if state.overlay_active {
                    state.mpv.overlay_remove(OVERLAY_ID);
                    state.overlay_active = false;
                }
            }
            MpvEventId::ClientMessage => {
                let msg_ptr = event.data as *const MpvEventClientMessage;
                if !msg_ptr.is_null() {
                    let msg = unsafe { &*msg_ptr };
                    let mut args = Vec::new();
                    if !msg.args.is_null() && msg.num_args > 0 {
                        for i in 0..msg.num_args as isize {
                            let str_ptr = unsafe { *msg.args.offset(i) };
                            if !str_ptr.is_null() {
                                let arg_str = unsafe { std::ffi::CStr::from_ptr(str_ptr).to_string_lossy() };
                                args.push(arg_str.into_owned());
                            }
                        }
                    }

                    if let Some(first_arg) = args.first() {
                        match first_arg.as_str() {
                            "key-binding" => {
                                if args.get(1).map(String::as_str) == Some("twitch_chat_mouse") {
                                    if let Some(key_state) = args.get(2) {
                                        state.on_mouse_binding(key_state);
                                    }
                                }
                            }
                            "click" | "twitch_chat/click" | "twitch_chat_click" => {
                                let exp_x = args.get(1).and_then(|s| s.parse::<f64>().ok()).map(|f| f.round() as i32);
                                let exp_y = args.get(2).and_then(|s| s.parse::<f64>().ok()).map(|f| f.round() as i32);
                                state.on_click_event(exp_x, exp_y);
                            }
                            "mouse_up" | "twitch_chat/mouse_up" | "twitch_chat_mouse_up" => {
                                state.on_mouse_binding("um-");
                            }
                            "wheel_up" | "twitch_chat/wheel_up" | "twitch_chat_wheel_up" => {
                                if state.ui.minimized { continue; }
                                if state.ui.current_view == UiView::Settings {
                                    state.ui.settings_scroll_offset = (state.ui.settings_scroll_offset - 32.0).max(0.0);
                                } else if state.ui.current_view == UiView::Search {
                                    state.ui.search_scroll_offset = state.ui.search_scroll_offset.saturating_sub(1);
                                } else {
                                    state.scroll_chat(4);
                                }
                                state.update_frame();
                            }
                            "wheel_down" | "twitch_chat/wheel_down" | "twitch_chat_wheel_down" => {
                                if state.ui.minimized { continue; }
                                if state.ui.current_view == UiView::Settings {
                                    state.ui.settings_scroll_offset = (state.ui.settings_scroll_offset + 32.0).min(max_settings_scroll(state.ui.panel_height));
                                } else if state.ui.current_view == UiView::Search {
                                    state.ui.search_scroll_offset = (state.ui.search_scroll_offset + 1)
                                        .min(state.ui.search_results.len().saturating_sub(1));
                                } else {
                                    state.scroll_chat(-4);
                                }
                                state.update_frame();
                            }
                            "sk" | "twitch_chat/sk" | "twitch_chat_key" | "twitch_chat/key" => {
                                if args.len() >= 2 {
                                    state.ui.search_query.push_str(&args[1]);
                                    state.perform_search();
                                    state.update_frame();
                                }
                            }
                            "sk_space" | "twitch_chat/sk_space" | "twitch_chat_space" | "twitch_chat/space" => {
                                state.ui.search_query.push(' ');
                                state.perform_search();
                                state.update_frame();
                            }
                            "sk_bs" | "twitch_chat/sk_bs" | "twitch_chat_bs" | "twitch_chat/bs" => {
                                state.ui.search_query.pop();
                                state.perform_search();
                                state.update_frame();
                            }
                            "sk_esc" | "twitch_chat/sk_esc" | "twitch_chat_esc" | "twitch_chat/esc" => {
                                state.ui.current_view = UiView::Chat;
                                state.update_search_keys_state();
                                state.update_frame();
                            }
                            "sk_enter" | "twitch_chat/sk_enter" | "twitch_chat_enter" | "twitch_chat/enter" => {
                                if !state.ui.search_results.is_empty() {
                                    let result = state.ui.search_results[0].clone();
                                    state.open_chat_search_result(&result);
                                    state.ui.current_view = UiView::Chat;
                                    state.update_search_keys_state();
                                    state.update_frame();
                                }
                            }
                            "twitch_chat_ignore" => {}
                            "toggle" | "twitch_chat/toggle" => {
                                if state.ui.minimized {
                                    state.ui.restore();
                                    state.ui.chat_enabled = true;
                                } else {
                                    state.ui.chat_enabled = !state.ui.chat_enabled;
                                }
                                state.update_search_keys_state();
                                state.update_frame();
                            }
                            "toggle_settings" | "twitch_chat/toggle_settings" => {
                                if state.ui.current_view == UiView::Settings {
                                    state.ui.current_view = UiView::Chat;
                                } else {
                                    state.ui.current_view = UiView::Settings;
                                }
                                state.update_search_keys_state();
                                state.update_frame();
                            }
                            "toggle_search" | "twitch_chat/toggle_search" => {
                                if state.ui.current_view == UiView::Search {
                                    state.ui.current_view = UiView::Chat;
                                } else {
                                    state.ui.current_view = UiView::Search;
                                    state.perform_search();
                                }
                                state.update_search_keys_state();
                                state.update_frame();
                            }
                            "offset_inc" | "twitch_chat/offset_inc" => {
                                state.ui.offset_ms += 1000;
                                state.update_frame();
                            }
                            "offset_dec" | "twitch_chat/offset_dec" => {
                                state.ui.offset_ms -= 1000;
                                state.update_frame();
                            }
                            "offset_reset" | "twitch_chat/offset_reset" => {
                                state.ui.offset_ms = 0;
                                state.update_frame();
                            }
                            _ => {}
                        }
                    }
                }
            }
            MpvEventId::Tick => {
                if state.vo_configured && state.playback_active && state.session.is_some() {
                    state.update_frame();
                }
            }
            _ => {}
        }
    }

    if state.overlay_active {
        state.mpv.overlay_remove(OVERLAY_ID);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_config_overlay_settings() {
        let temp_dir = std::env::temp_dir().join(format!("test_twitch_cfg_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let config_file = temp_dir.join("config.json");

        // Pre-populate config.json with launcher settings
        let initial_cfg = serde_json::json!({
            "streamlink_path": "streamlink",
            "mpv_path": "mpv",
            "records_dir": "records",
            "twitch_client_id": "test_client_id"
        });
        std::fs::write(&config_file, serde_json::to_string_pretty(&initial_cfg).unwrap()).unwrap();

        let mut ui = UiState::default();
        ui.panel_x = 500;
        ui.panel_y = 120;
        ui.panel_width = 400;
        ui.panel_height = 700;
        ui.font_size = 18.0;
        ui.bg_opacity = 0.85;
        ui.chat_enabled = true;
        ui.emotes_enabled = false;
        ui.badges_enabled = true;
        ui.text_effect = TextEffect::Shadow;
        ui.outline_width = 2.0;

        let mut map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(&config_file).unwrap()).unwrap();
        map.insert("chat_x".to_string(), serde_json::json!(ui.panel_x));
        map.insert("chat_y".to_string(), serde_json::json!(ui.panel_y));
        map.insert("chat_width".to_string(), serde_json::json!(ui.panel_width));
        map.insert("chat_height".to_string(), serde_json::json!(ui.panel_height));
        map.insert("chat_font_size".to_string(), serde_json::json!(ui.font_size));
        map.insert("chat_bg_opacity".to_string(), serde_json::json!(ui.bg_opacity));
        map.insert("chat_enabled".to_string(), serde_json::json!(ui.chat_enabled));
        map.insert("emotes_enabled".to_string(), serde_json::json!(ui.emotes_enabled));
        map.insert("badges_enabled".to_string(), serde_json::json!(ui.badges_enabled));
        map.insert("chat_text_effect".to_string(), serde_json::json!(ui.text_effect));
        map.insert("chat_outline_width".to_string(), serde_json::json!(ui.outline_width));
        std::fs::write(&config_file, serde_json::to_string_pretty(&map).unwrap()).unwrap();

        let saved_val: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config_file).unwrap()).unwrap();
        assert_eq!(saved_val["streamlink_path"], "streamlink");
        assert_eq!(saved_val["twitch_client_id"], "test_client_id");
        assert_eq!(saved_val["chat_x"], 500);
        assert_eq!(saved_val["chat_y"], 120);
        assert_eq!(saved_val["chat_width"], 400);
        assert_eq!(saved_val["chat_height"], 700);
        assert_eq!(saved_val["chat_font_size"], 18.0);
        assert!((saved_val["chat_bg_opacity"].as_f64().unwrap() - 0.85).abs() < 1e-4);
        assert_eq!(saved_val["emotes_enabled"], false);
        assert_eq!(saved_val["chat_text_effect"], "shadow");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
