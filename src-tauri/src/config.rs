use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub api_key: String,
    pub language: String,
    pub hotkey: String,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default = "default_microphone")]
    pub microphone: String,
}

fn default_microphone() -> String {
    "default".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            language: "auto".to_string(),
            hotkey: "Ctrl+Win".to_string(),
            autostart: false,
            microphone: "default".to_string(),
        }
    }
}

pub fn config_path() -> PathBuf {
    std::env::var("APPDATA")
        .map(|p| PathBuf::from(p).join("vibe-voice-tool").join("config.json"))
        .unwrap_or_else(|_| PathBuf::from("config.json"))
}

pub fn load_config() -> AppConfig {
    let path = config_path();
    if path.exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(config) = serde_json::from_str(&content) {
                return config;
            }
        }
    }
    AppConfig::default()
}

pub fn save_config(config: &AppConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())
}
