use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_streamlink")]
    pub streamlink_path: String,

    #[serde(default = "default_mpv")]
    pub mpv_path: String,

    #[serde(default = "default_true")]
    pub default_chat_enabled: bool,

    #[serde(default = "default_true")]
    pub default_emotes_enabled: bool,

    #[serde(default)]
    pub default_offset_ms: i64,

    #[serde(default = "default_records_dir")]
    pub records_dir: String,

    #[serde(default = "default_emote_cache_dir")]
    pub emote_cache_dir: String,

    #[serde(default = "default_chat_engine")]
    pub chat_engine: String,

    #[serde(default)]
    pub server_url: String,

    #[serde(default)]
    pub twitch_proxy_playlist: String,

    #[serde(default)]
    pub twitch_client_id: String,

    #[serde(default)]
    pub twitch_app_token: String,

    #[serde(default)]
    pub twitch_client_secret: String,

    #[serde(default)]
    pub twitch_refresh_token: String,

    #[serde(default)]
    pub twitch_token_expires_at: u64,
}

fn default_streamlink() -> String { "streamlink".to_string() }
fn default_mpv() -> String { "mpv".to_string() }
fn default_records_dir() -> String { "records".to_string() }
fn default_emote_cache_dir() -> String { "emote_cache".to_string() }
fn default_chat_engine() -> String { "Auto".to_string() }
fn default_true() -> bool { true }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            streamlink_path: default_streamlink(),
            mpv_path: default_mpv(),
            default_chat_enabled: true,
            default_emotes_enabled: true,
            default_offset_ms: 0,
            records_dir: default_records_dir(),
            emote_cache_dir: default_emote_cache_dir(),
            chat_engine: default_chat_engine(),
            server_url: String::new(),
            twitch_proxy_playlist: String::new(),
            twitch_client_id: String::new(),
            twitch_app_token: String::new(),
            twitch_client_secret: String::new(),
            twitch_refresh_token: String::new(),
            twitch_token_expires_at: 0,
        }
    }
}

pub fn get_config_path(base_dir: &Path) -> PathBuf {
    base_dir.join("config.json")
}

pub fn load_config(base_dir: &Path) -> AppConfig {
    let path = get_config_path(base_dir);
    if path.is_file() {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<AppConfig>(&content) {
                return cfg;
            }
        }
    }
    AppConfig::default()
}

pub fn save_config(base_dir: &Path, config: &AppConfig) -> bool {
    let target = get_config_path(base_dir);
    let tmp = base_dir.join("config.json.tmp");

    match serde_json::to_string_pretty(config) {
        Ok(json_str) => {
            if let Ok(mut f) = File::create(&tmp) {
                if f.write_all(json_str.as_bytes()).is_ok() {
                    let _ = f.flush();
                    drop(f);
                    if fs::rename(&tmp, &target).is_ok() {
                        return true;
                    }
                }
            }
            let _ = fs::remove_file(&tmp);
            false
        }
        Err(_) => false,
    }
}
