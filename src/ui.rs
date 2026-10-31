//! Chat UI layout state, views, tabs and styling options

use crate::session::ChatMessage;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum UiView {
    Chat,
    Settings,
    Search,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub msg_idx: usize,
    pub timestamp_ms: i64,
    pub sec: f64,
    pub time_str: String,
    pub author: String,
    pub color_hex: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct UiState {
    // Current Active View
    pub current_view: UiView,

    // Toggles and layout
    pub chat_enabled: bool,
    pub emotes_enabled: bool,
    pub offset_ms: i64,
    pub font_size: f32,
    pub panel_width: u32,
    pub panel_height: u32,
    pub panel_x: i32,
    pub panel_y: i32,
    pub bg_opacity: f32,

    // Chat scroll state (0 = live auto-scroll, >0 = scrolled back in history)
    pub scroll_offset: usize,

    // Selected message for 1-click sync calibration
    pub selected_sync_msg: Option<ChatMessage>,

    // Search state
    pub search_query: String,
    pub search_results: Vec<SearchResult>,
    pub search_scroll_offset: usize,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            current_view: UiView::Chat,
            chat_enabled: true,
            emotes_enabled: true,
            offset_ms: 0,
            font_size: 18.0,
            panel_width: 420,
            panel_height: 600,
            panel_x: 1470,
            panel_y: 60,
            bg_opacity: 0.75,
            scroll_offset: 0,
            selected_sync_msg: None,
            search_query: String::new(),
            search_results: Vec::new(),
            search_scroll_offset: 0,
        }
    }
}

/// Helper struct for button bounds and click detection
#[derive(Debug, Copy, Clone)]
pub struct HitBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl HitBox {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}
