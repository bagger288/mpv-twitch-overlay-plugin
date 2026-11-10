use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use twitch_chat::emotes::EmoteCache;
use twitch_chat::layout::{ChatLayouter, LayoutMessage};
use twitch_chat::renderer::ChatRenderer;
use twitch_chat::session::SessionManager;
use twitch_chat::ui::{TextEffect, UiState};

pub fn run_render_chat(args: &[String]) -> Result<(), String> {
    let _ = fs::write("render_chat.log", format!("Starting render-chat with args: {:?}\n", args));
    let mut session_path = String::new();
    let mut video_name = String::new();
    let mut start_sec: f64 = 0.0;
    let mut duration_sec: f64 = 0.0;
    let mut output_path = String::new();
    let mut fps: f64 = 25.0;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--session" => {
                if i + 1 < args.len() {
                    session_path = args[i + 1].clone();
                    i += 1;
                }
            }
            "--video" => {
                if i + 1 < args.len() {
                    video_name = args[i + 1].clone();
                    i += 1;
                }
            }
            "--start" => {
                if i + 1 < args.len() {
                    start_sec = args[i + 1].parse().unwrap_or(0.0);
                    i += 1;
                }
            }
            "--duration" => {
                if i + 1 < args.len() {
                    duration_sec = args[i + 1].parse().unwrap_or(0.0);
                    i += 1;
                }
            }
            "--output" => {
                if i + 1 < args.len() {
                    output_path = args[i + 1].clone();
                    i += 1;
                }
            }
            "--fps" => {
                if i + 1 < args.len() {
                    fps = args[i + 1].parse().unwrap_or(25.0);
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    if session_path.is_empty() {
        return Err("Missing required argument --session".to_string());
    }
    if output_path.is_empty() {
        return Err("Missing required argument --output".to_string());
    }
    if duration_sec <= 0.0 {
        return Err(format!("Invalid duration: {}", duration_sec));
    }

    let session = SessionManager::try_load(Path::new(&session_path))
        .ok_or_else(|| format!("Failed to load session from: {}", session_path))?;

    let mut ui = UiState::default();

    // Look for global config.json first, then fallback to session chat_settings.json
    let config_candidates = [
        session.session_dir.join("../../config.json"),
        session.session_dir.join("../config.json"),
        std::path::PathBuf::from("config.json"),
        session.session_dir.join("chat_settings.json"),
    ];

    for cfg_path in &config_candidates {
        if cfg_path.is_file() {
            if let Ok(settings_str) = fs::read_to_string(cfg_path) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&settings_str) {
                    let mut matched = false;
                    if let Some(w) = val.get("chat_width").and_then(|v| v.as_u64()) {
                        ui.panel_width = w as u32;
                        matched = true;
                    }
                    if let Some(h) = val.get("chat_height").and_then(|v| v.as_u64()) {
                        ui.panel_height = h as u32;
                        matched = true;
                    }
                    if let Some(x) = val.get("chat_x").and_then(|v| v.as_i64()) {
                        ui.panel_x = x as i32;
                        matched = true;
                    }
                    if let Some(y) = val.get("chat_y").and_then(|v| v.as_i64()) {
                        ui.panel_y = y as i32;
                        matched = true;
                    }
                    if let Some(fs) = val.get("chat_font_size").and_then(|v| v.as_f64()) {
                        ui.font_size = fs as f32;
                        matched = true;
                    }
                    if let Some(op) = val.get("chat_bg_opacity").and_then(|v| v.as_f64()) {
                        ui.bg_opacity = op as f32;
                        matched = true;
                    }
                    if let Some(em) = val.get("emotes_enabled").and_then(|v| v.as_bool()) {
                        ui.emotes_enabled = em;
                        matched = true;
                    }
                    if let Some(bg) = val.get("badges_enabled").and_then(|v| v.as_bool()) {
                        ui.badges_enabled = bg;
                        matched = true;
                    }
                    if let Some(te) = val.get("chat_text_effect").and_then(|v| v.as_str()) {
                        if let Some(eff) = TextEffect::parse(te) {
                            ui.text_effect = eff;
                            matched = true;
                        }
                    }
                    if let Some(ow) = val.get("chat_outline_width").and_then(|v| v.as_f64()) {
                        ui.outline_width = ow as f32;
                        matched = true;
                    }
                    if matched {
                        break;
                    }
                }
            }
        }
    }

    let video_start = session.video_start_ms(&video_name);
    let offset_ms = session.video_offset_ms(&video_name);
    let clip_start_ms = video_start + offset_ms + (start_sec * 1000.0).round() as i64;

    let layouter = ChatLayouter::new();
    let cache_dir = session.cache_directory();
    let emote_cache = EmoteCache::with_cache_dir(&session.session_dir, cache_dir.as_deref());
    let renderer = ChatRenderer::new();

    let total_frames = ((duration_sec * fps).ceil() as usize).max(1);

    let mut ffmpeg = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-y",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            &format!("{}x{}", ui.panel_width, ui.panel_height),
            "-framerate",
            &format!("{:.2}", fps),
            "-i",
            "pipe:0",
            "-c:v",
            "png",
            "-pix_fmt",
            "rgba",
            &output_path,
        ])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn ffmpeg: {}", e))?;

    let mut stdin = ffmpeg.stdin.take().ok_or("Failed to open ffmpeg stdin")?;

    let mut last_eff_idx: usize = usize::MAX;
    let mut cached_layout: Option<Vec<LayoutMessage>> = None;

    for frame_idx in 0..total_frames {
        let rel_sec = frame_idx as f64 / fps;
        let current_ms = clip_start_ms + (rel_sec * 1000.0) as i64;
        let elapsed = Duration::from_secs_f64(rel_sec);

        let eff_idx = session.messages.partition_point(|m| m.timestamp_ms <= current_ms);

        if eff_idx != last_eff_idx || cached_layout.is_none() {
            last_eff_idx = eff_idx;
            let msgs = session.visible_messages(current_ms, &ui, 80);
            cached_layout = Some(layouter.layout_messages_with_icons(
                msgs,
                &session.emotes_manifest,
                &emote_cache,
                ui.panel_width as f32,
                ui.font_size,
                ui.emotes_enabled,
                ui.badges_enabled,
            ));
        }

        let layout_msgs = cached_layout.as_ref().unwrap();
        let frame_bytes = if let Some(pixmap) = renderer.render_clip_frame(layout_msgs, &emote_cache, &ui, elapsed) {
            pixmap.data().to_vec()
        } else {
            vec![0u8; (ui.panel_width * ui.panel_height * 4) as usize]
        };

        if let Err(e) = stdin.write_all(&frame_bytes) {
            return Err(format!("Error writing to ffmpeg stdin: {}", e));
        }
    }

    drop(stdin);
    let status = ffmpeg.wait().map_err(|e| format!("FFmpeg wait error: {}", e))?;
    if !status.success() {
        return Err(format!("FFmpeg failed with code {:?}", status.code()));
    }

    println!("RENDER_CHAT_OK x={} y={} w={} h={}", ui.panel_x, ui.panel_y, ui.panel_width, ui.panel_height);
    Ok(())
}
