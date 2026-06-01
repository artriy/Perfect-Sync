//! GameLocator: detect Among Us installs across stores and map store -> architecture.
//!
//! The pure parsers (`parse_libraryfolders`, `parse_acf_installdir`,
//! `parse_epic_manifest`) are unit-tested against fixture strings. The
//! filesystem locators take explicit roots so they can be tested with temp dirs.

use crate::types::{Arch, Store};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// Steam application id for Among Us.
pub const STEAM_APP_ID: &str = "945360";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameInstall {
    pub path: PathBuf,
    pub store: Store,
    pub arch: Arch,
}

/// Architecture a store's Among Us build uses (Steam/Epic/itch = x86, MS Store = x64).
pub fn arch_for_store(store: Store) -> Arch {
    match store {
        Store::Msstore => Arch::X64,
        _ => Arch::X86,
    }
}

fn unescape_vdf(s: &str) -> String {
    s.replace("\\\\", "\\")
}

/// Extract every library `path` from a Steam `libraryfolders.vdf`.
pub fn parse_libraryfolders(vdf: &str) -> Vec<PathBuf> {
    let re = regex::Regex::new(r#""path"\s*"([^"]*)""#).unwrap();
    re.captures_iter(vdf)
        .map(|c| PathBuf::from(unescape_vdf(&c[1])))
        .collect()
}

/// Extract the `installdir` from a Steam `appmanifest_*.acf`.
pub fn parse_acf_installdir(acf: &str) -> Option<String> {
    let re = regex::Regex::new(r#""installdir"\s*"([^"]*)""#).unwrap();
    re.captures(acf).map(|c| unescape_vdf(&c[1]))
}

/// Locate Among Us within a known Steam root by walking its library folders.
pub fn locate_steam(steam_root: &Path) -> Option<GameInstall> {
    let vdf = fs::read_to_string(steam_root.join("steamapps").join("libraryfolders.vdf")).ok()?;
    for lib in parse_libraryfolders(&vdf) {
        let steamapps = lib.join("steamapps");
        let acf = steamapps.join(format!("appmanifest_{STEAM_APP_ID}.acf"));
        let Ok(acf_text) = fs::read_to_string(&acf) else {
            continue;
        };
        if let Some(installdir) = parse_acf_installdir(&acf_text) {
            let game = steamapps.join("common").join(&installdir);
            if game.is_dir() {
                return Some(GameInstall {
                    path: game,
                    store: Store::Steam,
                    arch: Arch::X86,
                });
            }
        }
    }
    None
}

#[derive(serde::Deserialize)]
struct EpicManifest {
    #[serde(rename = "InstallLocation")]
    install_location: String,
    #[serde(rename = "DisplayName", default)]
    display_name: String,
}

/// Parse an Epic `.item` manifest, returning its install location if it is Among Us.
pub fn parse_epic_manifest(json: &str) -> Option<PathBuf> {
    let m: EpicManifest = serde_json::from_str(json).ok()?;
    if m.display_name.to_lowercase().contains("among us") {
        Some(PathBuf::from(m.install_location))
    } else {
        None
    }
}

/// Scan an Epic manifests directory (`.item` files) for an Among Us install.
pub fn locate_epic(manifests_dir: &Path) -> Option<GameInstall> {
    let entries = fs::read_dir(manifests_dir).ok()?;
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("item") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(entry.path()) {
            if let Some(loc) = parse_epic_manifest(&text) {
                if loc.is_dir() {
                    return Some(GameInstall {
                        path: loc,
                        store: Store::Epic,
                        arch: Arch::X86,
                    });
                }
            }
        }
    }
    None
}

/// Candidate Steam install roots: the registry value (if present) plus defaults.
fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    // Registry: HKCU\Software\Valve\Steam\SteamPath
    if let Ok(out) = std::process::Command::new("reg")
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(line) = text.lines().find(|l| l.contains("SteamPath")) {
            if let Some(p) = line.split("REG_SZ").nth(1) {
                roots.push(PathBuf::from(p.trim()));
            }
        }
    }
    for d in [
        r"C:\Program Files (x86)\Steam",
        r"C:\Program Files\Steam",
    ] {
        roots.push(PathBuf::from(d));
    }
    roots
}

/// Best-effort detection across all stores on the current machine.
pub fn locate_all() -> Vec<GameInstall> {
    let mut found = Vec::new();
    for root in steam_roots() {
        if let Some(g) = locate_steam(&root) {
            found.push(g);
            break;
        }
    }
    if let Ok(program_data) = std::env::var("ProgramData") {
        let epic = PathBuf::from(program_data)
            .join("Epic")
            .join("EpicGamesLauncher")
            .join("Data")
            .join("Manifests");
        if let Some(g) = locate_epic(&epic) {
            found.push(g);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_library_paths_and_unescapes() {
        let vdf = r#"
"libraryfolders"
{
    "0"
    {
        "path"		"C:\\Program Files (x86)\\Steam"
    }
    "1"
    {
        "path"		"D:\\SteamLibrary"
    }
}
"#;
        let paths = parse_libraryfolders(vdf);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from(r"C:\Program Files (x86)\Steam"));
        assert_eq!(paths[1], PathBuf::from(r"D:\SteamLibrary"));
    }

    #[test]
    fn parses_acf_installdir() {
        let acf = r#""AppState" { "appid" "945360" "installdir" "Among Us" }"#;
        assert_eq!(parse_acf_installdir(acf), Some("Among Us".to_string()));
    }

    #[test]
    fn parses_epic_manifest_only_for_among_us() {
        let yes = r#"{"InstallLocation":"C:\\Games\\AmongUs","DisplayName":"Among Us"}"#;
        let no = r#"{"InstallLocation":"C:\\Games\\Fortnite","DisplayName":"Fortnite"}"#;
        assert_eq!(parse_epic_manifest(yes), Some(PathBuf::from(r"C:\Games\AmongUs")));
        assert_eq!(parse_epic_manifest(no), None);
    }

    #[test]
    fn locates_steam_install_from_fixture_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let steamapps = root.join("steamapps");
        fs::create_dir_all(steamapps.join("common").join("Among Us")).unwrap();
        // libraryfolders.vdf points at this same root as library "0"
        let vdf = format!(
            "\"libraryfolders\"\n{{\n\"0\"\n{{\n\"path\" \"{}\"\n}}\n}}\n",
            root.to_string_lossy().replace('\\', "\\\\")
        );
        fs::write(steamapps.join("libraryfolders.vdf"), vdf).unwrap();
        fs::write(
            steamapps.join("appmanifest_945360.acf"),
            r#""AppState" { "installdir" "Among Us" }"#,
        )
        .unwrap();

        let found = locate_steam(root).unwrap();
        assert_eq!(found.store, Store::Steam);
        assert_eq!(found.arch, Arch::X86);
        assert!(found.path.ends_with("Among Us"));
    }

    #[test]
    fn returns_none_when_game_absent() {
        let tmp = tempfile::tempdir().unwrap();
        // libraryfolders present but no appmanifest
        let steamapps = tmp.path().join("steamapps");
        fs::create_dir_all(&steamapps).unwrap();
        fs::write(steamapps.join("libraryfolders.vdf"), "\"libraryfolders\"{}").unwrap();
        assert!(locate_steam(tmp.path()).is_none());
    }

    #[test]
    fn arch_mapping() {
        assert_eq!(arch_for_store(Store::Steam), Arch::X86);
        assert_eq!(arch_for_store(Store::Epic), Arch::X86);
        assert_eq!(arch_for_store(Store::Msstore), Arch::X64);
    }
}
