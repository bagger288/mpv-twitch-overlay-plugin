//! Rich text layout and word-wrapping for Twitch chat messages
//! Fully compatible with renderer.py: supports badges, native Twitch emotes, and 3rd-party emotes (7TV, BTTV, FFZ).

use crate::emotes::EmoteCache;
use crate::session::{parse_twitch_emotes_tag, ChatMessage, EmotesManifest};
use fontdue::Font;

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

enum ContentToken {
    Text(String),
    Emote { path: String, fallback_text: String },
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

    /// Layouts messages into a vertical stream with word-wrapping, badges, and inline emotes
    pub fn layout_messages(
        &self,
        messages: &[ChatMessage],
        manifest: &EmotesManifest,
        emotes: &EmoteCache,
        panel_width: f32,
        font_size: f32,
        emotes_enabled: bool,
    ) -> Vec<LayoutMessage> {
        let mut result = Vec::with_capacity(messages.len());
        let margin_x = 12.0;
        let content_width = panel_width - (margin_x * 2.0);
        let mut cur_y = 8.0;

        // Proportional sizing matching renderer.py lines 314-316
        let badge_size = (font_size * 1.15).round().max(14.0) as u32;
        let emote_size = (font_size * 1.35).round().max(16.0) as u32;
        let line_height = (font_size * 1.62).round().max(20.0);
        let space_w = self.measure_text_width(" ", font_size).max(4.0);

        for (idx, msg) in messages.iter().enumerate() {
            let top_y = cur_y;
            let mut cur_x = margin_x;
            let mut tokens = Vec::new();

            if msg.is_gap {
                // Gap divider
                tokens.push(LayoutToken::Text {
                    text: msg.text.clone(),
                    color: [255, 136, 0, 255],
                    x: margin_x,
                    y: cur_y + (line_height - font_size) * 0.5,
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

            // 1. Render badges (if emotes_enabled is true)
            if emotes_enabled {
                for b in &msg.badges {
                    let badge_path = manifest.get_badge_path(&b.name, &b.version);
                    let bw = badge_size as f32;
                    tokens.push(LayoutToken::Image {
                        path: badge_path,
                        x: cur_x,
                        y: cur_y + (line_height - bw) * 0.5,
                        width: badge_size,
                        height: badge_size,
                    });
                    cur_x += bw + 4.0;
                }
            }

            // 2. Author nick
            let nick_color = parse_hex_color(&msg.color_hex, &msg.display_name);
            let author_text = format!("{}: ", msg.display_name);
            let author_w = self.measure_text_width(&author_text, font_size);

            tokens.push(LayoutToken::Text {
                text: author_text,
                color: nick_color,
                x: cur_x,
                y: cur_y + (line_height - font_size) * 0.5,
            });
            cur_x += author_w;

            // 3. Message words & emotes (with word wrapping)
            let text_color = [245, 245, 245, 255];
            let parsed_tokens = self.tokenize_message(&msg.text, &msg.twitch_emotes_raw, manifest, emotes_enabled);

            for tok in parsed_tokens {
                match tok {
                    ContentToken::Emote { path, fallback_text: _ } => {
                        let ew = emotes.get_width(&path, emote_size) as f32;
                        if cur_x + ew > margin_x + content_width && cur_x > margin_x {
                            // Wrap to next line
                            cur_x = margin_x;
                            cur_y += line_height;
                        }

                        tokens.push(LayoutToken::Image {
                            path,
                            x: cur_x,
                            y: cur_y + (line_height - emote_size as f32) * 0.5,
                            width: ew.round() as u32,
                            height: emote_size,
                        });
                        cur_x += ew + space_w;
                    }
                    ContentToken::Text(word) => {
                        let ww = self.measure_text_width(&word, font_size);
                        if cur_x + ww > margin_x + content_width && cur_x > margin_x {
                            // Wrap to next line
                            cur_x = margin_x;
                            cur_y += line_height;
                        }

                        tokens.push(LayoutToken::Text {
                            text: word,
                            color: text_color,
                            x: cur_x,
                            y: cur_y + (line_height - font_size) * 0.5,
                        });
                        cur_x += ww + space_w;
                    }
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

    /// Splits message text into words and inline emotes (Twitch native + 7TV/BTTV/FFZ)
    fn tokenize_message(
        &self,
        text: &str,
        twitch_emotes_raw: &str,
        manifest: &EmotesManifest,
        emotes_enabled: bool,
    ) -> Vec<ContentToken> {
        let mut tokens = Vec::new();
        if text.is_empty() {
            return tokens;
        }

        if !emotes_enabled {
            for word in text.split(' ') {
                if !word.is_empty() {
                    tokens.push(ContentToken::Text(word.to_string()));
                }
            }
            return tokens;
        }

        let twitch_spans = parse_twitch_emotes_tag(twitch_emotes_raw);

        if twitch_spans.is_empty() {
            // No native Twitch emotes, split words and check 3rd-party emote manifest
            for word in text.split(' ') {
                if word.is_empty() {
                    continue;
                }
                if let Some(emote_path) = manifest.get_word_emote_path(word) {
                    tokens.push(ContentToken::Emote {
                        path: emote_path,
                        fallback_text: word.to_string(),
                    });
                } else {
                    tokens.push(ContentToken::Text(word.to_string()));
                }
            }
            return tokens;
        }

        // Handle native Twitch emote spans by character index
        let chars: Vec<char> = text.chars().collect();
        let total_chars = chars.len();
        let mut cur_idx = 0;

        for span in twitch_spans {
            if span.start >= total_chars {
                break;
            }
            let valid_start = span.start.min(total_chars);
            let valid_end = span.end.min(total_chars - 1);

            if valid_start > cur_idx {
                let chunk: String = chars[cur_idx..valid_start].iter().collect();
                for word in chunk.split(' ') {
                    if word.is_empty() {
                        continue;
                    }
                    if let Some(emote_path) = manifest.get_word_emote_path(word) {
                        tokens.push(ContentToken::Emote {
                            path: emote_path,
                            fallback_text: word.to_string(),
                        });
                    } else {
                        tokens.push(ContentToken::Text(word.to_string()));
                    }
                }
            }

            let emote_text: String = chars[valid_start..=valid_end].iter().collect();
            tokens.push(ContentToken::Emote {
                path: format!("twitch/{}.png", span.emote_id),
                fallback_text: emote_text,
            });
            cur_idx = valid_end + 1;
        }

        if cur_idx < total_chars {
            let chunk: String = chars[cur_idx..].iter().collect();
            for word in chunk.split(' ') {
                if word.is_empty() {
                    continue;
                }
                if let Some(emote_path) = manifest.get_word_emote_path(word) {
                    tokens.push(ContentToken::Emote {
                        path: emote_path,
                        fallback_text: word.to_string(),
                    });
                } else {
                    tokens.push(ContentToken::Text(word.to_string()));
                }
            }
        }

        tokens
    }

    pub fn font(&self) -> &Font {
        &self.font
    }
}

/// Parses hex color or derives deterministic nick color identical to renderer.py hash_to_rgb
fn parse_hex_color(hex: &str, fallback_name: &str) -> [u8; 4] {
    let clean = hex.trim_start_matches('#');
    if clean.len() >= 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&clean[0..2], 16),
            u8::from_str_radix(&clean[2..4], 16),
            u8::from_str_radix(&clean[4..6], 16),
        ) {
            return [r, g, b, 255];
        }
    }

    // Matching renderer.py hash_to_rgb
    let mut h: u32 = 0;
    for byte in fallback_name.bytes() {
        h = (h.wrapping_mul(31).wrapping_add(byte as u32)) % 16777215;
    }
    let mut r = ((h >> 16) & 0xFF) as u8;
    let mut g = ((h >> 8) & 0xFF) as u8;
    let mut b = (h & 0xFF) as u8;
    if (r as f32 * 0.299 + g as f32 * 0.587 + b as f32 * 0.114) < 75.0 {
        r = r.saturating_add(90);
        g = g.saturating_add(90);
        b = b.saturating_add(90);
    }
    [r, g, b, 255]
}
