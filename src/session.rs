//! Session, Chat and Emotes parser with fast binary search

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartInfo {
    pub file: String,
    pub start_ms: i64,
    #[serde(default)]
    pub end_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub channel: String,
    #[serde(default)]
    pub room_id: Option<String>,
    pub session_start_ms: i64,
    #[serde(default)]
    pub parts: Vec<PartInfo>,
    #[serde(default)]
    pub offsets_ms: HashMap<String, i64>,
}

#[derive(Debug, Clone)]
pub struct BadgeTag {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub timestamp_ms: i64,
    pub author: String,
    pub display_name: String,
    pub color_hex: String,
    pub text: String,
    pub badges: Vec<BadgeTag>,
    pub twitch_emotes_raw: String,
    pub is_gap: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmoteMetadata {
    pub name: String,
    pub id: String,
    pub source: String, // "7tv", "bttv", "ffz", "twitch"
    #[serde(default)]
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmotesManifest {
    #[serde(default)]
    pub channel_emotes: HashMap<String, EmoteMetadata>,
    #[serde(default)]
    pub global_emotes: HashMap<String, EmoteMetadata>,
    #[serde(default)]
    pub badges: HashMap<String, HashMap<String, String>>, // badge_set -> version -> local_path
}

pub struct SessionManager {
    pub session_dir: PathBuf,
    pub session_file: PathBuf,
    pub chat_file: PathBuf,
    pub emotes_file: PathBuf,
    pub session_data: SessionData,
    pub messages: Vec<ChatMessage>,
    pub emotes_manifest: EmotesManifest,
}

impl SessionManager {
    /// Discovers and loads session files from video path or directory
    pub fn try_load(path: &Path) -> Option<Self> {
        let (session_dir, base_name) = if path.is_dir() {
            (path.to_path_buf(), path.file_name()?.to_string_lossy().to_string())
        } else {
            let parent = path.parent()?.to_path_buf();
            let stem = path.file_stem()?.to_string_lossy().to_string();
            // In case of multi-part filename like "channel_date.ts"
            (parent, stem)
        };

        // Find .session.json
        let mut session_file = session_dir.join(format!("{}.session.json", base_name));
        if !session_file.exists() {
            // Try scanning dir for any *.session.json
            if let Ok(entries) = fs::read_dir(&session_dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.extension().map_or(false, |ext| ext == "json")
                        && p.to_string_lossy().ends_with(".session.json")
                    {
                        session_file = p;
                        break;
                    }
                }
            }
        }

        if !session_file.exists() {
            return None;
        }

        let content = fs::read_to_string(&session_file).ok()?;
        let session_data: SessionData = serde_json::from_str(&content).ok()?;

        // Base prefix for associated files
        let session_stem = session_file
            .file_name()?
            .to_string_lossy()
            .trim_end_matches(".session.json")
            .to_string();

        let chat_file = session_dir.join(format!("{}.chat.jsonl", session_stem));
        let emotes_file = session_dir.join(format!("{}.emotes.json", session_stem));

        let messages = if chat_file.exists() {
            Self::parse_chat_jsonl(&chat_file)
        } else {
            Vec::new()
        };

        let emotes_manifest = if emotes_file.exists() {
            fs::read_to_string(&emotes_file)
                .ok()
                .and_then(|s| serde_json::from_str::<EmotesManifest>(&s).ok())
                .unwrap_or_default()
        } else {
            EmotesManifest::default()
        };

        Some(Self {
            session_dir,
            session_file,
            chat_file,
            emotes_file,
            session_data,
            messages,
            emotes_manifest,
        })
    }

    /// Fast parsing of IRC tags and raw PRIVMSG lines
    fn parse_chat_jsonl(path: &Path) -> Vec<ChatMessage> {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };

        let reader = BufReader::with_capacity(512 * 1024, file);
        let mut messages = Vec::with_capacity(8192);

        for line_res in reader.lines() {
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.trim().is_empty() {
                continue;
            }

            // Quick JSON parse for "recv", "raw", "type"
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(msg_type) = v.get("type").and_then(|t| t.as_str()) {
                    if msg_type == "GAP" {
                        let recv = v.get("recv").and_then(|r| r.as_i64()).unwrap_or(0);
                        let duration = v.get("duration_ms").and_then(|d| d.as_i64()).unwrap_or(0);
                        messages.push(ChatMessage {
                            timestamp_ms: recv,
                            author: "SYSTEM".to_string(),
                            display_name: "Переподключение".to_string(),
                            color_hex: "#AAAAAA".to_string(),
                            text: format!("--- Разрыв связи с сервером ({} с) ---", duration / 1000),
                            badges: Vec::new(),
                            twitch_emotes_raw: String::new(),
                            is_gap: true,
                        });
                        continue;
                    }
                }

                let recv_ts = v.get("recv").and_then(|r| r.as_i64()).unwrap_or(0);
                if let Some(raw_str) = v.get("raw").and_then(|r| r.as_str()) {
                    if let Some(msg) = Self::parse_raw_irc(raw_str, recv_ts) {
                        messages.push(msg);
                    }
                }
            }
        }

        // Ensure messages are strictly sorted by timestamp
        messages.sort_by_key(|m| m.timestamp_ms);
        messages
    }

    /// Parses raw IRC line format with tags
    fn parse_raw_irc(raw: &str, fallback_ts: i64) -> Option<ChatMessage> {
        if !raw.contains("PRIVMSG") {
            return None;
        }

        let mut tags_str = "";
        let mut rest = raw;

        if raw.starts_with('@') {
            if let Some(idx) = raw.find(' ') {
                tags_str = &raw[1..idx];
                rest = &raw[idx + 1..];
            }
        }

        let mut badges = Vec::new();
        let mut display_name = String::new();
        let mut color_hex = String::new();
        let mut emotes_raw = String::new();
        let mut tmi_sent_ts: i64 = fallback_ts;

        for tag in tags_str.split(';') {
            if let Some((k, v)) = tag.split_once('=') {
                match k {
                    "badges" => {
                        for b in v.split(',') {
                            if let Some((bname, bver)) = b.split_once('/') {
                                badges.push(BadgeTag {
                                    name: bname.to_string(),
                                    version: bver.to_string(),
                                });
                            }
                        }
                    }
                    "display-name" => {
                        display_name = v.to_string();
                    }
                    "color" => {
                        if !v.is_empty() {
                            color_hex = v.to_string();
                        }
                    }
                    "emotes" => {
                        emotes_raw = v.to_string();
                    }
                    "tmi-sent-ts" => {
                        if let Ok(ts) = v.parse::<i64>() {
                            tmi_sent_ts = ts;
                        }
                    }
                    _ => {}
                }
            }
        }

        // Extract author and message body from :author!author@... PRIVMSG #channel :Message text
        let colon_idx = rest.find(" PRIVMSG ")?;
        let after_privmsg = &rest[colon_idx + 9..];
        let msg_colon = after_privmsg.find(" :")?;
        let text = after_privmsg[msg_colon + 2..].to_string();

        let prefix = &rest[..colon_idx];
        let author = if let Some(bang) = prefix.find('!') {
            prefix[1..bang].to_string()
        } else {
            "user".to_string()
        };

        if display_name.is_empty() {
            display_name = author.clone();
        }

        if color_hex.is_empty() {
            // Deterministic nick color generator
            let colors = [
                "#FF4500", "#00FF7F", "#1E90FF", "#FF69B4", "#9146FF",
                "#00CED1", "#FFD700", "#FF6347", "#32CD32", "#BA55D3"
            ];
            let hash: usize = author.bytes().fold(0, |acc, b| acc.wrapping_add(b as usize));
            color_hex = colors[hash % colors.len()].to_string();
        }

        Some(ChatMessage {
            timestamp_ms: tmi_sent_ts,
            author,
            display_name,
            color_hex,
            text,
            badges,
            twitch_emotes_raw: emotes_raw,
            is_gap: false,
        })
    }

    /// Finds index range of messages up to `current_ms`
    pub fn get_messages_up_to(&self, current_ms: i64, max_count: usize) -> &[ChatMessage] {
        if self.messages.is_empty() {
            return &[];
        }

        let idx = match self.messages.binary_search_by_key(&current_ms, |m| m.timestamp_ms) {
            Ok(i) => i + 1,
            Err(i) => i,
        };

        let start = idx.saturating_sub(max_count);
        &self.messages[start..idx]
    }

    /// 1-Click Sync anchor: updates session_start_ms in memory and on disk
    pub fn calibrate_anchor(&mut self, target_msg_ts: i64, current_video_time_sec: f64) -> bool {
        let new_start = target_msg_ts - (current_video_time_sec * 1000.0).round() as i64;
        self.session_data.session_start_ms = new_start;

        // Reset default offset to 0
        self.session_data.offsets_ms.insert("default".to_string(), 0);

        if let Ok(json_str) = serde_json::to_string_pretty(&self.session_data) {
            if fs::write(&self.session_file, json_str).is_ok() {
                return true;
            }
        }
        false
    }
}
