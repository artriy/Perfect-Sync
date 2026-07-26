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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SteamClient {
    Native,
    Flatpak,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SteamRoot {
    path: PathBuf,
    client: SteamClient,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInstall {
    pub path: PathBuf,
    pub store: Store,
    pub arch: Arch,
    pub runtime: Runtime,
    pub build: Option<String>,
    pub writable: bool,
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

/// Read the Unity player version embedded in `globalgamemanagers`. Among Us
/// publishes calendar-style versions (for example `2026.3.31`), while the same
/// file also contains the older Unity engine version. Selecting the greatest
/// valid date tuple reliably distinguishes the game build without launching it.
pub fn detect_build(game_dir: &Path) -> Option<String> {
    use std::io::Read;

    const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;
    let path = game_dir.join("Among Us_Data").join("globalgamemanagers");
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_METADATA_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let pattern = regex::Regex::new(r"\b(20\d{2})\.(\d{1,2})\.(\d{1,2})(?:\.[0-9A-Za-z-]+)?\b")
        .expect("static build regex");
    pattern
        .captures_iter(&text)
        .filter_map(|capture| {
            let year = capture[1].parse::<u16>().ok()?;
            let month = capture[2].parse::<u8>().ok()?;
            let day = capture[3].parse::<u8>().ok()?;
            if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
                return None;
            }
            Some(((year, month, day), capture[0].to_string()))
        })
        .max_by_key(|(date, _)| *date)
        .map(|(_, version)| version)
}

/// Verify actual directory write access instead of trusting a read-only bit,
/// which does not reflect WindowsApps/Game Pass ACLs.
pub fn is_writable_game_dir(game_dir: &Path) -> bool {
    let probe = game_dir.join(format!(".perfectsync-write-test-{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            fs::remove_file(probe).is_ok()
        }
        Err(_) => false,
    }
}

/// Build a GameInstall, reading the real exe's bitness when present and falling
/// back to the store's known arch.
fn make_install(path: PathBuf, store: Store, runtime: Runtime) -> GameInstall {
    let arch =
        exe_arch(&path.join(crate::process::GAME_EXE)).unwrap_or_else(|| arch_for_store(store));
    let build = detect_build(&path);
    let writable = is_writable_game_dir(&path);
    GameInstall {
        path,
        store,
        arch,
        runtime,
        build,
        writable,
    }
}

/// Native on Windows, Proton on Linux, and Wine on other hosts.
fn steam_runtime() -> Runtime {
    if cfg!(target_os = "windows") {
        Runtime::Native
    } else if cfg!(target_os = "linux") {
        Runtime::Proton
    } else {
        Runtime::Wine
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

fn same_path(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ if cfg!(windows) => normalized_path(left) == normalized_path(right),
        _ => left == right,
    }
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn push_unique_path(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !paths.iter().any(|existing| same_path(existing, &candidate)) {
        paths.push(candidate);
    }
}

fn steam_library_roots(steam_root: &Path) -> Option<Vec<PathBuf>> {
    let vdf = fs::read_to_string(steam_root.join("steamapps").join("libraryfolders.vdf")).ok()?;
    let mut libraries = vec![steam_root.to_path_buf()];
    for library in parse_libraryfolders(&vdf) {
        push_unique_path(&mut libraries, library);
    }
    Some(libraries)
}

/// Locate every registered Among Us install reachable from one Steam root.
pub fn locate_steam_all(steam_root: &Path) -> Vec<GameInstall> {
    let Some(libraries) = steam_library_roots(steam_root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for library in libraries {
        let steamapps = library.join("steamapps");
        let acf = steamapps.join(format!("appmanifest_{STEAM_APP_ID}.acf"));
        let Ok(acf_text) = fs::read_to_string(&acf) else {
            continue;
        };
        let Some(installdir) = parse_acf_installdir(&acf_text) else {
            continue;
        };
        let game = steamapps.join("common").join(installdir);
        if game.join(crate::process::GAME_EXE).is_file()
            && !found
                .iter()
                .any(|install: &GameInstall| same_path(&install.path, &game))
        {
            found.push(make_install(game, Store::Steam, steam_runtime()));
        }
    }
    found
}

/// Locate the first registered install within one Steam root.
pub fn locate_steam(steam_root: &Path) -> Option<GameInstall> {
    locate_steam_all(steam_root).into_iter().next()
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
                if loc.join(crate::process::GAME_EXE).is_file() {
                    return Some(make_install(loc, Store::Epic, Runtime::Native));
                }
            }
        }
    }
    None
}

/// Extract Gaming Services `InstallLocation` values from `reg query` output.
/// This parser is deliberately indifferent to localized key names and spacing.
pub fn parse_msstore_install_locations(output: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in output.lines() {
        let Some((_, value)) = line.split_once("REG_SZ") else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        push_unique_path(&mut paths, PathBuf::from(value));
    }
    paths
}

#[cfg(windows)]
fn locate_msstore_all() -> Vec<GameInstall> {
    const ROOT: &str = r"HKLM\SOFTWARE\Microsoft\GamingServices\PackageRepository\Root";
    let Ok(output) = crate::process::command("reg")
        .args(["query", ROOT, "/s", "/v", "InstallLocation"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut found = Vec::new();
    for registered in parse_msstore_install_locations(&text) {
        let Some(game_dir) = find_exe_dir(&registered, 3) else {
            continue;
        };
        push_unique_install(
            &mut found,
            make_install(game_dir, Store::Msstore, Runtime::Native),
        );
    }
    found
}

#[cfg(not(windows))]
fn locate_msstore_all() -> Vec<GameInstall> {
    Vec::new()
}

/// Candidate Steam roots for the current host, retaining whether each belongs
/// to the native client or its Flatpak sandbox.
fn steam_roots() -> Vec<SteamRoot> {
    let mut roots = Vec::new();
    let mut push = |path: PathBuf, client: SteamClient| {
        if !roots
            .iter()
            .any(|root: &SteamRoot| root.client == client && same_path(&root.path, &path))
        {
            roots.push(SteamRoot { path, client });
        }
    };
    if cfg!(windows) {
        // Registry: HKCU\Software\Valve\Steam\SteamPath
        if let Ok(out) = crate::process::command("reg")
            .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = text.lines().find(|line| line.contains("SteamPath")) {
                if let Some(path) = line.split("REG_SZ").nth(1) {
                    push(PathBuf::from(path.trim()), SteamClient::Native);
                }
            }
        }
        for path in [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"] {
            push(PathBuf::from(path), SteamClient::Native);
        }
    } else if let Some(home) = home_dir() {
        if cfg!(target_os = "linux") {
            for relative in [".steam/steam", ".steam/root", ".local/share/Steam"] {
                push(home.join(relative), SteamClient::Native);
            }
            push(
                home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
                SteamClient::Flatpak,
            );
        } else if cfg!(target_os = "macos") {
            push(
                home.join("Library/Application Support/Steam"),
                SteamClient::Native,
            );
        }
    }
    roots
}

fn steam_installs_from_roots(roots: &[SteamRoot]) -> Vec<(GameInstall, SteamClient)> {
    let mut found = Vec::new();
    for root in roots {
        for install in locate_steam_all(&root.path) {
            if !found
                .iter()
                .any(|(existing, client): &(GameInstall, SteamClient)| {
                    *client == root.client && same_path(&existing.path, &install.path)
                })
            {
                found.push((install, root.client));
            }
        }
    }
    found
}

fn steam_installs() -> Vec<(GameInstall, SteamClient)> {
    steam_installs_from_roots(&steam_roots())
}

fn steam_client_for_install_from(
    installs: &[(GameInstall, SteamClient)],
    game_dir: &Path,
) -> Option<SteamClient> {
    let mut client = None;
    for (install, candidate) in installs {
        if !same_path(&install.path, game_dir) {
            continue;
        }
        match client {
            None => client = Some(*candidate),
            Some(existing) if existing == *candidate => {}
            Some(_) => return None,
        }
    }
    client
}

/// Return the one Steam client which has this exact path registered. Ambiguous
/// native/Flatpak registrations are rejected rather than choosing a client.
pub(crate) fn steam_client_for_install(game_dir: &Path) -> Option<SteamClient> {
    steam_client_for_install_from(&steam_installs(), game_dir)
}

fn steam_root_for_install_from<'a>(
    roots: &'a [SteamRoot],
    game_dir: &Path,
) -> Option<&'a SteamRoot> {
    let mut registration = None;
    for root in roots {
        if !locate_steam_all(&root.path)
            .iter()
            .any(|install| same_path(&install.path, game_dir))
        {
            continue;
        }
        match registration {
            None => registration = Some(root),
            Some(existing)
                if existing.client == root.client && same_path(&existing.path, &root.path) => {}
            Some(_) => return None,
        }
    }
    registration
}

fn native_steam_client_for_install_from(roots: &[SteamRoot], game_dir: &Path) -> Option<PathBuf> {
    let root = steam_root_for_install_from(roots, game_dir)?;
    if root.client != SteamClient::Native {
        return None;
    }
    let name = if cfg!(windows) { "steam.exe" } else { "steam" };
    let client = root.path.join(name);
    client.is_file().then_some(client)
}

/// Return the native Steam executable belonging to the unique Steam root that
/// registered this exact game path. Flatpak and ambiguous registrations return
/// `None` so launchers can use the runtime-specific compatibility path instead.
pub fn native_steam_client_for_install(game_dir: &Path) -> Option<PathBuf> {
    native_steam_client_for_install_from(&steam_roots(), game_dir)
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

/// Infer the storefront from the actual game path instead of assigning every
/// bottle the same store.
fn store_for_path(path: &Path, fallback: Store) -> Store {
    let p = path.to_string_lossy().replace('\\', "/").to_lowercase();
    if p.contains("/steamapps/") {
        Store::Steam
    } else if path.join(".egstore").is_dir() || p.contains("/epic games/") {
        Store::Epic
    } else if p.contains("/windowsapps/") || p.contains("/xboxgames/") {
        Store::Msstore
    } else {
        fallback
    }
}

/// Find Among Us inside a Wine/CrossOver prefix: known spots first, then a
/// bounded search of `drive_c`.
fn locate_in_prefix(prefix: &Path, fallback_store: Store, runtime: Runtime) -> Option<GameInstall> {
    let drive_c = prefix.join("drive_c");
    for c in [
        "Program Files (x86)/Steam/steamapps/common/Among Us",
        "Program Files/Steam/steamapps/common/Among Us",
        "Program Files (x86)/Among Us",
        "Program Files/Among Us",
    ] {
        let dir = drive_c.join(c);
        if dir.join(crate::process::GAME_EXE).is_file() {
            return Some(make_install(
                dir.clone(),
                store_for_path(&dir, fallback_store),
                runtime,
            ));
        }
    }
    find_exe_dir(&drive_c, 5).map(|dir| {
        let store = store_for_path(&dir, fallback_store);
        make_install(dir, store, runtime)
    })
}

fn collect_plist_paths(value: &plist::Value, out: &mut Vec<PathBuf>) {
    match value {
        plist::Value::String(s) => {
            let path = if let Ok(url) = url::Url::parse(s) {
                if url.scheme() == "file" {
                    url.to_file_path().ok()
                } else {
                    None
                }
            } else {
                Some(PathBuf::from(s))
            };
            if let Some(path) = path {
                if path.join("drive_c").is_dir() && !out.contains(&path) {
                    out.push(path);
                }
            }
        }
        plist::Value::Array(values) => {
            for value in values {
                collect_plist_paths(value, out);
            }
        }
        plist::Value::Dictionary(values) => {
            for value in values.values() {
                collect_plist_paths(value, out);
            }
        }
        _ => {}
    }
}

/// Whisky's current default plus bottle URLs persisted in `BottleVM.plist`.
/// Legacy locations remain candidates for users upgrading older Whisky builds.
pub fn whisky_bottle_paths(home: &Path) -> Vec<PathBuf> {
    let container = home.join("Library/Containers/com.isaacmarovitz.Whisky");
    let mut paths = Vec::new();
    if let Ok(value) = plist::Value::from_file(container.join("BottleVM.plist")) {
        collect_plist_paths(&value, &mut paths);
    }
    for root in [
        container.join("Bottles"),
        container.join("Data/Documents/Bottles"),
        home.join("Library/Application Support/com.isaacmarovitz.Whisky/Bottles"),
        home.join("Library/Application Support/Whisky/Bottles"),
    ] {
        if root.is_dir() && !paths.contains(&root) {
            paths.push(root);
        }
    }
    paths
}

fn push_unique_install(found: &mut Vec<GameInstall>, install: GameInstall) {
    if !found
        .iter()
        .any(|existing| same_path(&existing.path, &install.path))
    {
        found.push(install);
    }
}

fn locate_bottles(found: &mut Vec<GameInstall>, root: &Path, store: Store, runtime: Runtime) {
    if root.join("drive_c").is_dir() {
        if let Some(game) = locate_in_prefix(root, store, runtime) {
            push_unique_install(found, game);
        }
        return;
    }
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if let Some(game) = locate_in_prefix(&entry.path(), store, runtime) {
                push_unique_install(found, game);
            }
        }
    }
}

fn locate_wine_prefixes(found: &mut Vec<GameInstall>, home: &Path, explicit_prefix: Option<&Path>) {
    let mut prefixes = Vec::new();
    if let Some(prefix) = explicit_prefix {
        push_unique_path(&mut prefixes, prefix.to_path_buf());
    }
    push_unique_path(&mut prefixes, home.join(".wine"));
    for prefix in prefixes {
        locate_bottles(found, &prefix, Store::Manual, Runtime::Wine);
    }
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
            if let Some(game) = locate_epic(&epic) {
                push_unique_install(&mut found, game);
            }
        }
        for game in locate_msstore_all() {
            push_unique_install(&mut found, game);
        }
        return found;
    }
    let explicit_prefix = std::env::var_os("WINEPREFIX").map(PathBuf::from);
    let Some(home) = home_dir() else {
        if let Some(prefix) = explicit_prefix {
            locate_bottles(&mut found, &prefix, Store::Manual, Runtime::Wine);
        }
        return found;
    };
    locate_wine_prefixes(&mut found, &home, explicit_prefix.as_deref());
    if cfg!(target_os = "macos") {
        locate_bottles(
            &mut found,
            &home.join("Library/Application Support/CrossOver/Bottles"),
            Store::Manual,
            Runtime::Crossover,
        );
        for bottle in whisky_bottle_paths(&home) {
            locate_bottles(&mut found, &bottle, Store::Manual, Runtime::Whisky);
        }
    } else {
        for root in [
            home.join(".var/app/com.usebottles.bottles/data/bottles/bottles"),
            home.join(".local/share/bottles/bottles"),
        ] {
            locate_bottles(&mut found, &root, Store::Manual, Runtime::Bottles);
        }
    }
    found
}

/// Best-effort detection across stores + runtimes on the current machine.
pub fn locate_all() -> Vec<GameInstall> {
    let mut found = Vec::new();
    for (install, _) in steam_installs() {
        push_unique_install(&mut found, install);
    }
    for install in locate_other() {
        push_unique_install(&mut found, install);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_steam_registration(root: &Path, library: &Path, install_dir: &str) -> PathBuf {
        let root_steamapps = root.join("steamapps");
        fs::create_dir_all(&root_steamapps).unwrap();
        let vdf = format!(
            "\"libraryfolders\"\n{{\n\"0\"\n{{\n\"path\" \"{}\"\n}}\n}}\n",
            library.to_string_lossy().replace('\\', "\\\\")
        );
        fs::write(root_steamapps.join("libraryfolders.vdf"), vdf).unwrap();
        let steamapps = library.join("steamapps");
        let game = steamapps.join("common").join(install_dir);
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join(crate::process::GAME_EXE), b"MZ").unwrap();
        fs::write(
            steamapps.join(format!("appmanifest_{STEAM_APP_ID}.acf")),
            format!(r#""AppState" {{ "installdir" "{install_dir}" }}"#),
        )
        .unwrap();
        game
    }

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
        assert_eq!(
            parse_epic_manifest(yes),
            Some(PathBuf::from(r"C:\Games\AmongUs"))
        );
        assert_eq!(parse_epic_manifest(no), None);
    }

    #[test]
    fn parses_msstore_registry_locations() {
        let output = r#"
HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\GamingServices\PackageRepository\Root\one
    InstallLocation    REG_SZ    D:\XboxGames\Among Us\Content

HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\GamingServices\PackageRepository\Root\two
    InstallLocation    REG_SZ    C:\Program Files\WindowsApps\Innersloth.AmongUs
"#;
        assert_eq!(
            parse_msstore_install_locations(output),
            vec![
                PathBuf::from(r"D:\XboxGames\Among Us\Content"),
                PathBuf::from(r"C:\Program Files\WindowsApps\Innersloth.AmongUs"),
            ]
        );
    }

    #[test]
    fn detects_calendar_game_build_over_unity_version() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("Among Us_Data");
        fs::create_dir_all(&data).unwrap();
        fs::write(
            data.join("globalgamemanagers"),
            b"\0Unity 2022.3.44\0Among Us 2026.3.31\0fallback 2022.3.44",
        )
        .unwrap();
        assert_eq!(detect_build(temp.path()).as_deref(), Some("2026.3.31"));
    }

    #[test]
    fn write_probe_cleans_up_after_itself() {
        let temp = tempfile::tempdir().unwrap();
        assert!(is_writable_game_dir(temp.path()));
        assert!(!temp
            .path()
            .join(format!(".perfectsync-write-test-{}", std::process::id()))
            .exists());
    }

    #[test]
    fn locates_steam_install_from_fixture_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let steamapps = root.join("steamapps");
        fs::create_dir_all(steamapps.join("common").join("Among Us")).unwrap();
        fs::write(
            steamapps
                .join("common")
                .join("Among Us")
                .join(crate::process::GAME_EXE),
            b"MZ",
        )
        .unwrap();
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
    fn enumerates_and_deduplicates_native_flatpak_and_external_libraries() {
        let tmp = tempfile::tempdir().unwrap();
        let native_root = tmp.path().join("native Steam");
        let flatpak_root = tmp.path().join("flatpak Steam");
        let external = tmp.path().join("External Library");
        let native_game = write_steam_registration(&native_root, &native_root, "Among Us Native");
        let flatpak_game = write_steam_registration(&flatpak_root, &external, "Among Us Flatpak");
        let client_name = if cfg!(windows) { "steam.exe" } else { "steam" };
        let native_client = native_root.join(client_name);
        fs::write(&native_client, b"native client").unwrap();
        fs::write(flatpak_root.join(client_name), b"not a native client").unwrap();
        let roots = vec![
            SteamRoot {
                path: native_root.clone(),
                client: SteamClient::Native,
            },
            SteamRoot {
                path: native_root,
                client: SteamClient::Native,
            },
            SteamRoot {
                path: flatpak_root,
                client: SteamClient::Flatpak,
            },
        ];

        let found = steam_installs_from_roots(&roots);
        assert_eq!(found.len(), 2);
        assert!(found
            .iter()
            .any(|(install, client)| same_path(&install.path, &native_game)
                && *client == SteamClient::Native));
        assert!(found
            .iter()
            .any(|(install, client)| same_path(&install.path, &flatpak_game)
                && *client == SteamClient::Flatpak));
        assert_eq!(
            steam_client_for_install_from(&found, &native_game),
            Some(SteamClient::Native)
        );
        assert_eq!(
            steam_client_for_install_from(&found, &flatpak_game),
            Some(SteamClient::Flatpak)
        );
        assert_eq!(
            native_steam_client_for_install_from(&roots, &native_game),
            Some(native_client)
        );
        assert_eq!(
            native_steam_client_for_install_from(&roots, &flatpak_game),
            None
        );
    }

    #[test]
    fn ambiguous_native_and_flatpak_registration_has_no_native_client() {
        let tmp = tempfile::tempdir().unwrap();
        let native_root = tmp.path().join("native");
        let flatpak_root = tmp.path().join("flatpak");
        let shared_library = tmp.path().join("shared");
        let game = write_steam_registration(&native_root, &shared_library, "Shared Among Us");
        write_steam_registration(&flatpak_root, &shared_library, "Shared Among Us");
        let client_name = if cfg!(windows) { "steam.exe" } else { "steam" };
        fs::write(native_root.join(client_name), b"native client").unwrap();
        let roots = vec![
            SteamRoot {
                path: native_root,
                client: SteamClient::Native,
            },
            SteamRoot {
                path: flatpak_root,
                client: SteamClient::Flatpak,
            },
        ];

        assert!(steam_root_for_install_from(&roots, &game).is_none());
        assert!(native_steam_client_for_install_from(&roots, &game).is_none());
        assert!(steam_client_for_install_from(&steam_installs_from_roots(&roots), &game).is_none());
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
    #[test]
    fn reads_whisky_persisted_bottle_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bottle = tmp.path().join("Custom Whisky Bottle");
        fs::create_dir_all(bottle.join("drive_c")).unwrap();
        let container = home.join("Library/Containers/com.isaacmarovitz.Whisky");
        fs::create_dir_all(&container).unwrap();
        let url = url::Url::from_directory_path(&bottle).unwrap().to_string();
        plist::to_file_xml(container.join("BottleVM.plist"), &vec![url]).unwrap();

        assert_eq!(whisky_bottle_paths(&home), vec![bottle]);
    }

    #[test]
    fn discovers_explicit_and_default_wine_prefixes_on_any_unix_host() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let default_prefix = home.join(".wine");
        let custom_prefix = tmp.path().join("Custom macOS Wine Prefix");
        for (prefix, name) in [
            (&default_prefix, "Default Among Us"),
            (&custom_prefix, "Custom Among Us"),
        ] {
            let game = prefix.join("drive_c/Games").join(name);
            fs::create_dir_all(&game).unwrap();
            fs::write(game.join(crate::process::GAME_EXE), b"MZ").unwrap();
        }

        let mut found = Vec::new();
        locate_wine_prefixes(&mut found, &home, Some(&custom_prefix));
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|install| install.runtime == Runtime::Wine));
    }
}
