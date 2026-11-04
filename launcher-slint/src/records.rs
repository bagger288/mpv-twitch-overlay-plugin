use chrono::NaiveDateTime;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub dir: PathBuf,
    pub channel: String,
    pub date: Option<NaiveDateTime>,
    pub date_str: String,
    pub parts_count: usize,
    pub duration_sec: f64,
    pub size_bytes: u64,
    pub has_chat: bool,
    pub has_emotes: bool,
    pub session_file: Option<PathBuf>,
    pub video_parts: Vec<PathBuf>,
}

pub fn format_duration(seconds: f64) -> String {
    if seconds <= 0.0 {
        return "0m 00s".to_string();
    }
    let total_s = seconds as u64;
    let h = total_s / 3600;
    let m = (total_s % 3600) / 60;
    let s = total_s % 60;
    if h > 0 {
        format!("{}h {:02}m {:02}s", h, m, s)
    } else {
        format!("{}m {:02}s", m, s)
    }
}

pub fn format_bytes(num_bytes: u64) -> String {
    if num_bytes < 1024 * 1024 {
        format!("{:.1} KB", num_bytes as f64 / 1024.0)
    } else if num_bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", num_bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", num_bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

pub fn parse_channel_from_folder(folder_name: &str) -> (String, Option<NaiveDateTime>) {
    let re = Regex::new(r"^(.*)_(\d{2}-\d{2}-\d{4}_\d{2}-\d{2}-\d{2})$").unwrap();
    if let Some(caps) = re.captures(folder_name) {
        let channel = caps.get(1).unwrap().as_str().to_string();
        let ts_str = caps.get(2).unwrap().as_str();
        if let Ok(dt) = NaiveDateTime::parse_from_str(ts_str, "%d-%m-%Y_%H-%M-%S") {
            return (channel, Some(dt));
        }
    }
    (folder_name.to_string(), None)
}

pub fn scan_records(records_base: &Path) -> HashMap<String, Vec<SessionInfo>> {
    let mut channels: HashMap<String, Vec<SessionInfo>> = HashMap::new();
    if !records_base.exists() || !records_base.is_dir() {
        return channels;
    }

    let channel_name_re = Regex::new(r"^[A-Za-z0-9_]{1,25}$").unwrap();

    let entries = match fs::read_dir(records_base) {
        Ok(e) => e,
        Err(_) => return channels,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let folder_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        let (ch_name, dt_opt) = parse_channel_from_folder(folder_name);
        if dt_opt.is_none() || !channel_name_re.is_match(&ch_name) {
            continue;
        }

        let session_file = path.join(format!("{}.session.json", folder_name));
        if !session_file.is_file() {
            continue;
        }

        // Validate session manifest
        let manifest_content = match fs::read_to_string(&session_file) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let manifest: Value = match serde_json::from_str(&manifest_content) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if !manifest.is_object() {
            continue;
        }

        let m_channel = manifest.get("channel").and_then(|v| v.as_str()).unwrap_or("");
        if !m_channel.eq_ignore_ascii_case(&ch_name) {
            continue;
        }

        if manifest.get("session_start_ms").and_then(|v| v.as_i64()).is_none() {
            continue;
        }

        let mut chat_file = None;
        let mut emotes_file = None;
        let mut video_parts: Vec<PathBuf> = Vec::new();
        let mut total_bytes: u64 = 0;

        if let Ok(dir_entries) = fs::read_dir(&path) {
            for item in dir_entries.flatten() {
                let p = item.path();
                if let Ok(meta) = p.metadata() {
                    if meta.is_file() {
                        total_bytes += meta.len();
                        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        if name.ends_with(".chat.jsonl") || name == "chat.jsonl" {
                            chat_file = Some(p.clone());
                        } else if name.ends_with(".emotes.json") || name == "emotes.json" {
                            emotes_file = Some(p.clone());
                        } else if name.ends_with(".ts") || name.ends_with(".mp4") || name.ends_with(".mkv") {
                            video_parts.push(p.clone());
                        }
                    }
                }
            }
        }

        video_parts.sort();

        let mut duration_sec = 0.0;
        let mut parts_count = video_parts.len();

        if let Some(parts) = manifest.get("parts").and_then(|v| v.as_array()) {
            if !parts.is_empty() {
                parts_count = parts.len();
                let mut referenced_videos = Vec::new();
                for part in parts {
                    if let Some(file_str) = part.get("file").and_then(|f| f.as_str()) {
                        let f_path = PathBuf::from(file_str);
                        let resolved = if f_path.is_absolute() {
                            f_path
                        } else {
                            path.join(f_path)
                        };
                        referenced_videos.push(resolved);
                    }
                    let start_ms = part.get("start_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let end_ms = part.get("end_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    if end_ms > start_ms {
                        duration_sec += (end_ms - start_ms) / 1000.0;
                    }
                }
                if !referenced_videos.is_empty() {
                    video_parts = referenced_videos;
                }
            }
        }

        let date_str = dt_opt
            .map(|dt| dt.format("%d.%m.%Y %H:%M:%S").to_string())
            .unwrap_or_else(|| folder_name.to_string());

        let info = SessionInfo {
            dir: path,
            channel: ch_name.clone(),
            date: dt_opt,
            date_str,
            parts_count,
            duration_sec,
            size_bytes: total_bytes,
            has_chat: chat_file.is_some(),
            has_emotes: emotes_file.is_some(),
            session_file: Some(session_file),
            video_parts,
        };

        channels.entry(ch_name).or_default().push(info);
    }

    // Sort recordings newest first
    for sessions in channels.values_mut() {
        sessions.sort_by(|a, b| b.date.cmp(&a.date));
    }

    channels
}
