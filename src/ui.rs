//! OSD Buttons, Mouse Interaction, Click-to-Seek, Search and UI State

use crate::session::ChatMessage;

#[derive(Debug, Clone)]
pub struct RectArea {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl RectArea {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.width && py >= self.y && py <= self.y + self.height
    }
}

#[derive(Debug, Clone)]
pub struct SearchResultItem {
    pub timestamp_ms: i64,
    pub time_str: String,
    pub author: String,
    pub color_hex: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiAction {
    Nop,
    OffsetAdd(i64),     // ms
    OffsetReset,
    CalibrateAnchor,    // 1-Click Sync with current video frame
    FontSizeChange(i32),
    WidthChange(i32),
    HeightChange(i32),
    CornerPreset(u8),   // 0: TL, 1: BL, 2: TR, 3: BR
    ToggleBackground,
    ToggleChat,
    ToggleSettings,
    ToggleSearch,
    SetSearchQuery(&'static str),
    SearchPagePrev,
    SearchPageNext,
    UnpauseScroll,
    SeekToTimestamp(i64),
}

pub struct UiState {
    pub chat_enabled: bool,
    pub emotes_enabled: bool,
    pub offset_ms: i64,
    pub font_size: f32,
    pub panel_width: u32,
    pub panel_height: u32,
    pub panel_x: i32,
    pub panel_y: i32,
    pub bg_opacity: f32,
    pub scroll_offset_y: f32,
    pub is_paused: bool,
    pub selected_msg_ts: Option<i64>,
    pub show_settings: bool,
    pub search_visible: bool,
    pub search_query: String,
    pub search_results: Vec<SearchResultItem>,
    pub search_page: usize,
    pub search_page_size: usize,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            chat_enabled: true,
            emotes_enabled: true,
            offset_ms: 0,
            font_size: 16.0,
            panel_width: 440,
            panel_height: 620,
            panel_x: 24,
            panel_y: 60,
            bg_opacity: 0.75,
            scroll_offset_y: 0.0,
            is_paused: false,
            selected_msg_ts: None,
            show_settings: false,
            search_visible: false,
            search_query: String::new(),
            search_results: Vec::new(),
            search_page: 0,
            search_page_size: 8,
        }
    }
}

pub const QUICK_SEARCH_TAGS: &[&str] = &[
    "привет", "стрим", "звук", "хай", "клип", "LUL", "Pog", "GG", "KEKW"
];

impl UiState {
    pub fn handle_mouse_wheel(&mut self, delta_y: f32) {
        if delta_y < 0.0 {
            // Scroll down
            self.scroll_offset_y = (self.scroll_offset_y - 40.0).max(0.0);
            if self.scroll_offset_y <= 0.0 {
                self.is_paused = false;
            }
        } else if delta_y > 0.0 {
            // Scroll up (pause chat history)
            self.is_paused = true;
            self.scroll_offset_y += 40.0;
        }
    }

    pub fn execute_search(&mut self, all_messages: &[ChatMessage], query: &str) {
        self.search_query = query.to_string();
        self.search_page = 0;
        self.search_results.clear();

        if query.trim().is_empty() {
            return;
        }

        let q_lower = query.trim().to_lowercase();
        for msg in all_messages {
            if msg.is_gap {
                continue;
            }
            if msg.text.to_lowercase().contains(&q_lower) || msg.display_name.to_lowercase().contains(&q_lower) {
                let sec_total = (msg.timestamp_ms / 1000).max(0);
                let hours = sec_total / 3600;
                let mins = (sec_total % 3600) / 60;
                let secs = sec_total % 60;
                let time_str = if hours > 0 {
                    format!("{:02}:{:02}:{:02}", hours, mins, secs)
                } else {
                    format!("{:02}:{:02}", mins, secs)
                };

                self.search_results.push(SearchResultItem {
                    timestamp_ms: msg.timestamp_ms,
                    time_str,
                    author: msg.display_name.clone(),
                    color_hex: msg.color_hex.clone(),
                    text: msg.text.clone(),
                });
            }
        }
    }

    pub fn handle_action(&mut self, action: UiAction, all_messages: &[ChatMessage]) {
        match action {
            UiAction::Nop => {}
            UiAction::OffsetAdd(delta) => {
                self.offset_ms += delta;
            }
            UiAction::OffsetReset => {
                self.offset_ms = 0;
            }
            UiAction::FontSizeChange(delta) => {
                self.font_size = (self.font_size + delta as f32).clamp(11.0, 36.0);
            }
            UiAction::WidthChange(delta) => {
                self.panel_width = ((self.panel_width as i32 + delta).max(280)) as u32;
            }
            UiAction::HeightChange(delta) => {
                self.panel_height = ((self.panel_height as i32 + delta).max(300)) as u32;
            }
            UiAction::CornerPreset(corner) => {
                match corner {
                    0 => { self.panel_x = 24; self.panel_y = 60; }      // Top-Left
                    1 => { self.panel_x = 24; self.panel_y = 420; }     // Bottom-Left
                    2 => { self.panel_x = 1450; self.panel_y = 60; }    // Top-Right
                    3 => { self.panel_x = 1450; self.panel_y = 420; }   // Bottom-Right
                    _ => {}
                }
            }
            UiAction::ToggleBackground => {
                let opacities = [0.0, 0.25, 0.50, 0.75, 0.90, 1.0];
                let current_idx = opacities
                    .iter()
                    .position(|&op| (op - self.bg_opacity).abs() < 0.05)
                    .unwrap_or(3);
                self.bg_opacity = opacities[(current_idx + 1) % opacities.len()];
            }
            UiAction::ToggleChat => {
                self.chat_enabled = !self.chat_enabled;
            }
            UiAction::ToggleSettings => {
                self.show_settings = !self.show_settings;
            }
            UiAction::ToggleSearch => {
                self.search_visible = !self.search_visible;
                if self.search_visible && self.search_results.is_empty() && !all_messages.is_empty() {
                    // Pre-fill with first quick tag "привет"
                    self.execute_search(all_messages, "привет");
                }
            }
            UiAction::SetSearchQuery(q) => {
                self.execute_search(all_messages, q);
            }
            UiAction::SearchPagePrev => {
                if self.search_page > 0 {
                    self.search_page -= 1;
                }
            }
            UiAction::SearchPageNext => {
                let max_page = (self.search_results.len().saturating_sub(1)) / self.search_page_size;
                if self.search_page < max_page {
                    self.search_page += 1;
                }
            }
            UiAction::UnpauseScroll => {
                self.is_paused = false;
                self.scroll_offset_y = 0.0;
            }
            _ => {}
        }
    }
}
