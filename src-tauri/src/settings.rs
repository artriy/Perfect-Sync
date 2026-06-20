//! App settings + well-known paths under %APPDATA%/Perfect-Sync.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// A mod the user always wants merged into any lobby code they apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalMod {
    pub repo: String,
    pub tag: String,
    pub asset: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub github_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub game_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub catalog_url: Option<String>,
    #[serde(default)]
    pub personal_mods: Vec<PersonalMod>,
    #[serde(default)]
    pub setup_complete: bool,
    #[serde(default)]
    pub skip_launch_warning: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub store: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub active_profile: Option<String>,
}

pub fn cache_dir() -> PathBuf {
    app_data_dir().join("cache")
}

pub fn catalog_cache_path() -> PathBuf {
    app_data_dir().join("catalog.json")
}

pub fn user_catalog_path() -> PathBuf {
    app_data_dir().join("user_catalog.json")
}

pub fn app_data_dir() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("Perfect-Sync")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_mod_without_enabled_defaults_on() {
        let pm: PersonalMod =
            serde_json::from_str(r#"{"repo":"a/b","tag":"v1","asset":"x.dll"}"#).unwrap();
        assert!(pm.enabled);
    }
}
