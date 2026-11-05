use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

const BOT_NAMES: &[&str] = &[
    "nightbot",
    "streamelements",
    "moobot",
    "fossabot",
    "stayhydratedbot",
    "wizebot",
    "soundalerts",
    "streamlabs",
    "kofi_bot",
    "botisimo",
    "buttsbot",
    "creakbot",
    "streamraiders",
];

const USER_AGENT: &str = "TwitchLogImporter/1.0 (+https://github.com)";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRecord {
    pub normalized: bool,
    pub ts: i64,
    pub kind: String,
    #[serde(default)]
    pub msg_id: String,
    pub user: String,
    pub display: String,
    pub text: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub badges: String,
    #[serde(default)]
    pub badge_info: String,
    #[serde(default)]
    pub emotes: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub room_id: String,
}

pub struct ImportParams {
    pub channel: String,
    pub dates: Vec<NaiveDate>,
    pub video_paths: Vec<PathBuf>,
    pub start_dt: Option<DateTime<Utc>>,
    pub auto_detect: bool,
    pub records_dir: PathBuf,
}

/// Decode IRC tag escaped characters
fn decode_irc_tag_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    's' => out.push(' '),
                    ':' => out.push(';'),
                    'r' => out.push('\r'),
                    'n' => out.push('\n'),
                    '\\' => out.push('\\'),
                    other => out.push(other),
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse a raw IRC string line
pub fn parse_raw_irc(raw: &str) -> Option<ChatRecord> {
    let rest = raw.trim_end_matches(['\r', '\n']);
    let (tag_part, rest) = if rest.starts_with('@') {
        let (tags, rem) = rest[1..].split_once(' ')?;
        (Some(tags), rem.trim_start())
    } else {
        (None, rest)
    };

    let (prefix, rest) = if rest.starts_with(':') {
        let (pref, rem) = rest[1..].split_once(' ')?;
        (Some(pref), rem.trim_start())
    } else {
        (None, rest)
    };

    let (command_params, text) = if let Some((cp, body)) = rest.split_once(" :") {
        (cp, body.to_string())
    } else {
        (rest, String::new())
    };

    let mut parts = command_params.split_whitespace();
    let command = parts.next()?.to_uppercase();
    if command != "PRIVMSG" && command != "USERNOTICE" && command != "CLEARCHAT" && command != "CLEARMSG" && command != "NOTICE" {
        return None;
    }

    let mut tags_map = std::collections::HashMap::new();
    if let Some(t_str) = tag_part {
        for item in t_str.split(';') {
            if let Some((k, v)) = item.split_once('=') {
                tags_map.insert(k, decode_irc_tag_value(v));
            }
        }
    }

    let mut ts = 0i64;
    if let Some(tmi) = tags_map.get("tmi-sent-ts") {
        if let Ok(v) = tmi.parse::<i64>() {
            ts = v;
        }
    }
    if ts == 0 {
        return None;
    }

    let display = tags_map.get("display-name").cloned().unwrap_or_default();
    let login = if let Some(pref) = prefix {
        pref.split('!').next().unwrap_or("user").to_string()
    } else if !display.is_empty() {
        display.to_lowercase()
    } else {
        "user".to_string()
    };

    let display_final = if display.is_empty() { login.clone() } else { display };

    let kind = if tags_map.contains_key("target-user-id") || command == "CLEARCHAT" {
        "ban".to_string()
    } else if tags_map.contains_key("msg-id") || command == "USERNOTICE" {
        "event".to_string()
    } else if command == "CLEARMSG" {
        "delete".to_string()
    } else {
        "message".to_string()
    };

    Some(ChatRecord {
        normalized: true,
        ts,
        kind,
        msg_id: tags_map.get("msg-id").cloned().unwrap_or_default(),
        user: login,
        display: display_final,
        text,
        color: tags_map.get("color").cloned().unwrap_or_default(),
        badges: tags_map.get("badges").cloned().unwrap_or_default(),
        badge_info: tags_map.get("badge-info").cloned().unwrap_or_default(),
        emotes: tags_map.get("emotes").cloned().unwrap_or_default(),
        id: tags_map.get("id").cloned().unwrap_or_default(),
        room_id: tags_map.get("room-id").cloned().unwrap_or_default(),
    })
}

/// Normalize bestlogs message item (JSON object, raw IRC, or plain text)
fn normalize_message_item(val: &Value) -> Option<ChatRecord> {
    if let Some(raw_str) = val.as_str() {
        if raw_str.starts_with('@') || raw_str.starts_with(':') {
            return parse_raw_irc(raw_str);
        }
        // Plain text fallback: [YYYY-MM-DD HH:MM:SS] user: message
        let re = regex::Regex::new(r"^\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\]\s+([^:]+):\s+(.*)$").ok()?;
        if let Some(caps) = re.captures(raw_str) {
            let dt_str = caps.get(1)?.as_str();
            let user = caps.get(2)?.as_str().to_string();
            let text = caps.get(3)?.as_str().to_string();
            let naive = NaiveDateTime::parse_from_str(dt_str, "%Y-%m-%d %H:%M:%S").ok()?;
            let ts = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc).timestamp_millis();
            return Some(ChatRecord {
                normalized: true,
                ts,
                kind: "message".to_string(),
                msg_id: String::new(),
                user: user.clone(),
                display: user,
                text,
                color: String::new(),
                badges: String::new(),
                badge_info: String::new(),
                emotes: String::new(),
                id: String::new(),
                room_id: String::new(),
            });
        }
        return None;
    }

    if let Some(obj) = val.as_object() {
        // If already normalized
        if obj.get("normalized").and_then(|v| v.as_bool()) == Some(true) {
            if let Ok(rec) = serde_json::from_value::<ChatRecord>(val.clone()) {
                return Some(rec);
            }
        }

        // Check if there is raw field
        if let Some(raw) = obj.get("raw").and_then(|r| r.as_str()) {
            if let Some(parsed) = parse_raw_irc(raw) {
                return Some(parsed);
            }
        }

        let tags = obj.get("tags").and_then(|t| t.as_object());

        // Extract timestamp
        let mut ts = 0i64;
        if let Some(t_ts) = tags.and_then(|t| t.get("tmi-sent-ts")) {
            if let Some(s) = t_ts.as_str() {
                ts = s.parse::<i64>().unwrap_or(0);
            } else if let Some(n) = t_ts.as_i64() {
                ts = n;
            }
        }
        if ts == 0 {
            if let Some(iso_str) = obj.get("timestamp").or_else(|| obj.get("time")).and_then(|v| v.as_str()) {
                if let Ok(dt) = DateTime::parse_from_rfc3339(iso_str) {
                    ts = dt.timestamp_millis();
                } else if let Ok(ndt) = NaiveDateTime::parse_from_str(iso_str, "%Y-%m-%dT%H:%M:%S%.fZ") {
                    ts = DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc).timestamp_millis();
                }
            }
        }
        if ts == 0 {
            return None;
        }

        let text = obj.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();

        let display = obj
            .get("displayName")
            .or_else(|| tags.and_then(|t| t.get("display-name")))
            .or_else(|| obj.get("display"))
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();

        let user = obj
            .get("user")
            .and_then(|u| {
                if let Some(s) = u.as_str() {
                    Some(s.to_string())
                } else if let Some(uo) = u.as_object() {
                    uo.get("login").or_else(|| uo.get("name")).and_then(|n| n.as_str()).map(|s| s.to_string())
                } else {
                    None
                }
            })
            .or_else(|| obj.get("channel").and_then(|c| c.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| {
                if !display.is_empty() {
                    display.to_lowercase()
                } else {
                    "user".to_string()
                }
            });

        let display_final = if display.is_empty() { user.clone() } else { display };

        let color = tags
            .and_then(|t| t.get("color"))
            .or_else(|| obj.get("color"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let badges = tags
            .and_then(|t| t.get("badges"))
            .or_else(|| obj.get("badges"))
            .and_then(|b| b.as_str())
            .unwrap_or("")
            .to_string();

        let badge_info = tags
            .and_then(|t| t.get("badge-info").or_else(|| t.get("badge_info")))
            .or_else(|| obj.get("badge_info"))
            .and_then(|b| b.as_str())
            .unwrap_or("")
            .to_string();

        let emotes = tags
            .and_then(|t| t.get("emotes"))
            .or_else(|| obj.get("emotes"))
            .and_then(|e| e.as_str())
            .unwrap_or("")
            .to_string();

        let id = tags
            .and_then(|t| t.get("id"))
            .or_else(|| obj.get("id"))
            .and_then(|i| i.as_str())
            .unwrap_or("")
            .to_string();

        let room_id = tags
            .and_then(|t| t.get("room-id").or_else(|| t.get("room_id")))
            .or_else(|| obj.get("room_id"))
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();

        let msg_id = tags
            .and_then(|t| t.get("msg-id").or_else(|| t.get("msg_id")))
            .or_else(|| obj.get("msg_id"))
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();

        let kind = if tags.and_then(|t| t.get("target-user-id")).is_some() {
            "ban".to_string()
        } else if !msg_id.is_empty() {
            "event".to_string()
        } else {
            "message".to_string()
        };

        return Some(ChatRecord {
            normalized: true,
            ts,
            kind,
            msg_id,
            user,
            display: display_final,
            text,
            color,
            badges,
            badge_info,
            emotes,
            id,
            room_id,
        });
    }

    None
}

/// Estimate stream start timestamp using message density
fn detect_stream_start_ms(messages: &[ChatRecord], target_date: Option<NaiveDate>) -> (i64, String) {
    if messages.is_empty() {
        return (Utc::now().timestamp_millis(), "Нет сообщений, использовано текущее время".to_string());
    }

    let mut candidates: Vec<&ChatRecord> = messages.iter().collect();
    candidates.sort_by_key(|m| m.ts);

    if let Some(td) = target_date {
        let filtered: Vec<&ChatRecord> = candidates
            .iter()
            .copied()
            .filter(|m| {
                if let Some(ndt) = DateTime::from_timestamp_millis(m.ts) {
                    ndt.date_naive() >= td
                } else {
                    false
                }
            })
            .collect();

        if !filtered.is_empty() {
            candidates = filtered;
        }
    }

    let human: Vec<&ChatRecord> = candidates
        .iter()
        .copied()
        .filter(|m| m.kind == "message" && !BOT_NAMES.contains(&m.user.to_lowercase().as_str()))
        .collect();

    let pool = if !human.is_empty() { &human } else { &candidates };

    // Sliding window of 120 seconds, >= 10 messages from >= 3 unique users
    const WINDOW_MS: i64 = 120_000;
    let mut window: VecDeque<&ChatRecord> = VecDeque::new();

    for msg in pool {
        window.push_back(msg);
        while let Some(front) = window.front() {
            if msg.ts - front.ts > WINDOW_MS {
                window.pop_front();
            } else {
                break;
            }
        }

        let mut unique_users = HashSet::new();
        for m in &window {
            if !m.user.is_empty() && m.user != "user" {
                unique_users.insert(&m.user);
            }
        }

        if window.len() >= 10 && unique_users.len() >= 3 {
            let start_ts = window.front().unwrap().ts;
            let start_dt = DateTime::from_timestamp_millis(start_ts).unwrap_or_else(Utc::now);
            return (
                start_ts,
                format!(
                    "Автоопределение по плотности сообщений: {} ({} сообщ. за 120с от {} польз.)",
                    start_dt.format("%Y-%m-%d %H:%M:%S UTC"),
                    window.len(),
                    unique_users.len()
                ),
            );
        }
    }

    // Fallback: first available message
    let fallback = pool.first().unwrap();
    let fallback_dt = DateTime::from_timestamp_millis(fallback.ts).unwrap_or_else(Utc::now);
    (
        fallback.ts,
        format!(
            "Первое доступное сообщение (нет всплеска плотности): {}",
            fallback_dt.format("%Y-%m-%d %H:%M:%S UTC")
        ),
    )
}

/// Probe video duration in milliseconds using ffprobe
pub fn probe_video_duration_ms(path: &Path) -> Option<i64> {
    let output = std::process::Command::new("ffprobe")
        .args(&[
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json_val: Value = serde_json::from_slice(&output.stdout).ok()?;
    let dur_str = json_val
        .get("format")?
        .get("duration")?
        .as_str()?;

    let dur_f: f64 = dur_str.parse().ok()?;
    if dur_f > 0.0 && dur_f.is_finite() {
        Some((dur_f * 1000.0).round() as i64)
    } else {
        None
    }
}

pub async fn fetch_day_log(
    client: &reqwest::Client,
    channel: &str,
    year: &str,
    month: &str,
    day: &str,
) -> Result<Value, String> {
    let url = format!(
        "https://bestlogs.lurkology.com/channel/{}/{}/{}/{}?jsonBasic=1",
        channel.to_lowercase(),
        year,
        month,
        day
    );

    let res = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Сетевая ошибка при запросе {}: {}", url, e))?;

    if !res.status().is_success() {
        return Err(format!("Архив вернул HTTP {} для {}", res.status(), url));
    }

    res.json::<Value>()
        .await
        .map_err(|e| format!("Не удалось разобрать JSON-ответ от архива {}: {}", url, e))
}

pub async fn run_import_task<F: Fn(String) + Send + 'static>(
    params: ImportParams,
    log_fn: F,
) -> Result<PathBuf, String> {
    let channel = params.channel.to_lowercase();
    log_fn(format!(
        "Начало импорта для канала {}; дат: {}",
        channel,
        params.dates.len()
    ));

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Ошибка создания HTTP клиента: {}", e))?;

    let mut day_sources = Vec::new();
    for date in &params.dates {
        let y = date.format("%Y").to_string();
        let m = date.format("%m").to_string();
        let d = date.format("%d").to_string();
        log_fn(format!("Загрузка архива за {}...", date));

        match fetch_day_log(&client, &channel, &y, &m, &d).await {
            Ok(content) => day_sources.push((format!("{}-{}-{}", y, m, d), content)),
            Err(e) => log_fn(format!("Предупреждение: {}", e)),
        }
    }

    if day_sources.is_empty() {
        return Err("Не удалось загрузить ни одного лога за указанные даты.".to_string());
    }

    log_fn("Парсинг и нормализация сообщений чата...".to_string());
    let mut all_messages = Vec::new();

    for (_, source) in &day_sources {
        if let Some(messages) = source.get("messages").and_then(|m| m.as_array()) {
            for item in messages {
                if let Some(record) = normalize_message_item(item) {
                    all_messages.push(record);
                }
            }
        } else if let Some(arr) = source.as_array() {
            for item in arr {
                if let Some(record) = normalize_message_item(item) {
                    all_messages.push(record);
                }
            }
        }
    }

    if all_messages.is_empty() {
        return Err("В загруженных архивах не найдено ни одного сообщения.".to_string());
    }

    // Sort by timestamp
    all_messages.sort_by_key(|m| m.ts);

    // Deduplicate
    let mut seen_ids = HashSet::new();
    let mut unique_messages = Vec::with_capacity(all_messages.len());
    for msg in all_messages {
        let key = if !msg.id.is_empty() {
            format!("id:{}", msg.id)
        } else {
            format!("c:{}:{}:{}", msg.ts, msg.user, msg.text)
        };
        if seen_ids.insert(key) {
            unique_messages.push(msg);
        }
    }
    all_messages = unique_messages;

    log_fn(format!("Всего уникальных сообщений: {}", all_messages.len()));

    // Discover room_id
    let room_id = all_messages
        .iter()
        .find(|m| !m.room_id.is_empty())
        .map(|m| m.room_id.clone());

    if let Some(rid) = &room_id {
        log_fn(format!("Определен room_id канала: {}", rid));
    }

    // Determine session start
    let target_date = params.dates.first().copied();
    let (session_start_ms, detection_info) = if let Some(st) = params.start_dt {
        (st.timestamp_millis(), "Указано ручное время старта".to_string())
    } else if params.auto_detect {
        detect_stream_start_ms(&all_messages, target_date)
    } else {
        (all_messages[0].ts, "Первое сообщение архива".to_string())
    };

    log_fn(format!("[Старт] {}", detection_info));

    let session_start_dt = DateTime::from_timestamp_millis(session_start_ms)
        .unwrap_or_else(Utc::now);

    let folder_name = format!(
        "{}_{}",
        channel,
        session_start_dt.format("%d-%m-%Y_%H-%M-%S")
    );
    let session_dir = params.records_dir.join(&folder_name);
    fs::create_dir_all(&session_dir)
        .map_err(|e| format!("Не удалось создать директорию сессии {}: {}", session_dir.display(), e))?;

    let session_file = session_dir.join(format!("{}.session.json", folder_name));
    let chat_file = session_dir.join(format!("{}.chat.jsonl", folder_name));

    // Probe video parts
    let mut parts_json = Vec::new();
    let mut offsets_map = serde_json::Map::new();
    let mut cursor = session_start_ms;
    let mut total_video_duration_ms = 0i64;
    let mut has_video_durations = false;

    if !params.video_paths.is_empty() {
        log_fn(format!("Анализ видеофайлов ({}) с помощью ffprobe...", params.video_paths.len()));
        for (i, vp) in params.video_paths.iter().enumerate() {
            let canonical = vp.canonicalize().unwrap_or_else(|_| vp.clone());
            let file_str = canonical.to_string_lossy().to_string();
            let duration = probe_video_duration_ms(&canonical);

            let end_ms = if let Some(dur) = duration {
                total_video_duration_ms += dur;
                has_video_durations = true;
                log_fn(format!(
                    "  [Видео {}] {} -> длительность {:.1} с",
                    i + 1,
                    canonical.file_name().unwrap_or_default().to_string_lossy(),
                    dur as f64 / 1000.0
                ));
                Some(cursor + dur)
            } else {
                log_fn(format!(
                    "  [Видео {}] {} -> длительность не определена",
                    i + 1,
                    canonical.file_name().unwrap_or_default().to_string_lossy()
                ));
                None
            };

            parts_json.push(json!({
                "file": file_str,
                "start_ms": cursor,
                "end_ms": end_ms,
            }));

            offsets_map.insert(file_str, json!(0));

            if let Some(end) = end_ms {
                cursor = end;
            }
        }
    }

    // Clip chat to video duration if known
    if has_video_durations && total_video_duration_ms > 0 {
        let clip_start = session_start_ms;
        let clip_end = cursor;
        let before_count = all_messages.len();
        all_messages.retain(|m| m.ts >= clip_start && m.ts < clip_end);
        log_fn(format!(
            "Чат синхронизирован с видео: оставлено {} сообщений из {} (от старта до окончания видео)",
            all_messages.len(),
            before_count
        ));
    }

    // Write chat.jsonl
    log_fn(format!("Запись {} сообщений в {}...", all_messages.len(), chat_file.file_name().unwrap().to_string_lossy()));
    let mut chat_writer = File::create(&chat_file)
        .map_err(|e| format!("Не удалось создать chat.jsonl: {}", e))?;

    for msg in &all_messages {
        let line = serde_json::to_string(msg)
            .map_err(|e| format!("Ошибка сериализации сообщения: {}", e))?;
        writeln!(chat_writer, "{}", line)
            .map_err(|e| format!("Ошибка записи в chat.jsonl: {}", e))?;
    }

    // Write day source dumps for reprocessing
    let mut source_files = Vec::new();
    for (i, (_, source)) in day_sources.iter().enumerate() {
        let source_fn = format!("{}.day_{}.source.json", folder_name, i + 1);
        let source_path = session_dir.join(&source_fn);
        if let Ok(content) = serde_json::to_string_pretty(source) {
            let _ = fs::write(&source_path, content);
            source_files.push(source_fn);
        }
    }

    // Write session.json manifest
    let manifest = json!({
        "channel": channel,
        "room_id": room_id,
        "session_start_ms": session_start_ms,
        "start_time_iso": session_start_dt.to_rfc3339(),
        "start_detection_info": detection_info,
        "target_date": target_date.map(|d| d.format("%Y-%m-%d").to_string()),
        "timezone": "UTC",
        "parts": parts_json,
        "offsets_ms": Value::Object(offsets_map),
        "emote_cache_dir": "emote_cache",
        "source_files": source_files,
        "imported_message_count": all_messages.len(),
        "created_at": Utc::now().to_rfc3339(),
    });

    let manifest_str = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("Ошибка сериализации session.json: {}", e))?;
    fs::write(&session_file, manifest_str)
        .map_err(|e| format!("Не удалось сохранить session.json: {}", e))?;

    log_fn(format!(
        "Импорт успешно завершен! Сессия: {}, сообщений: {}",
        folder_name,
        all_messages.len()
    ));

    Ok(session_dir)
}
