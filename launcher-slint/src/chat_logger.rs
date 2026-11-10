use chrono::Utc;
use native_tls::TlsConnector;
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

pub static LIVE_CHAT_COUNT: AtomicU64 = AtomicU64::new(0);
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::net::TcpStream;
use tokio_native_tls::TlsConnector as TokioTlsConnector;

use crate::importer;

enum IrcReader {
    Tls(TokioBufReader<tokio::io::ReadHalf<tokio_native_tls::TlsStream<TcpStream>>>),
    Plain(TokioBufReader<tokio::io::ReadHalf<TcpStream>>),
}

enum IrcWriter {
    Tls(tokio::io::WriteHalf<tokio_native_tls::TlsStream<TcpStream>>),
    Plain(tokio::io::WriteHalf<TcpStream>),
}

impl IrcReader {
    async fn read_line(&mut self, buf: &mut String) -> std::io::Result<usize> {
        match self {
            IrcReader::Tls(r) => r.read_line(buf).await,
            IrcReader::Plain(r) => r.read_line(buf).await,
        }
    }
}

impl IrcWriter {
    async fn write_all(&mut self, src: &[u8]) -> std::io::Result<()> {
        match self {
            IrcWriter::Tls(w) => w.write_all(src).await,
            IrcWriter::Plain(w) => w.write_all(src).await,
        }
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        match self {
            IrcWriter::Tls(w) => w.flush().await,
            IrcWriter::Plain(w) => w.flush().await,
        }
    }
}

/// Extract Twitch room-id from IRC line tags (works on ROOMSTATE, USERNOTICE, PRIVMSG, etc.)
pub fn extract_room_id(line: &str) -> Option<String> {
    if !line.starts_with('@') {
        return None;
    }
    let (tag_part, _) = line[1..].split_once(' ')?;
    for item in tag_part.split(';') {
        if let Some((k, v)) = item.split_once('=') {
            if k == "room-id" && !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Update room_id in session.json manifest if not already populated
pub fn update_session_room_id(session_file: &Path, room_id: &str) {
    if !session_file.is_file() {
        return;
    }
    if let Ok(content) = fs::read_to_string(session_file) {
        if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&content) {
            let cur_room = val.get("room_id").and_then(|r| r.as_str()).unwrap_or("");
            if cur_room.is_empty() {
                val["room_id"] = serde_json::Value::String(room_id.to_string());
                if let Ok(updated) = serde_json::to_string_pretty(&val) {
                    let _ = fs::write(session_file, updated);
                }
            }
        }
    }
}

/// Update imported_message_count in session.json
pub fn update_session_message_count(session_file: &Path, count: usize) {
    if !session_file.is_file() {
        return;
    }
    if let Ok(content) = fs::read_to_string(session_file) {
        if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&content) {
            val["imported_message_count"] = json!(count);
            if let Ok(updated) = serde_json::to_string_pretty(&val) {
                let _ = fs::write(session_file, updated);
            }
        }
    }
}

/// Count lines in chat.jsonl
pub fn count_chat_messages(chat_file: &Path) -> usize {
    if let Ok(file) = fs::File::open(chat_file) {
        let reader = std::io::BufReader::new(file);
        reader.lines().count()
    } else {
        0
    }
}

/// Twitch Chat Logger (IRC over TLS on port 6697, with automatic fallback to port 6667)
/// Logs messages to chat.jsonl in normalized format identical to the Import tab.
/// Automatically logs GAP entries upon reconnects, discovers room-id, and flushes output continuously.
pub async fn run_chat_logger<F: Fn(String) + Send + Sync + Clone + 'static>(
    channel: String,
    chat_file: PathBuf,
    session_file: Option<PathBuf>,
    stop_signal: Arc<AtomicBool>,
    log_fn: F,
) {
    let chan = channel.trim().trim_start_matches('#').to_lowercase();
    if chan.is_empty() {
        return;
    }

    LIVE_CHAT_COUNT.store(0, Ordering::Relaxed);

    if let Some(parent) = chat_file.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let mut last_disconnect_time: Option<Instant> = None;
    let mut backoff_sec: u64 = 3;
    let max_backoff_sec: u64 = 60;
    let mut discovered_room_id = false;

    // Check if room_id is already in session_file
    if let Some(ref sf) = session_file {
        if sf.is_file() {
            if let Ok(content) = fs::read_to_string(sf) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(rid) = val.get("room_id").and_then(|r| r.as_str()) {
                        if !rid.is_empty() {
                            discovered_room_id = true;
                        }
                    }
                }
            }
        }
    }

    log_fn(format!("[IRC] Старт логгера чата для канала #{chan} -> {}", chat_file.file_name().unwrap_or_default().to_string_lossy()));

    while !stop_signal.load(Ordering::Relaxed) {
        // 1. Establish connection: try TLS on 6697 first, then fallback to 6667
        let (mut reader, mut writer) = match connect_irc(&chan, &log_fn).await {
            Ok(pair) => {
                backoff_sec = 3;
                pair
            }
            Err(e) => {
                if stop_signal.load(Ordering::Relaxed) {
                    break;
                }
                log_fn(format!("[IRC] Ошибка подключения к чату #{chan}: {e}. Повтор через {backoff_sec}с..."));
                let now_ms = Utc::now().timestamp_millis();
                log_gap(&chat_file, now_ms, None, &format!("connection_error: {e}"));
                last_disconnect_time = Some(Instant::now());
                tokio::time::sleep(Duration::from_secs(backoff_sec)).await;
                backoff_sec = (backoff_sec * 2).min(max_backoff_sec);
                continue;
            }
        };

        // 2. Log GAP event if reconnected after a break
        if let Some(disc) = last_disconnect_time.take() {
            let duration_ms = disc.elapsed().as_millis() as i64;
            if duration_ms > 2000 {
                let now_ms = Utc::now().timestamp_millis();
                let since_ms = now_ms.saturating_sub(duration_ms);
                log_gap(&chat_file, now_ms, Some(duration_ms), "reconnect");
                log_fn(format!("[IRC] Зафиксирован разрыв связи чата ({} с). Добавлена GAP-запись.", duration_ms / 1000));
                let _ = since_ms;
            }
        }

        // 3. Send Twitch IRC handshake
        let nick_num = (Utc::now().timestamp_millis().abs() % 90000 + 10000).to_string();
        let handshake = format!(
            "CAP REQ :twitch.tv/tags twitch.tv/commands\r\nPASS SCHMOOPIIE\r\nNICK justinfan{nick_num}\r\nJOIN #{chan}\r\n"
        );

        if let Err(e) = writer.write_all(handshake.as_bytes()).await {
            log_fn(format!("[IRC] Ошибка отправки хендшейка: {e}"));
            last_disconnect_time = Some(Instant::now());
            tokio::time::sleep(Duration::from_secs(backoff_sec)).await;
            continue;
        }
        let _ = writer.flush().await;

        log_fn(format!("[IRC] Подключено к Twitch чату #{chan}. Идет запись сообщений..."));

        let mut file_opt = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&chat_file)
            .ok();

        // 4. Message receive loop
        let mut line_buf = String::new();
        while !stop_signal.load(Ordering::Relaxed) {
            line_buf.clear();
            let read_fut = reader.read_line(&mut line_buf);
            let line_res = tokio::time::timeout(Duration::from_secs(45), read_fut).await;

            let bytes_read = match line_res {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    log_fn(format!("[IRC] Ошибка чтения сокета: {e}"));
                    break;
                }
                Err(_) => {
                    // 45s keepalive ping
                    if writer.write_all(b"PING :tmi.twitch.tv\r\n").await.is_err() {
                        log_fn("[IRC] Соединение зависло (таймаут ответа). Переподключение...".to_string());
                        break;
                    }
                    let _ = writer.flush().await;
                    continue;
                }
            };

            if bytes_read == 0 {
                log_fn("[IRC] Соединение закрыто сервером Twitch.".to_string());
                break;
            }

            let trimmed = line_buf.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                continue;
            }

            // Handle PING
            if trimmed.starts_with("PING") {
                let payload = if trimmed.len() > 5 { &trimmed[5..] } else { ":tmi.twitch.tv" };
                let pong = format!("PONG {payload}\r\n");
                let _ = writer.write_all(pong.as_bytes()).await;
                let _ = writer.flush().await;
                continue;
            }

            // Handle Twitch RECONNECT request
            if trimmed.contains(" RECONNECT") {
                log_fn("[IRC] Получена команда RECONNECT от сервера Twitch.".to_string());
                break;
            }

            // Room ID discovery from any message tags (e.g. ROOMSTATE, USERNOTICE, PRIVMSG)
            if !discovered_room_id {
                if let Some(rid) = extract_room_id(trimmed) {
                    discovered_room_id = true;
                    log_fn(format!("[IRC] Обнаружен Twitch room_id канала: {rid}"));
                    if let Some(ref sf) = session_file {
                        update_session_room_id(sf, &rid);
                    }
                }
            }

            // Parse message into normalized ChatRecord (100% compatible with Import tab)
            if let Some(record) = importer::parse_raw_irc(trimmed) {
                LIVE_CHAT_COUNT.fetch_add(1, Ordering::Relaxed);
                if let Some(f) = file_opt.as_mut() {
                    if let Ok(json_str) = serde_json::to_string(&record) {
                        let _ = writeln!(f, "{}", json_str);
                        let _ = f.flush();
                    }
                }
            }
        }

        last_disconnect_time = Some(Instant::now());
        if !stop_signal.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_secs(backoff_sec)).await;
            backoff_sec = (backoff_sec * 2).min(max_backoff_sec);
        }
    }

    log_fn(format!("[IRC] Логгер чата для #{chan} завершил работу."));
}

async fn connect_irc<F: Fn(String) + Send + Sync + 'static>(
    channel: &str,
    log_fn: &F,
) -> Result<(IrcReader, IrcWriter), String> {
    // 1. Try TLS on 6697
    match connect_tls("irc.chat.twitch.tv", 6697).await {
        Ok((r, w)) => {
            log_fn(format!("[IRC] Защищенное соединение TLS установлено (порт 6697, #{channel})"));
            return Ok((IrcReader::Tls(r), IrcWriter::Tls(w)));
        }
        Err(e) => {
            log_fn(format!("[IRC] Не удалось подключиться через TLS 6697 ({e}). Пробуем стандартный порт 6667..."));
        }
    }

    // 2. Fallback to plain TCP on 6667
    let tcp = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect("irc.chat.twitch.tv:6667"))
        .await
        .map_err(|_| "Таймаут подключения к irc.chat.twitch.tv:6667".to_string())?
        .map_err(|e| format!("Ошибка подключения к 6667: {e}"))?;

    let (r, w) = tokio::io::split(tcp);
    Ok((IrcReader::Plain(TokioBufReader::new(r)), IrcWriter::Plain(w)))
}

async fn connect_tls(
    host: &str,
    port: u16,
) -> Result<(TokioBufReader<tokio::io::ReadHalf<tokio_native_tls::TlsStream<TcpStream>>>, tokio::io::WriteHalf<tokio_native_tls::TlsStream<TcpStream>>), String> {
    let tcp = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect((host, port)))
        .await
        .map_err(|_| format!("Таймаут TCP соединения с {host}:{port}"))?
        .map_err(|e| format!("Ошибка TCP соединения: {e}"))?;

    let connector = TlsConnector::builder()
        .build()
        .map_err(|e| format!("Ошибка создания TLS коннектора: {e}"))?;
    let tokio_connector = TokioTlsConnector::from(connector);

    let tls_stream = tokio::time::timeout(Duration::from_secs(10), tokio_connector.connect(host, tcp))
        .await
        .map_err(|_| format!("Таймаут TLS хендшейка с {host}:{port}"))?
        .map_err(|e| format!("Ошибка TLS хендшейка: {e}"))?;

    let (r, w) = tokio::io::split(tls_stream);
    Ok((TokioBufReader::new(r), w))
}

fn log_gap(chat_file: &Path, recv: i64, duration_ms: Option<i64>, reason: &str) {
    let mut gap_map = serde_json::Map::new();
    gap_map.insert("type".to_string(), json!("GAP"));
    gap_map.insert("recv".to_string(), json!(recv));
    gap_map.insert("reason".to_string(), json!(reason));
    if let Some(dur) = duration_ms {
        gap_map.insert("duration_ms".to_string(), json!(dur));
        gap_map.insert("since_ms".to_string(), json!(recv.saturating_sub(dur)));
    }

    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(chat_file) {
        let _ = writeln!(f, "{}", serde_json::Value::Object(gap_map));
        let _ = f.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_room_id() {
        let line = "@badge-info=;badges=;color=;room-id=98765432;tmi-sent-ts=1700000000000 :user!user@user.tmi.twitch.tv PRIVMSG #channel :hello";
        assert_eq!(extract_room_id(line), Some("98765432".to_string()));

        let roomstate = "@room-id=123456;slow=0 :tmi.twitch.tv ROOMSTATE #channel";
        assert_eq!(extract_room_id(roomstate), Some("123456".to_string()));
    }

    #[test]
    fn test_parse_irc_to_chat_record_compatibility() {
        let raw = "@badge-info=subscriber/12;badges=broadcaster/1,subscriber/12;color=#FF0000;display-name=TestUser;emotes=25:0-4;id=abc-123;mod=0;room-id=111;subscriber=1;tmi-sent-ts=1700000000123;turbo=0;user-id=999;user-type= :testuser!testuser@testuser.tmi.twitch.tv PRIVMSG #testchan :Kappa hello world";
        let record = importer::parse_raw_irc(raw).expect("Failed to parse IRC line");

        assert_eq!(record.user, "testuser");
        assert_eq!(record.display, "TestUser");
        assert_eq!(record.text, "Kappa hello world");
        assert_eq!(record.color, "#FF0000");
        assert_eq!(record.id, "abc-123");
        assert_eq!(record.msg_id, "");
        assert_eq!(record.room_id, "111");
        assert_eq!(record.ts, 1700000000123);
        assert_eq!(record.badges, "broadcaster/1,subscriber/12");
        assert_eq!(record.badge_info, "subscriber/12");
        assert_eq!(record.emotes, "25:0-4");

        let serialized = serde_json::to_string(&record).expect("Failed to serialize");
        assert!(serialized.contains("\"normalized\":true"));
        assert!(serialized.contains("\"kind\":\"message\""));
    }
}
