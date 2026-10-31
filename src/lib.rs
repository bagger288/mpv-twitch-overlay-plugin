//! Twitch Chat Overlay mpv C-Plugin (Rust Native DLL)
//!
//! High-performance native mpv plugin for Windows / Linux.
//! Provides instantaneous emote rendering, interactive settings drawer,
//! bilingual search, and rock-solid Direct3D11 / gpu-next compatibility.

#![allow(dead_code)]

pub mod chat;
pub mod emotes;
pub mod layout;
pub mod mpv_api;
pub mod renderer;
pub mod session;
pub mod ui;

use emotes::EmoteCache;
use mpv_api::{log_plugin, MpvEventId, MpvFormat, MpvHandle, MpvHandleRaw};
use renderer::ChatRenderer;
use session::{swap_keyboard_layout, SessionManager};
use std::os::raw::c_int;
use std::path::PathBuf;
use ui::{HitBox, UiState, UiView};

// Unique overlay ID (ID 42 is used by thumbfast.lua and caused collisions!)
const OVERLAY_ID: u32 = 1;

#[derive(serde::Deserialize, Default)]
struct MousePos {
    x: i32,
    y: i32,
    #[serde(default)]
    hover: bool,
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
    bindings_initialized: bool,
    search_keys_active: bool,
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
            bindings_initialized: false,
            search_keys_active: false,
        }
    }

    /// Initializes hotkeys and mouse bindings safely after file is loaded and VO initialized
    fn init_bindings(&mut self) {
        if self.bindings_initialized {
            return;
        }
        self.bindings_initialized = true;

        log_plugin("Initializing plugin hotkeys and bindings");
        let bindings = "c script-message-to twitch_chat toggle\ns script-message-to twitch_chat toggle_settings\nCtrl+f script-message-to twitch_chat toggle_search\nMBTN_LEFT script-message-to twitch_chat click\nWHEEL_UP script-message-to twitch_chat wheel_up\nWHEEL_DOWN script-message-to twitch_chat wheel_down\n";
        self.mpv.command(&["define-section", "twitch_chat_keys", bindings, "default"]);
        self.mpv.command(&["enable-section", "twitch_chat_keys", "allow-hide-cursor"]);
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
            log_plugin(&format!(
                "Session successfully loaded from: {:?}",
                mgr.session_dir
            ));
            log_plugin(&format!(
                "Found {} chat messages, {} channel emotes",
                mgr.messages.len(),
                mgr.emotes_manifest.channel_emotes.len()
            ));
            self.emotes = Some(EmoteCache::new(&mgr.session_dir));
            self.session = Some(mgr);
            self.current_path = new_path.to_string();
            self.update_frame();
        } else {
            log_plugin(&format!("Could not load session from path: {:?}", path));
        }
    }

    fn execute_search(&mut self) {
        if let Some(ref session) = self.session {
            let start_ms = session.session_data.session_start_ms;
            self.ui.search_results = session.search_messages(
                &self.ui.search_query,
                start_ms,
                self.ui.offset_ms,
                150,
            );
            self.ui.search_scroll_offset = 0;
        }
    }

    fn set_view(&mut self, view: UiView) {
        self.ui.current_view = view;
        if view == UiView::Search {
            self.enable_search_keys();
            self.execute_search();
        } else {
            self.disable_search_keys();
        }
        self.update_frame();
    }

    fn enable_search_keys(&mut self) {
        if self.search_keys_active {
            return;
        }
        self.search_keys_active = true;

        let mut sec = String::with_capacity(4096);
        sec.push_str("SPACE script-message-to twitch_chat char \" \"\n");
        sec.push_str("BS script-message-to twitch_chat backspace\n");
        sec.push_str("ESC script-message-to twitch_chat escape\n");
        sec.push_str("ENTER script-message-to twitch_chat enter\n");

        for d in 0..=9 {
            sec.push_str(&format!("{d} script-message-to twitch_chat char {d}\n"));
        }
        for c in 'a'..='z' {
            sec.push_str(&format!("{c} script-message-to twitch_chat char {c}\n"));
            let up = c.to_ascii_uppercase();
            sec.push_str(&format!("Shift+{c} script-message-to twitch_chat char {up}\n"));
        }
        let ru_lower = [
            'а', 'б', 'в', 'г', 'д', 'е', 'ё', 'ж', 'з', 'и', 'й', 'к', 'л', 'м', 'н',
            'о', 'п', 'р', 'с', 'т', 'у', 'ф', 'х', 'ц', 'ч', 'ш', 'щ', 'ъ', 'ы', 'ь',
            'э', 'ю', 'я',
        ];
        for &c in &ru_lower {
            sec.push_str(&format!("{c} script-message-to twitch_chat char {c}\n"));
            for up in c.to_uppercase() {
                sec.push_str(&format!("Shift+{c} script-message-to twitch_chat char {up}\n"));
            }
        }

        self.mpv.command(&["define-section", "twitch_chat_search_keys", &sec, "force"]);
        self.mpv.command(&["enable-section", "twitch_chat_search_keys", "allow-hide-cursor"]);
    }

    fn disable_search_keys(&mut self) {
        if !self.search_keys_active {
            return;
        }
        self.search_keys_active = false;
        self.mpv.command(&["disable-section", "twitch_chat_search_keys"]);
    }

    fn on_click_event(&mut self) {
        // Query mouse position on demand only when clicked
        let pos = if let Some(json) = self.mpv.get_property_string("mouse-pos") {
            serde_json::from_str::<MousePos>(&json).unwrap_or_default()
        } else {
            return;
        };

        let inside = pos.x >= self.ui.panel_x
            && pos.x <= self.ui.panel_x + self.ui.panel_width as i32
            && pos.y >= self.ui.panel_y
            && pos.y <= self.ui.panel_y + self.ui.panel_height as i32;

        if !inside {
            // If clicked outside, close modal/drawers if open
            if self.ui.current_view != UiView::Chat {
                self.set_view(UiView::Chat);
            }
            return;
        }

        self.handle_click(pos.x, pos.y);
    }

    fn on_wheel_event(&mut self, is_up: bool) {
        let pos = if let Some(json) = self.mpv.get_property_string("mouse-pos") {
            serde_json::from_str::<MousePos>(&json).unwrap_or_default()
        } else {
            return;
        };

        let inside = pos.x >= self.ui.panel_x
            && pos.x <= self.ui.panel_x + self.ui.panel_width as i32
            && pos.y >= self.ui.panel_y
            && pos.y <= self.ui.panel_y + self.ui.panel_height as i32;

        if !inside {
            return;
        }

        match self.ui.current_view {
            UiView::Chat => {
                if is_up {
                    self.ui.scroll_offset += 6;
                } else {
                    self.ui.scroll_offset = self.ui.scroll_offset.saturating_sub(6);
                }
                self.update_frame();
            }
            UiView::Search => {
                if is_up {
                    self.ui.search_scroll_offset = self.ui.search_scroll_offset.saturating_sub(1);
                } else if self.ui.search_scroll_offset + 1 < self.ui.search_results.len() {
                    self.ui.search_scroll_offset += 1;
                }
                self.update_frame();
            }
            UiView::Settings => {}
        }
    }

    fn handle_click(&mut self, screen_x: i32, screen_y: i32) {
        let rel_x = (screen_x - self.ui.panel_x) as f32;
        let rel_y = (screen_y - self.ui.panel_y) as f32;
        let width = self.ui.panel_width as f32;

        // 1. Header Tab buttons
        if ChatRenderer::btn_tab_chat().contains(rel_x, rel_y) {
            self.set_view(UiView::Chat);
            return;
        }
        if ChatRenderer::btn_tab_search().contains(rel_x, rel_y) {
            self.set_view(UiView::Search);
            return;
        }
        if ChatRenderer::btn_tab_settings().contains(rel_x, rel_y) {
            self.set_view(UiView::Settings);
            return;
        }
        if ChatRenderer::btn_close(width).contains(rel_x, rel_y) {
            self.ui.chat_enabled = false;
            self.update_frame();
            return;
        }

        // 2. View specific clicks
        match self.ui.current_view {
            UiView::Settings => {
                // Reset offset
                if ChatRenderer::btn_offset_reset().contains(rel_x, rel_y) {
                    self.ui.offset_ms = 0;
                    self.update_frame();
                    return;
                }

                // Offset buttons [-10s]..[+10s]
                let deltas = [-10000, -1000, -100, 100, 1000, 10000];
                for (i, &delta) in deltas.iter().enumerate() {
                    if ChatRenderer::btn_offset_step(i, 84.0).contains(rel_x, rel_y) {
                        self.ui.offset_ms += delta;
                        self.update_frame();
                        return;
                    }
                }

                // Font size: [A-] [A+]
                if HitBox::new(65.0, 116.0, 28.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.font_size = (self.ui.font_size - 1.0).max(11.0);
                    self.update_frame();
                    return;
                }
                if HitBox::new(98.0, 116.0, 28.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.font_size = (self.ui.font_size + 1.0).min(38.0);
                    self.update_frame();
                    return;
                }

                // Width: [W-] [W+]
                if HitBox::new(230.0, 116.0, 28.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.panel_width = (self.ui.panel_width.saturating_sub(40)).max(280);
                    self.update_frame();
                    return;
                }
                if HitBox::new(262.0, 116.0, 28.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.panel_width = (self.ui.panel_width + 40).min(900);
                    self.update_frame();
                    return;
                }

                // Height: [H-] [H+]
                if HitBox::new(65.0, 146.0, 28.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.panel_height = (self.ui.panel_height.saturating_sub(40)).max(300);
                    self.update_frame();
                    return;
                }
                if HitBox::new(98.0, 146.0, 28.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.panel_height = (self.ui.panel_height + 40).min(1080);
                    self.update_frame();
                    return;
                }

                // Opacity cycle
                if HitBox::new(190.0, 146.0, 90.0, 22.0).contains(rel_x, rel_y) {
                    if self.ui.bg_opacity < 0.4 {
                        self.ui.bg_opacity = 0.50;
                    } else if self.ui.bg_opacity < 0.65 {
                        self.ui.bg_opacity = 0.75;
                    } else if self.ui.bg_opacity < 0.90 {
                        self.ui.bg_opacity = 1.0;
                    } else {
                        self.ui.bg_opacity = 0.25;
                    }
                    self.update_frame();
                    return;
                }

                // Position Presets: [↖ ВЛ] [↙ НЛ] [↗ ВП] [↘ НП]
                if HitBox::new(75.0, 180.0, 48.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.panel_x = 24;
                    self.ui.panel_y = 60;
                    self.update_frame();
                    return;
                }
                if HitBox::new(129.0, 180.0, 48.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.panel_x = 24;
                    self.ui.panel_y = (1080 - 60 - self.ui.panel_height as i32).max(60);
                    self.update_frame();
                    return;
                }
                if HitBox::new(183.0, 180.0, 48.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.panel_x = (1920 - 24 - self.ui.panel_width as i32).max(24);
                    self.ui.panel_y = 60;
                    self.update_frame();
                    return;
                }
                if HitBox::new(237.0, 180.0, 48.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.panel_x = (1920 - 24 - self.ui.panel_width as i32).max(24);
                    self.ui.panel_y = (1080 - 60 - self.ui.panel_height as i32).max(60);
                    self.update_frame();
                    return;
                }

                // 1-Click Sync Anchor Calibration
                let sync_btn_hb = HitBox::new(20.0, 258.0, width - 40.0, 24.0);
                if sync_btn_hb.contains(rel_x, rel_y) {
                    if let Some(ref msg) = self.ui.selected_sync_msg {
                        if let Some(ref mut session) = self.session {
                            if session.calibrate_anchor(msg.timestamp_ms, self.current_time_pos) {
                                self.ui.offset_ms = 0;
                                log_plugin("Sync anchor successfully calibrated!");
                            }
                        }
                    }
                    self.update_frame();
                    return;
                }
            }
            UiView::Search => {
                // RU ⇄ EN layout swapper
                if HitBox::new(12.0, 72.0, 76.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.search_query = swap_keyboard_layout(&self.ui.search_query);
                    self.execute_search();
                    self.update_frame();
                    return;
                }
                // Backspace
                if HitBox::new(94.0, 72.0, 76.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.search_query.pop();
                    self.execute_search();
                    self.update_frame();
                    return;
                }
                // Clear
                if HitBox::new(176.0, 72.0, 76.0, 22.0).contains(rel_x, rel_y) {
                    self.ui.search_query.clear();
                    self.execute_search();
                    self.update_frame();
                    return;
                }

                // Click on search result card to seek video
                if rel_y >= 102.0 {
                    let idx = self.ui.search_scroll_offset + ((rel_y - 102.0) / 36.0) as usize;
                    if let Some(res) = self.ui.search_results.get(idx) {
                        log_plugin(&format!("Seeking to search result at {:.2}s", res.sec));
                        self.mpv.seek_absolute(res.sec);
                    }
                }
            }
            UiView::Chat => {
                // If scrolled back, clicking bottom banner restores live
                if self.ui.scroll_offset > 0 && rel_y >= (self.ui.panel_height as f32 - 30.0) {
                    self.ui.scroll_offset = 0;
                    self.update_frame();
                    return;
                }

                // Select clicked message as sync anchor
                if let Some(ref session) = self.session {
                    let cur_ms = session.session_data.session_start_ms
                        + self.ui.offset_ms
                        + (self.current_time_pos * 1000.0).round() as i64;
                    let msgs = session.get_messages_up_to(cur_ms, 80);
                    if let Some(last) = msgs.last() {
                        self.ui.selected_sync_msg = Some(last.clone());
                    }
                }
            }
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

        // CRITICAL: Strictly guard against calling overlay-add before vo-configured is true!
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

        // Fetch recent messages
        let active_msgs = session.get_messages_up_to(current_ms, 80);

        // Layout messages
        let line_height = (self.ui.font_size * 1.45).round();
        let emote_size = (self.ui.font_size * 1.3).round() as u32;

        let layout_messages = self.renderer.layouter().layout_messages(
            active_msgs,
            &session.emotes_manifest,
            self.ui.panel_width as f32,
            self.ui.font_size,
            line_height,
            emote_size,
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

    // Observe essential playback properties ONLY (DO NOT observe mouse-pos continuously!)
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
        let event_ptr = state.mpv.wait_event(0.04);
        if event_ptr.is_null() {
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
                            state.init_bindings();
                            state.update_frame();
                        }
                    }
                    _ => {}
                }
            }
            MpvEventId::PlaybackRestart => {
                state.playback_active = true;
                state.vo_configured = is_vo_configured(&state.mpv);
                if state.vo_configured {
                    state.init_bindings();
                }
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
                if state.vo_configured {
                    state.init_bindings();
                }
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
                let msg_data = event.data as *mut *const std::ffi::c_char;
                if !msg_data.is_null() {
                    let mut args = Vec::new();
                    let mut cur = msg_data;
                    while !cur.is_null() && !(*cur).is_null() {
                        let arg_str = unsafe { std::ffi::CStr::from_ptr(*cur).to_string_lossy() };
                        args.push(arg_str.into_owned());
                        cur = unsafe { cur.add(1) };
                    }

                    if let Some(first_arg) = args.first() {
                        match first_arg.as_str() {
                            "toggle" | "twitch_chat/toggle" => {
                                state.ui.chat_enabled = !state.ui.chat_enabled;
                                state.update_frame();
                            }
                            "toggle_settings" | "twitch_chat/toggle_settings" => {
                                if state.ui.current_view == UiView::Settings {
                                    state.set_view(UiView::Chat);
                                } else {
                                    state.set_view(UiView::Settings);
                                }
                            }
                            "toggle_search" | "twitch_chat/toggle_search" => {
                                if state.ui.current_view == UiView::Search {
                                    state.set_view(UiView::Chat);
                                } else {
                                    state.set_view(UiView::Search);
                                }
                            }
                            "click" | "twitch_chat/click" => {
                                state.on_click_event();
                            }
                            "wheel_up" | "twitch_chat/wheel_up" => {
                                state.on_wheel_event(true);
                            }
                            "wheel_down" | "twitch_chat/wheel_down" => {
                                state.on_wheel_event(false);
                            }
                            "char" => {
                                if let Some(ch) = args.get(1) {
                                    state.ui.search_query.push_str(ch);
                                    state.execute_search();
                                    state.update_frame();
                                }
                            }
                            "backspace" => {
                                state.ui.search_query.pop();
                                state.execute_search();
                                state.update_frame();
                            }
                            "escape" => {
                                state.set_view(UiView::Chat);
                            }
                            "enter" => {
                                if let Some(first) = state.ui.search_results.first() {
                                    state.mpv.seek_absolute(first.sec);
                                }
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
