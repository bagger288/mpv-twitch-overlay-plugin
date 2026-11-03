//! Mouse gestures for the chat panel.

use crate::renderer::ChatRenderer;
use crate::ui::UiState;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MouseAction {
    None,
    DragStarted,
    DragFinished,
    Click(i32, i32),
}

#[derive(Debug, Copy, Clone)]
struct DragOrigin {
    cursor: (i32, i32),
    panel: (i32, i32),
    minimized: bool,
    moved: bool,
}

#[derive(Default)]
pub struct PanelMouse {
    pressed_at: Option<(i32, i32)>,
    drag: Option<DragOrigin>,
}

impl PanelMouse {
    pub fn is_pressed(&self) -> bool {
        self.pressed_at.is_some()
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn handle(&mut self, state: &str, cursor: Option<(i32, i32)>, ui: &UiState) -> MouseAction {
        if state.as_bytes().get(2) == Some(&b'c') {
            return self.cancel();
        }

        match state.as_bytes().first() {
            Some(b'd') => {
                if self.is_pressed() || !ui.chat_enabled {
                    return MouseAction::None;
                }
                let Some((mx, my)) = cursor else {
                    return MouseAction::None;
                };
                let (panel_x, panel_y, width, height) = ui.overlay_geometry();
                let x = (mx - panel_x) as f32;
                let y = (my - panel_y) as f32;
                let margin = if ui.minimized { 0.0 } else { 15.0 };
                if x < -margin || x >= width as f32 + margin || y < -margin || y >= height as f32 + margin {
                    return MouseAction::None;
                }
                self.pressed_at = Some((mx, my));
                if ui.minimized || ChatRenderer::is_drag_handle(x, y, ui.panel_width as f32) {
                    self.drag = Some(DragOrigin {
                        cursor: (mx, my),
                        panel: (panel_x, panel_y),
                        minimized: ui.minimized,
                        moved: false,
                    });
                    MouseAction::DragStarted
                } else {
                    MouseAction::None
                }
            }
            Some(b'u') => {
                let pressed_at = self.pressed_at.take();
                if let Some(origin) = self.drag.take() {
                    if origin.minimized && !origin.moved {
                        if let Some((mx, my)) = cursor {
                            if mx.abs_diff(origin.cursor.0) <= 5 && my.abs_diff(origin.cursor.1) <= 5 {
                                return MouseAction::Click(mx, my);
                            }
                        }
                    }
                    return MouseAction::DragFinished;
                }
                if let (Some((px, py)), Some((mx, my))) = (pressed_at, cursor) {
                    if mx.abs_diff(px) <= 5 && my.abs_diff(py) <= 5 {
                        return MouseAction::Click(mx, my);
                    }
                }
                MouseAction::None
            }
            Some(b'p') if ui.chat_enabled => cursor
                .map(|(x, y)| MouseAction::Click(x, y))
                .unwrap_or(MouseAction::None),
            _ => MouseAction::None,
        }
    }

    pub fn cancel(&mut self) -> MouseAction {
        self.pressed_at = None;
        if self.drag.take().is_some() {
            MouseAction::DragFinished
        } else {
            MouseAction::None
        }
    }

    pub fn drag_position(&mut self, cursor: (i32, i32), osd: (i32, i32)) -> Option<(i32, i32)> {
        let origin = self.drag.as_mut()?;
        let moved = cursor.0.abs_diff(origin.cursor.0) > 5 || cursor.1.abs_diff(origin.cursor.1) > 5;
        origin.moved |= moved;
        if origin.minimized && !origin.moved {
            return None;
        }
        let x = origin.panel.0 + cursor.0 - origin.cursor.0;
        let y = origin.panel.1 + cursor.1 - origin.cursor.1;
        let keep_x = if origin.minimized { crate::ui::MINIMIZED_SIZE as i32 } else { 60 };
        let keep_y = if origin.minimized { crate::ui::MINIMIZED_SIZE as i32 } else { 40 };
        Some((x.clamp(0, (osd.0 - keep_x).max(0)), y.clamp(0, (osd.1 - keep_y).max(0))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> UiState {
        UiState {
            panel_x: 100,
            panel_y: 60,
            ..UiState::default()
        }
    }

    #[test]
    fn drag_starts_on_down_and_moves_while_held() {
        let ui = panel();
        let mut mouse = PanelMouse::default();
        assert_eq!(
            mouse.handle("dm-", Some((350, 70)), &ui),
            MouseAction::DragStarted
        );
        assert_eq!(
            mouse.drag_position((450, 120), (1920, 1080)),
            Some((200, 110))
        );
        assert_eq!(
            mouse.handle("rm-", Some((450, 120)), &ui),
            MouseAction::None
        );
        assert!(mouse.is_dragging());
        assert_eq!(
            mouse.handle("um-", Some((450, 120)), &ui),
            MouseAction::DragFinished
        );
        assert!(!mouse.is_pressed());
        assert_eq!(mouse.drag_position((550, 220), (1920, 1080)), None);
    }

    #[test]
    fn header_buttons_click_on_release_without_dragging() {
        let ui = panel();
        for x in [130, 200, 270, ui.panel_x + ui.panel_width as i32 - 15] {
            let mut mouse = PanelMouse::default();
            assert_eq!(mouse.handle("dm-", Some((x, 70)), &ui), MouseAction::None);
            assert!(mouse.is_pressed());
            assert!(!mouse.is_dragging());
            assert_eq!(
                mouse.handle("um-", Some((x, 70)), &ui),
                MouseAction::Click(x, 70)
            );
        }
    }

    #[test]
    fn body_clicks_do_not_drag() {
        let ui = panel();
        let mut mouse = PanelMouse::default();
        assert_eq!(
            mouse.handle("dm-", Some((350, 180)), &ui),
            MouseAction::None
        );
        assert!(!mouse.is_dragging());
        assert_eq!(
            mouse.handle("um-", Some((350, 180)), &ui),
            MouseAction::Click(350, 180)
        );
    }

    #[test]
    fn release_without_down_and_moved_button_press_do_not_click() {
        let ui = panel();
        let mut mouse = PanelMouse::default();
        assert_eq!(mouse.handle("um-", Some((350, 70)), &ui), MouseAction::None);
        mouse.handle("dm-", Some((130, 70)), &ui);
        assert_eq!(mouse.handle("um-", Some((350, 70)), &ui), MouseAction::None);
        assert!(!mouse.is_dragging());
    }

    #[test]
    fn cancellation_finishes_drag_without_click() {
        let ui = panel();
        let mut mouse = PanelMouse::default();
        mouse.handle("dm-", Some((350, 70)), &ui);
        assert_eq!(mouse.handle("umc", None, &ui), MouseAction::DragFinished);
        assert!(!mouse.is_pressed());
        assert_eq!(mouse.handle("um-", Some((350, 70)), &ui), MouseAction::None);
        mouse.handle("dm-", Some((130, 70)), &ui);
        assert_eq!(mouse.handle("umc", Some((130, 70)), &ui), MouseAction::None);
    }

    #[test]
    fn drag_can_leave_panel_and_is_clamped_to_window() {
        let ui = panel();
        let mut mouse = PanelMouse::default();
        mouse.handle("dm-", Some((350, 70)), &ui);
        assert_eq!(
            mouse.drag_position((-500, -500), (1920, 1080)),
            Some((0, 0))
        );
        assert_eq!(
            mouse.drag_position((4000, 2000), (1920, 1080)),
            Some((1860, 1040))
        );
        assert_eq!(mouse.handle("um-", None, &ui), MouseAction::DragFinished);
    }

    #[test]
    fn disabled_panel_or_outside_press_cannot_start_drag() {
        let mut ui = panel();
        let mut mouse = PanelMouse::default();
        mouse.handle("dm-", Some((900, 70)), &ui);
        assert!(!mouse.is_pressed());
        ui.chat_enabled = false;
        mouse.handle("dm-", Some((350, 70)), &ui);
        assert!(!mouse.is_dragging());
    }

    #[test]
    fn minimized_click_restores_but_drag_release_does_not_click() {
        let mut ui = panel();
        ui.minimize((800, 700));
        let mut mouse = PanelMouse::default();
        assert_eq!(mouse.handle("dm-", Some((120, 80)), &ui), MouseAction::DragStarted);
        assert_eq!(mouse.drag_position((122, 82), (800, 700)), None);
        assert_eq!(mouse.handle("um-", Some((122, 82)), &ui), MouseAction::Click(122, 82));
        assert!(!mouse.is_pressed());
        assert_eq!(mouse.handle("dm-", Some((120, 80)), &ui), MouseAction::DragStarted);
        assert_eq!(mouse.drag_position((220, 180), (800, 700)), Some((200, 160)));
        assert_eq!(mouse.drag_position((120, 80), (800, 700)), Some((100, 60)));
        assert_eq!(mouse.handle("um-", Some((120, 80)), &ui), MouseAction::DragFinished);
    }

    #[test]
    fn minimized_hit_area_and_window_clamping_match_icon_size() {
        let mut ui = panel();
        ui.minimize((800, 700));
        let mut mouse = PanelMouse::default();
        assert_eq!(mouse.handle("dm-", Some((200, 200)), &ui), MouseAction::None);
        assert!(!mouse.is_pressed());
        mouse.handle("dm-", Some((120, 80)), &ui);
        assert_eq!(mouse.drag_position((2000, 2000), (800, 700)), Some((756, 656)));
        assert_eq!(mouse.handle("umc", None, &ui), MouseAction::DragFinished);
    }

    #[test]
    fn single_press_event_remains_a_click() {
        let ui = panel();
        let mut mouse = PanelMouse::default();
        assert_eq!(
            mouse.handle("pm-", Some((130, 70)), &ui),
            MouseAction::Click(130, 70)
        );
        assert!(!mouse.is_pressed());
    }
}
