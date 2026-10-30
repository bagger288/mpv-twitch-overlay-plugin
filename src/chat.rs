//! Chat log loader, IRC parser and instant binary search indexer.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub timestamp_ms: i64,
    pub sender: String,
    pub display_name: String,
    pub color: (u8, u8, u8),
    pub text: String,
    pub badges: Vec<String>,
    pub twitch_emotes: Vec<EmoteRange>,
    pub is_system: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmoteRange {
    pub id: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GapEvent {
    pub timestamp_ms: i64,
    pub duration_ms: i64,
    pub reason: String,
}

#[derive(Default)]
pub struct ChatIndex {
    pub messages: Vec<ChatMessage>,
    pub gaps: Vec<GapEvent>,
}

impl ChatIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load and index chat from a .chat.jsonl file.
    pub fn load_jsonl<P: AsRef<Path>>(&mut self, path: P) -> Result<usize, std::io::Error> {
        self.messages.clear();
        self.gaps.clear();

        let file = File::open(path)?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(event_type) = v.get("type").and_then(|t| t.as_str()) {
                    if event_type == "GAP" {
                        let timestamp_ms = v.get("recv").and_then(|r| r.as_i64()).unwrap_or(0);
                        let duration_ms = v.get("duration_ms").and_then(|d| d.as_i64()).unwrap_or(0);
                        let reason = v.get("reason").and_then(|r| r.as_str()).unwrap_or("disconnect").to_string();
                        self.gaps.push(GapEvent {
                            timestamp_ms,
                            duration_ms,
                            reason,
                        });
                        continue;
                    }
                }

                if let Some(raw) = v.get("raw").and_then(|r| r.as_str()) {
                    let recv = v.get("recv").and_then(|r| r.as_i64()).unwrap_or(0);
                    if let Some(msg) = parse_irc_raw(raw, recv) {
                        self.messages.push(msg);
                    }
                }
            }
        }

        // Keep strictly sorted by timestamp_ms for binary search
        self.messages.sort_by_key(|m| m.timestamp_ms);
        Ok(self.messages.len())
    }

    /// Find the range of messages visible at current_time_ms within a time window (e.g. past 60 seconds).
    pub fn get_messages_in_window(&self, current_time_ms: i64, window_ms: i64, max_count: usize) -> &[ChatMessage] {
        if self.messages.is_empty() {
            return &[];
        }

        // Right bound: latest message <= current_time_ms
        let right_idx = match self.messages.binary_search_by_key(&current_time_ms, |m| m.timestamp_ms) {
            Ok(idx) => {
                let mut i = idx;
                while i + 1 < self.messages.len() && self.messages[i + 1].timestamp_ms <= current_time_ms {
                    i += 1;
                }
                i + 1
            }
            Err(idx) => idx,
        };

        if right_idx == 0 {
            return &[];
        }

        let min_time = current_time_ms - window_ms;
        let left_idx = match self.messages[..right_idx].binary_search_by_key(&min_time, |m| m.timestamp_ms) {
            Ok(idx) => idx,
            Err(idx) => idx,
        };

        let slice = &self.messages[left_idx..right_idx];
        if slice.len() > max_count {
            &slice[slice.len() - max_count..]
        } else {
            slice
        }
    }
}

/// Parse IRC raw message with Twitch tags
fn parse_irc_raw(raw: &str, fallback_recv: i64) -> Option<ChatMessage> {
    if !raw.contains("PRIVMSG") {
        return None;
    }

    let mut tags = HashMap::new();
    let mut rest = raw;

    if raw.starts_with('@') {
        if let Some(space_pos) = raw.find(' ') {
            let tags_str = &raw[1..space_pos];
            rest = &raw[space_pos + 1..];

            for part in tags_str.split(';') {
                if let Some((k, v)) = part.split_once('=') {
                    tags.insert(k, v);
                }
            }
        }
    }

    // Extract sender nick from :nick!nick@nick.tmi.twitch.tv
    let mut sender = "unknown".to_string();
    if rest.starts_with(':') {
        if let Some(bang_pos) = rest.find('!') {
            sender = rest[1..bang_pos].to_string();
        }
    }

    // Extract text after 'PRIVMSG #channel :'
    let text = if let Some(colon_pos) = rest.find(" PRIVMSG ") {
        let msg_part = &rest[colon_pos + 9..];
        if let Some(colon_idx) = msg_part.find(" :") {
            &msg_part[colon_idx + 2..]
        } else {
            ""
        }
    } else {
        ""
    };

    if text.is_empty() {
        return None;
    }

    // Extract display-name
    let display_name = tags
        .get("display-name")
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| sender.clone());

    // Extract color
    let color = tags
        .get("color")
        .and_then(|c| parse_hex_color(c))
        .unwrap_or_else(|| generate_nick_color(&sender));

    // Extract badges
    let badges = tags
        .get("badges")
        .map(|b| {
            b.split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    // Extract twitch emotes
    let mut twitch_emotes = Vec::new();
    if let Some(emotes_str) = tags.get("emotes") {
        for emote_group in emotes_str.split('/') {
            if let Some((id, ranges)) = emote_group.split_once(':') {
                for r in ranges.split(',') {
                    if let Some((start_s, end_s)) = r.split_once('-') {
                        if let (Ok(start), Ok(end)) = (start_s.parse::<usize>(), end_s.parse::<usize>()) {
                            twitch_emotes.push(EmoteRange {
                                id: id.to_string(),
                                start,
                                end: end + 1,
                            });
                        }
                    }
                }
            }
        }
    }

    let timestamp_ms = tags
        .get("tmi-sent-ts")
        .and_then(|t| t.parse::<i64>().ok())
        .unwrap_or(fallback_recv);

    Some(ChatMessage {
        timestamp_ms,
        sender,
        display_name,
        color,
        text: text.to_string(),
        badges,
        twitch_emotes,
        is_system: false,
    })
}

fn parse_hex_color(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some((r, g, b))
    } else {
        None
    }
}

fn generate_nick_color(nick: &str) -> (u8, u8, u8) {
    let mut hash: u32 = 0;
    for b in nick.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(b as u32);
    }
    let r = ((hash >> 16) & 0xFF) as u8;
    let g = ((hash >> 8) & 0xFF) as u8;
    let b = (hash & 0xFF) as u8;

    // Ensure readable contrast on dark background
    let luminance = 0.299 * (r as f32) + 0.587 * (g as f32) + 0.114 * (b as f32);
    if luminance < 80.0 {
        (
            r.saturating_add(100),
            g.saturating_add(100),
            b.saturating_add(100),
        )
    } else {
        (r, g, b)
    }
}
