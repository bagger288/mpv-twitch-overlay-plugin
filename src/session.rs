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
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_json::Value>,
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
    #[serde(default)]
    pub emote_cache_dir: Option<String>,
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct BadgeTag {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct TwitchEmoteSpan {
    pub start: usize,
    pub end: usize,
    pub emote_id: String,
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
pub struct WordEmoteEntry {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub local_path: Option<String>,
    #[serde(default)]
    pub animation_file: Option<String>,
}

impl WordEmoteEntry {
    pub fn get_path(&self) -> Option<&str> {
        self.file.as_deref().or(self.local_path.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BadgeEntry {
    #[serde(default)]
    pub set_id: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub local_path: Option<String>,
}

impl BadgeEntry {
    pub fn get_path(&self) -> Option<&str> {
        self.file.as_deref().or(self.local_path.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmotesManifest {
    #[serde(default)]
    pub words: HashMap<String, WordEmoteEntry>,
    #[serde(default)]
    pub twitch_emotes: HashMap<String, WordEmoteEntry>,
    #[serde(default)]
    pub channel_emotes: HashMap<String, WordEmoteEntry>,
    #[serde(default)]
    pub global_emotes: HashMap<String, WordEmoteEntry>,
    #[serde(default)]
    pub badges: HashMap<String, BadgeEntry>,
    #[serde(default)]
    pub cache_root: Option<String>,
}

impl EmotesManifest {
    pub fn get_word_emote_path(&self, word: &str) -> Option<String> {
        if let Some(entry) = self.words.get(word) {
            if let Some(p) = entry.get_path() {
                return Some(p.to_string());
            }
        }
        if let Some(entry) = self.channel_emotes.get(word) {
            if let Some(p) = entry.get_path() {
                return Some(p.to_string());
            }
        }
        if let Some(entry) = self.global_emotes.get(word) {
            if let Some(p) = entry.get_path() {
                return Some(p.to_string());
            }
        }
        None
    }

    pub fn animation_path(&self, preview: &str) -> Option<&str> {
        self.words.values().chain(self.channel_emotes.values()).chain(self.global_emotes.values()).chain(self.twitch_emotes.values())
            .find(|entry| entry.get_path() == Some(preview)).and_then(|entry| entry.animation_file.as_deref())
    }

    pub fn get_badge_path(&self, set_id: &str, version: &str) -> String {
        let key = format!("{}/{}", set_id, version);
        if let Some(entry) = self.badges.get(&key) {
            if let Some(p) = entry.get_path() {
                return p.to_string();
            }
        }
        format!("badges/{}_{}.png", set_id, version)
    }

    pub fn total_emotes_count(&self) -> usize {
        self.words.len() + self.channel_emotes.len() + self.global_emotes.len()
    }
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
        let (session_dir, base_name, direct_session) = if path.is_dir() {
            (path.to_path_buf(), path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), None)
        } else if path.to_string_lossy().ends_with(".session.json") {
            let parent = path.parent().unwrap_or(Path::new(".")).to_path_buf();
            (parent, String::new(), Some(path.to_path_buf()))
        } else {
            let parent = path.parent().unwrap_or(Path::new(".")).to_path_buf();
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            (parent, stem, None)
        };

        // Find .session.json
        let mut session_file = if let Some(ds) = direct_session {
            ds
        } else {
            session_dir.join(format!("{}.session.json", base_name))
        };

        if !session_file.exists() {
            // Try scanning dir for any *.session.json
            if let Ok(entries) = fs::read_dir(&session_dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.to_string_lossy().ends_with(".session.json") {
                        session_file = p;
                        break;
                    }
                }
            }
        }

        if !session_file.exists() {
            // Also check parent directory if path was a sub-item
            if let Some(parent) = session_dir.parent() {
                if let Ok(entries) = fs::read_dir(parent) {
                    for e in entries.flatten() {
                        let p = e.path();
                        if p.to_string_lossy().ends_with(".session.json") {
                            session_file = p;
                            break;
                        }
                    }
                }
            }
        }

        if !session_file.exists() {
            return None;
        }

        let content = fs::read_to_string(&session_file).ok()?;
        let session_data: SessionData = serde_json::from_str(&content).ok()?;
        let session_dir = session_file.parent()?.to_path_buf();

        // Base prefix for associated files
        let session_stem = session_file
            .file_name()?
            .to_string_lossy()
            .trim_end_matches(".session.json")
            .to_string();

        let mut chat_file = session_dir.join(format!("{}.chat.jsonl", session_stem));
        if !chat_file.exists() {
            if let Ok(entries) = fs::read_dir(&session_dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if name.ends_with(".chat.jsonl") || name == "chat.jsonl" {
                        chat_file = p;
                        break;
                    }
                }
            }
        }

        let mut emotes_file = session_dir.join(format!("{}.emotes.json", session_stem));
        if !emotes_file.exists() {
            if let Ok(entries) = fs::read_dir(&session_dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if name.ends_with(".emotes.json") || name == "emotes.json" {
                        emotes_file = p;
                        break;
                    }
                }
            }
        }

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
                if let Some(raw_str) = v.get("raw").and_then(|r| r.as_str()).filter(|_| v.get("normalized").and_then(|v| v.as_bool()) != Some(true)) {
                    if let Some(msg) = Self::parse_raw_irc(raw_str, recv_ts) {
                        messages.push(msg);
                    }
                } else if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
                    let ts = v.get("ts").and_then(|t| t.as_i64()).unwrap_or(recv_ts);
                    let user = v.get("user").and_then(|u| u.as_str()).unwrap_or("user").to_string();
                    let display = v.get("display").and_then(|d| d.as_str()).unwrap_or(&user).to_string();
                    let color = v.get("color").and_then(|c| c.as_str()).unwrap_or("").to_string();
                    let badges_str = v.get("badges").and_then(|b| b.as_str()).unwrap_or("");
                    let emotes_str = v.get("emotes").and_then(|e| e.as_str()).unwrap_or("").to_string();

                    let mut badges = Vec::new();
                    for b in badges_str.split(',') {
                        let trimmed = b.trim();
                        if !trimmed.is_empty() {
                            if let Some((bname, bver)) = trimmed.split_once('/') {
                                badges.push(BadgeTag {
                                    name: bname.to_string(),
                                    version: bver.to_string(),
                                });
                            }
                        }
                    }

                    messages.push(ChatMessage {
                        timestamp_ms: ts,
                        author: user,
                        display_name: display,
                        color_hex: color,
                        text: text.to_string(),
                        badges,
                        twitch_emotes_raw: emotes_str,
                        is_gap: false,
                    });
                }
            }
        }

        // Ensure messages are strictly sorted by timestamp
        messages.sort_by_key(|m| m.timestamp_ms);
        messages
    }

    /// Parses raw IRC line format with tags (PRIVMSG and USERNOTICE)
    fn parse_raw_irc(raw: &str, fallback_ts: i64) -> Option<ChatMessage> {
        let is_privmsg = raw.contains(" PRIVMSG ");
        let is_usernotice = raw.contains(" USERNOTICE ");
        if !is_privmsg && !is_usernotice {
            return None;
        }
        let cmd_delimiter = if is_privmsg { " PRIVMSG " } else { " USERNOTICE " };

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

        // Extract author and message body
        let colon_idx = rest.find(cmd_delimiter)?;
        let after_cmd = &rest[colon_idx + cmd_delimiter.len()..];
        let text = if let Some(msg_colon) = after_cmd.find(" :") {
            after_cmd[msg_colon + 2..].to_string()
        } else if let Some(pos) = after_cmd.find(':') {
            after_cmd[pos + 1..].to_string()
        } else {
            String::new()
        };

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

    /// Parses Twitch IRC native emote spans from tag (e.g. 25:0-4,12-16/1902:6-10)
    pub fn parse_twitch_emotes_tag(tag: &str) -> Vec<TwitchEmoteSpan> {
        parse_twitch_emotes_tag(tag)
    }

    pub fn part_index(&self, video_path: &str) -> Option<usize> {
        let normalize = |path: &str| {
            let path = path.replace('\\', "/");
            if cfg!(target_os = "windows") { path.to_lowercase() } else { path }
        };
        let target = normalize(video_path);
        if let Some(index) = self.session_data.parts.iter().position(|part| {
            let path = Path::new(&part.file);
            let absolute = if path.is_absolute() { path.to_path_buf() } else { self.session_dir.join(path) };
            normalize(&absolute.to_string_lossy()) == target || normalize(&part.file) == target
        }) {
            return Some(index);
        }
        let basename = target.rsplit('/').next()?;
        let mut matches = self.session_data.parts.iter().enumerate().filter(|(_, part)| normalize(&part.file).rsplit('/').next() == Some(basename));
        let index = matches.next()?.0;
        if matches.next().is_none() { Some(index) } else { None }
    }

    pub fn video_start_ms(&self, video_path: &str) -> i64 {
        self.part_index(video_path).map(|index| self.session_data.parts[index].start_ms)
            .unwrap_or(self.session_data.session_start_ms)
    }

    pub fn video_offset_ms(&self, video_path: &str) -> i64 {
        let canonical_key = self.part_index(video_path).map(|i| self.session_data.parts[i].file.as_str());
        let basename = video_path.rsplit(['/', '\\']).next().unwrap_or(video_path);
        self.session_data.offsets_ms.get(video_path)
            .or_else(|| canonical_key.and_then(|key| self.session_data.offsets_ms.get(key)))
            .or_else(|| self.session_data.offsets_ms.get(basename))
            .or_else(|| self.session_data.offsets_ms.get("default"))
            .copied().unwrap_or(0)
    }

    pub fn cache_directory(&self) -> Option<PathBuf> {
        self.session_data.emote_cache_dir.as_deref().or(self.emotes_manifest.cache_root.as_deref())
            .map(|path| { let path = Path::new(path); if path.is_absolute() { path.to_path_buf() } else { self.session_dir.join(path) } })
    }

    /// Finds index range of messages up to `current_ms`, shifted by `scroll_offset`:
    /// - `scroll_offset > 0`: shifted backwards into past history
    /// - `scroll_offset < 0`: shifted forward into future messages
    /// - `scroll_offset == 0`: current live position
    pub fn get_messages_up_to(&self, current_ms: i64, scroll_offset: i32, max_count: usize) -> &[ChatMessage] {
        if self.messages.is_empty() {
            return &[];
        }

        let last_idx = self.messages.partition_point(|message| message.timestamp_ms <= current_ms);
        let max_idx = self.messages.len();
        let eff_idx = (last_idx as i64 - scroll_offset as i64).clamp(0, max_idx as i64) as usize;

        let start = eff_idx.saturating_sub(max_count);
        &self.messages[start..eff_idx]
    }

    pub fn visible_messages(&self, current_ms: i64, ui: &crate::ui::UiState, max_count: usize) -> &[ChatMessage] {
        if let Some(end) = ui.chat_scroll_anchor {
            let end = end.min(self.messages.len());
            &self.messages[end.saturating_sub(max_count)..end]
        } else {
            self.get_messages_up_to(current_ms, ui.scroll_offset, max_count)
        }
    }

    /// 1-Click Sync anchor: updates session_start_ms in memory and on disk
    pub fn calibrate_anchor(&mut self, target_msg_ts: i64, current_video_time_sec: f64, video_filename: &str) -> bool {
        let new_start = target_msg_ts - (current_video_time_sec * 1000.0).round() as i64;
        let delta = new_start - self.video_start_ms(video_filename);
        self.session_data.session_start_ms += delta;
        for part in &mut self.session_data.parts {
            part.start_ms += delta;
            if let Some(end) = &mut part.end_ms { *end += delta; }
        }
        if let Some(index) = self.part_index(video_filename) {
            let key = self.session_data.parts[index].file.clone();
            self.session_data.offsets_ms.insert(key, 0);
        }
        if !video_filename.is_empty() {
            self.session_data.offsets_ms.insert(video_filename.to_string(), 0);
            let basename = video_filename.rsplit(['/', '\\']).next().unwrap_or(video_filename);
            self.session_data.offsets_ms.insert(basename.to_string(), 0);
        }
        self.session_data.offsets_ms.insert("default".to_string(), 0);
        self.save_session_file()
    }

    /// Saves session_data to session_file on disk
    pub fn save_session_file(&self) -> bool {
        let Some(json) = serde_json::to_string_pretty(&self.session_data).ok() else { return false; };
        let temporary = self.session_file.with_extension(format!("json.{}.tmp", std::process::id()));
        if fs::write(&temporary, json).is_err() { return false; }
        if fs::rename(&temporary, &self.session_file).is_ok() { return true; }
        let _ = fs::remove_file(temporary);
        false
    }

    /// Saves manual offset for current video file
    pub fn save_offset(&mut self, video_filename: &str, offset_ms: i64) -> bool {
        if let Some(index) = self.part_index(video_filename) {
            let key = self.session_data.parts[index].file.clone();
            self.session_data.offsets_ms.insert(key, offset_ms);
        }
        if !video_filename.is_empty() {
            self.session_data.offsets_ms.insert(video_filename.to_string(), offset_ms);
        }
        self.session_data.offsets_ms.insert("default".to_string(), offset_ms);
        self.save_session_file()
    }

    pub fn search_for_video(&self, query: &str, current_path: &str, current_offset: i64, max_results: usize) -> Vec<crate::ui::SearchResult> {
        let mut results = self.search_messages(query, self.video_start_ms(current_path), current_offset, max_results);
        for result in &mut results {
            let part = self.session_data.parts.iter().enumerate().find(|(index, part)| {
                let offset = if self.part_index(current_path) == Some(*index) { current_offset } else { self.video_offset_ms(&part.file) };
                let end = part.end_ms.or_else(|| self.session_data.parts.get(index + 1).map(|next| next.start_ms));
                result.timestamp_ms >= part.start_ms + offset
                    && end.map(|end| result.timestamp_ms < end + offset).unwrap_or(true)
            }).map(|(_, part)| part);
            if let Some(part) = part {
                let path = Path::new(&part.file);
                let absolute = if path.is_absolute() { path.to_path_buf() } else { self.session_dir.join(path) };
                let offset = if self.part_index(current_path).map(|i| &self.session_data.parts[i].file) == Some(&part.file) { current_offset } else { self.video_offset_ms(&part.file) };
                result.sec = ((result.timestamp_ms - part.start_ms - offset) as f64 / 1000.0).max(0.0);
                result.time_str = format_hms(result.sec);
                result.video_path = Some(absolute.to_string_lossy().into_owned());
            }
        }
        results
    }

    /// Searches messages case-insensitively (supporting English and Russian)
    pub fn search_messages(
        &self,
        query: &str,
        session_start_ms: i64,
        offset_ms: i64,
        max_results: usize,
    ) -> Vec<crate::ui::SearchResult> {
        let q = query.trim().to_lowercase();
        if q.is_empty() || max_results == 0 {
            return Vec::new();
        }

        let mut results = Vec::new();
        for (idx, msg) in self.messages.iter().enumerate() {
            let matches_text = msg.text.to_lowercase().contains(&q);
            let matches_author = msg.author.to_lowercase().contains(&q)
                || msg.display_name.to_lowercase().contains(&q);

            if matches_text || matches_author {
                let sec = ((msg.timestamp_ms - session_start_ms - offset_ms) as f64 / 1000.0).max(0.0);
                results.push(crate::ui::SearchResult {
                    msg_idx: idx,
                    timestamp_ms: msg.timestamp_ms,
                    sec,
                    video_path: None,
                    time_str: format_hms(sec),
                    author: msg.display_name.clone(),
                    color_hex: msg.color_hex.clone(),
                    text: msg.text.clone(),
                });

                if results.len() >= max_results {
                    break;
                }
            }
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> SessionManager {
        let session_data = serde_json::from_value(serde_json::json!({
            "channel":"test", "session_start_ms":1000, "emote_cache_dir":"custom-cache",
            "start_detection_info":"keep me", "source_files":["source.json"],
            "parts":[{"file":"first.mp4","start_ms":1000,"end_ms":11000,"custom_part":true},
                     {"file":"second.mp4","start_ms":11000,"end_ms":21000}], "offsets_ms":{}
        })).unwrap();
        SessionManager { session_dir: PathBuf::from("/fixtures"), session_file: PathBuf::from("/fixtures/test.session.json"),
            chat_file: PathBuf::new(), emotes_file: PathBuf::new(), session_data, messages: vec![], emotes_manifest: EmotesManifest::default() }
    }

    #[test]
    fn fixed_chat_scroll_does_not_follow_video_time() {
        let mut session = manager();
        session.messages = (0..5).map(|index| ChatMessage { timestamp_ms: index * 1000, author:"user".into(), display_name:"user".into(), color_hex:String::new(), text:index.to_string(), badges:vec![], twitch_emotes_raw:String::new(), is_gap:false }).collect();
        let ui = crate::ui::UiState { chat_scroll_anchor: Some(3), ..Default::default() };
        assert_eq!(session.visible_messages(0, &ui, 80).last().unwrap().timestamp_ms, 2000);
        assert_eq!(session.visible_messages(4000, &ui, 80).last().unwrap().timestamp_ms, 2000);
    }

    #[test]
    fn multipart_start_and_search_select_correct_video() {
        let mut session = manager();
        assert_eq!(session.video_start_ms("/fixtures/second.mp4"), 11000);
        session.messages.push(ChatMessage { timestamp_ms:12500, author:"user".into(), display_name:"user".into(), color_hex:String::new(), text:"find target".into(), badges:vec![], twitch_emotes_raw:String::new(), is_gap:false });
        let result = session.search_for_video("target", "/fixtures/first.mp4", 0, 10);
        assert_eq!(result[0].sec, 1.5);
        assert!(result[0].video_path.as_ref().unwrap().ends_with("second.mp4"));
        assert_eq!(session.cache_directory().unwrap(), PathBuf::from("/fixtures/custom-cache"));
        let saved = serde_json::to_value(&session.session_data).unwrap();
        assert_eq!(saved["start_detection_info"], "keep me");
        assert_eq!(saved["parts"][0]["custom_part"], true);
    }

    #[test]
    fn calibration_preserves_relative_parts_and_metadata_on_disk() {
        let dir = std::env::temp_dir().join(format!("mpv-session-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut session = manager();
        session.session_file = dir.join("session.json");
        assert!(session.calibrate_anchor(14000, 2.0, "/fixtures/second.mp4"));
        assert_eq!(session.session_data.session_start_ms, 2000);
        assert_eq!(session.session_data.parts[0].start_ms, 2000);
        assert_eq!(session.session_data.parts[1].start_ms, 12000);
        assert_eq!(session.session_data.parts[1].end_ms, Some(22000));
        let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&session.session_file).unwrap()).unwrap();
        assert_eq!(json["source_files"][0], "source.json");
        fs::remove_file(&session.session_file).unwrap(); fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn normalized_records_override_preserved_raw_and_no_future_leak() {
        let dir = std::env::temp_dir().join(format!("mpv-normalized-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("chat.jsonl");
        fs::write(&file, "{\"normalized\":true,\"ts\":1000,\"user\":\"user\",\"text\":\"cleaned\",\"raw\":\"@tmi-sent-ts=invalid :user!user@host PRIVMSG #channel :original\"}\n").unwrap();
        let mut session = manager(); session.messages = SessionManager::parse_chat_jsonl(&file);
        assert_eq!(session.messages[0].text, "cleaned");
        assert!(session.get_messages_up_to(999, 0, 80).is_empty());
        assert_eq!(session.get_messages_up_to(1000, 0, 80).len(), 1);
        fs::remove_file(file).unwrap(); fs::remove_dir(dir).unwrap();
    }
}

/// Parses Twitch IRC native emote spans from tag (e.g. 25:0-4,12-16/1902:6-10)
pub fn parse_twitch_emotes_tag(tag: &str) -> Vec<TwitchEmoteSpan> {
    let mut spans = Vec::new();
    if tag.is_empty() {
        return spans;
    }
    for item in tag.split('/') {
        if let Some((eid, occs)) = item.split_once(':') {
            for occ in occs.split(',') {
                if let Some((s_str, e_str)) = occ.split_once('-') {
                    if let (Ok(s), Ok(e)) = (s_str.parse::<usize>(), e_str.parse::<usize>()) {
                        if s <= e {
                            spans.push(TwitchEmoteSpan {
                                start: s,
                                end: e,
                                emote_id: eid.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }
    spans.sort_by_key(|s| s.start);
    spans
}

pub fn format_hms(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as i64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, sec)
    } else {
        format!("{:02}:{:02}", m, sec)
    }
}

pub fn swap_keyboard_layout(input: &str) -> String {
    const EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,./`QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>?~";
    const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбю.ёЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮ,Ё";

    let en_chars: Vec<char> = EN.chars().collect();
    let ru_chars: Vec<char> = RU.chars().collect();

    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        if let Some(pos) = en_chars.iter().position(|&ec| ec == c) {
            out.push(ru_chars[pos]);
        } else if let Some(pos) = ru_chars.iter().position(|&rc| rc == c) {
            out.push(en_chars[pos]);
        } else {
            out.push(c);
        }
    }
    out
}

