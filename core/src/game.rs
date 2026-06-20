//! GameLocator: detect Among Us installs across stores and map store -> architecture.
//!
//! The pure parsers (`parse_libraryfolders`, `parse_acf_installdir`,
//! `parse_epic_manifest`) are unit-tested against fixture strings. The
//! filesystem locators take explicit roots so they can be tested with temp dirs.

use crate::types::{Arch, Runtime, Store};
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
    pub runtime: Runtime,
}

/// Architecture a store's Among Us build uses (Steam/Epic/itch = x86, MS Store = x64).
pub fn arch_for_store(store: Store) -> Arch {
    match store {
        Store::Msstore => Arch::X64,
        _ => Arch::X86,
    }
}

/// Read a PE's COFF machine field to tell x86 from x64. Works for any Windows
/// build regardless of host OS (the game is always a Windows exe). `None` when
/// the file is missing or not a PE, so callers fall back to the store's arch.
pub fn exe_arch(exe: &Path) -> Option<Arch> {
    use std::io::Read;
    let mut buf = [0u8; 4096];
    let n = fs::File::open(exe).ok()?.read(&mut buf).ok()?;
    let b = &buf[..n];
    if b.len() < 0x40 || &b[0..2] != b"MZ" {
        return None;
    }
    let e = u32::from_le_bytes(b[0x3C..0x40].try_into().ok()?) as usize;
    if b.len() < e + 6 || &b[e..e + 4] != b"PE\0\0" {
        return None;
    }
    match u16::from_le_bytes(b[e + 4..e + 6].try_into().ok()?) {
        0x014c => Some(Arch::X86),
        0x8664 => Some(Arch::X64),
        _ => None,
    }
}

/// Build a GameInstall, reading the real exe's bitness when present and falling
/// back to the store's known arch.
fn make_install(path: PathBuf, store: Store, runtime: Runtime) -> GameInstall {
    let arch =
        exe_arch(&path.join(crate::process::GAME_EXE)).unwrap_or_else(|| arch_for_store(store));
    GameInstall { path, store, arch, runtime }
}

/// Steam installs run natively on Windows, under Proton everywhere else.
fn steam_runtime() -> Runtime {
    if cfg!(windows) {
        Runtime::Native
    } else {
        Runtime::Proton
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

/// The Among Us folder Steam actually launches for `STEAM_APP_ID` (its
/// registered install), found across the host's known Steam roots. `None` when
/// no Steam install of Among Us is registered.
pub fn steam_install_path() -> Option<PathBuf> {
    steam_roots().into_iter().find_map(|r| locate_steam(&r).map(|g| g.path))
}

/// The Steam client executable, used to start Steam before a direct launch that
/// relies on `steam_appid.txt`. `None` when Steam isn't installed.
pub fn steam_exe() -> Option<PathBuf> {
    let name = if cfg!(windows) { "steam.exe" } else { "steam" };
    steam_roots().into_iter().map(|r| r.join(name)).find(|p| p.is_file())
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
                return Some(make_install(game, Store::Steam, steam_runtime()));
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
                    return Some(make_install(loc, Store::Epic, Runtime::Native));
                }
            }
        }
    }
    None
}

/// Candidate Steam roots for the current host (registry + defaults on Windows;
/// the common XDG/Flatpak/Deck paths on Linux; the app-support path on macOS).
fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(windows) {
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
        for d in [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"] {
            roots.push(PathBuf::from(d));
        }
    } else if let Some(home) = home_dir() {
        for rel in [
            ".steam/steam",
            ".steam/root",
            ".local/share/Steam",
            ".var/app/com.valvesoftware.Steam/data/Steam",
            "Library/Application Support/Steam",
        ] {
            roots.push(home.join(rel));
        }
    }
    roots
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Depth-limited search for the dir holding `Among Us.exe`, skipping noisy
/// system trees so scanning a Wine `drive_c` stays cheap.
fn find_exe_dir(root: &Path, depth: usize) -> Option<PathBuf> {
    if root.join(crate::process::GAME_EXE).is_file() {
        return Some(root.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    for e in fs::read_dir(root).ok()?.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if matches!(name, "windows" | "Windows" | "ProgramData" | "$Recycle.Bin") {
            continue;
        }
        if let Some(found) = find_exe_dir(&p, depth - 1) {
            return Some(found);
        }
    }
    None
}

/// Find Among Us inside a Wine/CrossOver prefix: known spots first, then a
/// bounded search of `drive_c`.
fn locate_in_prefix(prefix: &Path, store: Store, runtime: Runtime) -> Option<GameInstall> {
    let drive_c = prefix.join("drive_c");
    for c in [
        "Program Files (x86)/Steam/steamapps/common/Among Us",
        "Program Files/Steam/steamapps/common/Among Us",
        "Program Files (x86)/Among Us",
        "Program Files/Among Us",
    ] {
        let dir = drive_c.join(c);
        if dir.join(crate::process::GAME_EXE).is_file() {
            return Some(make_install(dir, store, runtime));
        }
    }
    find_exe_dir(&drive_c, 5).map(|dir| make_install(dir, store, runtime))
}

/// Detection beyond Steam: Epic on Windows; Wine/CrossOver/Whisky/Bottles off it.
fn locate_other() -> Vec<GameInstall> {
    let mut found = Vec::new();
    if cfg!(windows) {
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
        return found;
    }
    let Some(home) = home_dir() else {
        return found;
    };
    let mut bottle_roots: Vec<(PathBuf, Store, Runtime)> = Vec::new();
    if cfg!(target_os = "macos") {
        bottle_roots.push((
            home.join("Library/Application Support/CrossOver/Bottles"),
            Store::Steam,
            Runtime::Crossover,
        ));
        bottle_roots.push((
            home.join("Library/Application Support/com.isaacmarovitz.Whisky/Bottles"),
            Store::Steam,
            Runtime::Wine,
        ));
    } else {
        if let Some(g) = locate_in_prefix(&home.join(".wine"), Store::Manual, Runtime::Wine) {
            found.push(g);
        }
        bottle_roots.push((
            home.join(".var/app/com.usebottles.bottles/data/bottles/bottles"),
            Store::Manual,
            Runtime::Wine,
        ));
    }
    for (root, store, runtime) in bottle_roots {
        if let Ok(rd) = fs::read_dir(&root) {
            for b in rd.flatten() {
                if let Some(g) = locate_in_prefix(&b.path(), store, runtime) {
                    found.push(g);
                    break;
                }
            }
        }
    }
    found
}

/// Best-effort detection across stores + runtimes on the current machine.
pub fn locate_all() -> Vec<GameInstall> {
    let mut found = Vec::new();
    for root in steam_roots() {
        if let Some(g) = locate_steam(&root) {
            found.push(g);
            break;
        }
    }
    found.extend(locate_other());
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

    #[test]
    fn exe_arch_reads_pe_machine() {
        let tmp = tempfile::tempdir().unwrap();
        let mk = |machine: [u8; 2]| {
            let mut b = vec![0u8; 0x100];
            b[0] = b'M';
            b[1] = b'Z';
            b[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
            b[0x80] = b'P';
            b[0x81] = b'E';
            b[0x84] = machine[0];
            b[0x85] = machine[1];
            b
        };
        let x86 = tmp.path().join("x86.exe");
        fs::write(&x86, mk([0x4c, 0x01])).unwrap();
        assert_eq!(exe_arch(&x86), Some(Arch::X86));
        let x64 = tmp.path().join("x64.exe");
        fs::write(&x64, mk([0x64, 0x86])).unwrap();
        assert_eq!(exe_arch(&x64), Some(Arch::X64));
        assert_eq!(exe_arch(&tmp.path().join("missing.exe")), None);
    }
}
