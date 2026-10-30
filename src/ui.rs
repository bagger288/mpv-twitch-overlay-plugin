//! OSD Buttons, Mouse Interaction, Click-to-Seek and UI State

#[derive(Debug, Clone)]
pub struct RectArea {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl RectArea {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.width && py >= self.y && py <= self.y + self.height
    }
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
    pub last_mouse_activity: f64,
    pub show_controls: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            chat_enabled: true,
            emotes_enabled: true,
            offset_ms: 0,
            font_size: 16.0,
            panel_width: 420,
            panel_height: 600,
            panel_x: 24,
            panel_y: 60,
            bg_opacity: 0.75,
            scroll_offset_y: 0.0,
            is_paused: false,
            selected_msg_ts: None,
            last_mouse_activity: 0.0,
            show_controls: true,
        }
    }
}

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

    pub fn handle_action(&mut self, action: UiAction) {
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
                self.panel_width = ((self.panel_width as i32 + delta).max(240)) as u32;
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
            UiAction::UnpauseScroll => {
                self.is_paused = false;
                self.scroll_offset_y = 0.0;
            }
            _ => {}
        }
    }
}
