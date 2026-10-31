//! Fast 2D rasterizer producing native BGRA pixmaps for mpv overlay
//! Renders chat messages, interactive control bar, settings toolbar, and search overlay.

use crate::emotes::EmoteCache;
use crate::layout::{ChatLayouter, LayoutMessage, LayoutToken};
use crate::ui::{UiState, QUICK_SEARCH_TAGS};
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

    /// Renders chat panel, control bars and search modal to BGRA pixel buffer
    pub fn render_frame(
        &self,
        messages: &[LayoutMessage],
        emotes: &mut EmoteCache,
        ui: &UiState,
        channel_name: &str,
    ) -> Option<Vec<u8>> {
        let width = ui.panel_width;
        let height = ui.panel_height;
        let mut pixmap = Pixmap::new(width, height)?;

        // 1. Fill background panel with rounded rect / smooth color
        let bg_alpha = (ui.bg_opacity * 255.0).clamp(0.0, 255.0) as u8;
        if bg_alpha > 0 {
            let mut paint = Paint::default();
            paint.set_color_rgba8(14, 14, 16, bg_alpha);
            let rect = Rect::from_xywh(0.0, 0.0, width as f32, height as f32)?;
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }

        // 2. Render Top Control Bar (Header)
        let bar_h = 28.0;
        let mut bar_paint = Paint::default();
        bar_paint.set_color_rgba8(26, 26, 32, 240);
        if let Some(r) = Rect::from_xywh(0.0, 0.0, width as f32, bar_h) {
            pixmap.fill_rect(r, &bar_paint, Transform::identity(), None);
        }

        // Header Title & Channel
        let title_text = if !channel_name.is_empty() {
            format!("Twitch: {}", channel_name)
        } else {
            "Twitch Chat".to_string()
        };
        self.draw_text(&mut pixmap, &title_text, 8.0, 6.0, 13.0, [191, 148, 255, 255]);

        // Offset indicator: e.g. "+0.0s"
        let offset_sec = ui.offset_ms as f32 / 1000.0;
        let offset_str = format!("{:+0.1}s", offset_sec);
        let off_color = if ui.offset_ms == 0 {
            [160, 160, 170, 255]
        } else {
            [0, 255, 200, 255]
        };
        self.draw_button(&mut pixmap, &offset_str, 130.0, 4.0, 48.0, 20.0, [36, 36, 44, 255], off_color, 11.0);

        // Header Buttons: [ 🔍 Поиск ] [ ⚓ Синхр ] [ -1s ] [ +1s ] [ ⚙ ]
        self.draw_button(&mut pixmap, "🔍 Поиск", 184.0, 4.0, 56.0, 20.0, [45, 30, 65, 255], [0, 210, 255, 255], 11.0);
        self.draw_button(&mut pixmap, "⚓ Синхр", 244.0, 4.0, 52.0, 20.0, [26, 43, 60, 255], [0, 255, 200, 255], 11.0);
        self.draw_button(&mut pixmap, "-1s", 300.0, 4.0, 30.0, 20.0, [36, 36, 44, 255], [255, 255, 255, 255], 11.0);
        self.draw_button(&mut pixmap, "+1s", 334.0, 4.0, 30.0, 20.0, [36, 36, 44, 255], [255, 255, 255, 255], 11.0);
        
        let settings_bg = if ui.show_settings { [145, 70, 255, 255] } else { [36, 36, 44, 255] };
        self.draw_button(&mut pixmap, "⚙", width as f32 - 28.0, 4.0, 22.0, 20.0, settings_bg, [255, 255, 255, 255], 12.0);

        // 3. Render Settings Drawer (if expanded)
        let mut content_top = bar_h + 4.0;
        if ui.show_settings {
            let drawer_h = 56.0;
            let mut drawer_paint = Paint::default();
            drawer_paint.set_color_rgba8(22, 22, 28, 245);
            if let Some(r) = Rect::from_xywh(0.0, bar_h, width as f32, drawer_h) {
                pixmap.fill_rect(r, &drawer_paint, Transform::identity(), None);
            }

            // Row 1: Fine-tune offset [-10s] [-1s] [-0.1s] [+0.1s] [+1s] [+10s] [0s]
            let offset_btns = ["-10s", "-1s", "-0.1s", "+0.1s", "+1s", "+10s", "0s"];
            let mut bx = 8.0;
            for &lbl in &offset_btns {
                self.draw_button(&mut pixmap, lbl, bx, bar_h + 4.0, 38.0, 20.0, [40, 40, 50, 255], [240, 240, 250, 255], 10.0);
                bx += 42.0;
            }

            // Row 2: Font scale [A-] [A+], Size [W±] [H±], Opacity [Фон: XX%], Presets
            self.draw_button(&mut pixmap, "A-", 8.0, bar_h + 28.0, 26.0, 20.0, [45, 45, 55, 255], [255, 255, 255, 255], 11.0);
            self.draw_button(&mut pixmap, "A+", 38.0, bar_h + 28.0, 26.0, 20.0, [45, 45, 55, 255], [255, 255, 255, 255], 11.0);
            self.draw_button(&mut pixmap, "W-", 70.0, bar_h + 28.0, 26.0, 20.0, [45, 45, 55, 255], [255, 255, 255, 255], 11.0);
            self.draw_button(&mut pixmap, "W+", 100.0, bar_h + 28.0, 26.0, 20.0, [45, 45, 55, 255], [255, 255, 255, 255], 11.0);
            self.draw_button(&mut pixmap, "H-", 132.0, bar_h + 28.0, 26.0, 20.0, [45, 45, 55, 255], [255, 255, 255, 255], 11.0);
            self.draw_button(&mut pixmap, "H+", 162.0, bar_h + 28.0, 26.0, 20.0, [45, 45, 55, 255], [255, 255, 255, 255], 11.0);

            // Presets
            self.draw_button(&mut pixmap, "↖", 198.0, bar_h + 28.0, 24.0, 20.0, [45, 45, 55, 255], [255, 255, 255, 255], 11.0);
            self.draw_button(&mut pixmap, "↙", 226.0, bar_h + 28.0, 24.0, 20.0, [45, 45, 55, 255], [255, 255, 255, 255], 11.0);
            self.draw_button(&mut pixmap, "↗", 254.0, bar_h + 28.0, 24.0, 20.0, [45, 45, 55, 255], [255, 255, 255, 255], 11.0);
            self.draw_button(&mut pixmap, "↘", 282.0, bar_h + 28.0, 24.0, 20.0, [45, 45, 55, 255], [255, 255, 255, 255], 11.0);

            // Opacity toggle
            let op_label = format!("Фон: {}%", (ui.bg_opacity * 100.0).round() as i32);
            self.draw_button(&mut pixmap, &op_label, 314.0, bar_h + 28.0, 68.0, 20.0, [45, 45, 55, 255], [255, 255, 255, 255], 10.0);

            content_top += drawer_h;
        }

        // 4. Render visible chat messages
        let visible_top = content_top;
        let visible_bottom = height as f32 - if ui.is_paused { 34.0 } else { 4.0 };

        if messages.is_empty() {
            self.draw_text(&mut pixmap, "Ожидание сообщений чата...", 14.0, visible_top + 16.0, 13.0, [140, 140, 150, 255]);
        }

        for msg in messages {
            let shifted_top = visible_top + msg.top_y - ui.scroll_offset_y;
            let shifted_bottom = visible_top + msg.bottom_y - ui.scroll_offset_y;

            if shifted_bottom < visible_top || shifted_top > visible_bottom {
                continue;
            }

            for token in &msg.tokens {
                match token {
                    LayoutToken::Text { text, color, x, y } => {
                        let render_y = visible_top + y - ui.scroll_offset_y;
                        if render_y + 16.0 >= visible_top && render_y <= visible_bottom {
                            self.draw_text(&mut pixmap, text, *x, render_y, ui.font_size, *color);
                        }
                    }
                    LayoutToken::Image { path, x, y, width: _iw, height: ih } => {
                        let render_y = visible_top + y - ui.scroll_offset_y;
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

        // 5. Render paused banner at bottom if user scrolled up
        if ui.is_paused {
            let banner_y = height as f32 - 30.0;
            if let Some(banner_rect) = Rect::from_xywh(4.0, banner_y, width as f32 - 8.0, 26.0) {
                let mut banner_paint = Paint::default();
                banner_paint.set_color_rgba8(145, 70, 255, 235);
                pixmap.fill_rect(banner_rect, &banner_paint, Transform::identity(), None);
            }

            self.draw_text(
                &mut pixmap,
                "⬇ Чат приостановлен | Клик: Вниз к live",
                16.0,
                banner_y + 6.0,
                12.0,
                [255, 255, 255, 255],
            );
        }

        // 6. Render Search Modal (if open)
        if ui.search_visible {
            let modal_w = (width as f32 - 16.0).min(420.0);
            let modal_h = (height as f32 - 36.0).min(380.0);
            let mx = (width as f32 - modal_w) * 0.5;
            let my = 32.0;

            // Modal Background & Border
            let mut modal_paint = Paint::default();
            modal_paint.set_color_rgba8(20, 20, 26, 252);
            if let Some(r) = Rect::from_xywh(mx, my, modal_w, modal_h) {
                pixmap.fill_rect(r, &modal_paint, Transform::identity(), None);
            }

            // Modal Header
            let search_header = format!("🔍 Поиск по чату: \"{}\"", ui.search_query);
            self.draw_text(&mut pixmap, &search_header, mx + 10.0, my + 8.0, 13.0, [0, 210, 255, 255]);
            self.draw_button(&mut pixmap, "✕", mx + modal_w - 26.0, my + 6.0, 20.0, 20.0, [50, 30, 30, 255], [255, 120, 120, 255], 12.0);

            // Quick Tag Chips
            let mut tag_x = mx + 8.0;
            let tag_y = my + 32.0;
            for &tag in QUICK_SEARCH_TAGS.iter().take(7) {
                let is_active = ui.search_query == tag;
                let chip_bg = if is_active { [145, 70, 255, 255] } else { [35, 35, 45, 255] };
                let chip_text_color = if is_active { [255, 255, 255, 255] } else { [200, 200, 210, 255] };
                let chip_w = self.layouter.measure_text_width(tag, 10.0) + 12.0;
                self.draw_button(&mut pixmap, tag, tag_x, tag_y, chip_w, 18.0, chip_bg, chip_text_color, 10.0);
                tag_x += chip_w + 4.0;
            }

            // Search Results List
            let res_top = my + 56.0;
            let res_item_h = 28.0;
            let start_idx = ui.search_page * ui.search_page_size;
            let end_idx = (start_idx + ui.search_page_size).min(ui.search_results.len());

            if ui.search_results.is_empty() {
                self.draw_text(&mut pixmap, "Сообщений не найдено. Нажмите тег выше.", mx + 14.0, res_top + 20.0, 12.0, [150, 150, 160, 255]);
            } else {
                for (i, item) in ui.search_results[start_idx..end_idx].iter().enumerate() {
                    let ry = res_top + (i as f32 * res_item_h);
                    
                    // Subtle item background
                    let mut item_paint = Paint::default();
                    item_paint.set_color_rgba8(28, 28, 36, 200);
                    if let Some(r) = Rect::from_xywh(mx + 6.0, ry, modal_w - 12.0, res_item_h - 2.0) {
                        pixmap.fill_rect(r, &item_paint, Transform::identity(), None);
                    }

                    // Timestamp chip: [HH:MM:SS]
                    self.draw_text(&mut pixmap, &format!("[{}]", item.time_str), mx + 10.0, ry + 6.0, 10.0, [0, 255, 200, 255]);

                    // Author:
                    let nick_str = format!("{}:", item.author);
                    self.draw_text(&mut pixmap, &nick_str, mx + 74.0, ry + 6.0, 11.0, [191, 148, 255, 255]);

                    // Text snippet:
                    let author_w = self.layouter.measure_text_width(&nick_str, 11.0);
                    let snippet: String = item.text.chars().take(34).collect();
                    self.draw_text(&mut pixmap, &snippet, mx + 78.0 + author_w, ry + 6.0, 11.0, [240, 240, 245, 255]);
                }
            }

            // Pagination Footer: [ < Назад ]  Стр. 1/4  [ Вперёд > ]
            let foot_y = my + modal_h - 26.0;
            let total_pages = ((ui.search_results.len().saturating_sub(1)) / ui.search_page_size) + 1;
            let page_info = format!("Стр. {}/{}", ui.search_page + 1, total_pages.max(1));

            self.draw_button(&mut pixmap, "< Назад", mx + 8.0, foot_y, 56.0, 20.0, [40, 40, 50, 255], [240, 240, 250, 255], 10.0);
            self.draw_text(&mut pixmap, &page_info, mx + (modal_w * 0.5) - 20.0, foot_y + 4.0, 11.0, [180, 180, 190, 255]);
            self.draw_button(&mut pixmap, "Вперёд >", mx + modal_w - 68.0, foot_y, 60.0, 20.0, [40, 40, 50, 255], [240, 240, 250, 255], 10.0);
        }

        // Convert RGBA -> native mpv BGRA format
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

    fn draw_button(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        bg_color: [u8; 4],
        text_color: [u8; 4],
        font_size: f32,
    ) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(bg_color[0], bg_color[1], bg_color[2], bg_color[3]);
        if let Some(rect) = Rect::from_xywh(x, y, w, h) {
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
        let text_w = self.layouter.measure_text_width(text, font_size);
        let text_x = x + (w - text_w).max(0.0) * 0.5;
        let text_y = y + (h - font_size) * 0.5;
        self.draw_text(pixmap, text, text_x, text_y, font_size, text_color);
    }

    fn draw_text(
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
            let gy = (baseline_y + (font_size - metrics.ymin as f32 - metrics.height as f32)).round() as i32;

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
                            pixels[idx + 0] = (((color[0] as f32 * src_a) + (pixels[idx + 0] as f32 * (1.0 - src_a))) / out_a).min(255.0) as u8;
                            pixels[idx + 1] = (((color[1] as f32 * src_a) + (pixels[idx + 1] as f32 * (1.0 - src_a))) / out_a).min(255.0) as u8;
                            pixels[idx + 2] = (((color[2] as f32 * src_a) + (pixels[idx + 2] as f32 * (1.0 - src_a))) / out_a).min(255.0) as u8;
                            pixels[idx + 3] = (out_a * 255.0).min(255.0) as u8;
                        }
                    }
                }
            }

            start_x += metrics.advance_width;
        }
    }
}
