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
use mpv_api::{log_plugin, MpvEventId, MpvFormat, MpvHandle, MpvHandleRaw};
use renderer::ChatRenderer;
use session::SessionManager;
use std::os::raw::c_int;
use std::path::PathBuf;
use ui::{UiState, UiView};

// mpv overlay ID must be in the range [0..63].
// ID 1 is safe, valid, and avoids thumbfast.lua's default ID 42.
const OVERLAY_ID: u32 = 1;

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
                                    state.ui.current_view = UiView::Chat;
                                } else {
                                    state.ui.current_view = UiView::Settings;
                                }
                                state.update_frame();
                            }
                            "toggle_search" | "twitch_chat/toggle_search" => {
                                if state.ui.current_view == UiView::Search {
                                    state.ui.current_view = UiView::Chat;
                                } else {
                                    state.ui.current_view = UiView::Search;
                                }
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
