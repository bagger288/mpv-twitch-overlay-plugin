//! Fast 2D rasterizer producing native BGRA pixmaps for mpv overlay
//! Supports Chat view, Settings drawer, and Search interface.

use crate::emotes::EmoteCache;
use crate::layout::{ChatLayouter, LayoutMessage, LayoutToken};
use crate::session::format_hms;
use crate::ui::{HitBox, UiState, UiView};
use tiny_skia::{Paint, Pixmap, Rect, Transform};

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
        emotes: &mut EmoteCache,
        ui: &UiState,
        channel_name: &str,
    ) -> Option<Vec<u8>> {
        let width = ui.panel_width;
        let height = ui.panel_height;
        if width == 0 || height == 0 {
            return None;
        }

        let mut pixmap = Pixmap::new(width, height)?;

        // 1. Fill background panel with configured opacity
        let bg_alpha = (ui.bg_opacity * 255.0).clamp(10.0, 255.0) as u8;
        let mut bg_paint = Paint::default();
        bg_paint.set_color_rgba8(13, 14, 18, bg_alpha);
        if let Some(rect) = Rect::from_xywh(0.0, 0.0, width as f32, height as f32) {
            pixmap.fill_rect(rect, &bg_paint, Transform::identity(), None);
        }

        // 2. Render Header with tabs [💬 Чат] [🔍 Поиск] [⚙ Опции] [✕]
        self.render_header(&mut pixmap, ui, channel_name);

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

        // 4. Convert RGBA -> native mpv BGRA format
        let rgba_data = pixmap.data();
        let mut bgra_data = vec![0u8; rgba_data.len()];
        for (src, dst) in rgba_data.chunks_exact(4).zip(bgra_data.chunks_exact_mut(4)) {
            dst[0] = src[2]; // B
            dst[1] = src[1]; // G
            dst[2] = src[0]; // R
            dst[3] = src[3]; // A
        }

        Some(bgra_data)
    }

    fn render_header(&self, pixmap: &mut Pixmap, ui: &UiState, channel_name: &str) {
        let width = ui.panel_width as f32;
        let bar_h = 28.0;

        // Header background
        let mut header_paint = Paint::default();
        header_paint.set_color_rgba8(22, 23, 29, 245);
        if let Some(r) = Rect::from_xywh(0.0, 0.0, width, bar_h) {
            pixmap.fill_rect(r, &header_paint, Transform::identity(), None);
        }

        // Bottom border under header
        let mut border_paint = Paint::default();
        border_paint.set_color_rgba8(45, 45, 58, 255);
        if let Some(r) = Rect::from_xywh(0.0, bar_h - 1.0, width, 1.0) {
            pixmap.fill_rect(r, &border_paint, Transform::identity(), None);
        }

        // Tab: [💬 Чат]
        let is_chat = ui.current_view == UiView::Chat;
        self.draw_button(
            pixmap,
            &Self::btn_tab_chat(),
            "💬 Чат",
            is_chat,
            if is_chat { [145, 71, 255, 255] } else { [36, 37, 46, 255] },
            if is_chat { [255, 255, 255, 255] } else { [160, 160, 175, 255] },
        );

        // Tab: [🔍 Поиск]
        let is_search = ui.current_view == UiView::Search;
        self.draw_button(
            pixmap,
            &Self::btn_tab_search(),
            "🔍 Поиск",
            is_search,
            if is_search { [145, 71, 255, 255] } else { [36, 37, 46, 255] },
            if is_search { [255, 255, 255, 255] } else { [160, 160, 175, 255] },
        );

        // Tab: [⚙ Настройки]
        let is_settings = ui.current_view == UiView::Settings;
        self.draw_button(
            pixmap,
            &Self::btn_tab_settings(),
            "⚙ Опции",
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
                [191, 148, 255, 255],
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
        emotes: &mut EmoteCache,
        ui: &UiState,
    ) {
        let width = ui.panel_width as f32;
        let height = ui.panel_height as f32;
        let visible_top = 32.0;
        let visible_bottom = height - 4.0;

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
            let shifted_top = visible_top + msg.top_y;
            let shifted_bottom = visible_top + msg.bottom_y;

            if shifted_bottom < visible_top || shifted_top > visible_bottom {
                continue;
            }

            for token in &msg.tokens {
                match token {
                    LayoutToken::Text { text, color, x, y } => {
                        let render_y = visible_top + y;
                        if render_y + 16.0 >= visible_top && render_y <= visible_bottom {
                            self.draw_text(pixmap, text, *x, render_y, ui.font_size, *color);
                        }
                    }
                    LayoutToken::Image {
                        path,
                        x,
                        y,
                        width: _iw,
                        height: ih,
                    } => {
                        let render_y = visible_top + y;
                        if render_y + *ih as f32 >= visible_top && render_y <= visible_bottom {
                            if let Some(emote_px) = emotes.get_or_load(path, *ih) {
                                pixmap.draw_pixmap(
                                    *x as i32,
                                    render_y as i32,
                                    emote_px.as_ref(),
                                    &tiny_skia::PixmapPaint::default(),
                                    Transform::identity(),
                                    None,
                                );
                            }
                        }
                    }
                }
            }
        }

        // If scrolled away from live (either direction), render banner at bottom
        if ui.scroll_offset != 0 {
            let ban_h = 24.0;
            let ban_y = height - ban_h - 4.0;
            let mut ban_paint = Paint::default();
            ban_paint.set_color_rgba8(37, 24, 56, 240);
            if let Some(r) = Rect::from_xywh(4.0, ban_y, width - 8.0, ban_h) {
                pixmap.fill_rect(r, &ban_paint, Transform::identity(), None);
            }

            // Mirrors Lua: positive offset = scrolled back, negative = scrolled forward.
            let msg_text = if ui.scroll_offset > 0 {
                format!("⬇ Чат: назад (-{}) | Клик: к live", ui.scroll_offset)
            } else {
                format!("⬆ Чат: вперёд (+{}) | Клик: к live", -ui.scroll_offset)
            };
            self.draw_text(pixmap, &msg_text, 14.0, ban_y + 5.0, 11.0, [0, 255, 230, 255]);
        }
    }

    fn render_settings_view(&self, pixmap: &mut Pixmap, ui: &UiState) {
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
        let op_str = format!("Фон: {}%", op_pct);
        self.draw_button(
            pixmap,
            &HitBox::new(190.0, cur_y, 90.0, 22.0),
            &op_str,
            false,
            [35, 36, 46, 255],
            [0, 220, 255, 255],
        );
        cur_y += 34.0;

        // Row 4: Position Presets: [↖ ВЛ] [↙ НЛ] [↗ ВП] [↘ НП]
        self.draw_text(pixmap, "Позиция:", 12.0, cur_y + 3.0, 12.0, [200, 200, 220, 255]);
        let pos_labels = ["↖ ВЛ", "↙ НЛ", "↗ ВП", "↘ НП"];
        let mut px = 75.0;
        for plbl in &pos_labels {
            let hb = HitBox::new(px, cur_y, 48.0, 22.0);
            self.draw_button(pixmap, &hb, plbl, false, [32, 33, 42, 255], [240, 240, 255, 255]);
            px += 54.0;
        }
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
        cur_y += sync_box_h + 16.0;

        // Tips
        self.draw_text(
            pixmap,
            "Горячие клавиши: C - скрыть чат, S - настройки,",
            14.0,
            cur_y,
            11.0,
            [120, 120, 140, 255],
        );
        self.draw_text(
            pixmap,
            "Ctrl+F - поиск. Колесо мыши - скролл истории.",
            14.0,
            cur_y + 16.0,
            11.0,
            [120, 120, 140, 255],
        );
    }

    fn render_search_view(&self, pixmap: &mut Pixmap, ui: &UiState) {
        let width = ui.panel_width as f32;
        let height = ui.panel_height as f32;
        let mut cur_y = 36.0;

        // Search bar box
        let sb_w = width - 24.0;
        let sb_h = 28.0;
        let mut sbar_paint = Paint::default();
        sbar_paint.set_color_rgba8(28, 29, 38, 255);
        if let Some(r) = Rect::from_xywh(12.0, cur_y, sb_w, sb_h) {
            pixmap.fill_rect(r, &sbar_paint, Transform::identity(), None);
        }

        let q_display = if ui.search_query.is_empty() {
            "Поиск по автору или тексту (начните вводить)...".to_string()
        } else {
            format!("{}_", ui.search_query)
        };
        let q_color = if ui.search_query.is_empty() {
            [130, 130, 150, 255]
        } else {
            [255, 255, 255, 255]
        };
        self.draw_text(pixmap, &q_display, 18.0, cur_y + 7.0, 12.0, q_color);
        cur_y += sb_h + 8.0;

        // Quick Search Action Buttons: [RU ⇄ EN] [⌫ Стереть] [🗑 Очистить]
        self.draw_button(
            pixmap,
            &HitBox::new(12.0, cur_y, 76.0, 22.0),
            "RU ⇄ EN",
            false,
            [30, 42, 54, 255],
            [0, 210, 255, 255],
        );
        self.draw_button(
            pixmap,
            &HitBox::new(94.0, cur_y, 76.0, 22.0),
            "⌫ Стереть",
            false,
            [38, 38, 48, 255],
            [220, 220, 240, 255],
        );
        self.draw_button(
            pixmap,
            &HitBox::new(176.0, cur_y, 76.0, 22.0),
            "🗑 Очистить",
            false,
            [48, 32, 36, 255],
            [255, 140, 150, 255],
        );

        let count_str = format!("Найдено: {}", ui.search_results.len());
        self.draw_text(pixmap, &count_str, width - 110.0, cur_y + 4.0, 11.0, [145, 71, 255, 255]);
        cur_y += 30.0;

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

        let item_h = 36.0;
        let visible_items = ((height - cur_y) / item_h) as usize;
        let start_idx = ui.search_scroll_offset;
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
            self.draw_text(pixmap, &res.author, 84.0, item_y + 5.0, 11.0, nick_col);

            // Text preview
            let snip: String = res.text.chars().take(40).collect();
            self.draw_text(pixmap, &snip, 18.0, item_y + 18.0, 11.0, [220, 220, 230, 255]);
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
        mut start_x: f32,
        baseline_y: f32,
        font_size: f32,
        color: [u8; 4],
    ) {
        let font = self.layouter.font();
        let pw = pixmap.width() as i32;
        let ph = pixmap.height() as i32;
        let pixels = pixmap.data_mut();

        for c in text.chars() {
            let (metrics, bitmap) = font.rasterize(c, font_size);
            let gx = (start_x + metrics.xmin as f32).round() as i32;
            let gy = (baseline_y + (font_size - metrics.ymin as f32 - metrics.height as f32)).round()
                as i32;

            for y in 0..metrics.height {
                for x in 0..metrics.width {
                    let alpha_val = bitmap[y * metrics.width + x];
                    if alpha_val == 0 {
                        continue;
                    }

                    let px = gx + x as i32;
                    let py = gy + y as i32;

                    if px >= 0 && px < pw && py >= 0 && py < ph {
                        let idx = (py as usize * pw as usize + px as usize) * 4;
                        let src_a = (alpha_val as f32 / 255.0) * (color[3] as f32 / 255.0);
                        let dst_a = pixels[idx + 3] as f32 / 255.0;

                        let out_a = src_a + dst_a * (1.0 - src_a);
                        if out_a > 0.0 {
                            pixels[idx + 0] = (((color[0] as f32 * src_a)
                                + (pixels[idx + 0] as f32 * (1.0 - src_a)))
                                / out_a)
                                .min(255.0) as u8;
                            pixels[idx + 1] = (((color[1] as f32 * src_a)
                                + (pixels[idx + 1] as f32 * (1.0 - src_a)))
                                / out_a)
                                .min(255.0) as u8;
                            pixels[idx + 2] = (((color[2] as f32 * src_a)
                                + (pixels[idx + 2] as f32 * (1.0 - src_a)))
                                / out_a)
                                .min(255.0) as u8;
                            pixels[idx + 3] = (out_a * 255.0).min(255.0) as u8;
                        }
                    }
                }
            }

            start_x += metrics.advance_width;
        }
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
