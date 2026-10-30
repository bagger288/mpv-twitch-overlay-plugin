//! Rich text layout and word-wrapping for Twitch chat messages

use crate::session::{ChatMessage, EmotesManifest};
use fontdue::Font;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum LayoutToken {
    Text {
        text: String,
        color: [u8; 4], // RGBA
        x: f32,
        y: f32,
    },
    Image {
        path: String,
        x: f32,
        y: f32,
        width: u32,
        height: u32,
    },
}

#[derive(Debug, Clone)]
pub struct LayoutMessage {
    pub msg_index: usize,
    pub timestamp_ms: i64,
    pub top_y: f32,
    pub bottom_y: f32,
    pub tokens: Vec<LayoutToken>,
}

pub struct ChatLayouter {
    font: Font,
}

impl ChatLayouter {
    pub fn new() -> Self {
        // Load default system font or clean embedded fallback
        let font_bytes = include_bytes!("../assets/Roboto-Regular.ttf");
        let font = Font::from_bytes(&font_bytes[..], fontdue::FontSettings::default())
            .expect("Failed to parse embedded font");

        Self { font }
    }

    pub fn with_font_bytes(bytes: &[u8]) -> Option<Self> {
        let font = Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()?;
        Some(Self { font })
    }

    pub fn measure_text_width(&self, text: &str, font_size: f32) -> f32 {
        let mut width = 0.0;
        for c in text.chars() {
            let metrics = self.font.metrics(c, font_size);
            width += metrics.advance_width;
        }
        width
    }

    /// Layouts messages into a vertical stream with word-wrapping and inline emotes
    pub fn layout_messages(
        &self,
        messages: &[ChatMessage],
        manifest: &EmotesManifest,
        panel_width: f32,
        font_size: f32,
        line_height: f32,
        emote_size: u32,
    ) -> Vec<LayoutMessage> {
        let mut result = Vec::with_capacity(messages.len());
        let margin_x = 12.0;
        let content_width = panel_width - (margin_x * 2.0);
        let mut cur_y = 8.0;

        // Build fast emote lookup table: word -> local_path
        let mut emote_map = HashMap::new();
        for (name, meta) in &manifest.channel_emotes {
            if let Some(ref p) = meta.local_path {
                emote_map.insert(name.as_str(), p.as_str());
            }
        }
        for (name, meta) in &manifest.global_emotes {
            if let Some(ref p) = meta.local_path {
                emote_map.insert(name.as_str(), p.as_str());
            }
        }

        for (idx, msg) in messages.iter().enumerate() {
            let top_y = cur_y;
            let mut cur_x = margin_x;
            let mut tokens = Vec::new();

            if msg.is_gap {
                // Gap divider
                tokens.push(LayoutToken::Text {
                    text: msg.text.clone(),
                    color: [180, 180, 180, 255],
                    x: margin_x,
                    y: cur_y,
                });
                cur_y += line_height * 1.2;
                result.push(LayoutMessage {
                    msg_index: idx,
                    timestamp_ms: msg.timestamp_ms,
                    top_y,
                    bottom_y: cur_y,
                    tokens,
                });
                continue;
            }

            // 1. Render badges
            let badge_size = emote_size.min(font_size.round() as u32);
            for b in &msg.badges {
                if let Some(versions) = manifest.badges.get(&b.name) {
                    if let Some(path) = versions.get(&b.version) {
                        tokens.push(LayoutToken::Image {
                            path: path.clone(),
                            x: cur_x,
                            y: cur_y + (line_height - badge_size as f32) * 0.5,
                            width: badge_size,
                            height: badge_size,
                        });
                        cur_x += badge_size as f32 + 3.0;
                    }
                }
            }

            // 2. Author nick
            let nick_color = parse_hex_color(&msg.color_hex);
            let author_text = format!("{}: ", msg.display_name);
            let author_w = self.measure_text_width(&author_text, font_size);

            tokens.push(LayoutToken::Text {
                text: author_text,
                color: nick_color,
                x: cur_x,
                y: cur_y,
            });
            cur_x += author_w;

            // 3. Message words & emotes (with word wrapping)
            let text_color = [240, 240, 245, 255];
            let space_w = self.measure_text_width(" ", font_size);

            for word in msg.text.split(' ') {
                if word.is_empty() {
                    continue;
                }

                if let Some(&emote_path) = emote_map.get(word) {
                    // Inline emote
                    let ew = emote_size as f32;
                    if cur_x + ew > margin_x + content_width && cur_x > margin_x {
                        // Wrap to next line
                        cur_x = margin_x;
                        cur_y += line_height;
                    }

                    tokens.push(LayoutToken::Image {
                        path: emote_path.to_string(),
                        x: cur_x,
                        y: cur_y + (line_height - ew) * 0.5,
                        width: emote_size,
                        height: emote_size,
                    });
                    cur_x += ew + space_w;
                } else {
                    // Plain text word
                    let ww = self.measure_text_width(word, font_size);
                    if cur_x + ww > margin_x + content_width && cur_x > margin_x {
                        // Wrap to next line
                        cur_x = margin_x;
                        cur_y += line_height;
                    }

                    tokens.push(LayoutToken::Text {
                        text: word.to_string(),
                        color: text_color,
                        x: cur_x,
                        y: cur_y,
                    });
                    cur_x += ww + space_w;
                }
            }

            cur_y += line_height + 4.0; // message gap
            result.push(LayoutMessage {
                msg_index: idx,
                timestamp_ms: msg.timestamp_ms,
                top_y,
                bottom_y: cur_y,
                tokens,
            });
        }

        result
    }

    pub fn font(&self) -> &Font {
        &self.font
    }
}

fn parse_hex_color(hex: &str) -> [u8; 4] {
    let clean = hex.trim_start_matches('#');
    if clean.len() >= 6 {
        let r = u8::from_str_radix(&clean[0..2], 16).unwrap_or(255);
        let g = u8::from_str_radix(&clean[2..4], 16).unwrap_or(255);
        let b = u8::from_str_radix(&clean[4..6], 16).unwrap_or(255);
        [r, g, b, 255]
    } else {
        [145, 70, 255, 255] // Default Twitch purple
    }
}
