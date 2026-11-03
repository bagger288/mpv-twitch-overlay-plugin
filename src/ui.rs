//! Chat UI layout state, views, tabs and styling options

use crate::session::ChatMessage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextEffect {
    #[default]
    None,
    Outline,
    Shadow,
}

impl TextEffect {
    pub fn next(self) -> Self {
        match self {
            Self::None => Self::Outline,
            Self::Outline => Self::Shadow,
            Self::Shadow => Self::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Выкл.",
            Self::Outline => "Обводка",
            Self::Shadow => "Тень",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "outline" => Some(Self::Outline),
            "shadow" => Some(Self::Shadow),
            _ => None,
        }
    }
}

pub fn clamp_outline_width(width: f32) -> f32 {
    if width.is_finite() { width.clamp(0.5, 4.0) } else { 1.5 }
}

pub const SETTINGS_CONTENT_HEIGHT: f32 = 520.0;

pub fn max_settings_scroll(panel_height: u32) -> f32 {
    (SETTINGS_CONTENT_HEIGHT - panel_height as f32).max(0.0)
}

#[derive(Default)]
pub struct HeaderFade {
    progress: f32,
    pub hovered: bool,
}

impl HeaderFade {
    pub fn advance(&mut self, seconds: f32) -> f32 {
        let step = seconds.max(0.0) / if self.hovered { 0.18 } else { 0.22 };
        self.progress = if self.hovered {
            (self.progress + step).min(1.0)
        } else {
            (self.progress - step).max(0.0)
        };
        self.progress * self.progress * (3.0 - 2.0 * self.progress)
    }

    pub fn is_animating(&self) -> bool {
        if self.hovered { self.progress < 1.0 } else { self.progress > 0.0 }
    }
}

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
    pub video_path: Option<String>,
    pub time_str: String,
    pub author: String,
    pub color_hex: String,
    pub text: String,
}

pub const SEARCH_RESULTS_TOP: f32 = 90.0;
pub const SEARCH_RESULT_HEIGHT: f32 = 40.0;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SearchHit {
    Message(usize),
    Timestamp(usize),
}

pub fn search_result_hit(ui: &UiState, x: f32, y: f32) -> Option<SearchHit> {
    if x < 12.0 || x >= ui.panel_width as f32 - 12.0 || y < SEARCH_RESULTS_TOP || y >= ui.panel_height as f32 { return None; }
    let relative = y - SEARCH_RESULTS_TOP;
    let row = (relative / SEARCH_RESULT_HEIGHT) as usize;
    if relative % SEARCH_RESULT_HEIGHT >= SEARCH_RESULT_HEIGHT - 4.0 { return None; }
    let index = ui.search_scroll_offset + row;
    if index >= ui.search_results.len() { return None; }
    Some(if x < 82.0 { SearchHit::Timestamp(index) } else { SearchHit::Message(index) })
}

pub const MINIMIZED_SIZE: u32 = 44;
pub const CHANNEL_TEXT_COLOR: [u8; 4] = [191, 148, 255, 255];

#[derive(Debug, Clone)]
pub struct UiState {
    // Current Active View
    pub current_view: UiView,

    // Toggles and layout
    pub chat_enabled: bool,
    pub minimized: bool,
    pub minimized_position: Option<(i32, i32)>,
    pub emotes_enabled: bool,
    pub badges_enabled: bool,
    pub offset_ms: i64,
    pub font_size: f32,
    pub panel_width: u32,
    pub panel_height: u32,
    pub panel_x: i32,
    pub panel_y: i32,
    pub bg_opacity: f32,
    pub header_opacity: f32,
    pub text_effect: TextEffect,
    pub outline_width: f32,
    pub settings_scroll_offset: f32,

    // Chat scroll state (0 = live auto-scroll, >0 = history/past, <0 = future)
    pub scroll_offset: i32,
    pub chat_scroll_anchor: Option<usize>,
    pub highlighted_message_ts: Option<i64>,

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
            minimized: false,
            minimized_position: None,
            emotes_enabled: true,
            badges_enabled: true,
            offset_ms: 0,
            font_size: 14.0,
            panel_width: 340,
            panel_height: 600,
            panel_x: 1470,
            panel_y: 60,
            bg_opacity: 0.75,
            header_opacity: 0.0,
            text_effect: TextEffect::None,
            outline_width: 1.5,
            settings_scroll_offset: 0.0,
            scroll_offset: 0,
            chat_scroll_anchor: None,
            highlighted_message_ts: None,
            selected_sync_msg: None,
            search_query: String::new(),
            search_results: Vec::new(),
            search_scroll_offset: 0,
        }
    }
}

impl UiState {
    pub fn overlay_geometry(&self) -> (i32, i32, u32, u32) {
        if self.minimized {
            let (x, y) = self.minimized_position.unwrap_or((self.panel_x, self.panel_y));
            (x, y, MINIMIZED_SIZE, MINIMIZED_SIZE)
        } else {
            (self.panel_x, self.panel_y, self.panel_width, self.panel_height)
        }
    }

    pub fn minimize(&mut self, osd: (i32, i32)) {
        let (x, y) = self.minimized_position.unwrap_or((self.panel_x, self.panel_y));
        self.minimized_position = Some((x.clamp(0, (osd.0 - MINIMIZED_SIZE as i32).max(0)), y.clamp(0, (osd.1 - MINIMIZED_SIZE as i32).max(0))));
        self.minimized = true;
    }

    pub fn restore(&mut self) {
        self.minimized = false;
    }

    pub fn set_overlay_position(&mut self, x: i32, y: i32) {
        if self.minimized {
            self.minimized_position = Some((x, y));
        } else {
            self.panel_x = x;
            self.panel_y = y;
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct ChatViewport {
    pub top: f32,
    pub bottom: f32,
    pub base_y: f32,
}

impl ChatViewport {
    pub fn new(ui: &UiState, total_message_height: f32) -> Self {
        let top = 32.0;
        let banner_height = if ui.scroll_offset != 0 || ui.chat_scroll_anchor.is_some() { 28.0 } else { 0.0 };
        let bottom = (ui.panel_height as f32 - banner_height - 10.0).max(top);
        let available = bottom - top;
        let base_y = if total_message_height > available {
            bottom - total_message_height
        } else {
            top
        };
        Self { top, bottom, base_y }
    }

    pub fn contains_y(&self, y: f32) -> bool {
        y >= self.top && y < self.bottom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_hits_distinguish_timestamp_and_message() {
        let result = SearchResult { msg_idx: 0, timestamp_ms: 0, sec: 0.0, video_path: None, time_str: "00:00".into(), author: "user".into(), color_hex: String::new(), text: "text".into() };
        let ui = UiState { search_results: vec![result.clone(), result], ..Default::default() };
        assert_eq!(search_result_hit(&ui, 20.0, 95.0), Some(SearchHit::Timestamp(0)));
        assert_eq!(search_result_hit(&ui, 100.0, 115.0), Some(SearchHit::Message(0)));
        assert_eq!(search_result_hit(&ui, 100.0, 135.0), Some(SearchHit::Message(1)));
        assert_eq!(search_result_hit(&ui, 100.0, 70.0), None);
        assert_eq!(search_result_hit(&ui, 100.0, 127.0), None);
        assert_eq!(search_result_hit(&ui, 100.0, 200.0), None);
    }

    #[test]
    fn minimized_position_is_independent_of_restored_panel() {
        let mut ui = UiState { panel_x: 100, panel_y: 60, ..Default::default() };
        ui.minimize((800, 700));
        assert_eq!(ui.overlay_geometry(), (100, 60, 44, 44));
        ui.set_overlay_position(300, 200);
        ui.restore();
        assert_eq!(ui.overlay_geometry(), (100, 60, 340, 600));
        ui.minimize((800, 700));
        assert_eq!(ui.overlay_geometry(), (300, 200, 44, 44));
        ui.minimize((200, 150));
        assert_eq!(ui.overlay_geometry(), (156, 106, 44, 44));
    }

    #[test]
    fn header_fade_appears_disappears_and_reverses_smoothly() {
        let mut fade = HeaderFade::default();
        assert_eq!(fade.advance(0.1), 0.0);
        assert!(!fade.is_animating());
        fade.hovered = true;
        assert!(fade.is_animating());
        let halfway = fade.advance(0.09);
        assert!((halfway - 0.5).abs() < 0.001);
        assert_eq!(fade.advance(0.1), 1.0);
        assert!(!fade.is_animating());
        fade.hovered = false;
        let fading = fade.advance(0.11);
        assert!((fading - 0.5).abs() < 0.001);
        fade.hovered = true;
        let reversed = fade.advance(0.02);
        assert!(reversed > fading && reversed < 1.0);
        fade.hovered = false;
        assert_eq!(fade.advance(1.0), 0.0);
        assert!(!fade.is_animating());
    }

    #[test]
    fn text_effect_modes_and_width_limits_are_stable() {
        assert_eq!(TextEffect::None.next(), TextEffect::Outline);
        assert_eq!(TextEffect::Outline.next(), TextEffect::Shadow);
        assert_eq!(TextEffect::Shadow.next(), TextEffect::None);
        assert_eq!(TextEffect::parse("outline"), Some(TextEffect::Outline));
        assert_eq!(TextEffect::parse("unexpected"), None);
        assert_eq!(clamp_outline_width(10.0), 4.0);
        assert_eq!(clamp_outline_width(-1.0), 0.5);
        assert_eq!(clamp_outline_width(f32::NAN), 1.5);
        assert_eq!(serde_json::to_string(&TextEffect::Shadow).unwrap(), "\"shadow\"");
        assert_eq!(max_settings_scroll(220), 300.0);
    }

    #[test]
    fn chat_viewport_excludes_header_and_scroll_banner() {
        let mut ui = UiState { panel_height: 220, ..UiState::default() };
        let live = ChatViewport::new(&ui, 500.0);
        assert_eq!(live.top, 32.0);
        assert_eq!(live.bottom, 210.0);
        assert!(!live.contains_y(25.0));
        assert!(live.contains_y(32.0));
        assert!(!live.contains_y(216.0));
        assert_eq!(live.base_y + 500.0, live.bottom);
        ui.scroll_offset = 4;
        let history = ChatViewport::new(&ui, 500.0);
        assert_eq!(history.bottom, 182.0);
        assert!(!history.contains_y(200.0));
        ui.panel_height = 10;
        let tiny = ChatViewport::new(&ui, 500.0);
        assert!(!tiny.contains_y(32.0));
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
