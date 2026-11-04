use crate::config::AppConfig;
use crate::records::SessionInfo;
use std::path::Path;
use std::process::Command;

pub fn launch_player(
    project_root: &Path,
    config: &AppConfig,
    session: &SessionInfo,
) -> Result<(), String> {
    if session.video_parts.is_empty() {
        return Err("Сессия не содержит видеофайлов для воспроизведения.".to_string());
    }

    let mpv_bin = if config.mpv_path.is_empty() {
        "mpv"
    } else {
        &config.mpv_path
    };

    let ipc_socket = if cfg!(windows) {
        r"\\.\pipe\mpv_socket"
    } else {
        "/tmp/mpv_socket"
    };

    let session_path = session
        .session_file
        .as_ref()
        .unwrap_or(&session.dir);

    let script_opts = format!("twitch_session={}", session_path.display());

    let rust_dll_root = project_root.join("twitch_chat.dll");
    let rust_dll_build = project_root.join("build").join("twitch_chat.dll");
    let lua_script = project_root.join("twitch_chat.lua");
    let lua_input_script = project_root.join("twitch_chat_input.lua");

    let engine = config.chat_engine.to_lowercase();
    let (chosen_script, is_rust) = if engine == "lua" {
        (lua_script, false)
    } else if engine == "rust" {
        if rust_dll_root.is_file() {
            (rust_dll_root, true)
        } else if rust_dll_build.is_file() {
            (rust_dll_build, true)
        } else {
            (lua_script, false)
        }
    } else {
        // Auto
        if rust_dll_root.is_file() {
            (rust_dll_root, true)
        } else if rust_dll_build.is_file() {
            (rust_dll_build, true)
        } else {
            (lua_script, false)
        }
    };

    let mpv_log = project_root.join("mpv.log");

    let mut cmd = Command::new(mpv_bin);
    cmd.current_dir(project_root);
    cmd.arg(format!("--script={}", chosen_script.display()));
    cmd.arg(format!("--script-opts={}", script_opts));
    cmd.arg(format!("--input-ipc-server={}", ipc_socket));
    cmd.arg("--keep-open=yes");
    cmd.arg(format!("--log-file={}", mpv_log.display()));

    if is_rust && lua_input_script.is_file() {
        cmd.arg(format!("--script={}", lua_input_script.display()));
    }

    // Add video parts
    for vp in &session.video_parts {
        cmd.arg(vp);
    }

    match cmd.spawn() {
        Ok(_) => Ok(()),
        Err(e) => Err(format!(
            "Не удалось запустить плеер mpv ('{}'): {}",
            mpv_bin, e
        )),
    }
}
