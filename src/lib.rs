//! Twitch Chat Overlay mpv C-Plugin (Rust Native DLL)
//!
//! High-performance native mpv plugin for Windows / Linux.
//! Provides instantaneous emote rendering and rock-solid Direct3D11 / gpu-next compatibility.

#![allow(dead_code)]

pub mod chat;
pub mod emotes;
pub mod layout;
pub mod mpv_api;
pub mod renderer;
pub mod session;
pub mod ui;

use emotes::EmoteCache;
use mpv_api::{log_plugin, MpvEventClientMessage, MpvEventId, MpvFormat, MpvHandle, MpvHandleRaw};
use renderer::ChatRenderer;
use session::{swap_keyboard_layout, SessionManager};
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use ui::{HitBox, UiState, UiView};

// mpv overlay ID must be in the range [0..63].
// ID 1 is safe, valid, and avoids thumbfast.lua's default ID 42.
const OVERLAY_ID: u32 = 1;

/// Safely retrieves client cursor coordinates on Windows without triggering mpv node formatting crashes
#[cfg(target_os = "windows")]
fn get_client_cursor_pos(mpv: &MpvHandle) -> Option<(i32, i32)> {
    use std::os::raw::c_void;

    #[repr(C)]
    #[derive(Default, Copy, Clone)]
    struct WinPoint {
        x: i32,
        y: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetCursorPos(lpPoint: *mut WinPoint) -> i32;
        fn ScreenToClient(hWnd: *mut c_void, lpPoint: *mut WinPoint) -> i32;
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
            if ScreenToClient(target_hwnd, &mut pt) != 0 {
                return Some((pt.x, pt.y));
            }
        }

        None
    }
}

#[cfg(not(target_os = "windows"))]
fn get_client_cursor_pos(_mpv: &MpvHandle) -> Option<(i32, i32)> {
    None
}

/// Checks if physical left mouse button is pressed on Windows
#[cfg(target_os = "windows")]
fn is_mouse_lbutton_down() -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(vKey: i32) -> i16;
    }
    unsafe {
        (GetAsyncKeyState(0x01 /* VK_LBUTTON */) as u16 & 0x8000) != 0
    }
}

#[cfg(not(target_os = "windows"))]
fn is_mouse_lbutton_down() -> bool {
    true
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
    vo_configured: bool,
    playback_active: bool,
    overlay_active: bool,
    mouse_section_defined: bool,
    mouse_section_enabled: bool,
    search_section_defined: bool,
    search_section_enabled: bool,
    // Header drag state
    is_dragging: bool,
    drag_start_cursor_x: i32,
    drag_start_cursor_y: i32,
    drag_start_panel_x: i32,
    drag_start_panel_y: i32,
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
            vo_configured: false,
            playback_active: false,
            overlay_active: false,
            mouse_section_defined: false,
            mouse_section_enabled: false,
            search_section_defined: false,
            search_section_enabled: false,
            is_dragging: false,
            drag_start_cursor_x: 0,
            drag_start_cursor_y: 0,
            drag_start_panel_x: 0,
            drag_start_panel_y: 0,
        }
    }

    fn init_mouse_section(&mut self) {
        if !self.mouse_section_defined {
            self.mpv.command(&[
                "define-section",
                "twitch_chat_mouse",
                "MBTN_LEFT script-message click\nMBTN_LEFT_UP script-message mouse_up\nWHEEL_UP script-message wheel_up\nWHEEL_DOWN script-message wheel_down",
                "force",
            ]);
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
        let should_enable = self.ui.chat_enabled && self.ui.current_view == UiView::Search;
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

    fn update_drag(&mut self) {
        if !self.is_dragging {
            return;
        }

        // Check if physical left mouse button was released
        #[cfg(target_os = "windows")]
        if !is_mouse_lbutton_down() {
            self.is_dragging = false;
            return;
        }

        if let Some((cur_mx, cur_my)) = get_client_cursor_pos(&self.mpv) {
            let dx = cur_mx - self.drag_start_cursor_x;
            let dy = cur_my - self.drag_start_cursor_y;

            let new_x = self.drag_start_panel_x + dx;
            let new_y = self.drag_start_panel_y + dy;

            let osd_w = self.mpv.get_property_double("osd-width").unwrap_or(1920.0) as i32;
            let osd_h = self.mpv.get_property_double("osd-height").unwrap_or(1080.0) as i32;

            let max_x = (osd_w - 60).max(0);
            let max_y = (osd_h - 40).max(0);

            let clamped_x = new_x.clamp(0, max_x);
            let clamped_y = new_y.clamp(0, max_y);

            if clamped_x != self.ui.panel_x || clamped_y != self.ui.panel_y {
                self.ui.panel_x = clamped_x;
                self.ui.panel_y = clamped_y;
                self.update_frame();
            }
        }
    }

    fn update_mouse_hover(&mut self) {
        if !self.ui.chat_enabled || !self.vo_configured || !self.playback_active {
            if self.mouse_section_enabled {
                self.mpv.command(&["disable-section", "twitch_chat_mouse"]);
                self.mouse_section_enabled = false;
            }
            return;
        }

        self.init_mouse_section();

        let is_hovering = if self.is_dragging {
            true
        } else if let Some((mx, my)) = get_client_cursor_pos(&self.mpv) {
            mx >= self.ui.panel_x
                && mx <= self.ui.panel_x + self.ui.panel_width as i32
                && my >= self.ui.panel_y
                && my <= self.ui.panel_y + self.ui.panel_height as i32
        } else {
            false
        };

        if is_hovering && !self.mouse_section_enabled {
            self.mpv.command(&["enable-section", "twitch_chat_mouse", "allow-hide-cursor+allow-vo-dragging"]);
            self.mouse_section_enabled = true;
        } else if !is_hovering && self.mouse_section_enabled {
            self.mpv.command(&["disable-section", "twitch_chat_mouse"]);
            self.mouse_section_enabled = false;
        }
    }

    fn perform_search(&mut self) {
        if let Some(session) = &self.session {
            let session_start = session.session_data.session_start_ms;
            self.ui.search_results = session.search_messages(
                &self.ui.search_query,
                session_start,
                self.ui.offset_ms,
                50,
            );
            self.ui.search_scroll_offset = 0;
        }
    }

    fn on_click_event(&mut self) {
        let (mx, my) = match get_client_cursor_pos(&self.mpv) {
            Some(pos) => pos,
            None => return,
        };

        let rel_x = (mx - self.ui.panel_x) as f32;
        let rel_y = (my - self.ui.panel_y) as f32;

        if rel_x < 0.0
            || rel_x > self.ui.panel_width as f32
            || rel_y < 0.0
            || rel_y > self.ui.panel_height as f32
        {
            if self.mouse_section_enabled && !self.is_dragging {
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
            self.ui.chat_enabled = false;
            if self.overlay_active {
                self.mpv.overlay_remove(OVERLAY_ID);
                self.overlay_active = false;
            }
            if self.mouse_section_enabled {
                self.mpv.command(&["disable-section", "twitch_chat_mouse"]);
                self.mouse_section_enabled = false;
            }
            self.update_search_keys_state();
            return;
        }

        // Header Drag Bar (any click in the top 28px that is not on a tab/close button)
        if rel_y >= 0.0 && rel_y <= 28.0 {
            self.is_dragging = true;
            self.drag_start_cursor_x = mx;
            self.drag_start_cursor_y = my;
            self.drag_start_panel_x = self.ui.panel_x;
            self.drag_start_panel_y = self.ui.panel_y;
            return;
        }

        // 2. View specific clicks
        match self.ui.current_view {
            UiView::Chat => {
                let height = self.ui.panel_height as f32;
                // If scrolled (into history or future), clicking bottom banner returns to live
                if self.ui.scroll_offset != 0 && rel_y >= height - 32.0 {
                    self.ui.scroll_offset = 0;
                    self.update_frame();
                    return;
                }

                // Hit-test messages and jump to clicked message timestamp
                let session = match self.session.as_ref() {
                    Some(s) => s,
                    None => return,
                };

                let current_video_sec = self.current_time_pos;
                let session_start = session.session_data.session_start_ms;
                let current_ms = session_start
                    + self.ui.offset_ms
                    + (current_video_sec * 1000.0).round() as i64;
                let active_msgs = session.get_messages_up_to(current_ms, self.ui.scroll_offset, 80);

                let dummy_cache;
                let emotes_ref = match self.emotes.as_ref() {
                    Some(e) => e,
                    None => {
                        dummy_cache = EmoteCache::new(&session.session_dir);
                        &dummy_cache
                    }
                };

                let layout_messages = self.renderer.layouter().layout_messages(
                    active_msgs,
                    &session.emotes_manifest,
                    emotes_ref,
                    self.ui.panel_width as f32,
                    self.ui.font_size,
                    self.ui.emotes_enabled,
                );

                let visible_top = 32.0;
                let ban_h = if self.ui.scroll_offset != 0 { 28.0 } else { 0.0 };
                let visible_bottom = height - ban_h - 4.0;
                let total_msg_h = layout_messages.last().map(|m| m.bottom_y).unwrap_or(0.0);
                let available_h = visible_bottom - visible_top;
                let base_y = if total_msg_h > available_h {
                    visible_top + (available_h - total_msg_h)
                } else {
                    visible_top + 4.0
                };

                for msg in &layout_messages {
                    let msg_top = base_y + msg.top_y;
                    let msg_bottom = base_y + msg.bottom_y;

                    if rel_y >= msg_top && rel_y <= msg_bottom {
                        if msg.msg_index < active_msgs.len() {
                            let clicked_msg = &active_msgs[msg.msg_index];
                            self.ui.selected_sync_msg = Some(clicked_msg.clone());
                            let seek_sec = ((clicked_msg.timestamp_ms - session_start - self.ui.offset_ms) as f64 / 1000.0).max(0.0);
                            self.mpv.seek_absolute(seek_sec);
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
        let width = self.ui.panel_width as f32;
        let mut cur_y = 36.0 + 24.0; // after title

        // Row 1: Reset offset
        if ChatRenderer::btn_offset_reset().contains(rx, ry) {
            self.ui.offset_ms = 0;
            let video_filename = Path::new(&self.current_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
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
                let video_filename = Path::new(&self.current_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
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
            self.update_frame();
            return;
        }
        if HitBox::new(98.0, cur_y, 28.0, 22.0).contains(rx, ry) {
            self.ui.font_size = (self.ui.font_size + 1.0).min(36.0);
            self.update_frame();
            return;
        }

        // Panel Width: [W-] (230, cur_y, 28, 22), [W+] (262, cur_y, 28, 22)
        if HitBox::new(230.0, cur_y, 28.0, 22.0).contains(rx, ry) {
            self.ui.panel_width = (self.ui.panel_width.saturating_sub(40)).max(260);
            self.update_frame();
            return;
        }
        if HitBox::new(262.0, cur_y, 28.0, 22.0).contains(rx, ry) {
            self.ui.panel_width = (self.ui.panel_width + 40).min(1200);
            self.update_frame();
            return;
        }
        cur_y += 30.0;

        // Row 3: Panel Height: [H-] (65, cur_y, 28, 22), [H+] (98, cur_y, 28, 22)
        if HitBox::new(65.0, cur_y, 28.0, 22.0).contains(rx, ry) {
            self.ui.panel_height = (self.ui.panel_height.saturating_sub(40)).max(220);
            self.update_frame();
            return;
        }
        if HitBox::new(98.0, cur_y, 28.0, 22.0).contains(rx, ry) {
            self.ui.panel_height = (self.ui.panel_height + 40).min(1400);
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
            self.update_frame();
            return;
        }
        cur_y += 34.0;

        // Row 4: Position Presets: [↖ ВЛ] [↙ НЛ] [↗ ВП] [↘ НП]
        let osd_w = self.mpv.get_property_double("osd-width").unwrap_or(1920.0) as i32;
        let osd_h = self.mpv.get_property_double("osd-height").unwrap_or(1080.0) as i32;
        let mut px = 75.0;
        // 0: TL
        if HitBox::new(px, cur_y, 48.0, 22.0).contains(rx, ry) {
            self.ui.panel_x = 24;
            self.ui.panel_y = 60;
            self.update_frame();
            return;
        }
        px += 54.0;
        // 1: BL
        if HitBox::new(px, cur_y, 48.0, 22.0).contains(rx, ry) {
            self.ui.panel_x = 24;
            self.ui.panel_y = (osd_h - self.ui.panel_height as i32 - 40).max(40);
            self.update_frame();
            return;
        }
        px += 54.0;
        // 2: TR
        if HitBox::new(px, cur_y, 48.0, 22.0).contains(rx, ry) {
            self.ui.panel_x = (osd_w - self.ui.panel_width as i32 - 24).max(24);
            self.ui.panel_y = 60;
            self.update_frame();
            return;
        }
        px += 54.0;
        // 3: BR
        if HitBox::new(px, cur_y, 48.0, 22.0).contains(rx, ry) {
            self.ui.panel_x = (osd_w - self.ui.panel_width as i32 - 24).max(24);
            self.ui.panel_y = (osd_h - self.ui.panel_height as i32 - 40).max(40);
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
                let session_start = session.session_data.session_start_ms;
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
                let video_filename = Path::new(&self.current_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                if let Some(session) = &mut self.session {
                    let success = session.calibrate_anchor(target_ts, current_video_sec, &video_filename);
                    self.ui.offset_ms = 0;
                    self.update_frame();

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
                            session.session_data.session_start_ms,
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
                }
            } else {
                self.mpv.osd_message("Сначала кликните по сообщению в чате для выбора якоря!", 2500);
            }
        }
    }

    fn handle_search_click(&mut self, rx: f32, ry: f32) {
        let width = self.ui.panel_width as f32;
        let height = self.ui.panel_height as f32;

        // 1. Action Buttons: [RU ⇄ EN], [⌫ Стереть], [🗑 Очистить] at y=72.0
        let btn_y = 72.0;
        if HitBox::new(12.0, btn_y, 76.0, 22.0).contains(rx, ry) {
            self.ui.search_query = swap_keyboard_layout(&self.ui.search_query);
            self.perform_search();
            self.update_frame();
            return;
        }
        if HitBox::new(94.0, btn_y, 76.0, 22.0).contains(rx, ry) {
            self.ui.search_query.pop();
            self.perform_search();
            self.update_frame();
            return;
        }
        if HitBox::new(176.0, btn_y, 76.0, 22.0).contains(rx, ry) {
            self.ui.search_query.clear();
            self.ui.search_results.clear();
            self.update_frame();
            return;
        }

        // 2. Results list cards: start at y=102.0
        let cur_y = 102.0;
        let item_h = 36.0;

        let visible_items = ((height - cur_y) / item_h) as usize;
        let start_idx = self.ui.search_scroll_offset;
        let end_idx = (start_idx + visible_items).min(self.ui.search_results.len());

        for (i, res) in self.ui.search_results[start_idx..end_idx].iter().enumerate() {
            let item_y = cur_y + (i as f32 * item_h);
            let hb = HitBox::new(12.0, item_y, width - 24.0, item_h - 4.0);
            if hb.contains(rx, ry) {
                self.mpv.seek_absolute(res.sec);
                self.ui.current_view = UiView::Chat;
                self.update_search_keys_state();
                self.update_frame();
                return;
            }
        }
    }

    fn parse_script_opts(&mut self, opts: &str) {
        log_plugin(&format!("Processing script-opts: {}", opts));
        for item in opts.split(',') {
            if let Some((k, v)) = item.split_once('=') {
                let key = k.trim();
                let val = v.trim();
                match key {
                    "twitch_session" => {
                        log_plugin(&format!("twitch_session specified: {}", val));
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
        let path = PathBuf::from(new_path);
        if let Some(mgr) = SessionManager::try_load(&path) {
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            let initial_offset = mgr
                .session_data
                .offsets_ms
                .get(&filename)
                .or_else(|| mgr.session_data.offsets_ms.get("default"))
                .copied()
                .unwrap_or(0);
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
            self.emotes = Some(EmoteCache::new(&mgr.session_dir));
            self.session = Some(mgr);
            self.current_path = new_path.to_string();
            self.update_frame();
        } else {
            log_plugin(&format!("Could not load session from path: {:?}", path));
        }
    }

    fn update_frame(&mut self) {
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

        let session = match self.session.as_mut() {
            Some(s) => s,
            None => return,
        };
        let emotes = match self.emotes.as_mut() {
            Some(e) => e,
            None => return,
        };

        let current_video_sec = self.current_time_pos;
        let session_start = session.session_data.session_start_ms;
        let current_ms =
            session_start + self.ui.offset_ms + (current_video_sec * 1000.0).round() as i64;

        // Fetch recent messages with scroll offset applied
        let active_msgs = session.get_messages_up_to(current_ms, self.ui.scroll_offset, 80);

        // Layout messages with badges and inline emotes
        let layout_messages = self.renderer.layouter().layout_messages(
            active_msgs,
            &session.emotes_manifest,
            emotes,
            self.ui.panel_width as f32,
            self.ui.font_size,
            self.ui.emotes_enabled,
        );

        let channel = self
            .session
            .as_ref()
            .map(|s| s.session_data.channel.as_str())
            .unwrap_or("");
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

    let mut state = PluginState::new(mpv);

    // Read initial script-opts
    if let Some(opts) = state.mpv.get_property_string("script-opts") {
        state.parse_script_opts(&opts);
    }
    if let Some(opts) = state.mpv.get_property_string("options/script-opts") {
        state.parse_script_opts(&opts);
    }

    // Main event loop (runs in mpv's dedicated plugin thread)
    loop {
        // Continuously update position if dragging
        state.update_drag();

        // Dynamically update hover state so mouse section is enabled ONLY when cursor is over the panel
        state.update_mouse_hover();
        state.update_search_keys_state();

        let wait_timeout = if state.is_dragging { 0.01 } else { 0.04 };
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
            }
            MpvEventId::EndFile => {
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
                            "click" | "twitch_chat/click" | "twitch_chat_click" => {
                                state.on_click_event();
                            }
                            "mouse_up" | "twitch_chat/mouse_up" | "twitch_chat_mouse_up" => {
                                state.is_dragging = false;
                            }
                            "wheel_up" | "twitch_chat/wheel_up" | "twitch_chat_wheel_up" => {
                                if state.ui.current_view == UiView::Search {
                                    state.ui.search_scroll_offset = state.ui.search_scroll_offset.saturating_sub(1);
                                } else {
                                    // Scroll backward in time (history)
                                    let max_past = state.session.as_ref().map(|s| s.messages.len() as i32).unwrap_or(50000);
                                    state.ui.scroll_offset = (state.ui.scroll_offset + 4).min(max_past);
                                }
                                state.update_frame();
                            }
                            "wheel_down" | "twitch_chat/wheel_down" | "twitch_chat_wheel_down" => {
                                if state.ui.current_view == UiView::Search {
                                    state.ui.search_scroll_offset = (state.ui.search_scroll_offset + 1)
                                        .min(state.ui.search_results.len().saturating_sub(1));
                                } else {
                                    // Scroll forward in time (future messages)
                                    let max_future = state.session.as_ref().map(|s| s.messages.len() as i32).unwrap_or(50000);
                                    state.ui.scroll_offset = (state.ui.scroll_offset - 4).max(-max_future);
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
                                    let sec = state.ui.search_results[0].sec;
                                    state.mpv.seek_absolute(sec);
                                    state.ui.current_view = UiView::Chat;
                                    state.update_search_keys_state();
                                    state.update_frame();
                                }
                            }
                            "twitch_chat_ignore" => {}
                            "toggle" | "twitch_chat/toggle" => {
                                state.ui.chat_enabled = !state.ui.chat_enabled;
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
