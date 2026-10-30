//! Twitch Chat Overlay mpv C-Plugin (Rust Native DLL)
//!
//! High-performance native mpv plugin for Windows / Linux.
//! Provides instantaneous mouse response, smooth scrolling, rich emote rendering,
//! click-to-seek, and 1-click time synchronization.

pub mod emotes;
pub mod layout;
pub mod mpv_api;
pub mod session;
pub mod renderer;
pub mod ui;

use emotes::EmoteCache;
use mpv_api::{MpvEventId, MpvFormat, MpvHandle, MpvHandleRaw};
use renderer::ChatRenderer;
use session::SessionManager;
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
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

    fn on_path_changed(&mut self, new_path: &str) {
        if new_path.is_empty() || new_path == self.current_path {
            return;
        }
        self.current_path = new_path.to_string();

        let path = PathBuf::from(new_path);
        if let Some(mgr) = SessionManager::try_load(&path) {
            self.emotes = Some(EmoteCache::new(&mgr.session_dir));
            self.session = Some(mgr);
            self.ui.is_paused = false;
            self.ui.scroll_offset_y = 0.0;
        } else {
            self.session = None;
            self.emotes = None;
            self.mpv.overlay_remove(OVERLAY_ID);
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
        if active_msgs.is_empty() {
            self.mpv.overlay_remove(OVERLAY_ID);
            return;
        }

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

        // Render BGRA frame
        let bgra_data = match self.renderer.render_frame(
            &layout_messages,
            emotes,
            self.ui.panel_width,
            self.ui.panel_height,
            self.ui.bg_opacity,
            self.ui.scroll_offset_y,
            self.ui.is_paused,
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
            return;
        }

        // Check if clicked paused banner at bottom
        if self.ui.is_paused && rel_y >= (self.ui.panel_height as f32 - 32.0) {
            self.ui.handle_action(UiAction::UnpauseScroll);
            self.update_frame();
            return;
        }

        // Click-to-Seek: find which message was clicked
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

            let adjusted_y = rel_y + self.ui.scroll_offset_y;
            for msg in layout_messages {
                if adjusted_y >= msg.top_y && adjusted_y <= msg.bottom_y {
                    // Calculate video seek target
                    let target_sec = (msg.timestamp_ms - session_start) as f64 / 1000.0;
                    if target_sec >= 0.0 {
                        self.mpv.seek_absolute(target_sec);
                        self.ui.selected_msg_ts = Some(msg.timestamp_ms);
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
    let mpv = match MpvHandle::from_raw(ctx) {
        Some(h) => h,
        None => return -1,
    };

    // Observe essential playback properties
    mpv.observe_property(1, "time-pos", MpvFormat::Double);
    mpv.observe_property(2, "path", MpvFormat::String);
    mpv.observe_property(3, "pause", MpvFormat::Flag);

    // Bind custom keybindings and mouse events directly inside mpv
    mpv.command(&["enable-section", "twitch_chat_section", "allow-hide-cursor+allow-vo-dragging"]);

    let mut state = PluginState::new(mpv);

    // Initial query
    if let Some(p) = state.mpv.get_property_string("path") {
        state.on_path_changed(&p);
    }
    if let Some(tp) = state.mpv.get_property_double("time-pos") {
        state.current_time_pos = tp;
    }

    // Main event loop (runs in mpv's dedicated plugin thread)
    loop {
        let event_ptr = state.mpv.wait_event(0.03); // ~33ms tick for 30fps smooth overlay
        if event_ptr.is_null() {
            continue;
        }

        let event = &*event_ptr;
        match event.event_id {
            MpvEventId::Shutdown => {
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
            MpvEventId::Tick => {
                if state.session.is_some() {
                    state.update_frame();
                }
            }
            MpvEventId::ClientMessage => {
                // Custom client messages if received from input bindings
            }
            _ => {}
        }
    }

    state.mpv.overlay_remove(OVERLAY_ID);
    0
}
