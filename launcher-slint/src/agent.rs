use chrono::Local;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WARN_FREE_GB: f64 = 3.0;
const FULL_DISK_MB: u64 = 150;
const ALERT_COOLDOWN_SECS: u64 = 300; // 5 minutes

#[derive(Debug, Clone, Deserialize)]
pub struct StreamerEntry {
    pub login: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub record: Option<bool>,
    #[serde(default)]
    pub quality: Option<String>,
}

#[cfg(windows)]
pub fn get_free_disk_space_mb(path: &Path) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);

    let mut free_bytes_available: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_free_bytes: u64 = 0;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            lpDirectoryName: *const u16,
            lpFreeBytesAvailableToCaller: *mut u64,
            lpTotalNumberOfBytes: *mut u64,
            lpTotalNumberOfFreeBytes: *mut u64,
        ) -> i32;
    }

    let success = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_bytes_available,
            &mut total_bytes,
            &mut total_free_bytes,
        )
    };

    if success != 0 {
        free_bytes_available / (1024 * 1024)
    } else {
        100_000
    }
}

#[cfg(not(windows))]
pub fn get_free_disk_space_mb(_path: &Path) -> u64 {
    100_000
}

pub fn notify_windows(title: &str, message: &str, critical: bool) {
    let title = title.to_string();
    let message = message.to_string();

    std::thread::spawn(move || {
        #[cfg(windows)]
        {
            #[link(name = "user32")]
            extern "system" {
                fn MessageBeep(uType: u32) -> i32;
            }
            unsafe {
                MessageBeep(if critical { 0x00000010 } else { 0x00000030 });
            }

            let script = format!(
                r#"[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null;
$template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02);
$textNodes = $template.GetElementsByTagName("text");
$textNodes.Item(0).AppendChild($template.CreateTextNode("{}")) > $null;
$textNodes.Item(1).AppendChild($template.CreateTextNode("{}")) > $null;
$toast = [Windows.UI.Notifications.ToastNotification]::new($template);
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier("PC Stream Agent").Show($toast);"#,
                title.replace('"', "\\\""),
                message.replace('"', "\\\"")
            );

            let mut cmd = Command::new("powershell");
            cmd.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script]);
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
            let _ = cmd.output();
        }
        #[cfg(not(windows))]
        {
            let _ = (title, message, critical);
        }
    });
}

fn resolve_quality(raw: Option<&str>) -> String {
    match raw {
        Some("audio_only") => "audio_only".to_string(),
        Some(q) if q.contains(',') => q.to_string(),
        Some(q) if !q.is_empty() => format!("{},audio_only", q),
        _ => "best,audio_only".to_string(),
    }
}

pub struct AgentWorkerHandle {
    pub stop_flag: Arc<AtomicBool>,
    pub child: Arc<Mutex<Option<Child>>>,
}

pub async fn run_pc_agent<F: Fn(String) + Send + Sync + Clone + 'static>(
    server_url: String,
    proxy_playlist: String,
    records_dir: PathBuf,
    streamlink_bin: String,
    stop_signal: Arc<AtomicBool>,
    log_fn: F,
) {
    if server_url.trim().is_empty() {
        log_fn("[ERROR] SERVER_URL не настроен! Укажите адрес сервера во вкладке 'Настройки' лаунчера.".to_string());
        return;
    }

    let _ = fs::create_dir_all(&records_dir);
    let free_mb = get_free_disk_space_mb(&records_dir);
    log_fn(format!(
        "[*] Native PC Agent запущен | Свободно на диске: {:.2} ГБ",
        free_mb as f64 / 1024.0
    ));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    // Print initial streamer list
    let api_url = format!("{}/api/streamers", server_url.trim_end_matches('/'));
    match client.get(&api_url).send().await {
        Ok(res) if res.status().is_success() => {
            if let Ok(streamers) = res.json::<Vec<StreamerEntry>>().await {
                log_fn(format!("[LIST] Отслеживаемые стримеры ({}):", streamers.len()));
                for s in &streamers {
                    let icon = if s.status == "online" { "[ON]" } else { "[--]" };
                    let rec = if s.record.unwrap_or(false) { "ON" } else { "OFF" };
                    let qual = s.quality.as_deref().unwrap_or("best");
                    log_fn(format!(
                        "   {} {:<20} status={:<8} record={:<3} quality={}",
                        icon, s.login, s.status, rec, qual
                    ));
                }
            }
        }
        _ => log_fn(format!("[WARN] Не удалось подключиться к {} на старте", api_url)),
    }

    let workers: Arc<Mutex<HashMap<String, AgentWorkerHandle>>> = Arc::new(Mutex::new(HashMap::new()));
    let last_warn_time = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(ALERT_COOLDOWN_SECS)));
    let last_full_time = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(ALERT_COOLDOWN_SECS)));

    log_fn("[*] Запуск цикла мониторинга стримеров (опрос каждые 10 секунд)...".to_string());

    while !stop_signal.load(Ordering::Relaxed) {
        // Query server
        match client.get(&api_url).send().await {
            Ok(res) if res.status().is_success() => {
                if let Ok(streamers) = res.json::<Vec<StreamerEntry>>().await {
                    let mut active_logins = std::collections::HashSet::new();

                    for s in streamers {
                        let login = s.login.clone();
                        active_logins.insert(login.clone());
                        let is_online = s.status == "online";
                        let record_enabled = s.record.unwrap_or(false);

                        if is_online && record_enabled {
                            // Start worker if not already recording
                            let mut map = workers.lock().unwrap();
                            if !map.contains_key(&login) {
                                log_fn(format!("[START] Обнаружен стрим {} онлайн! Инициализация записи...", login));
                                let stop_flag = Arc::new(AtomicBool::new(false));
                                let child_arc = Arc::new(Mutex::new(None));
                                let qual = resolve_quality(s.quality.as_deref());

                                let handle = AgentWorkerHandle {
                                    stop_flag: stop_flag.clone(),
                                    child: child_arc.clone(),
                                };
                                map.insert(login.clone(), handle);

                                let login_clone = login.clone();
                                let rec_dir_clone = records_dir.clone();
                                let sl_bin_clone = streamlink_bin.clone();
                                let proxy_clone = proxy_playlist.clone();
                                let log_fn_worker = log_fn.clone();
                                let last_warn_worker = last_warn_time.clone();
                                let last_full_worker = last_full_time.clone();

                                std::thread::spawn(move || {
                                    run_worker_thread(
                                        login_clone,
                                        qual,
                                        rec_dir_clone,
                                        sl_bin_clone,
                                        proxy_clone,
                                        stop_flag,
                                        child_arc,
                                        log_fn_worker,
                                        last_warn_worker,
                                        last_full_worker,
                                    );
                                });
                            }
                        } else {
                            // Stop worker if recording
                            let mut map = workers.lock().unwrap();
                            if let Some(h) = map.remove(&login) {
                                log_fn(format!("[STOP] Стрим {} оффлайн или запись выключена. Остановка...", login));
                                h.stop_flag.store(true, Ordering::Relaxed);
                                if let Ok(mut lock) = h.child.lock() {
                                    if let Some(mut c) = lock.take() {
                                        let _ = c.kill();
                                    }
                                }
                            }
                        }
                    }

                    // Clean up any workers no longer in server response
                    let mut map = workers.lock().unwrap();
                    let to_remove: Vec<String> = map
                        .keys()
                        .filter(|k| !active_logins.contains(*k))
                        .cloned()
                        .collect();

                    for rk in to_remove {
                        if let Some(h) = map.remove(&rk) {
                            log_fn(format!("[STOP] Стример {} удален из отслеживаемых.", rk));
                            h.stop_flag.store(true, Ordering::Relaxed);
                            if let Ok(mut lock) = h.child.lock() {
                                if let Some(mut c) = lock.take() {
                                    let _ = c.kill();
                                }
                            }
                        }
                    }
                }
            }
            Ok(res) => {
                log_fn(format!("[WARN] Сервер вернул HTTP {}", res.status()));
            }
            Err(e) => {
                log_fn(format!("[WAIT] Ошибка соединения с сервером ({}). Повтор через 10с...", e));
            }
        }

        // Wait 10 seconds checking stop_signal every 500ms
        for _ in 0..20 {
            if stop_signal.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    // Stop all active workers
    log_fn("[BYE] Остановка всех активных записей PC Agent...".to_string());
    let mut map = workers.lock().unwrap();
    for (_, h) in map.drain() {
        h.stop_flag.store(true, Ordering::Relaxed);
        if let Ok(mut lock) = h.child.lock() {
            if let Some(mut c) = lock.take() {
                let _ = c.kill();
            }
        }
    }

    log_fn("[BYE] PC Agent остановлен.".to_string());
}

fn run_worker_thread<F: Fn(String) + Send + Sync + Clone + 'static>(
    login: String,
    quality: String,
    records_dir: PathBuf,
    streamlink_bin: String,
    proxy_playlist: String,
    stop_flag: Arc<AtomicBool>,
    child_arc: Arc<Mutex<Option<Child>>>,
    log_fn: F,
    last_warn_time: Arc<Mutex<Instant>>,
    last_full_time: Arc<Mutex<Instant>>,
) {
    while !stop_flag.load(Ordering::Relaxed) {
        let _ = fs::create_dir_all(&records_dir);
        let free_mb = get_free_disk_space_mb(&records_dir);

        if free_mb < FULL_DISK_MB {
            let mut lft = last_full_time.lock().unwrap();
            if lft.elapsed() > Duration::from_secs(ALERT_COOLDOWN_SECS) {
                *lft = Instant::now();
                let msg = format!(
                    "Запись {} остановилась: на диске осталось всего {} МБ! Освободите место.",
                    login, free_mb
                );
                log_fn(format!("\n[ALERT] {}", msg));
                notify_windows("[ALERT] Запись остановлена: нет места!", &msg, true);
            }
            for _ in 0..20 {
                if stop_flag.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            continue;
        }

        let timestamp = Local::now().format("%d-%m-%Y_%H-%M-%S").to_string();
        let filename = format!("{}_{}.ts", login, timestamp);
        let file_path = records_dir.join(&filename);

        log_fn(format!(
            "\n[REC] Подключение к twitch.tv/{} (качество: {})...",
            login, quality
        ));

        let mut cmd = Command::new(&streamlink_bin);
        cmd.arg(format!("https://twitch.tv/{}", login));
        cmd.arg(&quality);
        cmd.arg("-o");
        cmd.arg(&file_path);
        cmd.arg("--twitch-disable-ads");
        cmd.arg("--retry-streams");
        cmd.arg("5");
        cmd.arg("--retry-max");
        cmd.arg("3");

        if !proxy_playlist.trim().is_empty() {
            cmd.arg(format!("--twitch-proxy-playlist={}", proxy_playlist.trim()));
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
                log_fn(format!("[ERROR] Не удалось запустить Streamlink: {}", e));
                return;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        if let Ok(mut lock) = child_arc.lock() {
            *lock = Some(child);
        }

        // Spawn stdout reader
        let log_fn_out = log_fn.clone();
        if let Some(out) = stdout {
            std::thread::spawn(move || {
                use std::io::{BufRead, BufReader};
                let reader = BufReader::new(out);
                for line in reader.lines().flatten() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        log_fn_out(trimmed.to_string());
                    }
                }
            });
        }

        // Spawn stderr reader
        let log_fn_err = log_fn.clone();
        if let Some(err) = stderr {
            std::thread::spawn(move || {
                use std::io::{BufRead, BufReader};
                let reader = BufReader::new(err);
                for line in reader.lines().flatten() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        log_fn_err(trimmed.to_string());
                    }
                }
            });
        }

        let mut ticks: u64 = 0;
        let start_time = Instant::now();

        // Monitor child execution
        loop {
            if stop_flag.load(Ordering::Relaxed) {
                if let Ok(mut lock) = child_arc.lock() {
                    if let Some(mut c) = lock.take() {
                        let _ = c.kill();
                    }
                }
                return;
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

            std::thread::sleep(Duration::from_secs(1));
            ticks += 1;

            // Log size every 3 minutes (180 seconds)
            if ticks % 180 == 0 {
                if let Ok(meta) = fs::metadata(&file_path) {
                    let bytes = meta.len();
                    if bytes > 0 {
                        let size_str = if bytes < 1024 * 1024 {
                            format!("{:.1} КБ", bytes as f64 / 1024.0)
                        } else if bytes < 1024 * 1024 * 1024 {
                            format!("{:.1} МБ", bytes as f64 / (1024.0 * 1024.0))
                        } else {
                            format!("{:.2} ГБ", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
                        };

                        let elapsed = start_time.elapsed().as_secs();
                        let h = elapsed / 3600;
                        let m = (elapsed % 3600) / 60;
                        let s = elapsed % 60;
                        let dur_str = if h > 0 {
                            format!("{:02}:{:02}:{:02}", h, m, s)
                        } else {
                            format!("{:02}:{:02}", m, s)
                        };

                        log_fn(format!("[REC] {}: записано {} (длительность {})", login, size_str, dur_str));
                    }
                }
            }

            // Low disk space check every 10 seconds
            if ticks % 10 == 0 {
                let free_gb = get_free_disk_space_mb(&records_dir) as f64 / 1024.0;
                if free_gb < WARN_FREE_GB {
                    let mut lwt = last_warn_time.lock().unwrap();
                    if lwt.elapsed() > Duration::from_secs(ALERT_COOLDOWN_SECS) {
                        *lwt = Instant::now();
                        let msg = format!(
                            "Идет запись ({}), на диске осталось {:.2} ГБ! Освободите место.",
                            login, free_gb
                        );
                        log_fn(format!("\n[WARN] {}", msg));
                        notify_windows("[WARN] Скоро закончится место на диске!", &msg, false);
                    }
                }
            }
        }

        // Clean up after streamlink finished
        if let Ok(meta) = fs::metadata(&file_path) {
            if meta.len() > 0 {
                log_fn(format!("[OK] [FINISHED] Запись для {} сохранена: {}", login, filename));
            } else {
                let _ = fs::remove_file(&file_path);
            }
        }

        let free_mb_after = get_free_disk_space_mb(&records_dir);
        if free_mb_after < FULL_DISK_MB {
            let mut lft = last_full_time.lock().unwrap();
            *lft = Instant::now();
            let msg = format!(
                "Запись {} прервалась — закончилось место на диске ({} МБ свободно)! Освободите место.",
                login, free_mb_after
            );
            log_fn(format!("\n[ALERT] {}", msg));
            notify_windows("[ALERT] Запись прервалась: диск полон!", &msg, true);
        } else {
            log_fn(format!("[RETRY] Стрим для {} завершился или оффлайн. Повторная проверка через 10с...", login));
        }

        for _ in 0..20 {
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}
