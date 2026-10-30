//! Fast 2D rasterizer producing native BGRA pixmaps for mpv overlay

use crate::emotes::EmoteCache;
use crate::layout::{ChatLayouter, LayoutMessage, LayoutToken};
use fontdue::Font;
use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};

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

    /// Renders chat panel to BGRA pixel buffer
    pub fn render_frame(
        &self,
        messages: &[LayoutMessage],
        emotes: &mut EmoteCache,
        width: u32,
        height: u32,
        bg_opacity: f32,
        scroll_offset_y: f32,
        is_paused: bool,
    ) -> Option<Vec<u8>> {
        let mut pixmap = Pixmap::new(width, height)?;

        // 1. Fill background panel with rounded rect / smooth color
        let bg_alpha = (bg_opacity * 255.0).clamp(0.0, 255.0) as u8;
        if bg_alpha > 0 {
            let mut paint = Paint::default();
            // Dark Twitch-themed panel (#18181b / #0e0e10)
            paint.set_color_rgba8(14, 14, 16, bg_alpha);
            let rect = Rect::from_xywh(0.0, 0.0, width as f32, height as f32)?;
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }

        // 2. Render visible chat messages
        let visible_top = 0.0;
        let visible_bottom = height as f32 - if is_paused { 36.0 } else { 0.0 };

        for msg in messages {
            let shifted_top = msg.top_y - scroll_offset_y;
            let shifted_bottom = msg.bottom_y - scroll_offset_y;

            // Frustum cull messages outside viewport
            if shifted_bottom < visible_top || shifted_top > visible_bottom {
                continue;
            }

            for token in &msg.tokens {
                match token {
                    LayoutToken::Text { text, color, x, y } => {
                        let render_y = y - scroll_offset_y;
                        if render_y + 20.0 >= visible_top && render_y <= visible_bottom {
                            self.draw_text(&mut pixmap, text, *x, render_y, 16.0, *color);
                        }
                    }
                    LayoutToken::Image { path, x, y, width: iw, height: ih } => {
                        let render_y = y - scroll_offset_y;
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

        // 3. Render paused banner at bottom if user scrolled up
        if is_paused {
            let banner_y = height as f32 - 32.0;
            let banner_rect = Rect::from_xywh(4.0, banner_y, width as f32 - 8.0, 28.0)?;
            let mut banner_paint = Paint::default();
            banner_paint.set_color_rgba8(145, 70, 255, 230); // Twitch purple
            pixmap.fill_rect(banner_rect, &banner_paint, Transform::identity(), None);

            self.draw_text(
                &mut pixmap,
                "Чат приостановлен | Клик: Вниз к live",
                16.0,
                banner_y + 6.0,
                13.0,
                [255, 255, 255, 255],
            );
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
