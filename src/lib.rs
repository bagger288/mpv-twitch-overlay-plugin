//! Twitch Chat Overlay mpv C-Plugin (Rust Native DLL)
//!
//! High-performance native mpv plugin for Windows / Linux.
//! Provides instantaneous mouse response, smooth scrolling, rich emote rendering,
//! click-to-seek, and 1-click time synchronization.

#![allow(dead_code)]

pub mod emotes;
pub mod layout;
pub mod mpv_api;
pub mod session;
pub mod renderer;
pub mod ui;

use emotes::EmoteCache;
use mpv_api::{log_plugin, MpvEventId, MpvFormat, MpvHandle, MpvHandleRaw};
use renderer::ChatRenderer;
use session::SessionManager;
use std::os::raw::c_int;
use std::path::PathBuf;
use ui::{UiAction, UiState};

const OVERLAY_ID: u32 = 42;

struct PluginState {
    mpv: MpvHandle,
    session: Option<SessionManager>,
    emotes: Option<EmoteCache>,
    renderer: ChatRenderer,
    ui: UiState,
    current_time_pos: f64,
    current_path: String,
    mouse_x: f32,
    mouse_y: f32,
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
            mouse_x: 0.0,
            mouse_y: 0.0,
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
            log_plugin(&format!("Session successfully loaded from: {:?}", mgr.session_dir));
            log_plugin(&format!("Found {} chat messages, {} channel emotes", mgr.messages.len(), mgr.emotes_manifest.channel_emotes.len()));
            self.emotes = Some(EmoteCache::new(&mgr.session_dir));
            self.session = Some(mgr);
            self.current_path = new_path.to_string();
            self.ui.is_paused = false;
            self.ui.scroll_offset_y = 0.0;
            self.update_frame();
        } else {
            log_plugin(&format!("Could not load session from path: {:?}", path));
        }
    }

    fn update_frame(&mut self) {
        if !self.ui.chat_enabled {
            self.mpv.overlay_remove(OVERLAY_ID);
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
        let current_ms = session_start + self.ui.offset_ms + (current_video_sec * 1000.0).round() as i64;

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

        let channel = self.session.as_ref().map(|s| s.session_data.channel.as_str()).unwrap_or("");
        let bgra_data = match self.renderer.render_frame(
            &layout_messages,
            emotes,
            &self.ui,
            channel,
        ) {
            Some(d) => d,
            None => return,
        };

        let stride = self.ui.panel_width * 4;
        self.mpv.overlay_add_bgra(
            OVERLAY_ID,
            self.ui.panel_x,
            self.ui.panel_y,
            &bgra_data,
            self.ui.panel_width,
            self.ui.panel_height,
            stride,
        );
    }

    fn on_click(&mut self, click_x: f32, click_y: f32) {
        let rel_x = click_x - self.ui.panel_x as f32;
        let rel_y = click_y - self.ui.panel_y as f32;

        if rel_x < 0.0 || rel_x > self.ui.panel_width as f32 || rel_y < 0.0 || rel_y > self.ui.panel_height as f32 {
            if self.ui.search_visible {
                self.ui.search_visible = false;
                self.update_frame();
            }
            return;
        }

        // 1. If search modal is visible, handle search clicks
        if self.ui.search_visible {
            let modal_w = (self.ui.panel_width as f32 - 16.0).min(420.0);
            let modal_h = (self.ui.panel_height as f32 - 36.0).min(380.0);
            let mx = (self.ui.panel_width as f32 - modal_w) * 0.5;
            let my = 32.0;

            if rel_x >= mx && rel_x <= mx + modal_w && rel_y >= my && rel_y <= my + modal_h {
                // Close button [ ✕ ]
                if rel_x >= mx + modal_w - 30.0 && rel_y >= my + 4.0 && rel_y <= my + 28.0 {
                    self.ui.search_visible = false;
                    self.update_frame();
                    return;
                }

                // Quick tags bar
                let tag_y = my + 32.0;
                if rel_y >= tag_y && rel_y <= tag_y + 20.0 {
                    let mut tag_x = mx + 8.0;
                    for &tag in crate::ui::QUICK_SEARCH_TAGS.iter().take(7) {
                        let chip_w = self.renderer.layouter().measure_text_width(tag, 10.0) + 12.0;
                        if rel_x >= tag_x && rel_x <= tag_x + chip_w {
                            if let Some(session) = &self.session {
                                self.ui.execute_search(&session.messages, tag);
                                self.update_frame();
                                return;
                            }
                        }
                        tag_x += chip_w + 4.0;
                    }
                }

                // Search result rows
                let res_top = my + 56.0;
                let res_item_h = 28.0;
                let start_idx = self.ui.search_page * self.ui.search_page_size;
                let end_idx = (start_idx + self.ui.search_page_size).min(self.ui.search_results.len());

                for (i, item) in self.ui.search_results[start_idx..end_idx].iter().enumerate() {
                    let ry = res_top + (i as f32 * res_item_h);
                    if rel_y >= ry && rel_y <= ry + res_item_h {
                        if let Some(session) = &self.session {
                            let target_sec = (item.timestamp_ms - session.session_data.session_start_ms) as f64 / 1000.0;
                            if target_sec >= 0.0 {
                                self.mpv.seek_absolute(target_sec);
                                self.ui.search_visible = false;
                                self.update_frame();
                                return;
                            }
                        }
                    }
                }

                // Pagination footer
                let foot_y = my + modal_h - 26.0;
                if rel_y >= foot_y && rel_y <= foot_y + 22.0 {
                    if rel_x >= mx + 8.0 && rel_x <= mx + 68.0 {
                        self.ui.handle_action(UiAction::SearchPagePrev, &[]);
                        self.update_frame();
                        return;
                    } else if rel_x >= mx + modal_w - 68.0 && rel_x <= mx + modal_w - 8.0 {
                        self.ui.handle_action(UiAction::SearchPageNext, &[]);
                        self.update_frame();
                        return;
                    }
                }

                return;
            } else {
                self.ui.search_visible = false;
                self.update_frame();
                return;
            }
        }

        // 2. Control Bar Header (y: 0..28)
        if rel_y <= 28.0 {
            // Offset click -> reset
            if rel_x >= 130.0 && rel_x <= 180.0 {
                self.ui.offset_ms = 0;
                self.update_frame();
                return;
            }
            // Search [ 🔍 Поиск ]
            if rel_x >= 184.0 && rel_x <= 240.0 {
                if let Some(session) = &self.session {
                    self.ui.handle_action(UiAction::ToggleSearch, &session.messages);
                }
                self.update_frame();
                return;
            }
            // Sync anchor [ ⚓ Синхр ]
            if rel_x >= 244.0 && rel_x <= 296.0 {
                if let Some(session) = self.session.as_mut() {
                    let anchor_ts = self.ui.selected_msg_ts.unwrap_or_else(|| {
                        session.session_data.session_start_ms + (self.current_time_pos * 1000.0).round() as i64
                    });
                    if session.calibrate_anchor(anchor_ts, self.current_time_pos) {
                        self.ui.offset_ms = 0;
                        log_plugin("Calibrate anchor succeeded and saved to session.json");
                    }
                }
                self.update_frame();
                return;
            }
            // [ -1s ]
            if rel_x >= 300.0 && rel_x <= 330.0 {
                self.ui.offset_ms -= 1000;
                self.update_frame();
                return;
            }
            // [ +1s ]
            if rel_x >= 334.0 && rel_x <= 364.0 {
                self.ui.offset_ms += 1000;
                self.update_frame();
                return;
            }
            // Settings [ ⚙ ]
            if rel_x >= self.ui.panel_width as f32 - 30.0 {
                self.ui.show_settings = !self.ui.show_settings;
                self.update_frame();
                return;
            }
            return;
        }

        // 3. Settings Drawer (if open, y: 28..84)
        if self.ui.show_settings && rel_y >= 28.0 && rel_y <= 84.0 {
            let ry = rel_y - 28.0;
            if ry <= 26.0 {
                // Offset row: [-10s] [-1s] [-0.1s] [+0.1s] [+1s] [+10s] [0s]
                let deltas = [-10000, -1000, -100, 100, 1000, 10000, 0];
                let mut bx = 8.0;
                for &d in &deltas {
                    if rel_x >= bx && rel_x <= bx + 38.0 {
                        if d == 0 {
                            self.ui.offset_ms = 0;
                        } else {
                            self.ui.offset_ms += d;
                        }
                        self.update_frame();
                        return;
                    }
                    bx += 42.0;
                }
            } else {
                // Row 2: Font scale [A-] [A+], Size [W±] [H±], Presets, Opacity
                if rel_x >= 8.0 && rel_x <= 34.0 {
                    self.ui.font_size = (self.ui.font_size - 1.0).max(11.0);
                } else if rel_x >= 38.0 && rel_x <= 64.0 {
                    self.ui.font_size = (self.ui.font_size + 1.0).min(36.0);
                } else if rel_x >= 70.0 && rel_x <= 96.0 {
                    self.ui.panel_width = (self.ui.panel_width.saturating_sub(40)).max(280);
                } else if rel_x >= 100.0 && rel_x <= 126.0 {
                    self.ui.panel_width += 40;
                } else if rel_x >= 132.0 && rel_x <= 158.0 {
                    self.ui.panel_height = (self.ui.panel_height.saturating_sub(40)).max(300);
                } else if rel_x >= 162.0 && rel_x <= 188.0 {
                    self.ui.panel_height += 40;
                } else if rel_x >= 198.0 && rel_x <= 222.0 {
                    self.ui.panel_x = 24; self.ui.panel_y = 60; // TL
                } else if rel_x >= 226.0 && rel_x <= 250.0 {
                    self.ui.panel_x = 24; self.ui.panel_y = 420; // BL
                } else if rel_x >= 254.0 && rel_x <= 278.0 {
                    self.ui.panel_x = 1450; self.ui.panel_y = 60; // TR
                } else if rel_x >= 282.0 && rel_x <= 306.0 {
                    self.ui.panel_x = 1450; self.ui.panel_y = 420; // BR
                } else if rel_x >= 314.0 && rel_x <= 382.0 {
                    self.ui.handle_action(UiAction::ToggleBackground, &[]);
                }
                self.update_frame();
                return;
            }
            return;
        }

        // 4. Paused banner at bottom
        if self.ui.is_paused && rel_y >= (self.ui.panel_height as f32 - 32.0) {
            self.ui.is_paused = false;
            self.ui.scroll_offset_y = 0.0;
            self.update_frame();
            return;
        }

        // 5. Click-to-Seek: click message
        if let Some(session) = &self.session {
            let session_start = session.session_data.session_start_ms;
            let current_ms = session_start + self.ui.offset_ms + (self.current_time_pos * 1000.0).round() as i64;
            let active_msgs = session.get_messages_up_to(current_ms, 80);

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

            let content_top = if self.ui.show_settings { 88.0 } else { 32.0 };
            let adjusted_y = (rel_y - content_top) + self.ui.scroll_offset_y;
            for msg in layout_messages {
                if adjusted_y >= msg.top_y && adjusted_y <= msg.bottom_y {
                    let target_sec = (msg.timestamp_ms - session_start) as f64 / 1000.0;
                    if target_sec >= 0.0 {
                        self.mpv.seek_absolute(target_sec);
                        self.ui.selected_msg_ts = Some(msg.timestamp_ms);
                        self.update_frame();
                        return;
                    }
                }
            }
        }
    }
}

/// Native mpv C-plugin entry point exported from DLL
#[no_mangle]
pub unsafe extern "C" fn mpv_open_cplugin(ctx: MpvHandleRaw) -> c_int {
    log_plugin("=== mpv_open_cplugin entered ===");
    let mpv = match MpvHandle::from_raw(ctx) {
        Some(h) => {
            log_plugin(&format!("MpvHandle initialized. Client name: {}", h.client_name()));
            h
        }
        None => {
            log_plugin("FATAL: Failed to initialize MpvHandle / vtable!");
            return -1;
        }
    };

    // Observe essential playback properties
    mpv.observe_property(1, "time-pos", MpvFormat::Double);
    mpv.observe_property(2, "path", MpvFormat::String);
    mpv.observe_property(3, "pause", MpvFormat::Flag);

    // Bind custom keybindings and mouse events directly inside mpv
    mpv.command(&[
        "define-section",
        "twitch_chat_input",
        "MBTN_LEFT script-binding twitch_chat/click\nWHEEL_UP script-binding twitch_chat/wheel_up\nWHEEL_DOWN script-binding twitch_chat/wheel_down\nCtrl+f script-binding twitch_chat/toggle_search",
        "default",
    ]);
    mpv.command(&["enable-section", "twitch_chat_input", "allow-hide-cursor+allow-vo-dragging"]);

    let mut state = PluginState::new(mpv);

    // Read initial script-opts
    if let Some(opts) = state.mpv.get_property_string("script-opts") {
        state.parse_script_opts(&opts);
    }
    if let Some(opts) = state.mpv.get_property_string("options/script-opts") {
        state.parse_script_opts(&opts);
    }

    // Initial query
    if let Some(p) = state.mpv.get_property_string("path") {
        state.on_path_changed(&p);
    }
    if let Some(tp) = state.mpv.get_property_double("time-pos") {
        state.current_time_pos = tp;
    }

    state.update_frame();

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
                    _ => {}
                }
            }
            MpvEventId::FileLoaded | MpvEventId::StartFile | MpvEventId::PlaybackRestart => {
                if let Some(p) = state.mpv.get_property_string("path") {
                    state.on_path_changed(&p);
                }
                if let Some(tp) = state.mpv.get_property_double("time-pos") {
                    state.current_time_pos = tp;
                }
                state.update_frame();
            }
            MpvEventId::ClientMessage => {
                let msg_data = event.data as *mut *const std::ffi::c_char;
                if !msg_data.is_null() {
                    let first_arg = unsafe { std::ffi::CStr::from_ptr(*msg_data).to_string_lossy() };
                    match first_arg.as_ref() {
                        "twitch_chat/click" => {
                            if let (Some(mx), Some(my)) = (state.mpv.get_property_double("mouse-pos/x"), state.mpv.get_property_double("mouse-pos/y")) {
                                state.on_click(mx as f32, my as f32);
                            }
                        }
                        "twitch_chat/wheel_up" => {
                            state.ui.handle_mouse_wheel(1.0);
                            state.update_frame();
                        }
                        "twitch_chat/wheel_down" => {
                            state.ui.handle_mouse_wheel(-1.0);
                            state.update_frame();
                        }
                        "twitch_chat/toggle_search" => {
                            if let Some(session) = &state.session {
                                state.ui.handle_action(UiAction::ToggleSearch, &session.messages);
                                state.update_frame();
                            }
                        }
                        _ => {}
                    }
                }
            }
            MpvEventId::Tick => {
                if state.session.is_some() {
                    state.update_frame();
                }
            }
            _ => {}
        }
    }

    state.mpv.overlay_remove(OVERLAY_ID);
    0
}
