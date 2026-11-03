//! Coverage-based text effects that do not change glyph advances.

use crate::ui::{clamp_outline_width, TextEffect};
use tiny_skia::Pixmap;

pub struct TextBitmap {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
    pub coverage: Vec<u8>,
}

impl TextBitmap {
    pub fn paint(
        &self,
        pixmap: &mut Pixmap,
        color: [u8; 4],
        effect: TextEffect,
        thickness: f32,
        clip: Option<(f32, f32)>,
    ) {
        match effect {
            TextEffect::None => {}
            TextEffect::Outline => {
                let radius = clamp_outline_width(thickness);
                let pad = radius.ceil() as usize;
                let width = self.width + pad * 2;
                let height = self.height + pad * 2;
                let mut mask = vec![0u8; width * height];
                let mut offsets = Vec::new();
                for dy in -(pad as i32)..=pad as i32 {
                    for dx in -(pad as i32)..=pad as i32 {
                        let distance = ((dx * dx + dy * dy) as f32).sqrt();
                        let coverage = (radius + 1.0 - distance).clamp(0.0, 1.0);
                        if coverage > 0.0 { offsets.push((dx, dy, coverage)); }
                    }
                }
                for y in 0..self.height {
                    for x in 0..self.width {
                        let alpha = self.coverage[y * self.width + x];
                        if alpha == 0 {
                            continue;
                        }
                        for &(dx, dy, coverage) in &offsets {
                            let value = (alpha as f32 * coverage).round() as u8;
                            let index = (y as i32 + pad as i32 + dy) as usize * width
                                + (x as i32 + pad as i32 + dx) as usize;
                            mask[index] = mask[index].max(value);
                        }
                    }
                }
                blend(
                    pixmap,
                    &mask,
                    width,
                    height,
                    self.x - pad as i32,
                    self.y - pad as i32,
                    [0, 0, 0, color[3]],
                    clip,
                );
            }
            TextEffect::Shadow => {
                let width = self.width + 2;
                let height = self.height + 2;
                let mut mask = vec![0u8; width * height];
                for y in 0..height {
                    for x in 0..width {
                        let mut sum = 0u32;
                        for dy in -1..=1 {
                            for dx in -1..=1 {
                                let sx = x as i32 - 1 + dx;
                                let sy = y as i32 - 1 + dy;
                                if sx >= 0
                                    && sy >= 0
                                    && sx < self.width as i32
                                    && sy < self.height as i32
                                {
                                    sum += self.coverage[sy as usize * self.width + sx as usize]
                                        as u32;
                                }
                            }
                        }
                        mask[y * width + x] = ((sum + 4) / 9) as u8;
                    }
                }
                blend(
                    pixmap,
                    &mask,
                    width,
                    height,
                    self.x + 1,
                    self.y + 1,
                    [0, 0, 0, (color[3] as f32 * 0.85).round() as u8],
                    clip,
                );
            }
        }
        blend(
            pixmap,
            &self.coverage,
            self.width,
            self.height,
            self.x,
            self.y,
            color,
            clip,
        );
    }
}

fn blend(
    pixmap: &mut Pixmap,
    mask: &[u8],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
    color: [u8; 4],
    clip: Option<(f32, f32)>,
) {
    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    let pixels = pixmap.data_mut();
    for sy in 0..height {
        let py = y + sy as i32;
        if py < 0
            || py >= ph
            || clip
                .map(|(top, bottom)| (py as f32) < top || py as f32 >= bottom)
                .unwrap_or(false)
        {
            continue;
        }
        for sx in 0..width {
            let px = x + sx as i32;
            let coverage = mask[sy * width + sx];
            if px < 0 || px >= pw || coverage == 0 {
                continue;
            }
            let alpha = coverage as f32 / 255.0 * color[3] as f32 / 255.0;
            let index = (py as usize * pw as usize + px as usize) * 4;
            for channel in 0..3 {
                pixels[index + channel] = (color[channel] as f32 * alpha
                    + pixels[index + channel] as f32 * (1.0 - alpha))
                    .round()
                    .min(255.0) as u8;
            }
            pixels[index + 3] = (255.0 * alpha + pixels[index + 3] as f32 * (1.0 - alpha))
                .round()
                .min(255.0) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph() -> TextBitmap {
        TextBitmap {
            x: 10,
            y: 10,
            width: 3,
            height: 3,
            coverage: vec![255; 9],
        }
    }

    fn alpha_count(pixmap: &Pixmap) -> usize {
        pixmap.data().chunks_exact(4).filter(|p| p[3] != 0).count()
    }

    #[test]
    fn outline_expands_coverage_and_keeps_text_color() {
        let glyph = glyph();
        let mut thin = Pixmap::new(30, 30).unwrap();
        let mut thick = Pixmap::new(30, 30).unwrap();
        glyph.paint(
            &mut thin,
            [240, 240, 240, 255],
            TextEffect::Outline,
            1.0,
            None,
        );
        glyph.paint(
            &mut thick,
            [240, 240, 240, 255],
            TextEffect::Outline,
            3.0,
            None,
        );
        assert!(alpha_count(&thick) > alpha_count(&thin));
        assert_eq!(
            &thick.data()[(11 * 30 + 11) * 4..(11 * 30 + 11) * 4 + 4],
            &[240, 240, 240, 255]
        );
    }

    #[test]
    fn half_pixel_outline_is_visible() {
        let mut plain = Pixmap::new(30, 30).unwrap();
        let mut outlined = Pixmap::new(30, 30).unwrap();
        glyph().paint(&mut plain, [255, 255, 255, 255], TextEffect::None, 0.5, None);
        glyph().paint(&mut outlined, [255, 255, 255, 255], TextEffect::Outline, 0.5, None);
        assert!(alpha_count(&outlined) > alpha_count(&plain));
    }

    #[test]
    fn shadow_is_offset_and_effects_respect_clip() {
        let glyph = glyph();
        let mut shadow = Pixmap::new(30, 30).unwrap();
        glyph.paint(
            &mut shadow,
            [255, 255, 255, 255],
            TextEffect::Shadow,
            1.5,
            None,
        );
        assert!(shadow.data()[(14 * 30 + 14) * 4 + 3] > 0);
        for effect in [TextEffect::None, TextEffect::Outline, TextEffect::Shadow] {
            let mut pixmap = Pixmap::new(30, 30).unwrap();
            glyph.paint(
                &mut pixmap,
                [255, 255, 255, 255],
                effect,
                4.0,
                Some((11.0, 13.0)),
            );
            assert!(pixmap.data()[..11 * 30 * 4].iter().all(|p| *p == 0));
            assert!(pixmap.data()[13 * 30 * 4..].iter().all(|p| *p == 0));
        }
    }

    #[test]
    fn output_is_premultiplied_with_transparent_text() {
        let mut pixmap = Pixmap::new(30, 30).unwrap();
        glyph().paint(
            &mut pixmap,
            [255, 120, 60, 120],
            TextEffect::Outline,
            2.0,
            None,
        );
        for p in pixmap.data().chunks_exact(4) {
            assert!(p[..3].iter().all(|channel| *channel <= p[3]));
        }
    }
}
