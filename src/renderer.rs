//! Fast 2D rasterizer producing native BGRA pixmaps for mpv overlay
//! Supports Chat view, Settings drawer, and Search interface.

use crate::emotes::EmoteCache;
use crate::layout::{ChatLayouter, LayoutMessage, LayoutToken, TextStyle};
use crate::text_effects::TextBitmap;
use crate::ui::{max_settings_scroll, ChatViewport, HitBox, TextEffect, UiState, UiView, CHANNEL_TEXT_COLOR, MINIMIZED_SIZE, SEARCH_RESULTS_TOP, SEARCH_RESULT_HEIGHT};
use tiny_skia::{Mask, Paint, Pixmap, Rect, Transform};

const HEADER_BACKGROUND: [u8; 4] = [22, 23, 29, 245];
const HEADER_BORDER: [u8; 4] = [45, 45, 58, 255];

pub struct ChatRenderer {
    layouter: ChatLayouter,
}

impl ChatRenderer {
    pub fn new() -> Self {
        Self {
            layouter: ChatLayouter::new(),
        }
    }

    pub fn layouter(&self) -> &ChatLayouter {
        &self.layouter
    }

    /// Renders chat panel, settings, or search into BGRA pixel buffer
    pub fn render_frame(
        &self,
        messages: &[LayoutMessage],
        emotes: &EmoteCache,
        ui: &UiState,
        channel_name: &str,
    ) -> Option<Vec<u8>> {
        if ui.minimized {
            return self.render_minimized();
        }
        let width = ui.panel_width;
        let height = ui.panel_height;
        if width == 0 || height == 0 {
            return None;
        }

        let mut pixmap = Pixmap::new(width, height)?;

        let opacity = if ui.current_view == UiView::Chat { ui.bg_opacity } else { 0.8 };
        let bg_alpha = (opacity * 255.0).clamp(0.0, 255.0).round() as u8;
        let mut bg_paint = Paint::default();
        bg_paint.set_color_rgba8(13, 14, 18, bg_alpha);
        if let Some(rect) = Rect::from_xywh(0.0, 0.0, width as f32, height as f32) {
            pixmap.fill_rect(rect, &bg_paint, Transform::identity(), None);
        }

        self.render_fading_header(&mut pixmap, ui, channel_name);

        // 3. Render content based on current view
        match ui.current_view {
            UiView::Chat => {
                self.render_chat_view(&mut pixmap, messages, emotes, ui);
            }
            UiView::Settings => {
                self.render_settings_view(&mut pixmap, ui);
            }
            UiView::Search => {
                self.render_search_view(&mut pixmap, ui);
            }
        }

        Some(Self::to_bgra(&pixmap))
    }

    fn to_bgra(pixmap: &Pixmap) -> Vec<u8> {
        let mut data = pixmap.data().to_vec();
        for pixel in data.chunks_exact_mut(4) { pixel.swap(0, 2); }
        data
    }

    fn render_minimized(&self) -> Option<Vec<u8>> {
        let mut pixmap = Pixmap::new(MINIMIZED_SIZE, MINIMIZED_SIZE)?;
        pixmap.fill(tiny_skia::Color::from_rgba8(22, 23, 29, 160));
        let mut outline = Paint::default();
        outline.set_color_rgba8(145, 71, 255, 180);
        for (x, y, w, h) in [(0.0, 0.0, 44.0, 1.0), (0.0, 43.0, 44.0, 1.0), (0.0, 0.0, 1.0, 44.0), (43.0, 0.0, 1.0, 44.0)] {
            pixmap.fill_rect(Rect::from_xywh(x, y, w, h)?, &outline, Transform::identity(), None);
        }
        let mut path = tiny_skia::PathBuilder::new();
        path.move_to(11.0, 7.0);
        path.line_to(37.0, 7.0);
        path.line_to(37.0, 26.0);
        path.line_to(28.0, 35.0);
        path.line_to(21.0, 35.0);
        path.line_to(16.0, 40.0);
        path.line_to(16.0, 35.0);
        path.line_to(7.0, 35.0);
        path.line_to(7.0, 15.0);
        path.close();
        let mut purple = Paint::default();
        purple.set_color_rgba8(145, 71, 255, 255);
        pixmap.fill_path(&path.finish()?, &purple, tiny_skia::FillRule::Winding, Transform::identity(), None);
        let mut face = tiny_skia::PathBuilder::new();
        face.move_to(14.0, 10.0);
        face.line_to(34.0, 10.0);
        face.line_to(34.0, 25.0);
        face.line_to(27.0, 32.0);
        face.line_to(21.0, 32.0);
        face.line_to(17.0, 36.0);
        face.line_to(17.0, 32.0);
        face.line_to(14.0, 32.0);
        face.close();
        let mut white = Paint::default();
        white.set_color_rgba8(239, 239, 241, 255);
        pixmap.fill_path(&face.finish()?, &white, tiny_skia::FillRule::Winding, Transform::identity(), None);
        for x in [21.0, 28.0] {
            pixmap.fill_rect(Rect::from_xywh(x, 15.0, 3.0, 10.0)?, &purple, Transform::identity(), None);
        }
        Some(Self::to_bgra(&pixmap))
    }

    fn render_fading_header(&self, pixmap: &mut Pixmap, ui: &UiState, channel_name: &str) {
        let opacity = ui.header_opacity.clamp(0.0, 1.0);
        if opacity == 0.0 { return; }
        let Some(mut header) = Pixmap::new(pixmap.width(), 32) else { return; };
        self.render_header(&mut header, ui, channel_name);
        let mut paint = tiny_skia::PixmapPaint::default();
        paint.opacity = opacity;
        pixmap.draw_pixmap(0, 0, header.as_ref(), &paint, Transform::identity(), None);
    }

    fn render_header(&self, pixmap: &mut Pixmap, ui: &UiState, channel_name: &str) {
        let width = ui.panel_width as f32;
        let bar_h = 28.0;

        // Header background
        let mut header_paint = Paint::default();
        header_paint.set_color_rgba8(HEADER_BACKGROUND[0], HEADER_BACKGROUND[1], HEADER_BACKGROUND[2], HEADER_BACKGROUND[3]);
        if let Some(r) = Rect::from_xywh(0.0, 0.0, width, bar_h) {
            pixmap.fill_rect(r, &header_paint, Transform::identity(), None);
        }

        // Bottom border under header
        let mut border_paint = Paint::default();
        border_paint.set_color_rgba8(HEADER_BORDER[0], HEADER_BORDER[1], HEADER_BORDER[2], HEADER_BORDER[3]);
        if let Some(r) = Rect::from_xywh(0.0, bar_h - 1.0, width, 1.0) {
            pixmap.fill_rect(r, &border_paint, Transform::identity(), None);
        }

        // Tab: Chat
        let is_chat = ui.current_view == UiView::Chat;
        self.draw_button(
            pixmap,
            &Self::btn_tab_chat(),
            "Чат",
            is_chat,
            if is_chat { [145, 71, 255, 255] } else { [36, 37, 46, 255] },
            if is_chat { [255, 255, 255, 255] } else { [160, 160, 175, 255] },
        );

        // Tab: Search
        let is_search = ui.current_view == UiView::Search;
        self.draw_button(
            pixmap,
            &Self::btn_tab_search(),
            "Поиск",
            is_search,
            if is_search { [145, 71, 255, 255] } else { [36, 37, 46, 255] },
            if is_search { [255, 255, 255, 255] } else { [160, 160, 175, 255] },
        );

        // Tab: Settings
        let is_settings = ui.current_view == UiView::Settings;
        self.draw_button(
            pixmap,
            &Self::btn_tab_settings(),
            "Опции",
            is_settings,
            if is_settings { [145, 71, 255, 255] } else { [36, 37, 46, 255] },
            if is_settings { [255, 255, 255, 255] } else { [160, 160, 175, 255] },
        );

        // Display channel name tag if available
        if !channel_name.is_empty() && width >= 380.0 {
            let ch_display = format!("#{}", channel_name);
            self.draw_text(
                pixmap,
                &ch_display,
                214.0,
                7.0,
                11.0,
                CHANNEL_TEXT_COLOR,
            );
        }

        // Offset indicator if non-zero
        if ui.offset_ms != 0 {
            let offset_sec = ui.offset_ms as f32 / 1000.0;
            let offset_str = format!("{:+0.1}s", offset_sec);
            self.draw_text(
                pixmap,
                &offset_str,
                width - 68.0,
                7.0,
                11.0,
                [0, 255, 210, 255],
            );
        }

        // Drag handle indicator (3 subtle vertical grip bars)
        let grip_x = width - 42.0;
        let mut grip_paint = Paint::default();
        grip_paint.set_color_rgba8(80, 85, 105, 160);
        for i in 0..3 {
            if let Some(r) = Rect::from_xywh(grip_x + (i as f32 * 3.5), 9.0, 1.5, 10.0) {
                pixmap.fill_rect(r, &grip_paint, Transform::identity(), None);
            }
        }

        // Close button [ ✕ ]
        self.draw_button(
            pixmap,
            &Self::btn_close(width),
            "✕",
            false,
            [40, 30, 35, 255],
            [220, 120, 130, 255],
        );
    }

    fn render_chat_view(
        &self,
        pixmap: &mut Pixmap,
        messages: &[LayoutMessage],
        emotes: &EmoteCache,
        ui: &UiState,
    ) {
        let width = ui.panel_width as f32;
        let height = ui.panel_height as f32;
        let viewport = ChatViewport::new(ui, messages.last().map(|m| m.bottom_y).unwrap_or(0.0));
        let visible_top = viewport.top;
        let visible_bottom = viewport.bottom;
        let base_y = viewport.base_y;
        let mut mask = match Mask::new(pixmap.width(), pixmap.height()) {
            Some(mask) => mask,
            None => return,
        };
        let row_width = pixmap.width() as usize;
        let top_row = (visible_top.ceil() as usize).min(pixmap.height() as usize);
        let bottom_row = (visible_bottom.ceil() as usize).min(pixmap.height() as usize);
        mask.data_mut()[top_row * row_width..bottom_row * row_width].fill(255);

        if messages.is_empty() {
            self.draw_text(
                pixmap,
                "Ожидание сообщений стрима...",
                14.0,
                visible_top + 16.0,
                13.0,
                [140, 140, 155, 255],
            );
            return;
        }

        for msg in messages {
            let shifted_top = base_y + msg.top_y;
            let shifted_bottom = base_y + msg.bottom_y;

            if shifted_bottom < visible_top || shifted_top > visible_bottom {
                continue;
            }

            if ui.highlighted_message_ts == Some(msg.timestamp_ms) {
                let top = shifted_top.max(visible_top);
                let bottom = shifted_bottom.min(visible_bottom);
                if let Some(rect) = Rect::from_xywh(8.0, top, width - 16.0, bottom - top) {
                    let mut paint = Paint::default();
                    paint.set_color_rgba8(145, 71, 255, 70);
                    pixmap.fill_rect(rect, &paint, Transform::identity(), Some(&mask));
                }
            }
            for token in &msg.tokens {
                match token {
                    LayoutToken::Text { text, color, style, x, y } => {
                        let render_y = base_y + y;
                        if render_y + ui.font_size >= visible_top && render_y - ui.font_size * 2.0 <= visible_bottom {
                            if let Some(bitmap) = self.rasterize_text(text, *x, render_y - ui.font_size, ui.font_size, *style) {
                                bitmap.paint(pixmap, *color, ui.text_effect, ui.outline_width, Some((visible_top, visible_bottom)));
                            }
                        }
                    }
                    LayoutToken::Image {
                        path,
                        x,
                        y,
                        width: _iw,
                        height: ih,
                    } => {
                        let render_y = base_y + y;
                        if render_y + *ih as f32 >= visible_top && render_y <= visible_bottom {
                            if let Some(emote_px) = emotes.get_or_load(path, *ih) {
                                pixmap.draw_pixmap(
                                    *x as i32,
                                    render_y as i32,
                                    emote_px.as_ref(),
                                    &tiny_skia::PixmapPaint::default(),
                                    Transform::identity(),
                                    Some(&mask),
                                );
                            }
                        }
                    }
                }
            }
        }

        // If scrolled into history or into future, render interactive banner at bottom
        if ui.scroll_offset != 0 || ui.chat_scroll_anchor.is_some() {
            let ban_y = height - 26.0;
            let mut ban_paint = Paint::default();
            ban_paint.set_color_rgba8(HEADER_BACKGROUND[0], HEADER_BACKGROUND[1], HEADER_BACKGROUND[2], HEADER_BACKGROUND[3]);
            if let Some(r) = Rect::from_xywh(4.0, ban_y, width - 8.0, 24.0) {
                pixmap.fill_rect(r, &ban_paint, Transform::identity(), None);
            }

            // Match the header divider.
            let mut b_paint = Paint::default();
            b_paint.set_color_rgba8(HEADER_BORDER[0], HEADER_BORDER[1], HEADER_BORDER[2], HEADER_BORDER[3]);
            if let Some(r) = Rect::from_xywh(4.0, ban_y, width - 8.0, 1.0) {
                pixmap.fill_rect(r, &b_paint, Transform::identity(), None);
            }

            let text = if ui.scroll_offset == 0 { "Чат приостановлен".to_string() } else { Self::paused_banner_text(ui.scroll_offset) };
            self.draw_text(pixmap, &text, 14.0, ban_y + 5.0, 11.0, CHANNEL_TEXT_COLOR);
        }
    }

    fn paused_banner_text(scroll_offset: i32) -> String {
        if scroll_offset > 0 {
            format!("⬆ Чат: история (-{}) | Чат приостановлен", scroll_offset)
        } else {
            format!("⬇ Чат: будущее (+{}) | Чат приостановлен", scroll_offset.unsigned_abs())
        }
    }

    fn render_settings_view(&self, pixmap: &mut Pixmap, ui: &UiState) {
        let scroll = ui.settings_scroll_offset.min(max_settings_scroll(ui.panel_height));
        let height = crate::ui::SETTINGS_CONTENT_HEIGHT.max(ui.panel_height as f32) as u32;
        let Some(mut content) = Pixmap::new(ui.panel_width, height) else { return; };
        self.render_settings_content(&mut content, ui);
        let Some(mut mask) = Mask::new(pixmap.width(), pixmap.height()) else { return; };
        let start = (32usize.min(pixmap.height() as usize)) * pixmap.width() as usize;
        mask.data_mut()[start..].fill(255);
        pixmap.draw_pixmap(0, -(scroll as i32), content.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), Some(&mask));
    }

    fn render_settings_content(&self, pixmap: &mut Pixmap, ui: &UiState) {
        let width = ui.panel_width as f32;
        let mut cur_y = 36.0;

        // Title
        self.draw_text(
            pixmap,
            "Настройки оверлея и синхронизации",
            12.0,
            cur_y,
            13.0,
            [191, 148, 255, 255],
        );
        cur_y += 24.0;

        // Row 1: Time Offset
        let offset_sec = ui.offset_ms as f32 / 1000.0;
        let offset_str = format!("Смещение: {:+0.1}с", offset_sec);
        self.draw_text(pixmap, &offset_str, 12.0, cur_y + 3.0, 12.0, [255, 255, 255, 255]);

        self.draw_button(
            pixmap,
            &Self::btn_offset_reset(),
            "Сброс 0.0с",
            false,
            [40, 42, 54, 255],
            [100, 220, 255, 255],
        );
        cur_y += 26.0;

        // Offset Buttons: [-10s] [-1s] [-0.1s] [+0.1s] [+1s] [+10s]
        let labels = ["-10s", "-1s", "-0.1s", "+0.1s", "+1s", "+10s"];
        for (i, lbl) in labels.iter().enumerate() {
            let hb = Self::btn_offset_step(i, cur_y);
            self.draw_button(pixmap, &hb, lbl, false, [32, 33, 42, 255], [230, 230, 240, 255]);
        }
        cur_y += 32.0;

        // Row 2: Font Size [A-] [A+] <val>px
        self.draw_text(pixmap, "Шрифт:", 12.0, cur_y + 3.0, 12.0, [200, 200, 220, 255]);
        self.draw_button(
            pixmap,
            &HitBox::new(65.0, cur_y, 28.0, 22.0),
            "A-",
            false,
            [35, 36, 46, 255],
            [255, 255, 255, 255],
        );
        self.draw_button(
            pixmap,
            &HitBox::new(98.0, cur_y, 28.0, 22.0),
            "A+",
            false,
            [35, 36, 46, 255],
            [255, 255, 255, 255],
        );
        let fs_str = format!("{:.0}px", ui.font_size);
        self.draw_text(pixmap, &fs_str, 134.0, cur_y + 4.0, 12.0, [0, 210, 255, 255]);

        // Panel Width: [W-] [W+] <val>
        self.draw_text(pixmap, "Шир:", 190.0, cur_y + 3.0, 12.0, [200, 200, 220, 255]);
        self.draw_button(
            pixmap,
            &HitBox::new(230.0, cur_y, 28.0, 22.0),
            "W-",
            false,
            [35, 36, 46, 255],
            [255, 255, 255, 255],
        );
        self.draw_button(
            pixmap,
            &HitBox::new(262.0, cur_y, 28.0, 22.0),
            "W+",
            false,
            [35, 36, 46, 255],
            [255, 255, 255, 255],
        );
        let w_str = format!("{}", ui.panel_width);
        self.draw_text(pixmap, &w_str, 296.0, cur_y + 4.0, 12.0, [0, 210, 255, 255]);
        cur_y += 30.0;

        // Row 3: Panel Height: [H-] [H+] <val>
        self.draw_text(pixmap, "Высота:", 12.0, cur_y + 3.0, 12.0, [200, 200, 220, 255]);
        self.draw_button(
            pixmap,
            &HitBox::new(65.0, cur_y, 28.0, 22.0),
            "H-",
            false,
            [35, 36, 46, 255],
            [255, 255, 255, 255],
        );
        self.draw_button(
            pixmap,
            &HitBox::new(98.0, cur_y, 28.0, 22.0),
            "H+",
            false,
            [35, 36, 46, 255],
            [255, 255, 255, 255],
        );
        let h_str = format!("{}", ui.panel_height);
        self.draw_text(pixmap, &h_str, 134.0, cur_y + 4.0, 12.0, [0, 210, 255, 255]);

        // Background Opacity
        let op_pct = (ui.bg_opacity * 100.0).round() as u32;
        let op_str = format!("Чат: {}%", op_pct);
        self.draw_button(
            pixmap,
            &HitBox::new(190.0, cur_y, 90.0, 22.0),
            &op_str,
            false,
            [35, 36, 46, 255],
            [0, 220, 255, 255],
        );
        cur_y += 34.0;

        self.draw_button(pixmap, &Self::btn_emotes(), if ui.emotes_enabled { "Эмоуты: вкл." } else { "Эмоуты: выкл." }, ui.emotes_enabled, [32, 33, 42, 255], [240, 240, 255, 255]);
        self.draw_button(pixmap, &Self::btn_badges(), if ui.badges_enabled { "Значки: вкл." } else { "Значки: выкл." }, ui.badges_enabled, [32, 33, 42, 255], [240, 240, 255, 255]);
        cur_y += 34.0;

        // Row 5: 1-Click Sync Anchor Section
        let sync_box_h = 76.0;
        let mut sbox_paint = Paint::default();
        sbox_paint.set_color_rgba8(26, 28, 38, 255);
        if let Some(r) = Rect::from_xywh(12.0, cur_y, width - 24.0, sync_box_h) {
            pixmap.fill_rect(r, &sbox_paint, Transform::identity(), None);
        }

        self.draw_text(
            pixmap,
            "1-Click Синхронизация по сообщению:",
            20.0,
            cur_y + 6.0,
            11.0,
            [145, 71, 255, 255],
        );

        let anchor_preview = if let Some(ref m) = ui.selected_sync_msg {
            let snip: String = m.text.chars().take(28).collect();
            format!("Якорь: {}: {}", m.display_name, snip)
        } else {
            "Кликните сообщение в чате для выбора якоря".to_string()
        };
        self.draw_text(pixmap, &anchor_preview, 20.0, cur_y + 24.0, 11.0, [220, 220, 235, 255]);

        let sync_btn_hb = HitBox::new(20.0, cur_y + 44.0, width - 40.0, 24.0);
        self.draw_button(
            pixmap,
            &sync_btn_hb,
            "⚓ Синхронизировать кадр видео с якорем",
            false,
            [26, 52, 46, 255],
            [0, 255, 200, 255],
        );
        self.draw_text(pixmap, "Читаемость текста:", 12.0, 404.0, 12.0, [200, 200, 220, 255]);
        self.draw_button(pixmap, &Self::btn_text_effect(width), ui.text_effect.label(), false, [35, 36, 46, 255], [0, 220, 255, 255]);
        self.draw_text(pixmap, &format!("Толщина: {:.1} px", ui.outline_width), 12.0, 436.0, 12.0, [200, 200, 220, 255]);
        self.draw_button(pixmap, &Self::btn_outline_minus(), "−", false, [35, 36, 46, 255], [255, 255, 255, 255]);
        self.draw_button(pixmap, &Self::btn_outline_plus(), "+", false, [35, 36, 46, 255], [255, 255, 255, 255]);
        self.draw_text(pixmap, "Колесо мыши: прокрутка настроек", 12.0, 476.0, 11.0, [120, 120, 140, 255]);
    }

    fn render_search_view(&self, pixmap: &mut Pixmap, ui: &UiState) {
        let width = ui.panel_width as f32;
        let height = ui.panel_height as f32;
        let mut cur_y = 36.0;

        // Search bar box with active glowing border
        let sb_w = width - 24.0;
        let sb_h = 28.0;
        let mut sbar_paint = Paint::default();
        sbar_paint.set_color_rgba8(28, 29, 38, 255);
        if let Some(r) = Rect::from_xywh(12.0, cur_y, sb_w, sb_h) {
            pixmap.fill_rect(r, &sbar_paint, Transform::identity(), None);
        }
        let mut sborder_paint = Paint::default();
        sborder_paint.set_color_rgba8(145, 71, 255, 255);
        if let Some(r) = Rect::from_xywh(12.0, cur_y + sb_h - 2.0, sb_w, 2.0) {
            pixmap.fill_rect(r, &sborder_paint, Transform::identity(), None);
        }

        let blink = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() / 500) % 2 == 0;
        let cursor_char = if blink { "|" } else { " " };
        let q_display = if ui.search_query.is_empty() {
            format!("Поиск сообщения или ника{}", cursor_char)
        } else {
            format!("{}{}", ui.search_query, cursor_char)
        };
        let q_color = if ui.search_query.is_empty() {
            [160, 160, 180, 255]
        } else {
            [255, 255, 255, 255]
        };
        self.draw_text(pixmap, &q_display, 18.0, cur_y + 7.0, 12.0, q_color);
        let count_str = format!("Найдено: {}", ui.search_results.len());
        self.draw_text(pixmap, &count_str, 14.0, 71.0, 11.0, CHANNEL_TEXT_COLOR);
        cur_y = SEARCH_RESULTS_TOP;

        // Search Results List
        if ui.search_results.is_empty() {
            if !ui.search_query.is_empty() {
                self.draw_text(
                    pixmap,
                    "Ничего не найдено по запросу.",
                    14.0,
                    cur_y + 12.0,
                    12.0,
                    [150, 150, 160, 255],
                );
            } else {
                self.draw_text(
                    pixmap,
                    "Напечатайте текст или никнейм для поиска.",
                    14.0,
                    cur_y + 12.0,
                    12.0,
                    [130, 130, 145, 255],
                );
            }
            return;
        }

        let item_h = SEARCH_RESULT_HEIGHT;
        let visible_items = ((height - cur_y).max(0.0) / item_h).ceil() as usize;
        let start_idx = ui.search_scroll_offset.min(ui.search_results.len());
        let end_idx = (start_idx + visible_items).min(ui.search_results.len());

        for (i, res) in ui.search_results[start_idx..end_idx].iter().enumerate() {
            let item_y = cur_y + (i as f32 * item_h);

            // Card background
            let mut card_paint = Paint::default();
            card_paint.set_color_rgba8(20, 22, 30, 240);
            if let Some(r) = Rect::from_xywh(12.0, item_y, width - 24.0, item_h - 4.0) {
                pixmap.fill_rect(r, &card_paint, Transform::identity(), None);
            }

            // Timestamp badge [01:23:45]
            self.draw_text(pixmap, &res.time_str, 18.0, item_y + 5.0, 11.0, [0, 255, 200, 255]);

            // Author nick
            let nick_col = parse_hex_color(&res.color_hex);
            self.draw_styled_text(pixmap, &res.author, 84.0, item_y + 5.0, 11.0, nick_col, TextStyle::Nickname, None);

            // Text preview
            let snip: String = res.text.chars().take(40).collect();
            self.draw_styled_text(pixmap, &snip, 84.0, item_y + 20.0, 11.0, [220, 220, 230, 255], TextStyle::Message, None);
        }
    }

    pub fn draw_button(
        &self,
        pixmap: &mut Pixmap,
        hb: &HitBox,
        text: &str,
        is_active: bool,
        bg_color: [u8; 4],
        text_color: [u8; 4],
    ) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(bg_color[0], bg_color[1], bg_color[2], bg_color[3]);
        if let Some(r) = Rect::from_xywh(hb.x, hb.y, hb.w, hb.h) {
            pixmap.fill_rect(r, &paint, Transform::identity(), None);
        }

        // Border if active
        if is_active {
            let mut border_paint = Paint::default();
            border_paint.set_color_rgba8(191, 148, 255, 255);
            if let Some(r) = Rect::from_xywh(hb.x, hb.y + hb.h - 2.0, hb.w, 2.0) {
                pixmap.fill_rect(r, &border_paint, Transform::identity(), None);
            }
        }

        let tw = self.layouter.measure_text_width(text, 11.0);
        let tx = hb.x + (hb.w - tw).max(0.0) / 2.0;
        let ty = hb.y + (hb.h - 11.0) / 2.0;
        self.draw_text(pixmap, text, tx, ty, 11.0, text_color);
    }

    pub fn draw_text(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        start_x: f32,
        baseline_y: f32,
        font_size: f32,
        color: [u8; 4],
    ) {
        self.draw_styled_text(pixmap, text, start_x, baseline_y, font_size, color, TextStyle::Ui, None);
    }

    fn draw_styled_text(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        start_x: f32,
        baseline_y: f32,
        font_size: f32,
        color: [u8; 4],
        style: TextStyle,
        clip_y: Option<(f32, f32)>,
    ) {
        if let Some(bitmap) = self.rasterize_text(text, start_x, baseline_y, font_size, style) {
            bitmap.paint(pixmap, color, TextEffect::None, 1.5, clip_y);
        }
    }

    fn rasterize_text(&self, text: &str, mut start_x: f32, baseline_y: f32, font_size: f32, style: TextStyle) -> Option<TextBitmap> {
        let mut glyphs = Vec::new();
        let mut previous = None;
        let mut left = i32::MAX;
        let mut top = i32::MAX;
        let mut right = i32::MIN;
        let mut bottom = i32::MIN;
        for c in text.chars() {
            if let Some(p) = previous {
                start_x += self.layouter.kerning(p, c, font_size, style);
            }
            let font = self.layouter.font_for_char(c, style);
            let (metrics, bitmap) = font.rasterize(c, font_size);
            let x = (start_x + metrics.xmin as f32).round() as i32;
            let y = (baseline_y + font_size - metrics.ymin as f32 - metrics.height as f32).round() as i32;
            if metrics.width > 0 && metrics.height > 0 {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + metrics.width as i32);
                bottom = bottom.max(y + metrics.height as i32);
                glyphs.push((x, y, metrics.width, metrics.height, bitmap));
            }
            start_x += metrics.advance_width;
            previous = Some(c);
        }
        if glyphs.is_empty() { return None; }
        let width = (right - left) as usize;
        let height = (bottom - top) as usize;
        let mut coverage = vec![0u8; width.checked_mul(height)?];
        for (x, y, glyph_width, glyph_height, bitmap) in glyphs {
            for sy in 0..glyph_height {
                for sx in 0..glyph_width {
                    let index = (y - top) as usize * width + sy * width + (x - left) as usize + sx;
                    let source = bitmap[sy * glyph_width + sx] as u32;
                    let destination = coverage[index] as u32;
                    coverage[index] = (source + (destination * (255 - source) + 127) / 255).min(255) as u8;
                }
            }
        }
        Some(TextBitmap { x: left, y: top, width, height, coverage })
    }

    pub fn btn_emotes() -> HitBox {
        HitBox::new(12.0, 182.0, 112.0, 22.0)
    }

    pub fn btn_badges() -> HitBox {
        HitBox::new(136.0, 182.0, 112.0, 22.0)
    }

    pub fn btn_text_effect(width: f32) -> HitBox {
        HitBox::new(160.0, 400.0, (width - 172.0).max(70.0), 22.0)
    }

    pub fn btn_outline_minus() -> HitBox {
        HitBox::new(166.0, 432.0, 28.0, 22.0)
    }

    pub fn btn_outline_plus() -> HitBox {
        HitBox::new(202.0, 432.0, 28.0, 22.0)
    }

    pub fn is_drag_handle(x: f32, y: f32, width: f32) -> bool {
        x >= -15.0 && x <= width + 15.0 && y >= -5.0 && y <= 30.0
            && !Self::btn_tab_chat().contains(x, y)
            && !Self::btn_tab_search().contains(x, y)
            && !Self::btn_tab_settings().contains(x, y)
            && !Self::btn_close(width).contains(x, y)
    }

    // Static HitBox generators for header buttons
    pub fn btn_tab_chat() -> HitBox {
        HitBox::new(4.0, 3.0, 60.0, 22.0)
    }

    pub fn btn_tab_search() -> HitBox {
        HitBox::new(68.0, 3.0, 68.0, 22.0)
    }

    pub fn btn_tab_settings() -> HitBox {
        HitBox::new(140.0, 3.0, 68.0, 22.0)
    }

    pub fn btn_close(width: f32) -> HitBox {
        HitBox::new(width - 26.0, 3.0, 22.0, 22.0)
    }

    pub fn btn_offset_reset() -> HitBox {
        HitBox::new(140.0, 58.0, 80.0, 22.0)
    }

    pub fn btn_offset_step(idx: usize, y: f32) -> HitBox {
        let x = 12.0 + (idx as f32 * 56.0);
        HitBox::new(x, y, 50.0, 22.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimized_button_is_small_translucent_and_contains_logo() {
        let renderer = ChatRenderer::new();
        let cache = EmoteCache::new(std::path::Path::new("."));
        let ui = UiState { minimized: true, ..Default::default() };
        let pixels = renderer.render_frame(&[], &cache, &ui, "channel").unwrap();
        assert_eq!(pixels.len(), 44 * 44 * 4);
        assert_eq!(pixels[(2 * 44 + 2) * 4 + 3], 160);
        assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] == 255));
        assert!(pixels.chunks_exact(4).all(|pixel| pixel[..3].iter().all(|c| *c <= pixel[3])));
    }

    #[test]
    fn paused_banner_uses_channel_color() {
        let renderer = ChatRenderer::new();
        let cache = EmoteCache::new(std::path::Path::new("."));
        for scroll in [-4, 4] {
            let ui = UiState { panel_height: 220, scroll_offset: scroll, ..Default::default() };
            let mut pixels = Pixmap::new(340, 220).unwrap();
            let message = LayoutMessage { msg_index: 0, timestamp_ms: 0, top_y: 0.0, bottom_y: 20.0, tokens: vec![] };
            renderer.render_chat_view(&mut pixels, &[message], &cache, &ui);
            let mut expected = Pixmap::new(340, 220).unwrap();
            let mut paint = Paint::default();
            paint.set_color_rgba8(HEADER_BACKGROUND[0], HEADER_BACKGROUND[1], HEADER_BACKGROUND[2], HEADER_BACKGROUND[3]);
            expected.fill_rect(Rect::from_xywh(4.0, 194.0, 332.0, 24.0).unwrap(), &paint, Transform::identity(), None);
            paint.set_color_rgba8(HEADER_BORDER[0], HEADER_BORDER[1], HEADER_BORDER[2], HEADER_BORDER[3]);
            expected.fill_rect(Rect::from_xywh(4.0, 194.0, 332.0, 1.0).unwrap(), &paint, Transform::identity(), None);
            let text = if scroll > 0 {
                "⬆ Чат: история (-4) | Чат приостановлен"
            } else {
                "⬇ Чат: будущее (+4) | Чат приостановлен"
            };
            assert_eq!(ChatRenderer::paused_banner_text(scroll), text);
            renderer.draw_text(&mut expected, text, 14.0, 199.0, 11.0, CHANNEL_TEXT_COLOR);
            assert!(pixels.data()[194 * 340 * 4..] == expected.data()[194 * 340 * 4..]);
            let mut header = Pixmap::new(340, 32).unwrap();
            renderer.render_header(&mut header, &ui, "");
            let header_pixel = (10 * 340 + 250) * 4;
            let banner_pixel = (214 * 340 + 330) * 4;
            assert_eq!(&header.data()[header_pixel..header_pixel + 4], &pixels.data()[banner_pixel..banner_pixel + 4]);
        }
    }

    #[test]
    fn search_and_settings_background_ignore_chat_opacity() {
        let renderer = ChatRenderer::new();
        let cache = EmoteCache::new(std::path::Path::new("."));
        for view in [UiView::Search, UiView::Settings] {
            let ui = UiState { current_view: view, panel_height: 600, bg_opacity: 0.0, ..Default::default() };
            let transparent = renderer.render_frame(&[], &cache, &ui, "").unwrap();
            let opaque = renderer.render_frame(&[], &cache, &UiState { bg_opacity: 1.0, ..ui }, "").unwrap();
            for (index, (a, b)) in transparent.chunks_exact(4).zip(opaque.chunks_exact(4)).enumerate() {
                let x = index % 340;
                let y = index / 340;
                // The settings control still displays the saved chat-opacity value.
                if view == UiView::Settings && (190..=280).contains(&x) && (148..=180).contains(&y) { continue; }
                assert!(a == b, "Background changed at ({x}, {y}) in {view:?}");
            }
            let corner = (599 * 340 + 339) * 4;
            assert_eq!(transparent[corner + 3], 204);
        }
        let chat = UiState { bg_opacity: 0.0, ..Default::default() };
        let transparent = renderer.render_frame(&[], &cache, &chat, "").unwrap();
        let opaque = renderer.render_frame(&[], &cache, &UiState { bg_opacity: 1.0, ..chat }, "").unwrap();
        assert_eq!(transparent[3], 0);
        assert_eq!(opaque[3], 255);
    }

    #[test]
    fn header_opacity_changes_only_top_panel() {
        let renderer = ChatRenderer::new();
        let cache = EmoteCache::new(std::path::Path::new("."));
        let ui = UiState { bg_opacity: 0.0, header_opacity: 0.0, ..Default::default() };
        let hidden = renderer.render_frame(&[], &cache, &ui, "test").unwrap();
        let half = renderer.render_frame(&[], &cache, &UiState { header_opacity: 0.5, ..ui.clone() }, "test").unwrap();
        let shown = renderer.render_frame(&[], &cache, &UiState { header_opacity: 1.0, ..ui }, "test").unwrap();
        let top = 32 * 340 * 4;
        assert!(hidden[..top].iter().all(|c| *c == 0));
        assert_ne!(&hidden[..top], &shown[..top]);
        assert_eq!(&hidden[top..], &shown[top..]);
        let pixel = (10 * 340 + 250) * 4;
        assert!(half[pixel + 3] > 0 && half[pixel + 3] < shown[pixel + 3]);
    }

    #[test]
    fn render_twitch_chat_preview() {
        use crate::session::{BadgeTag, ChatMessage, EmotesManifest, WordEmoteEntry};
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/twitch-chat-preview");
        std::fs::create_dir_all(&dir).unwrap();
        let badge_path = dir.join("badge.png");
        let emote_path = dir.join("emote.png");
        let mut badge = Pixmap::new(18, 18).unwrap();
        badge.fill(tiny_skia::Color::from_rgba8(145, 71, 255, 255));
        badge.save_png(&badge_path).unwrap();
        let mut emote = Pixmap::new(28, 28).unwrap();
        emote.fill(tiny_skia::Color::from_rgba8(255, 210, 60, 255));
        emote.save_png(&emote_path).unwrap();
        let mut manifest = EmotesManifest::default();
        manifest.words.insert("Kappa".into(), WordEmoteEntry { file: Some(emote_path.to_string_lossy().into_owned()), ..Default::default() });
        manifest.badges.insert("mod/1".into(), crate::session::BadgeEntry { file: Some(badge_path.to_string_lossy().into_owned()), ..Default::default() });
        let messages: Vec<_> = [
            ("Nickname", "Hello chat! Привет всем!", 0),
            ("РусскийНик", "Сообщение с двумя значками.", 2),
            ("EmoteFan", "Kappa Kappa hello Kappa", 1),
            ("LongMessage", "Это длинное сообщение проверяет перенос текста на новую строку от левого отступа чата, а не от конца ника.", 1),
            ("VeryLongNickname12345678901234567890", "Averylongwordwithoutspacestotestwrappingandclipping", 0),
        ].into_iter().enumerate().map(|(index, (nick, text, badges))| ChatMessage {
            timestamp_ms: index as i64, author: nick.into(), display_name: nick.into(), color_hex: "#55AAFF".into(), text: text.into(),
            badges: vec![BadgeTag { name: "mod".into(), version: "1".into() }; badges], twitch_emotes_raw: String::new(), is_gap: false,
        }).collect();
        let renderer = ChatRenderer::new();
        let ui = UiState { panel_width: 340, panel_height: 380, bg_opacity: 1.0, ..Default::default() };
        let cache = EmoteCache::new(&dir);
        let layout = renderer.layouter.layout_messages(&messages, &manifest, &cache, 340.0, 14.0, true);
        assert_eq!(layout[0].bottom_y, 30.0);
        assert_eq!(layout[2].bottom_y - layout[2].top_y, 36.0);
        for (mode, name) in [(TextEffect::Outline, "chat-outline.png"), (TextEffect::Shadow, "chat-shadow.png")] {
            let styled_ui = UiState { text_effect: mode, outline_width: 2.0, ..ui.clone() };
            let mut pixels = renderer.render_frame(&layout, &cache, &styled_ui, "preview").unwrap();
            for pixel in pixels.chunks_exact_mut(4) { pixel.swap(0, 2); }
            Pixmap::from_vec(pixels, tiny_skia::IntSize::from_wh(340, 380).unwrap()).unwrap().save_png(dir.join(name)).unwrap();
        }
        let mut bgra = renderer.render_frame(&layout, &cache, &ui, "preview").unwrap();
        for pixel in bgra.chunks_exact_mut(4) { pixel.swap(0, 2); }
        Pixmap::from_vec(bgra, tiny_skia::IntSize::from_wh(340, 380).unwrap()).unwrap().save_png(dir.join("chat.png")).unwrap();
    }

    #[test]
    fn effects_keep_header_unchanged_and_settings_scroll_is_clipped() {
        let renderer = ChatRenderer::new();
        let cache = EmoteCache::new(std::path::Path::new("."));
        let ui = UiState { panel_width: 340, panel_height: 220, ..Default::default() };
        let messages = [LayoutMessage { msg_index: 0, timestamp_ms: 0, top_y: 0.0, bottom_y: 300.0,
            tokens: vec![LayoutToken::Text { text: "Читаемый текст".into(), color: [239, 239, 241, 255], style: TextStyle::Message, x: 16.0, y: 135.0 }] }];
        let plain = renderer.render_frame(&messages, &cache, &ui, "test").unwrap();
        for effect in [TextEffect::Outline, TextEffect::Shadow] {
            let styled = renderer.render_frame(&messages, &cache, &UiState { text_effect: effect, outline_width: 4.0, ..ui.clone() }, "test").unwrap();
            assert_eq!(&plain[..32 * 340 * 4], &styled[..32 * 340 * 4]);
            assert_ne!(plain, styled);
        }
        let mut pixmap = Pixmap::new(340, 220).unwrap();
        renderer.render_header(&mut pixmap, &ui, "test");
        let before = pixmap.data()[..32 * 340 * 4].to_vec();
        renderer.render_settings_view(&mut pixmap, &UiState { settings_scroll_offset: max_settings_scroll(220), ..ui });
        assert_eq!(&pixmap.data()[..32 * 340 * 4], before.as_slice());
    }

    #[test]
    fn overflowing_text_and_emotes_cannot_change_header_or_banner() {
        let renderer = ChatRenderer::new();
        let dir = std::env::temp_dir().join(format!("mpv-chat-clip-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image_path = dir.join("emote.png");
        let mut image = Pixmap::new(24, 24).unwrap();
        image.fill(tiny_skia::Color::from_rgba8(255, 40, 20, 255));
        image.save_png(&image_path).unwrap();
        let emotes = EmoteCache::new(&dir);
        for scroll in [0, 4] {
            let ui = UiState { panel_width: 420, panel_height: 160, scroll_offset: scroll, ..UiState::default() };
            let mut baseline = Pixmap::new(ui.panel_width, ui.panel_height).unwrap();
            renderer.render_header(&mut baseline, &ui, "test");
            let before = baseline.data().to_vec();
            let viewport = ChatViewport::new(&ui, 300.0);
            let tokens = [-8.0, 24.0, 145.0, 195.0].into_iter().flat_map(|screen_y| {
                [
                    LayoutToken::Text { text: "Привет Nickname".into(), color: [255, 255, 255, 255], style: TextStyle::Message, x: 70.0, y: screen_y - viewport.base_y },
                    LayoutToken::Image { path: image_path.to_string_lossy().into_owned(), x: 150.0, y: screen_y - viewport.base_y, width: 24, height: 24 },
                ]
            }).collect();
            let messages = [LayoutMessage { msg_index: 0, timestamp_ms: 0, top_y: 0.0, bottom_y: 300.0, tokens }];
            renderer.render_chat_view(&mut baseline, &messages, &emotes, &ui);
            let header_end = 32 * ui.panel_width as usize * 4;
            assert_eq!(&baseline.data()[..header_end], &before[..header_end]);
            assert_ne!(&baseline.data()[header_end..], &before[header_end..]);
            if scroll != 0 {
                let mut banner_only = Pixmap::new(ui.panel_width, ui.panel_height).unwrap();
                renderer.render_chat_view(&mut banner_only, &[LayoutMessage { tokens: vec![], ..messages[0].clone() }], &emotes, &ui);
                let banner_start = viewport.bottom as usize * ui.panel_width as usize * 4;
                assert_eq!(&baseline.data()[banner_start..], &banner_only.data()[banner_start..]);
            }
        }
        std::fs::remove_file(image_path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

fn parse_hex_color(hex: &str) -> [u8; 4] {
    if hex.starts_with('#') && hex.len() == 7 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&hex[1..3], 16),
            u8::from_str_radix(&hex[3..5], 16),
            u8::from_str_radix(&hex[5..7], 16),
        ) {
            return [r, g, b, 255];
        }
    }
    [145, 71, 255, 255]
}
