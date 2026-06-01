//! App settings + well-known paths under %APPDATA%/Perfect-Sync.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub github_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub game_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arch: Option<String>,
}

pub fn app_data_dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("Perfect-Sync")
}

pub fn profiles_root() -> PathBuf {
    app_data_dir().join("profiles")
}

fn settings_path() -> PathBuf {
    app_data_dir().join("settings.json")
}

pub fn load() -> Settings {
    fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(s: &Settings) -> std::io::Result<()> {
    fs::create_dir_all(app_data_dir())?;
    fs::write(settings_path(), serde_json::to_string_pretty(s)?)
}
