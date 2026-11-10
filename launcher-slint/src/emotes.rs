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
pub struct BadgeMeta {
    pub provider: String,
    pub source: String,
    pub set_id: String,
    pub version: String,
    pub url: String,
    pub file: String,
    #[serde(default)]
    pub channel: bool,
    #[serde(default)]
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmoteSnapshot {
    pub channel: String,
    pub room_id: Option<String>,
    pub words: HashMap<String, EmoteMeta>,
    #[serde(default)]
    pub badges: HashMap<String, BadgeMeta>,
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

pub async fn fetch_badges(
    client: &reqwest::Client,
    room_id: Option<&str>,
    client_id: Option<&str>,
    token: Option<&str>,
    log_fn: &(impl Fn(String) + Send + Sync),
) -> HashMap<String, BadgeMeta> {
    let mut map = HashMap::new();
    let use_helix = client_id.map(|s| !s.is_empty()).unwrap_or(false)
        && token.map(|s| !s.is_empty()).unwrap_or(false);

    log_fn("[Badges] Запрос глобальных значков Twitch...".to_string());
    if use_helix {
        let clean_token = token.unwrap().trim_start_matches("oauth:").trim_start_matches("Bearer ");
        if let Ok(res) = client
            .get("https://api.twitch.tv/helix/chat/badges/global")
            .header("Client-Id", client_id.unwrap())
            .header("Authorization", format!("Bearer {}", clean_token))
            .send()
            .await
        {
            if let Ok(v) = res.json::<Value>().await {
                parse_helix_badges(&v, false, &mut map);
            }
        }
    } else {
        if let Ok(res) = client
            .get("https://badges.twitch.tv/v1/badges/global/display")
            .send()
            .await
        {
            if let Ok(v) = res.json::<Value>().await {
                parse_legacy_badges(&v, false, &mut map);
            }
        }
    }

    if let Some(rid) = room_id {
        if !rid.is_empty() {
            log_fn(format!("[Badges] Запрос значков канала Twitch (room_id: {})...", rid));
            if use_helix {
                let clean_token = token.unwrap().trim_start_matches("oauth:").trim_start_matches("Bearer ");
                let url = format!("https://api.twitch.tv/helix/chat/badges?broadcaster_id={}", rid);
                if let Ok(res) = client
                    .get(&url)
                    .header("Client-Id", client_id.unwrap())
                    .header("Authorization", format!("Bearer {}", clean_token))
                    .send()
                    .await
                {
                    if let Ok(v) = res.json::<Value>().await {
                        parse_helix_badges(&v, true, &mut map);
                    }
                }
            } else {
                let url = format!("https://badges.twitch.tv/v1/badges/channels/{}/display", rid);
                if let Ok(res) = client.get(&url).send().await {
                    if let Ok(v) = res.json::<Value>().await {
                        parse_legacy_badges(&v, true, &mut map);
                    }
                }
            }
        }
    }

    log_fn(format!("[Badges] Найдено значков: {}", map.len()));
    map
}

fn parse_legacy_badges(val: &Value, is_channel: bool, out: &mut HashMap<String, BadgeMeta>) {
    if let Some(sets) = val.get("badge_sets").and_then(|s| s.as_object()) {
        for (set_id, set_data) in sets {
            if let Some(versions) = set_data.get("versions").and_then(|v| v.as_object()) {
                for (ver_id, ver_data) in versions {
                    let url = ver_data
                        .get("image_url_2x")
                        .or_else(|| ver_data.get("image_url_4x"))
                        .or_else(|| ver_data.get("image_url_1x"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    if !url.is_empty() {
                        let key = format!("{}/{}", set_id, ver_id);
                        let file = format!("badges/{}_{}.png", set_id, ver_id);
                        out.insert(
                            key,
                            BadgeMeta {
                                provider: "twitch".to_string(),
                                source: "twitch".to_string(),
                                set_id: set_id.clone(),
                                version: ver_id.clone(),
                                url: url.to_string(),
                                file,
                                channel: is_channel,
                                scope: if is_channel { "channel".to_string() } else { "global".to_string() },
                            },
                        );
                    }
                }
            }
        }
    }
}

fn parse_helix_badges(val: &Value, is_channel: bool, out: &mut HashMap<String, BadgeMeta>) {
    if let Some(data) = val.get("data").and_then(|d| d.as_array()) {
        for item in data {
            let set_id = item.get("set_id").and_then(|s| s.as_str()).unwrap_or("");
            if let Some(versions) = item.get("versions").and_then(|v| v.as_array()) {
                for ver in versions {
                    let ver_id = ver.get("id").and_then(|i| i.as_str()).unwrap_or("");
                    let url = ver
                        .get("image_url_2x")
                        .or_else(|| ver.get("image_url_4x"))
                        .or_else(|| ver.get("image_url_1x"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    if !set_id.is_empty() && !ver_id.is_empty() && !url.is_empty() {
                        let key = format!("{}/{}", set_id, ver_id);
                        let file = format!("badges/{}_{}.png", set_id, ver_id);
                        out.insert(
                            key,
                            BadgeMeta {
                                provider: "twitch".to_string(),
                                source: "twitch".to_string(),
                                set_id: set_id.to_string(),
                                version: ver_id.to_string(),
                                url: url.to_string(),
                                file,
                                channel: is_channel,
                                scope: if is_channel { "channel".to_string() } else { "global".to_string() },
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
    client_id: Option<&str>,
    token: Option<&str>,
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

    let session_file = session_dir.join(format!("{}.session.json", folder_name));
    let room_id_from_manifest = if room_id.is_none() && session_file.is_file() {
        fs::read_to_string(&session_file).ok().and_then(|c| {
            serde_json::from_str::<Value>(&c).ok().and_then(|v| {
                v.get("room_id").and_then(|r| r.as_str()).map(|s| s.to_string())
            })
        })
    } else {
        None
    };
    let effective_room_id = room_id.or(room_id_from_manifest.as_deref());

    let emotes_file = session_dir.join(format!("{}.emotes.json", folder_name));
    let chat_file = session_dir.join(format!("{}.chat.jsonl", folder_name));

    // 1. Create or load snapshot
    let snapshot: EmoteSnapshot = if emotes_file.is_file() {
        log_fn(format!("[Emotes] Использование существующего снимка: {}", emotes_file.display()));
        let content = fs::read_to_string(&emotes_file).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).unwrap_or_else(|_| EmoteSnapshot {
            channel: channel.to_string(),
            room_id: effective_room_id.map(|s| s.to_string()),
            words: HashMap::new(),
            badges: HashMap::new(),
            schema_version: 2,
            cache_root: cache_root.to_string_lossy().to_string(),
        })
    } else {
        log_fn(format!("[Emotes] Создание нового снимка эмоутов и баджей для канала #{}...", channel));

        let mut words = HashMap::new();

        // 7TV first (highest priority)
        let s7 = fetch_7tv_emotes(&client, effective_room_id, &log_fn).await;
        words.extend(s7);

        // BTTV
        let bttv = fetch_bttv_emotes(&client, effective_room_id, &log_fn).await;
        for (k, v) in bttv {
            words.entry(k).or_insert(v);
        }

        // FFZ
        let ffz = fetch_ffz_emotes(&client, effective_room_id, &log_fn).await;
        for (k, v) in ffz {
            words.entry(k).or_insert(v);
        }

        // Twitch Badges
        let mut badges = fetch_badges(&client, effective_room_id, client_id, token, &log_fn).await;

        // Fallback: If badges failed to fetch from Twitch API (e.g. expired token or offline),
        // inherit badges from existing session files in records if available
        if badges.is_empty() {
            if let Some(parent) = session_dir.parent() {
                if let Ok(entries) = fs::read_dir(parent) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            if let Ok(files) = fs::read_dir(&path) {
                                for f in files.flatten() {
                                    let name = f.file_name().to_string_lossy().to_string();
                                    if name.ends_with(".emotes.json") || name == "emotes.json" {
                                        if let Ok(content) = fs::read_to_string(f.path()) {
                                            if let Ok(prev_snap) = serde_json::from_str::<EmoteSnapshot>(&content) {
                                                if !prev_snap.badges.is_empty() {
                                                    log_fn(format!(
                                                        "[Badges] Восстановлено {} значков из существующей сессии",
                                                        prev_snap.badges.len()
                                                    ));
                                                    badges = prev_snap.badges;
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if !badges.is_empty() {
                                break;
                            }
                        }
                    }
                }
            }
        }

        let snap = EmoteSnapshot {
            channel: channel.to_string(),
            room_id: effective_room_id.map(|s| s.to_string()),
            words,
            badges,
            schema_version: 2,
            cache_root: cache_root.to_string_lossy().to_string(),
        };

        if let Ok(json_str) = serde_json::to_string_pretty(&snap) {
            let _ = fs::write(&emotes_file, json_str);
            log_fn(format!(
                "[Emotes] Снимок сохранен: {} эмоутов, {} значков в {}",
                snap.words.len(),
                snap.badges.len(),
                emotes_file.display()
            ));
        }

        snap
    };

    // 2. Download Twitch Badges
    let mut badge_downloaded = 0;
    let mut badge_cached = 0;
    let badge_dir = cache_root.join("badges");
    let _ = fs::create_dir_all(&badge_dir);
    for badge in snapshot.badges.values() {
        let target = cache_root.join(&badge.file);
        if target.is_file() && target.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            badge_cached += 1;
            continue;
        }
        if let Ok(resp) = client.get(&badge.url).send().await {
            if resp.status().is_success() {
                if let Ok(bytes) = resp.bytes().await {
                    if fs::write(&target, bytes).is_ok() {
                        badge_downloaded += 1;
                    }
                }
            }
        }
    }

    // 3. Scan chat.jsonl for used words and Twitch native emotes
    if !chat_file.is_file() {
        log_fn("[Emotes] Чат-файл не найден, пропуск загрузки картинок из чата.".to_string());
        return Ok(());
    }

    log_fn("[Emotes] Поиск использованных эмоутов в chat.jsonl...".to_string());
    let mut used_words = HashSet::new();
    let mut used_twitch_ids = HashSet::new();

    if let Ok(file) = File::open(&chat_file) {
        let reader = BufReader::new(file);
        for line in reader.lines().flatten() {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                // Word emotes (7TV, BTTV, FFZ)
                if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
                    for word in msg.split_whitespace() {
                        if snapshot.words.contains_key(word) {
                            used_words.insert(word.to_string());
                        }
                    }
                }

                // Twitch native emotes from "emotes" object or string
                if let Some(emotes_obj) = v.get("emotes").and_then(|e| e.as_object()) {
                    for key in emotes_obj.keys() {
                        if !key.is_empty() {
                            used_twitch_ids.insert(key.clone());
                        }
                    }
                } else if let Some(emotes_str) = v.get("emotes").and_then(|e| e.as_str()) {
                    for grp in emotes_str.split('/') {
                        if let Some((id, _)) = grp.split_once(':') {
                            if !id.is_empty() {
                                used_twitch_ids.insert(id.to_string());
                            }
                        }
                    }
                }

                // Twitch emotes from tags.emotes
                if let Some(tags) = v.get("tags").and_then(|t| t.as_object()) {
                    if let Some(emotes_str) = tags.get("emotes").and_then(|e| e.as_str()) {
                        for grp in emotes_str.split('/') {
                            if let Some((id, _)) = grp.split_once(':') {
                                if !id.is_empty() {
                                used_twitch_ids.insert(id.to_string());
                                }
                            }
                        }
                    }
                }

                // Twitch emotes from twitch_emotes array
                if let Some(t_emotes) = v.get("twitch_emotes").and_then(|te| te.as_array()) {
                    for item in t_emotes {
                        if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                            if !id.is_empty() {
                                used_twitch_ids.insert(id.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    log_fn(format!(
        "[Emotes] Найдено: сторонних эмоутов: {}, нативных Twitch эмоутов: {}",
        used_words.len(),
        used_twitch_ids.len()
    ));

    // 4. Cache missing 7TV/BTTV/FFZ emote images
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

    // 5. Cache missing native Twitch emotes
    let mut twitch_downloaded = 0;
    let mut twitch_cached = 0;
    let twitch_dir = cache_root.join("twitch");
    let _ = fs::create_dir_all(&twitch_dir);

    for eid in &used_twitch_ids {
        let target_path = twitch_dir.join(format!("{}.png", eid));
        if target_path.is_file() && target_path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            twitch_cached += 1;
            continue;
        }

        let url = format!("https://static-cdn.jtvnw.net/emoticons/v2/{}/default/dark/2.0", eid);
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                if let Ok(bytes) = resp.bytes().await {
                    if fs::write(&target_path, bytes).is_ok() {
                        twitch_downloaded += 1;
                        log_fn(format!("  [OK] Twitch emote {} -> {}.png", eid, eid));
                        continue;
                    }
                }
            }
        }
    }

    log_fn(format!(
        "[Emotes] Готово! Сторонних эмоутов: +{}/{} (ошибок: {}), Twitch эмоутов: +{}/{}, значков Twitch: +{}/{}",
        downloaded, already_cached, failed, twitch_downloaded, twitch_cached, badge_downloaded, badge_cached
    ));

    Ok(())
}
