#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod emotes;
mod importer;
mod player;
mod process_manager;
mod records;
mod twitch_auth;

slint::include_modules!();

use config::{load_config, save_config};
use process_manager::ManagedProcess;
use records::{scan_records, SessionInfo};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

fn get_project_root() -> PathBuf {
    // 1. Check if current working directory has config.json or records
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.join("config.json").is_file() || cwd.join("records").is_dir() {
            return cwd;
        }
    }

    // 2. Check executable directory
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            if parent.join("config.json").is_file() || parent.join("records").is_dir() {
                return parent.to_path_buf();
            }

            // 3. If running inside target/debug or target/release, traverse up to workspace root
            if let Some(target_dir) = parent.parent() {
                if target_dir.file_name().and_then(|n| n.to_str()) == Some("target") {
                    if let Some(proj_root) = target_dir.parent() {
                        return proj_root.to_path_buf();
                    }
                }
            }

            return parent.to_path_buf();
        }
    }

    PathBuf::from(".")
}

fn main() -> Result<(), slint::PlatformError> {
    let main_window = MainWindow::new()?;
    let handle = main_window.as_weak();

    let root_dir = get_project_root();
    let config_arc = Arc::new(Mutex::new(load_config(&root_dir)));
    let sessions_cache: Arc<Mutex<HashMap<String, Vec<SessionInfo>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let rec_process = Arc::new(ManagedProcess::new());
    let agent_process = Arc::new(ManagedProcess::new());

    // 1. Initialize settings from config
    {
        let cfg = config_arc.lock().unwrap();
        main_window.set_cfg_streamlink(cfg.streamlink_path.clone().into());
        main_window.set_cfg_mpv(cfg.mpv_path.clone().into());
        main_window.set_cfg_python(cfg.python_path.clone().into());
        main_window.set_cfg_client_id(cfg.twitch_client_id.clone().into());
        main_window.set_cfg_app_token(cfg.twitch_app_token.clone().into());
        main_window.set_cfg_client_secret(cfg.twitch_client_secret.clone().into());
        main_window.set_cfg_refresh_token(cfg.twitch_refresh_token.clone().into());
        main_window.set_cfg_records_dir(cfg.records_dir.clone().into());
        main_window.set_cfg_server_url(cfg.server_url.clone().into());
        main_window.set_cfg_proxy_playlist(cfg.twitch_proxy_playlist.clone().into());
        main_window.set_cfg_engine(cfg.chat_engine.clone().into());
    }

    // Helper: Refresh records list
    let refresh_ui_records = {
        let handle_weak = handle.clone();
        let sessions_arc = sessions_cache.clone();
        let cfg_arc = config_arc.clone();
        let root = root_dir.clone();

        move || {
            let records_dir_str = cfg_arc.lock().unwrap().records_dir.clone();
            let base_path = root.join(&records_dir_str);
            let map = scan_records(&base_path);
            *sessions_arc.lock().unwrap() = map.clone();

            if let Some(ui) = handle_weak.upgrade() {
                let mut channel_names: Vec<String> = map.keys().cloned().collect();
                channel_names.sort();

                let ch_items: Vec<ChannelItem> = channel_names
                    .iter()
                    .map(|name| ChannelItem {
                        name: name.clone().into(),
                        count: map.get(name).map(|v| v.len()).unwrap_or(0) as i32,
                    })
                    .collect();

                let model: Rc<VecModel<ChannelItem>> = Rc::new(VecModel::from(ch_items));
                ui.set_channels(ModelRc::from(model));

                // Auto select first channel if current is empty or not in map
                let cur = ui.get_selected_channel().to_string();
                let sel_ch = if !cur.is_empty() && map.contains_key(&cur) {
                    cur
                } else {
                    channel_names.first().cloned().unwrap_or_default()
                };

                ui.set_selected_channel(sel_ch.clone().into());

                if let Some(sessions) = map.get(&sel_ch) {
                    let rec_items: Vec<RecordingItem> = sessions
                        .iter()
                        .map(|s| RecordingItem {
                            id: s.dir.to_string_lossy().to_string().into(),
                            date_str: s.date_str.clone().into(),
                            parts_count: s.parts_count as i32,
                            duration_str: records::format_duration(s.duration_sec).into(),
                            size_str: records::format_bytes(s.size_bytes).into(),
                            has_chat: s.has_chat,
                            has_emotes: s.has_emotes,
                        })
                        .collect();

                    let rec_model: Rc<VecModel<RecordingItem>> =
                        Rc::new(VecModel::from(rec_items));
                    ui.set_recordings(ModelRc::from(rec_model));
                }
            }
        }
    };

    // Initial scan
    refresh_ui_records();

    // Channel selection callback
    {
        let handle_weak = handle.clone();
        let sessions_arc = sessions_cache.clone();
        main_window.on_select_channel(move |ch_name| {
            if let Some(ui) = handle_weak.upgrade() {
                ui.set_selected_channel(ch_name.clone());
                let map = sessions_arc.lock().unwrap();
                if let Some(sessions) = map.get(ch_name.as_str()) {
                    let rec_items: Vec<RecordingItem> = sessions
                        .iter()
                        .map(|s| RecordingItem {
                            id: s.dir.to_string_lossy().to_string().into(),
                            date_str: s.date_str.clone().into(),
                            parts_count: s.parts_count as i32,
                            duration_str: records::format_duration(s.duration_sec).into(),
                            size_str: records::format_bytes(s.size_bytes).into(),
                            has_chat: s.has_chat,
                            has_emotes: s.has_emotes,
                        })
                        .collect();

                    let rec_model = Rc::new(VecModel::from(rec_items));
                    ui.set_recordings(ModelRc::from(rec_model));
                }
            }
        });
    }

    // Refresh records button
    {
        let refresh_fn = refresh_ui_records.clone();
        main_window.on_refresh_records(move || {
            refresh_fn();
        });
    }

    // Open records folder
    {
        let root = root_dir.clone();
        let cfg_arc = config_arc.clone();
        main_window.on_open_records_folder(move || {
            let dir_name = cfg_arc.lock().unwrap().records_dir.clone();
            let path = root.join(dir_name);
            let _ = open::that(path);
        });
    }

    // Open session folder
    {
        let handle_weak = handle.clone();
        main_window.on_open_session_folder(move || {
            if let Some(ui) = handle_weak.upgrade() {
                let id = ui.get_selected_recording_id().to_string();
                if !id.is_empty() {
                    let _ = open::that(Path::new(&id));
                }
            }
        });
    }

    // Launch mpv
    {
        let handle_weak = handle.clone();
        let root = root_dir.clone();
        let cfg_arc = config_arc.clone();
        let sessions_arc = sessions_cache.clone();
        let refresh_fn = refresh_ui_records.clone();

        main_window.on_launch_mpv(move || {
            if let Some(ui) = handle_weak.upgrade() {
                let sel_id = ui.get_selected_recording_id().to_string();
                let sel_ch = ui.get_selected_channel().to_string();

                let map = sessions_arc.lock().unwrap();
                if let Some(sessions) = map.get(&sel_ch) {
                    let target_session = if !sel_id.is_empty() {
                        sessions.iter().find(|s| s.dir.to_string_lossy() == sel_id)
                    } else {
                        sessions.first()
                    };

                    if let Some(s) = target_session {
                        let s_clone = s.clone();
                        let cfg = cfg_arc.lock().unwrap().clone();
                        let root_clone = root.clone();
                        let cache_root = root_clone.join(&cfg.emote_cache_dir);
                        let h = handle_weak.clone();
                        let rf = refresh_fn.clone();

                        std::thread::spawn(move || {
                            let rt = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .unwrap();

                            let h_for_log = h.clone();
                            let log_cb = move |msg: String| {
                                let h_inner = h_for_log.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(ui) = h_inner.upgrade() {
                                        let mut text = ui.get_imp_console_text().to_string();
                                        text.push_str(&msg);
                                        text.push('\n');
                                        ui.set_imp_console_text(text.into());
                                    }
                                });
                            };

                            log_cb(format!("[Player] Подготовка эмоутов для сессии {}...", s_clone.dir.display()));
                            let _ = rt.block_on(emotes::ensure_session_emotes(
                                &s_clone.dir,
                                &s_clone.channel,
                                None,
                                &cache_root,
                                log_cb,
                            ));

                            let _ = slint::invoke_from_event_loop(move || {
                                rf();
                            });

                            if let Err(e) = player::launch_player(&root_clone, &cfg, &s_clone) {
                                eprintln!("[Launcher Error] {}", e);
                            }
                        });
                    }
                }
            }
        });
    }

    // Save Settings
    {
        let handle_weak = handle.clone();
        let root = root_dir.clone();
        let cfg_arc = config_arc.clone();
        let refresh_fn = refresh_ui_records.clone();

        main_window.on_save_settings(move || {
            if let Some(ui) = handle_weak.upgrade() {
                let mut cfg = cfg_arc.lock().unwrap();
                cfg.streamlink_path = ui.get_cfg_streamlink().to_string();
                cfg.mpv_path = ui.get_cfg_mpv().to_string();
                cfg.python_path = ui.get_cfg_python().to_string();
                cfg.twitch_client_id = ui.get_cfg_client_id().to_string();
                cfg.twitch_app_token = ui.get_cfg_app_token().to_string();
                cfg.twitch_client_secret = ui.get_cfg_client_secret().to_string();
                cfg.twitch_refresh_token = ui.get_cfg_refresh_token().to_string();
                cfg.records_dir = ui.get_cfg_records_dir().to_string();
                cfg.server_url = ui.get_cfg_server_url().to_string();
                cfg.twitch_proxy_playlist = ui.get_cfg_proxy_playlist().to_string();
                cfg.chat_engine = ui.get_cfg_engine().to_string();

                save_config(&root, &cfg);
                drop(cfg);
                refresh_fn();
            }
        });
    }

    // PC Agent Start/Stop
    {
        let handle_weak = handle.clone();
        let agent_proc = agent_process.clone();
        let root = root_dir.clone();
        let cfg_arc = config_arc.clone();

        main_window.on_toggle_pc_agent(move || {
            if agent_proc.is_running() {
                agent_proc.stop();
                if let Some(ui) = handle_weak.upgrade() {
                    ui.set_agent_active(false);
                    ui.set_agent_status_text("Листенер остановлен".into());
                }
            } else {
                let (py_bin, server, proxy, records, streamlink) = {
                    let cfg = cfg_arc.lock().unwrap();
                    let miniconda = PathBuf::from(r"C:\Users\User\miniconda3\python.exe");
                    let py = if (cfg.python_path == "python" || cfg.python_path.is_empty())
                        && miniconda.is_file()
                    {
                        miniconda.to_string_lossy().to_string()
                    } else {
                        cfg.python_path.clone()
                    };
                    (
                        py,
                        cfg.server_url.clone(),
                        cfg.twitch_proxy_playlist.clone(),
                        cfg.records_dir.clone(),
                        cfg.streamlink_path.clone(),
                    )
                };

                let script = root.join("PC_AGENT.py");
                let mut cmd = Command::new(&py_bin);
                cmd.arg(&script);
                cmd.env("PYTHONUNBUFFERED", "1");
                cmd.env("PYTHONIOENCODING", "utf-8");
                cmd.env("SERVER_URL", server);
                cmd.env("TWITCH_PROXY_PLAYLIST", proxy);
                cmd.env("RECORDS_DIR", records);
                cmd.env("STREAMLINK_PATH", streamlink);

                let handle_for_line = handle_weak.clone();
                let on_line = move |line: String| {
                    let h = handle_for_line.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = h.upgrade() {
                            let mut text = ui.get_agent_console_text().to_string();
                            text.push_str(&line);
                            text.push('\n');
                            ui.set_agent_console_text(text.into());
                        }
                    });
                };

                let handle_for_exit = handle_weak.clone();
                let on_exit = move || {
                    let h = handle_for_exit.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = h.upgrade() {
                            ui.set_agent_active(false);
                            ui.set_agent_status_text("Листенер остановлен".into());
                        }
                    });
                };

                if let Ok(_) = agent_proc.start(cmd, on_line, on_exit) {
                    if let Some(ui) = handle_weak.upgrade() {
                        ui.set_agent_active(true);
                        ui.set_agent_status_text("🟢 Листенер активен".into());
                    }
                }
            }
        });
    }

    // Clear PC Agent console
    {
        let handle_weak = handle.clone();
        main_window.on_clear_pc_agent_console(move || {
            if let Some(ui) = handle_weak.upgrade() {
                ui.set_agent_console_text("".into());
            }
        });
    }

    // Browse import video
    {
        let handle_weak = handle.clone();
        main_window.on_browse_import_video(move || {
            if let Some(files) = rfd::FileDialog::new()
                .add_filter("Video", &["ts", "mp4", "mkv"])
                .pick_files()
            {
                let path_strs: Vec<String> = files
                    .into_iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                if let Some(ui) = handle_weak.upgrade() {
                    ui.set_imp_video(path_strs.join(";").into());
                }
            }
        });
    }

    // Live Recording Start/Stop
    {
        let handle_weak = handle.clone();
        let rec_proc = rec_process.clone();
        let root = root_dir.clone();
        let cfg_arc = config_arc.clone();
        let refresh_fn = refresh_ui_records.clone();

        main_window.on_toggle_recording(move || {
            if rec_proc.is_running() {
                rec_proc.stop();
                if let Some(ui) = handle_weak.upgrade() {
                    ui.set_rec_active(false);
                    ui.set_rec_status_text("Запись остановлена".into());
                }
            } else {
                let chan = if let Some(ui) = handle_weak.upgrade() {
                    ui.get_rec_channel().to_string()
                } else {
                    String::new()
                };

                if chan.is_empty() {
                    if let Some(ui) = handle_weak.upgrade() {
                        ui.set_rec_status_text("Укажите канал Twitch!".into());
                    }
                    return;
                }

                let (py_bin, qual) = {
                    let cfg = cfg_arc.lock().unwrap();
                    let miniconda = PathBuf::from(r"C:\Users\User\miniconda3\python.exe");
                    let py = if (cfg.python_path == "python" || cfg.python_path.is_empty())
                        && miniconda.is_file()
                    {
                        miniconda.to_string_lossy().to_string()
                    } else {
                        cfg.python_path.clone()
                    };
                    let q = if let Some(ui) = handle_weak.upgrade() {
                        ui.get_rec_quality().to_string()
                    } else {
                        "best".to_string()
                    };
                    (py, q)
                };

                let script = root.join("recorder.py");
                let mut cmd = Command::new(&py_bin);
                cmd.arg(&script);
                cmd.arg(&chan);
                cmd.arg("--quality");
                cmd.arg(&qual);
                cmd.current_dir(&root);
                cmd.env("PYTHONUNBUFFERED", "1");
                cmd.env("PYTHONIOENCODING", "utf-8");

                let handle_for_line = handle_weak.clone();
                let on_line = move |line: String| {
                    let h = handle_for_line.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = h.upgrade() {
                            let mut text = ui.get_rec_console_text().to_string();
                            text.push_str(&line);
                            text.push('\n');
                            ui.set_rec_console_text(text.into());
                        }
                    });
                };

                let handle_for_exit = handle_weak.clone();
                let refresh_on_exit = refresh_fn.clone();
                let on_exit = move || {
                    let h = handle_for_exit.clone();
                    let rf = refresh_on_exit.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = h.upgrade() {
                            ui.set_rec_active(false);
                            ui.set_rec_status_text("Запись завершена".into());
                        }
                        rf();
                    });
                };

                if let Ok(_) = rec_proc.start(cmd, on_line, on_exit) {
                    if let Some(ui) = handle_weak.upgrade() {
                        ui.set_rec_active(true);
                        ui.set_rec_status_text(format!("Идет запись стрима #{}...", chan).into());
                    }
                }
            }
        });
    }

    // Refresh Twitch OAuth Token manually
    {
        let handle_weak = handle.clone();
        let cfg_arc = config_arc.clone();
        let root = root_dir.clone();

        main_window.on_refresh_token_manually(move || {
            let (cid, secret, refresh) = if let Some(ui) = handle_weak.upgrade() {
                (
                    ui.get_cfg_client_id().to_string(),
                    ui.get_cfg_client_secret().to_string(),
                    ui.get_cfg_refresh_token().to_string(),
                )
            } else {
                return;
            };

            if let Some(ui) = handle_weak.upgrade() {
                ui.set_cfg_token_status("Обновление токена…".into());
            }

            let h = handle_weak.clone();
            let c_arc = cfg_arc.clone();
            let r_dir = root.clone();

            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();

                let res = rt.block_on(twitch_auth::refresh_twitch_token(&cid, &secret, &refresh));
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = h.upgrade() {
                        match res {
                            Ok(token_info) => {
                                ui.set_cfg_app_token(token_info.access_token.clone().into());
                                ui.set_cfg_refresh_token(token_info.refresh_token.clone().into());
                                ui.set_cfg_token_status("Токен успешно обновлен и сохранен".into());

                                let mut cfg = c_arc.lock().unwrap();
                                cfg.twitch_app_token = token_info.access_token;
                                cfg.twitch_refresh_token = token_info.refresh_token;
                                save_config(&r_dir, &cfg);
                            }
                            Err(e) => {
                                ui.set_cfg_token_status(format!("Ошибка: {}", e).into());
                            }
                        }
                    }
                });
            });
        });
    }

    // Import Tab: Run Import
    {
        let handle_weak = handle.clone();
        let root = root_dir.clone();
        let cfg_arc = config_arc.clone();
        let refresh_fn = refresh_ui_records.clone();

        main_window.on_run_import(move || {
            let (ch, video_str, dates_str) = if let Some(ui) = handle_weak.upgrade() {
                (
                    ui.get_imp_channel().to_string(),
                    ui.get_imp_video().to_string(),
                    ui.get_imp_dates().to_string(),
                )
            } else {
                return;
            };

            if ch.is_empty() {
                if let Some(ui) = handle_weak.upgrade() {
                    ui.set_imp_console_text("Ошибка: укажите имя канала\n".into());
                }
                return;
            }

            let records_dir = {
                let cfg = cfg_arc.lock().unwrap();
                root.join(&cfg.records_dir)
            };

            let video_paths: Vec<PathBuf> = video_str
                .split(';')
                .filter(|s| !s.trim().is_empty())
                .map(|s| PathBuf::from(s.trim()))
                .collect();

            let mut parsed_dates = Vec::new();
            for part in dates_str.split(',') {
                let trimmed = part.trim();
                if let Ok(d) = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
                    parsed_dates.push(d);
                }
            }
            if parsed_dates.is_empty() {
                parsed_dates.push(chrono::Utc::now().date_naive());
            }

            if let Some(ui) = handle_weak.upgrade() {
                ui.set_imp_busy(true);
                ui.set_imp_console_text(format!("Запуск импорта чата для канала {}...\n", ch).into());
            }

            let h = handle_weak.clone();
            let rf = refresh_fn.clone();
            let root_thread = root.clone();
            let cfg_arc_thread = cfg_arc.clone();
            let ch_for_thread = ch.clone();

            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();

                let h_for_log = h.clone();
                let log_cb = move |msg: String| {
                    let h_inner = h_for_log.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = h_inner.upgrade() {
                            let mut cur = ui.get_imp_console_text().to_string();
                            cur.push_str(&msg);
                            cur.push('\n');
                            ui.set_imp_console_text(cur.into());
                        }
                    });
                };

                let params = importer::ImportParams {
                    channel: ch_for_thread.clone(),
                    dates: parsed_dates,
                    video_paths,
                    start_dt: None,
                    auto_detect: true,
                    records_dir,
                };

                let cache_root = {
                    let cfg = cfg_arc_thread.lock().unwrap();
                    root_thread.join(&cfg.emote_cache_dir)
                };

                let res = rt.block_on(importer::run_import_task(params, log_cb.clone()));
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = h.upgrade() {
                        ui.set_imp_busy(false);
                        match res {
                            Ok(session_path) => {
                                let mut cur = ui.get_imp_console_text().to_string();
                                cur.push_str(&format!("Готово! Создана сессия: {}\n", session_path.display()));
                                ui.set_imp_console_text(cur.into());

                                // Ensure emotes
                                let h2 = h.clone();
                                let ch_clone = ch_for_thread.clone();
                                let sp_clone = session_path.clone();
                                let cr_clone = cache_root.clone();
                                let rf_clone = rf.clone();

                                std::thread::spawn(move || {
                                    let rt2 = tokio::runtime::Builder::new_current_thread()
                                        .enable_all()
                                        .build()
                                        .unwrap();

                                    let log_cb2 = move |msg: String| {
                                        let h_inner = h2.clone();
                                        let _ = slint::invoke_from_event_loop(move || {
                                            if let Some(ui) = h_inner.upgrade() {
                                                let mut text = ui.get_imp_console_text().to_string();
                                                text.push_str(&msg);
                                                text.push('\n');
                                                ui.set_imp_console_text(text.into());
                                            }
                                        });
                                    };

                                    let _ = rt2.block_on(emotes::ensure_session_emotes(
                                        &sp_clone,
                                        &ch_clone,
                                        None,
                                        &cr_clone,
                                        log_cb2,
                                    ));

                                    let _ = slint::invoke_from_event_loop(move || {
                                        rf_clone();
                                    });
                                });
                            }
                            Err(e) => {
                                let mut cur = ui.get_imp_console_text().to_string();
                                cur.push_str(&format!("Ошибка импорта: {}\n", e));
                                ui.set_imp_console_text(cur.into());
                            }
                        }
                    }
                });
            });
        });
    }

    main_window.run()
}
