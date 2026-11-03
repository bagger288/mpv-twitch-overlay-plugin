//! Inline chat layout matching the standard Twitch message view.

use crate::emotes::EmoteCache;
use crate::session::{parse_twitch_emotes_tag, ChatMessage, EmotesManifest};
use fontdue::Font;

pub const MESSAGE_PADDING_X: f32 = 16.0;
pub const MESSAGE_PADDING_Y: f32 = 4.0;
pub const BADGE_SIZE: u32 = 18;
pub const BADGE_GAP: f32 = 3.0;
pub const EMOTE_SIZE: u32 = 28;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TextStyle {
    Ui,
    Message,
    Nickname,
}

#[derive(Debug, Clone)]
pub enum LayoutToken {
    Text {
        text: String,
        color: [u8; 4],
        style: TextStyle,
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

#[derive(Debug)]
enum ContentToken {
    Text(String),
    Space,
    Emote {
        path: String,
        fallback_text: String,
        native: bool,
    },
}

pub struct ChatLayouter {
    font: Font,
    fallback_bold: Font,
    message_font: Font,
    nickname_font: Font,
    cyrillic_font: Font,
    cyrillic_bold: Font,
}

impl ChatLayouter {
    pub fn new() -> Self {
        fn load(bytes: &[u8]) -> Font {
            Font::from_bytes(bytes, fontdue::FontSettings::default())
                .expect("Failed to parse embedded font")
        }
        Self {
            font: load(include_bytes!("../assets/DejaVuSans.ttf")),
            fallback_bold: load(include_bytes!("../assets/DejaVuSans-Bold.ttf")),
            message_font: load(include_bytes!("../assets/twitch-inter-latin-400.ttf")),
            nickname_font: load(include_bytes!("../assets/twitch-inter-latin-700.ttf")),
            cyrillic_font: load(include_bytes!("../assets/twitch-inter-cyrillic-400.ttf")),
            cyrillic_bold: load(include_bytes!("../assets/twitch-inter-cyrillic-700.ttf")),
        }
    }

    pub fn with_font_bytes(bytes: &[u8]) -> Option<Self> {
        let font = Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()?;
        let mut layouter = Self::new();
        layouter.font = font;
        Some(layouter)
    }

    pub fn font_for_char(&self, character: char, style: TextStyle) -> &Font {
        let cyrillic = if style == TextStyle::Nickname {
            &self.cyrillic_bold
        } else {
            &self.cyrillic_font
        };
        if matches!(character as u32, 0x0400..=0x052f | 0x1c80..=0x1c8f | 0x2de0..=0x2dff | 0xa640..=0xa69f | 0x2116)
            && cyrillic.lookup_glyph_index(character) != 0
        {
            return cyrillic;
        }
        let font = match style {
            TextStyle::Ui => &self.font,
            TextStyle::Message => &self.message_font,
            TextStyle::Nickname => &self.nickname_font,
        };
        if font.lookup_glyph_index(character) != 0 {
            font
        } else if style == TextStyle::Nickname {
            &self.fallback_bold
        } else {
            &self.font
        }
    }

    pub fn kerning(&self, previous: char, current: char, size: f32, style: TextStyle) -> f32 {
        let font = self.font_for_char(current, style);
        if std::ptr::eq(self.font_for_char(previous, style), font) {
            font.horizontal_kern(previous, current, size).unwrap_or(0.0)
        } else {
            0.0
        }
    }

    pub fn measure_text_width(&self, text: &str, size: f32) -> f32 {
        self.measure_styled_text_width(text, size, TextStyle::Ui)
    }

    pub fn measure_styled_text_width(&self, text: &str, size: f32, style: TextStyle) -> f32 {
        let mut width = 0.0;
        let mut previous = None;
        for c in text.chars() {
            if let Some(p) = previous {
                width += self.kerning(p, c, size, style);
            }
            width += self.font_for_char(c, style).metrics(c, size).advance_width;
            previous = Some(c);
        }
        width
    }

    pub fn layout_messages(
        &self,
        messages: &[ChatMessage],
        manifest: &EmotesManifest,
        emotes: &EmoteCache,
        panel_width: f32,
        font_size: f32,
        emotes_enabled: bool,
    ) -> Vec<LayoutMessage> {
        self.layout_messages_with_icons(messages, manifest, emotes, panel_width, font_size, emotes_enabled, emotes_enabled)
    }

    pub fn layout_messages_with_icons(
        &self,
        messages: &[ChatMessage],
        manifest: &EmotesManifest,
        emotes: &EmoteCache,
        panel_width: f32,
        font_size: f32,
        emotes_enabled: bool,
        badges_enabled: bool,
    ) -> Vec<LayoutMessage> {
        let mut result = Vec::with_capacity(messages.len());
        let mut y = 0.0;
        for (index, msg) in messages.iter().enumerate() {
            let top_y = y;
            let mut line =
                MessageLines::new(self, panel_width, font_size, top_y + MESSAGE_PADDING_Y);
            if !msg.is_gap {
                if badges_enabled {
                    for badge in &msg.badges {
                        let path = manifest.get_badge_path(&badge.name, &badge.version);
                        line.image(emotes, path, BADGE_SIZE, true, true);
                    }
                }
                line.text(
                    &msg.display_name,
                    TextStyle::Nickname,
                    parse_hex_color(&msg.color_hex, &msg.display_name),
                );
                line.text(":", TextStyle::Message, [239, 239, 241, 255]);
                line.space();
            }
            let color = if msg.is_gap {
                [255, 136, 0, 255]
            } else {
                [239, 239, 241, 255]
            };
            for token in
                self.tokenize_message(&msg.text, &msg.twitch_emotes_raw, manifest, emotes_enabled)
            {
                match token {
                    ContentToken::Text(text) => line.text(&text, TextStyle::Message, color),
                    ContentToken::Space => line.space(),
                    ContentToken::Emote {
                        path,
                        fallback_text,
                        native,
                    } => {
                        if emotes.resolve_path(&path).is_some() {
                            line.image(emotes, path, EMOTE_SIZE, native, false);
                        } else {
                            line.text(&fallback_text, TextStyle::Message, color);
                        }
                    }
                }
            }
            line.finish_line();
            y = line.y + MESSAGE_PADDING_Y;
            result.push(LayoutMessage {
                msg_index: index,
                timestamp_ms: msg.timestamp_ms,
                top_y,
                bottom_y: y,
                tokens: line.finished,
            });
        }
        result
    }

    fn tokenize_message(
        &self,
        text: &str,
        raw: &str,
        manifest: &EmotesManifest,
        enabled: bool,
    ) -> Vec<ContentToken> {
        fn chunk(
            text: &str,
            manifest: &EmotesManifest,
            enabled: bool,
            tokens: &mut Vec<ContentToken>,
        ) {
            let mut word = String::new();
            fn flush(
                word: &mut String,
                manifest: &EmotesManifest,
                enabled: bool,
                tokens: &mut Vec<ContentToken>,
            ) {
                if word.is_empty() {
                    return;
                }
                let text = std::mem::take(word);
                if let Some(path) = enabled
                    .then(|| manifest.get_word_emote_path(&text))
                    .flatten()
                {
                    tokens.push(ContentToken::Emote {
                        path,
                        fallback_text: text,
                        native: false,
                    });
                } else {
                    tokens.push(ContentToken::Text(text));
                }
            }
            for c in text.chars() {
                if c.is_ascii_whitespace() {
                    flush(&mut word, manifest, enabled, tokens);
                    if !matches!(tokens.last(), Some(ContentToken::Space)) {
                        tokens.push(ContentToken::Space);
                    }
                } else {
                    word.push(c);
                }
            }
            flush(&mut word, manifest, enabled, tokens);
        }
        let mut tokens = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        let mut cursor = 0;
        if enabled {
            for span in parse_twitch_emotes_tag(raw) {
                if span.start >= chars.len() {
                    break;
                }
                if span.start < cursor {
                    continue;
                }
                if span.start > cursor {
                    chunk(
                        &chars[cursor..span.start].iter().collect::<String>(),
                        manifest,
                        enabled,
                        &mut tokens,
                    );
                }
                let end = span.end.min(chars.len() - 1);
                tokens.push(ContentToken::Emote {
                    path: format!("twitch/{}.png", span.emote_id),
                    fallback_text: chars[span.start..=end].iter().collect(),
                    native: true,
                });
                cursor = end + 1;
            }
        }
        if cursor < chars.len() {
            chunk(
                &chars[cursor..].iter().collect::<String>(),
                manifest,
                enabled,
                &mut tokens,
            );
        }
        tokens
    }

    pub fn font(&self) -> &Font {
        &self.font
    }
}

struct MessageLines<'a> {
    layouter: &'a ChatLayouter,
    size: f32,
    right: f32,
    x: f32,
    y: f32,
    above: f32,
    below: f32,
    text_above: f32,
    text_below: f32,
    x_height: f32,
    pending_space: bool,
    line: Vec<(LayoutToken, f32)>,
    finished: Vec<LayoutToken>,
}

impl<'a> MessageLines<'a> {
    fn new(layouter: &'a ChatLayouter, width: f32, size: f32, y: f32) -> Self {
        let metrics = layouter.message_font.horizontal_line_metrics(size).unwrap();
        let ascent = metrics.ascent.ceil();
        let descent = (-metrics.descent).floor();
        let height = size * 22.0 / 14.0;
        let above = ascent + ((height - ascent - descent) * 0.5).floor();
        Self {
            layouter,
            size,
            right: (width - MESSAGE_PADDING_X).max(MESSAGE_PADDING_X + 1.0),
            x: MESSAGE_PADDING_X,
            y,
            above,
            below: height - above,
            text_above: above,
            text_below: height - above,
            x_height: layouter.message_font.metrics('x', size).bounds.height,
            pending_space: false,
            line: Vec::new(),
            finished: Vec::new(),
        }
    }

    fn space(&mut self) {
        self.pending_space = true;
    }

    fn reserve(&mut self, width: f32) {
        let space = if self.pending_space && !self.line.is_empty() {
            self.layouter
                .measure_styled_text_width(" ", self.size, TextStyle::Message)
        } else {
            0.0
        };
        if self.x + space + width > self.right && !self.line.is_empty() {
            self.finish_line();
        } else {
            self.x += space;
        }
        self.pending_space = false;
    }

    fn text(&mut self, text: &str, style: TextStyle, color: [u8; 4]) {
        let width = self
            .layouter
            .measure_styled_text_width(text, self.size, style);
        self.reserve(width.min(self.right - MESSAGE_PADDING_X));
        let mut run = String::new();
        let mut run_width = 0.0;
        for c in text.chars() {
            let kern = run
                .chars()
                .last()
                .map(|p| self.layouter.kerning(p, c, self.size, style))
                .unwrap_or(0.0);
            let advance = self
                .layouter
                .font_for_char(c, style)
                .metrics(c, self.size)
                .advance_width;
            if self.x + run_width + kern + advance > self.right
                && (!run.is_empty() || !self.line.is_empty())
            {
                self.push_text(std::mem::take(&mut run), run_width, style, color);
                self.finish_line();
                run_width = 0.0;
            } else {
                run_width += kern;
            }
            run.push(c);
            run_width += advance;
        }
        self.push_text(run, run_width, style, color);
    }

    fn push_text(&mut self, text: String, width: f32, style: TextStyle, color: [u8; 4]) {
        if !text.is_empty() {
            self.line.push((
                LayoutToken::Text {
                    text,
                    color,
                    style,
                    x: self.x,
                    y: 0.0,
                },
                0.0,
            ));
            self.x += width;
        }
    }

    fn image(&mut self, cache: &EmoteCache, path: String, size: u32, square: bool, badge: bool) {
        let raw_width = cache.get_width(&path, size);
        let height = if square && raw_width > size {
            ((size as f32 * size as f32 / raw_width as f32).floor() as u32).max(1)
        } else {
            size
        };
        let width = cache.get_width(&path, height);
        let slot_width = if square { size } else { width };
        self.reserve(slot_width as f32);
        let margin_bottom = if badge { 1.5 } else { 0.0 };
        let above = size as f32 * 0.5 + self.x_height * 0.5 + margin_bottom * 0.5;
        let below = size as f32 * 0.5 - self.x_height * 0.5 + margin_bottom * 0.5;
        self.above = self.above.max(above);
        self.below = self.below.max(below);
        let offset = -above + (size - height) as f32 * 0.5;
        self.line.push((
            LayoutToken::Image {
                path,
                x: self.x + (slot_width - width) as f32 * 0.5,
                y: 0.0,
                width,
                height,
            },
            offset,
        ));
        self.x += slot_width as f32 + if badge { BADGE_GAP } else { 0.0 };
    }

    fn finish_line(&mut self) {
        if self.line.is_empty() {
            return;
        }
        let baseline = self.y + self.above;
        for (mut token, offset) in self.line.drain(..) {
            match &mut token {
                LayoutToken::Text { y, .. } => *y = baseline,
                LayoutToken::Image { y, .. } => *y = baseline + offset,
            }
            self.finished.push(token);
        }
        self.y += self.above + self.below;
        self.x = MESSAGE_PADDING_X;
        self.above = self.text_above;
        self.below = self.text_below;
        self.pending_space = false;
    }
}

fn parse_hex_color(hex: &str, fallback_name: &str) -> [u8; 4] {
    let clean = hex.trim_start_matches('#');
    if let Some(value) = clean.get(..6).and_then(|s| u32::from_str_radix(s, 16).ok()) {
        return [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255];
    }
    let hash = fallback_name.bytes().fold(0u32, |h, b| {
        h.wrapping_mul(31).wrapping_add(b as u32) % 16777215
    });
    let mut color = [(hash >> 16) as u8, (hash >> 8) as u8, hash as u8, 255];
    if color[0] as f32 * 0.299 + color[1] as f32 * 0.587 + color[2] as f32 * 0.114 < 75.0 {
        for c in &mut color[..3] {
            *c = c.saturating_add(90);
        }
    }
    color
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{BadgeTag, WordEmoteEntry};

    fn message(text: &str) -> ChatMessage {
        ChatMessage {
            timestamp_ms: 0,
            author: "Nick".into(),
            display_name: "Nick".into(),
            color_hex: "#55AAFF".into(),
            text: text.into(),
            badges: vec![],
            twitch_emotes_raw: String::new(),
            is_gap: false,
        }
    }

    fn layout(msg: ChatMessage, width: f32) -> LayoutMessage {
        ChatLayouter::new()
            .layout_messages(
                &[msg],
                &EmotesManifest::default(),
                &EmoteCache::new(std::path::Path::new(".")),
                width,
                14.0,
                true,
            )
            .remove(0)
    }

    #[test]
    fn badges_and_emotes_can_be_disabled_independently() {
        let l = ChatLayouter::new();
        let cache = EmoteCache::new(std::path::Path::new("."));
        let mut msg = message("Kappa");
        msg.twitch_emotes_raw = "25:0-4".into();
        msg.badges.push(BadgeTag { name: "mod".into(), version: "1".into() });
        let manifest = EmotesManifest::default();
        let badges_only = l.layout_messages_with_icons(&[msg.clone()], &manifest, &cache, 340.0, 14.0, false, true);
        assert!(badges_only[0].tokens.iter().any(|t| matches!(t, LayoutToken::Image { .. })));
        assert!(badges_only[0].tokens.iter().any(|t| matches!(t, LayoutToken::Text { text, .. } if text == "Kappa")));
        let disabled = l.layout_messages_with_icons(&[msg], &manifest, &cache, 340.0, 14.0, false, false);
        assert!(!disabled[0].tokens.iter().any(|t| matches!(t, LayoutToken::Image { .. })));
    }

    #[test]
    fn latin_and_russian_fonts_use_the_correct_weight() {
        let l = ChatLayouter::new();
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_".chars() {
            assert!(std::ptr::eq(
                l.font_for_char(c, TextStyle::Message),
                &l.message_font
            ));
            assert!(std::ptr::eq(
                l.font_for_char(c, TextStyle::Nickname),
                &l.nickname_font
            ));
        }
        for c in "АБВГДЕЁЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯабвгдеёжзийклмнопрстуфхцчшщъыьэюя".chars()
        {
            assert_ne!(l.cyrillic_font.lookup_glyph_index(c), 0);
            assert_ne!(l.cyrillic_bold.lookup_glyph_index(c), 0);
            assert!(std::ptr::eq(
                l.font_for_char(c, TextStyle::Message),
                &l.cyrillic_font
            ));
            assert!(std::ptr::eq(
                l.font_for_char(c, TextStyle::Nickname),
                &l.cyrillic_bold
            ));
        }
    }

    #[test]
    fn kerning_is_included_in_measurement() {
        let l = ChatLayouter::new();
        for style in [TextStyle::Message, TextStyle::Nickname] {
            let pair = l.kerning('A', 'V', 14.0, style);
            assert!(pair < 0.0);
            let expected = l.font_for_char('A', style).metrics('A', 14.0).advance_width
                + l.font_for_char('V', style).metrics('V', 14.0).advance_width
                + pair;
            assert_eq!(l.measure_styled_text_width("AV", 14.0, style), expected);
        }
    }

    #[test]
    fn standard_text_has_thirty_pixel_message_height() {
        let msg = layout(message("Привет chat"), 340.0);
        assert_eq!(msg.top_y, 0.0);
        assert_eq!(msg.bottom_y, 30.0);
        assert!(
            matches!(&msg.tokens[0], LayoutToken::Text { x, y, style: TextStyle::Nickname, .. } if *x == 16.0 && *y == 20.0)
        );
        assert!(
            matches!(&msg.tokens[1], LayoutToken::Text { text, style: TextStyle::Message, .. } if text == ":")
        );
    }

    #[test]
    fn badges_have_eighteen_pixel_size_and_three_pixel_spacing() {
        let mut msg = message("hello");
        msg.badges = vec![
            BadgeTag {
                name: "mod".into(),
                version: "1".into()
            };
            2
        ];
        let row = layout(msg, 340.0);
        assert_eq!(row.bottom_y, 30.0);
        for (index, token) in row.tokens[..2].iter().enumerate() {
            assert!(
                matches!(token, LayoutToken::Image { x, width: 18, height: 18, y, .. } if *x == 16.0 + index as f32 * 21.0 && (*y - 6.43).abs() < 0.1)
            );
        }
        assert!(matches!(&row.tokens[2], LayoutToken::Text { x, .. } if *x == 58.0));
    }

    #[test]
    fn long_words_wrap_at_left_padding_without_overflow() {
        let row = layout(message(&"A".repeat(100)), 120.0);
        let l = ChatLayouter::new();
        let mut found_second_line = false;
        for token in row.tokens {
            if let LayoutToken::Text {
                text, style, x, y, ..
            } = token
            {
                assert!(x + l.measure_styled_text_width(&text, 14.0, style) <= 104.01);
                if y > 20.0 {
                    found_second_line = true;
                    assert_eq!(x, 16.0);
                }
            }
        }
        assert!(found_second_line);
    }

    #[test]
    fn spaces_and_native_emote_offsets_are_preserved() {
        let l = ChatLayouter::new();
        let tokens = l.tokenize_message(
            "яKappa  Kappa!",
            "25:1-5,8-12",
            &EmotesManifest::default(),
            true,
        );
        assert!(
            matches!(&tokens[..], [ContentToken::Text(t), ContentToken::Emote { .. }, ContentToken::Space, ContentToken::Emote { .. }, ContentToken::Text(end)] if t == "я" && end == "!")
        );
    }

    #[test]
    fn emotes_use_twenty_eight_pixel_boxes_and_real_spaces() {
        let dir = std::env::temp_dir().join(format!("twitch-inline-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("emote.png");
        tiny_skia::Pixmap::new(28, 28)
            .unwrap()
            .save_png(&path)
            .unwrap();
        let cache = EmoteCache::new(&dir);
        let mut manifest = EmotesManifest::default();
        manifest.words.insert(
            "Kappa".into(),
            WordEmoteEntry {
                file: Some(path.to_string_lossy().into_owned()),
                ..Default::default()
            },
        );
        let l = ChatLayouter::new();
        let row = l
            .layout_messages(
                &[message("Kappa Kappa")],
                &manifest,
                &cache,
                340.0,
                14.0,
                true,
            )
            .remove(0);
        assert_eq!(row.bottom_y, 36.0);
        let images: Vec<_> = row
            .tokens
            .iter()
            .filter_map(|t| {
                if let LayoutToken::Image {
                    x,
                    y,
                    width,
                    height,
                    ..
                } = t
                {
                    Some((*x, *y, *width, *height))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].1, 4.0);
        assert_eq!(images[0].2, 28);
        assert!(
            (images[1].0
                - images[0].0
                - 28.0
                - l.measure_styled_text_width(" ", 14.0, TextStyle::Message))
            .abs()
                < 0.001
        );
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
