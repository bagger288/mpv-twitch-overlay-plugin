use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmoteMeta {
    pub source: String,
    pub id: String,
    pub url: String,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmoteSnapshot {
    pub channel: String,
    pub room_id: Option<String>,
    pub words: HashMap<String, EmoteMeta>,
    #[serde(default)]
    pub badges: HashMap<String, Value>,
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub cache_root: String,
}

pub async fn fetch_7tv_emotes(
    client: &reqwest::Client,
    room_id: Option<&str>,
    log_fn: &(impl Fn(String) + Send + Sync),
) -> HashMap<String, EmoteMeta> {
    let mut map = HashMap::new();

    // 1. Global
    log_fn("[7TV] Запрос глобальных эмоутов...".to_string());
    if let Ok(res) = client
        .get("https://7tv.io/v3/emote-sets/global")
        .send()
        .await
    {
        if let Ok(v) = res.json::<Value>().await {
            parse_7tv_set(&v, "global", &mut map);
        }
    }

    // 2. Channel
    if let Some(rid) = room_id {
        if !rid.is_empty() {
            log_fn(format!("[7TV] Запрос эмоутов канала (room_id: {})...", rid));
            let url = format!("https://7tv.io/v3/users/twitch/{}", rid);
            if let Ok(res) = client.get(&url).send().await {
                if let Ok(v) = res.json::<Value>().await {
                    if let Some(set) = v.get("emote_set") {
                        parse_7tv_set(set, "channel", &mut map);
                    }
                }
            }
        }
    }

    log_fn(format!("[7TV] Найдено эмоутов: {}", map.len()));
    map
}

fn parse_7tv_set(val: &Value, scope: &str, out: &mut HashMap<String, EmoteMeta>) {
    if let Some(emotes) = val.get("emotes").and_then(|e| e.as_array()) {
        for item in emotes {
            let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let id = item.get("id").and_then(|i| i.as_str()).unwrap_or("");
            if name.is_empty() || id.is_empty() {
                continue;
            }

            if let Some(data) = item.get("data") {
                if let Some(host) = data.get("host") {
                    let base_url = host.get("url").and_then(|u| u.as_str()).unwrap_or("");
                    if let Some(files) = host.get("files").and_then(|f| f.as_array()) {
                        let candidates = ["2x.webp", "1x.webp", "2x.gif", "1x.gif", "2x.png", "1x.png"];
                        let mut selected_file = None;
                        for cand in candidates {
                            if files.iter().any(|f| f.get("name").and_then(|n| n.as_str()) == Some(cand)) {
                                selected_file = Some(cand);
                                break;
                            }
                        }

                        if let Some(file_name) = selected_file {
                            let url = format!("https:{}/{}", base_url.trim_start_matches(':'), file_name);
                            out.insert(
                                name.to_string(),
                                EmoteMeta {
                                    source: "7tv".to_string(),
                                    id: id.to_string(),
                                    url,
                                    scope: scope.to_string(),
                                },
                            );
                        }
                    }
                }
            }
        }
    }
}

pub async fn fetch_bttv_emotes(
    client: &reqwest::Client,
    room_id: Option<&str>,
    log_fn: &(impl Fn(String) + Send + Sync),
) -> HashMap<String, EmoteMeta> {
    let mut map = HashMap::new();

    // 1. Global
    log_fn("[BTTV] Запрос глобальных эмоутов...".to_string());
    if let Ok(res) = client
        .get("https://api.betterttv.net/3/cached/emotes/global")
        .send()
        .await
    {
        if let Ok(arr) = res.json::<Vec<Value>>().await {
            for item in arr {
                if let (Some(code), Some(id)) = (
                    item.get("code").and_then(|c| c.as_str()),
                    item.get("id").and_then(|i| i.as_str()),
                ) {
                    map.insert(
                        code.to_string(),
                        EmoteMeta {
                            source: "bttv".to_string(),
                            id: id.to_string(),
                            url: format!("https://cdn.betterttv.net/emote/{}/2x.png", id),
                            scope: "global".to_string(),
                        },
                    );
                }
            }
        }
    }

    // 2. Channel
    if let Some(rid) = room_id {
        if !rid.is_empty() {
            log_fn(format!("[BTTV] Запрос эмоутов канала (room_id: {})...", rid));
            let url = format!("https://api.betterttv.net/3/cached/users/twitch/{}", rid);
            if let Ok(res) = client.get(&url).send().await {
                if let Ok(v) = res.json::<Value>().await {
                    let mut channel_items = Vec::new();
                    if let Some(arr) = v.get("channelEmotes").and_then(|a| a.as_array()) {
                        channel_items.extend(arr.iter());
                    }
                    if let Some(arr) = v.get("sharedEmotes").and_then(|a| a.as_array()) {
                        channel_items.extend(arr.iter());
                    }
                    for item in channel_items {
                        if let (Some(code), Some(id)) = (
                            item.get("code").and_then(|c| c.as_str()),
                            item.get("id").and_then(|i| i.as_str()),
                        ) {
                            map.insert(
                                code.to_string(),
                                EmoteMeta {
                                    source: "bttv".to_string(),
                                    id: id.to_string(),
                                    url: format!("https://cdn.betterttv.net/emote/{}/2x.png", id),
                                    scope: "channel".to_string(),
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    log_fn(format!("[BTTV] Найдено эмоутов: {}", map.len()));
    map
}

pub async fn fetch_ffz_emotes(
    client: &reqwest::Client,
    room_id: Option<&str>,
    log_fn: &(impl Fn(String) + Send + Sync),
) -> HashMap<String, EmoteMeta> {
    let mut map = HashMap::new();

    // 1. Global
    log_fn("[FFZ] Запрос глобальных эмоутов...".to_string());
    if let Ok(res) = client
        .get("https://api.frankerfacez.com/v1/set/global")
        .send()
        .await
    {
        if let Ok(v) = res.json::<Value>().await {
            parse_ffz_sets(&v, "global", &mut map);
        }
    }

    // 2. Channel
    if let Some(rid) = room_id {
        if !rid.is_empty() {
            log_fn(format!("[FFZ] Запрос эмоутов канала (room_id: {})...", rid));
            let url = format!("https://api.frankerfacez.com/v1/room/id/{}", rid);
            if let Ok(res) = client.get(&url).send().await {
                if let Ok(v) = res.json::<Value>().await {
                    parse_ffz_sets(&v, "channel", &mut map);
                }
            }
        }
    }

    log_fn(format!("[FFZ] Найдено эмоутов: {}", map.len()));
    map
}

fn parse_ffz_sets(val: &Value, scope: &str, out: &mut HashMap<String, EmoteMeta>) {
    if let Some(sets) = val.get("sets").and_then(|s| s.as_object()) {
        for (_k, set_obj) in sets {
            if let Some(emoticons) = set_obj.get("emoticons").and_then(|e| e.as_array()) {
                for item in emoticons {
                    let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let id_val = item.get("id");
                    let id_str = match id_val {
                        Some(Value::Number(n)) => n.to_string(),
                        Some(Value::String(s)) => s.clone(),
                        _ => String::new(),
                    };

                    if name.is_empty() || id_str.is_empty() {
                        continue;
                    }

                    let url_opt = item
                        .get("animated")
                        .or_else(|| item.get("urls"))
                        .and_then(|u| {
                            u.get("2")
                                .or_else(|| u.get("4"))
                                .or_else(|| u.get("1"))
                        })
                        .and_then(|u| u.as_str());

                    if let Some(url) = url_opt {
                        let full_url = if url.starts_with("//") {
                            format!("https:{}", url)
                        } else {
                            url.to_string()
                        };
                        out.insert(
                            name.to_string(),
                            EmoteMeta {
                                source: "ffz".to_string(),
                                id: id_str,
                                url: full_url,
                                scope: scope.to_string(),
                            },
                        );
                    }
                }
            }
        }
    }
}

pub async fn ensure_session_emotes<F: Fn(String) + Send + Sync + 'static>(
    session_dir: &Path,
    channel: &str,
    room_id: Option<&str>,
    cache_root: &Path,
    log_fn: F,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent("TwitchMpvOverlay/1.0 (+https://github.com)")
        .build()
        .map_err(|e| e.to_string())?;

    let folder_name = session_dir
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("session");

    let emotes_file = session_dir.join(format!("{}.emotes.json", folder_name));
    let chat_file = session_dir.join(format!("{}.chat.jsonl", folder_name));

    // 1. Create or load snapshot
    let snapshot: EmoteSnapshot = if emotes_file.is_file() {
        log_fn(format!("[Emotes] Использование существующего снимка: {}", emotes_file.display()));
        let content = fs::read_to_string(&emotes_file).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).unwrap_or_else(|_| EmoteSnapshot {
            channel: channel.to_string(),
            room_id: room_id.map(|s| s.to_string()),
            words: HashMap::new(),
            badges: HashMap::new(),
            schema_version: 2,
            cache_root: cache_root.to_string_lossy().to_string(),
        })
    } else {
        log_fn(format!("[Emotes] Создание нового снимка эмоутов для канала #{}...", channel));

        let mut words = HashMap::new();

        // 7TV first (highest priority)
        let s7 = fetch_7tv_emotes(&client, room_id, &log_fn).await;
        words.extend(s7);

        // BTTV
        let bttv = fetch_bttv_emotes(&client, room_id, &log_fn).await;
        for (k, v) in bttv {
            words.entry(k).or_insert(v);
        }

        // FFZ
        let ffz = fetch_ffz_emotes(&client, room_id, &log_fn).await;
        for (k, v) in ffz {
            words.entry(k).or_insert(v);
        }

        let snap = EmoteSnapshot {
            channel: channel.to_string(),
            room_id: room_id.map(|s| s.to_string()),
            words,
            badges: HashMap::new(),
            schema_version: 2,
            cache_root: cache_root.to_string_lossy().to_string(),
        };

        if let Ok(json_str) = serde_json::to_string_pretty(&snap) {
            let _ = fs::write(&emotes_file, json_str);
            log_fn(format!("[Emotes] Снимок сохранен: {} эмоутов в {}", snap.words.len(), emotes_file.display()));
        }

        snap
    };

    // 2. Scan chat.jsonl for used emotes
    if !chat_file.is_file() {
        log_fn("[Emotes] Чат-файл не найден, пропуск загрузки картинок.".to_string());
        return Ok(());
    }

    log_fn("[Emotes] Поиск использованных эмоутов в chat.jsonl...".to_string());
    let mut used_words = HashSet::new();

    if let Ok(file) = File::open(&chat_file) {
        let reader = BufReader::new(file);
        for line in reader.lines().flatten() {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
                    for word in msg.split_whitespace() {
                        if snapshot.words.contains_key(word) {
                            used_words.insert(word.to_string());
                        }
                    }
                }
            }
        }
    }

    log_fn(format!("[Emotes] Найдено уникальных эмоутов в чате: {}", used_words.len()));

    // 3. Cache missing emote images
    let mut downloaded = 0;
    let mut already_cached = 0;
    let mut failed = 0;

    for word in &used_words {
        if let Some(meta) = snapshot.words.get(word) {
            let ext = if meta.url.contains(".webp") {
                "webp"
            } else if meta.url.contains(".gif") {
                "gif"
            } else {
                "png"
            };

            let prov_dir = cache_root.join(&meta.source);
            let _ = fs::create_dir_all(&prov_dir);
            let target_path = prov_dir.join(format!("{}.{}", meta.id, ext));

            if target_path.is_file() && target_path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                already_cached += 1;
                continue;
            }

            match client.get(&meta.url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        if let Ok(bytes) = resp.bytes().await {
                            if fs::write(&target_path, bytes).is_ok() {
                                downloaded += 1;
                                log_fn(format!("  [OK] {} ({}) -> {}", word, meta.source, target_path.file_name().unwrap().to_string_lossy()));
                                continue;
                            }
                        }
                    }
                    failed += 1;
                }
                Err(_) => {
                    failed += 1;
                }
            }
        }
    }

    log_fn(format!(
        "[Emotes] Готово! Скачано новых: {}, уже в кэше: {}, ошибок: {}",
        downloaded, already_cached, failed
    ));

    Ok(())
}
