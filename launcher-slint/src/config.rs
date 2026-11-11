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

    #[serde(default)]
    pub window_x: Option<i32>,

    #[serde(default)]
    pub window_y: Option<i32>,

    #[serde(default)]
    pub window_width: Option<u32>,

    #[serde(default)]
    pub window_height: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_x: Option<i32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_y: Option<i32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_width: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_height: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_font_size: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_bg_opacity: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_enabled: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emotes_enabled: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badges_enabled: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_text_effect: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_outline_width: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_minimized_position: Option<(i32, i32)>,

    #[serde(default, flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
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
            window_x: None,
            window_y: None,
            window_width: None,
            window_height: None,
            chat_x: None,
            chat_y: None,
            chat_width: None,
            chat_height: None,
            chat_font_size: None,
            chat_bg_opacity: None,
            chat_enabled: None,
            emotes_enabled: None,
            badges_enabled: None,
            chat_text_effect: None,
            chat_outline_width: None,
            chat_minimized_position: None,
            extra: std::collections::HashMap::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_config_preserves_overlay_settings() {
        let temp_dir = std::env::temp_dir().join(format!("test_app_cfg_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = fs::create_dir_all(&temp_dir);

        let mut cfg = load_config(&temp_dir);
        cfg.window_x = Some(150);
        cfg.window_y = Some(200);
        cfg.window_width = Some(720);
        cfg.window_height = Some(820);
        cfg.chat_x = Some(350);
        cfg.chat_y = Some(80);
        cfg.chat_width = Some(420);
        cfg.chat_height = Some(650);
        cfg.chat_font_size = Some(16.5);
        cfg.chat_bg_opacity = Some(0.8);
        cfg.chat_enabled = Some(true);
        cfg.emotes_enabled = Some(true);
        cfg.badges_enabled = Some(false);
        cfg.chat_text_effect = Some("outline".to_string());
        cfg.chat_outline_width = Some(2.5);
        cfg.extra.insert("custom_property".to_string(), serde_json::json!("preserved_value"));

        assert!(save_config(&temp_dir, &cfg));

        let reloaded = load_config(&temp_dir);
        assert_eq!(reloaded.window_x, Some(150));
        assert_eq!(reloaded.window_y, Some(200));
        assert_eq!(reloaded.window_width, Some(720));
        assert_eq!(reloaded.window_height, Some(820));
        assert_eq!(reloaded.chat_x, Some(350));
        assert_eq!(reloaded.chat_y, Some(80));
        assert_eq!(reloaded.chat_width, Some(420));
        assert_eq!(reloaded.chat_height, Some(650));
        assert_eq!(reloaded.chat_font_size, Some(16.5));
        assert_eq!(reloaded.chat_bg_opacity, Some(0.8));
        assert_eq!(reloaded.chat_enabled, Some(true));
        assert_eq!(reloaded.badges_enabled, Some(false));
        assert_eq!(reloaded.chat_text_effect.as_deref(), Some("outline"));
        assert_eq!(reloaded.chat_outline_width, Some(2.5));
        assert_eq!(reloaded.extra.get("custom_property"), Some(&serde_json::json!("preserved_value")));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
