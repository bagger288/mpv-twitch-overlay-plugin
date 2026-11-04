use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use regex::Regex;
use serde_json::json;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

pub async fn fetch_day_log(channel: &str, year: &str, month: &str, day: &str) -> Result<String, String> {
    let url = format!(
        "https://bestlogs.lurkology.com/channel/{}/{}/{}/{}",
        channel.to_lowercase(),
        year,
        month,
        day
    );

    let client = reqwest::Client::new();
    let res = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Сетевая ошибка при запросе {}: {}", url, e))?;

    if !res.status().is_success() {
        return Err(format!("Архив вернул HTTP {} для {}", res.status(), url));
    }

    res.text()
        .await
        .map_err(|e| format!("Не удалось прочитать ответ: {}", e))
}

pub struct ImportParams {
    pub channel: String,
    pub dates: Vec<NaiveDate>,
    pub video_paths: Vec<PathBuf>,
    pub start_dt: Option<DateTime<Utc>>,
    pub auto_detect: bool,
    pub records_dir: PathBuf,
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

    let mut all_logs = Vec::new();
    for date in &params.dates {
        let y = date.format("%Y").to_string();
        let m = date.format("%m").to_string();
        let d = date.format("%d").to_string();
        log_fn(format!("Загрузка архива за {}...", date));

        match fetch_day_log(&channel, &y, &m, &d).await {
            Ok(content) => all_logs.push(content),
            Err(e) => log_fn(format!("Предупреждение: {}", e)),
        }
    }

    if all_logs.is_empty() {
        return Err("Не удалось загрузить ни одного лога за указанные даты.".to_string());
    }

    // Determine session start
    let session_start_dt = if let Some(st) = params.start_dt {
        st
    } else {
        Utc::now()
    };

    let folder_name = format!(
        "{}_{}",
        channel,
        session_start_dt.format("%d-%m-%Y_%H-%M-%S")
    );
    let session_dir = params.records_dir.join(&folder_name);
    fs::create_dir_all(&session_dir)
        .map_err(|e| format!("Не удалось создать директорию сессии: {}", e))?;

    let session_file = session_dir.join(format!("{}.session.json", folder_name));
    let chat_file = session_dir.join(format!("{}.chat.jsonl", folder_name));

    // Parse messages into chat.jsonl
    let msg_re = Regex::new(r"^\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\]\s+([^:]+):\s+(.*)$").unwrap();
    let mut total_messages = 0;

    let mut chat_writer = File::create(&chat_file)
        .map_err(|e| format!("Не удалось создать chat.jsonl: {}", e))?;

    let session_start_ms = session_start_dt.timestamp_millis();

    for log_chunk in all_logs {
        for line in log_chunk.lines() {
            if let Some(caps) = msg_re.captures(line) {
                let dt_str = caps.get(1).unwrap().as_str();
                let username = caps.get(2).unwrap().as_str();
                let message = caps.get(3).unwrap().as_str();

                if let Ok(naive_dt) = NaiveDateTime::parse_from_str(dt_str, "%Y-%m-%d %H:%M:%S") {
                    let msg_time = DateTime::<Utc>::from_naive_utc_and_offset(naive_dt, Utc);
                    let msg_ms = msg_time.timestamp_millis();
                    let offset_sec = (msg_ms - session_start_ms) as f64 / 1000.0;

                    let chat_record = json!({
                        "timestamp": msg_time.to_rfc3339(),
                        "time_ms": msg_ms,
                        "offset_sec": offset_sec,
                        "username": username,
                        "display_name": username,
                        "message": message,
                        "badges": {},
                        "emotes": {}
                    });

                    if let Ok(json_line) = serde_json::to_string(&chat_record) {
                        let _ = writeln!(chat_writer, "{}", json_line);
                        total_messages += 1;
                    }
                }
            }
        }
    }

    // Build session manifest
    let mut parts_json = Vec::new();
    for vp in &params.video_paths {
        if let Some(file_name) = vp.file_name().and_then(|f| f.to_str()) {
            parts_json.push(json!({
                "file": file_name,
                "start_ms": session_start_ms,
                "end_ms": session_start_ms + 1000,
            }));
        }
    }

    let manifest = json!({
        "channel": channel,
        "room_id": null,
        "session_start_ms": session_start_ms,
        "parts": parts_json,
        "offsets_ms": {},
        "import_info": {
            "messages_count": total_messages,
            "created_at": Utc::now().to_rfc3339(),
        }
    });

    let manifest_str = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("Ошибка сериализации session.json: {}", e))?;
    fs::write(&session_file, manifest_str)
        .map_err(|e| format!("Не удалось сохранить session.json: {}", e))?;

    log_fn(format!(
        "Импорт завершен! Импортировано сообщений: {}. Сессия: {}",
        total_messages, folder_name
    ));

    Ok(session_dir)
}
