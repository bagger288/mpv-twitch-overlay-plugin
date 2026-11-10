use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::emotes;

pub static REC_ACTIVE_SESSION_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

#[derive(Debug, Clone)]
pub struct RecorderConfig {
    pub channel: String,
    pub quality: String,
    pub records_dir: PathBuf,
    pub emote_cache_dir: PathBuf,
    pub streamlink_bin: String,
    pub proxy_playlist: String,
    pub twitch_client_id: String,
    pub twitch_app_token: String,
    pub reconnect_delay_sec: u64,
    pub offline_timeout_min: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartInfo {
    pub file: String,
    pub start_ms: i64,
    #[serde(default)]
    pub end_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionManifest {
    pub channel: String,
    #[serde(default)]
    pub room_id: Option<String>,
    pub session_start_ms: i64,
    pub parts: Vec<PartInfo>,
    pub offsets_ms: HashMap<String, i64>,
}

fn format_timestamp(dt: DateTime<Local>) -> String {
    dt.format("%d-%m-%Y_%H-%M-%S").to_string()
}

fn resolve_quality(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "best" {
        "best,1080p60,1080p,720p60,720p,480p,worst".to_string()
    } else if trimmed.contains(',') {
        trimmed.to_string()
    } else {
        format!("{},worst", trimmed)
    }
}

fn save_session(path: &Path, manifest: &SessionManifest) {
    if let Ok(json_str) = serde_json::to_string_pretty(manifest) {
        let _ = fs::write(path, json_str);
    }
}

pub async fn run_live_recorder<F: Fn(String) + Send + Sync + Clone + 'static>(
    config: RecorderConfig,
    stop_signal: Arc<AtomicBool>,
    log_fn: F,
) {
    let channel = config.channel.trim().trim_start_matches('#').to_lowercase();
    if channel.is_empty() {
        log_fn("[ERROR] Не указан канал Twitch!".to_string());
        return;
    }

    let now_local = Local::now();
    let session_prefix = format!("{}_{}", channel, format_timestamp(now_local));
    let session_dir = config.records_dir.join(&session_prefix);

    if let Err(e) = fs::create_dir_all(&session_dir) {
        log_fn(format!("[ERROR] Не удалось создать папку сессии {}: {}", session_dir.display(), e));
        return;
    }

    *REC_ACTIVE_SESSION_DIR.lock().unwrap() = Some(session_dir.clone());

    let session_file = session_dir.join(format!("{}.session.json", session_prefix));
    let chat_file = session_dir.join(format!("{}.chat.jsonl", session_prefix));

    let manifest = SessionManifest {
        channel: channel.clone(),
        room_id: None,
        session_start_ms: Utc::now().timestamp_millis(),
        parts: Vec::new(),
        offsets_ms: HashMap::new(),
    };
    save_session(&session_file, &manifest);

    let manifest_arc = Arc::new(Mutex::new(manifest));

    log_fn("==================================================".to_string());
    log_fn(format!(" [REC] Запись Twitch канала: {}", channel));
    log_fn(format!(" [REC] Папка сессии:        {}", session_dir.display()));
    log_fn(format!(" [REC] Файл чата:           {}", chat_file.display()));
    log_fn("==================================================".to_string());

    // 1. Launch Twitch IRC chat logger task (TLS 6697 with fallback)
    let chat_stop = stop_signal.clone();
    let chat_chan = channel.clone();
    let chat_file_clone = chat_file.clone();
    let session_file_for_chat = session_file.clone();
    let log_fn_chat = log_fn.clone();

    let chat_handle = tokio::spawn(async move {
        crate::chat_logger::run_chat_logger(
            chat_chan,
            chat_file_clone,
            Some(session_file_for_chat),
            chat_stop,
            log_fn_chat,
        )
        .await;
    });

    // 2. Run Streamlink video capture loop
    let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
    let reconnect_delay = if config.reconnect_delay_sec == 0 { 5 } else { config.reconnect_delay_sec };
    let offline_threshold_seconds = if config.offline_timeout_min == 0 { 600 } else { config.offline_timeout_min * 60 };
    let mut consecutive_offline_seconds: u64 = 0;

    log_fn(format!("[REC] Запуск видеозахвата Streamlink для #{}...", channel));

    while !stop_signal.load(Ordering::Relaxed) {
        let part_ts = format_timestamp(Local::now());
        let part_filename = format!("{}_{}.ts", channel, part_ts);
        let part_path = session_dir.join(&part_filename);

        let start_ms = Utc::now().timestamp_millis();
        let part_info = PartInfo {
            file: part_filename.clone(),
            start_ms,
            end_ms: None,
        };

        {
            let mut lock = manifest_arc.lock().unwrap();
            lock.parts.push(part_info);
            lock.offsets_ms.entry(part_filename.clone()).or_insert(0);
            save_session(&session_file, &lock);
        }

        let qual = resolve_quality(&config.quality);
        let target_url = format!("https://www.twitch.tv/{}", channel);

        log_fn(format!("[REC] Подключение Streamlink к {} (качество: {})...", target_url, qual));

        let mut cmd = Command::new(&config.streamlink_bin);
        cmd.arg(&target_url);
        cmd.arg(&qual);
        cmd.arg("-o");
        cmd.arg(&part_path);
        cmd.arg("--force");
        cmd.arg("--twitch-disable-ads");
        cmd.arg("--retry-streams");
        cmd.arg("3");
        cmd.arg("--retry-max");
        cmd.arg("3");

        if !config.proxy_playlist.trim().is_empty() {
            cmd.arg(format!("--twitch-proxy-playlist={}", config.proxy_playlist.trim()));
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                log_fn(format!("[ERROR] Не удалось запустить Streamlink '{}': {}. Проверьте путь в настройках.", config.streamlink_bin, e));
                break;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        if let Ok(mut lock) = child_arc.lock() {
            *lock = Some(child);
        }

        // Stdout reader
        let log_fn_out = log_fn.clone();
        if let Some(out) = stdout {
            std::thread::spawn(move || {
                let reader = BufReader::new(out);
                for line in reader.lines().flatten() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        log_fn_out(format!("[Streamlink] {}", trimmed));
                    }
                }
            });
        }

        // Stderr reader
        let log_fn_err = log_fn.clone();
        if let Some(err) = stderr {
            std::thread::spawn(move || {
                let reader = BufReader::new(err);
                for line in reader.lines().flatten() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        log_fn_err(format!("[Streamlink] {}", trimmed));
                    }
                }
            });
        }

        // Monitor streamlink execution
        loop {
            if stop_signal.load(Ordering::Relaxed) {
                if let Ok(mut lock) = child_arc.lock() {
                    if let Some(mut c) = lock.take() {
                        let _ = c.kill();
                    }
                }
                break;
            }

            let mut finished = false;
            if let Ok(mut lock) = child_arc.lock() {
                if let Some(c) = lock.as_mut() {
                    if let Ok(Some(_)) = c.try_wait() {
                        finished = true;
                    }
                } else {
                    finished = true;
                }
            }

            if finished {
                break;
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        let end_ms = Utc::now().timestamp_millis();
        let file_exists_with_data = fs::metadata(&part_path).map(|m| m.len() > 0).unwrap_or(false);

        {
            let mut lock = manifest_arc.lock().unwrap();
            if file_exists_with_data {
                if let Some(last) = lock.parts.last_mut() {
                    last.end_ms = Some(end_ms);
                }
                save_session(&session_file, &lock);

                let duration_sec = (end_ms - start_ms) as f64 / 1000.0;
                let size_kb = fs::metadata(&part_path).map(|m| m.len() / 1024).unwrap_or(0);
                log_fn(format!(
                    "[REC] Видео-фрагмент {} записан ({:.1}с, {} КБ).",
                    part_filename, duration_sec, size_kb
                ));
                consecutive_offline_seconds = 0;
            } else {
                log_fn(format!(
                    "[REC] Видео-данные не поступили для {} (стрим оффлайн или завершен).",
                    part_filename
                ));
                if part_path.exists() {
                    let _ = fs::remove_file(&part_path);
                }
                lock.parts.pop();
                lock.offsets_ms.remove(&part_filename);
                save_session(&session_file, &lock);

                consecutive_offline_seconds += reconnect_delay;
                if consecutive_offline_seconds >= offline_threshold_seconds {
                    log_fn(format!(
                        "[REC] Стример оффлайн более {} минут. Автоматическая остановка записи.",
                        config.offline_timeout_min
                    ));
                    break;
                }
            }
        }

        if stop_signal.load(Ordering::Relaxed) {
            break;
        }

        log_fn(format!("[REC] Ожидание {}с перед следующей попыткой подключения...", reconnect_delay));
        for _ in 0..(reconnect_delay * 2) {
            if stop_signal.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    // 3. Graceful shutdown
    stop_signal.store(true, Ordering::Relaxed);
    if let Ok(mut lock) = child_arc.lock() {
        if let Some(mut c) = lock.take() {
            let _ = c.kill();
        }
    }

    // Finalize open part end_ms if needed
    {
        let mut lock = manifest_arc.lock().unwrap();
        let now_ms = Utc::now().timestamp_millis();
        if let Some(last) = lock.parts.last_mut() {
            if last.end_ms.is_none() {
                last.end_ms = Some(now_ms);
            }
        }
        save_session(&session_file, &lock);
    }

    let _ = chat_handle.await;

    // Update message count in session.json
    let msg_count = crate::chat_logger::count_chat_messages(&chat_file);
    crate::chat_logger::update_session_message_count(&session_file, msg_count);

    // 4. Cache emotes and badges automatically
    log_fn(format!("[REC] Запись завершена. Записано сообщений чата: {}. Загрузка и кэширование эмоутов...", msg_count));
    let room_id_opt = {
        let lock = manifest_arc.lock().unwrap();
        lock.room_id.clone()
    };

    let client_id_opt = if config.twitch_client_id.trim().is_empty() {
        None
    } else {
        Some(config.twitch_client_id.as_str())
    };

    let app_token_opt = if config.twitch_app_token.trim().is_empty() {
        None
    } else {
        Some(config.twitch_app_token.as_str())
    };

    let _ = emotes::ensure_session_emotes(
        &session_file,
        &channel,
        room_id_opt.as_deref(),
        client_id_opt,
        app_token_opt,
        &config.emote_cache_dir,
        log_fn.clone(),
    )
    .await;

    *REC_ACTIVE_SESSION_DIR.lock().unwrap() = None;
    log_fn(format!("[REC] Сессия успешно сохранена в {}", session_dir.display()));
}
