//! LoaderManager: install the Doorstop + BepInEx loader directly into the game
//! folder (the layout every manual install uses, so BepInEx finds everything),
//! and sync the active profile's plugins into it at launch.
//!
//! Why game-dir, not per-profile env redirect: BepInEx-IL2CPP derives its
//! `plugins/`, `config/`, `interop/` paths from the GAME executable directory,
//! not from the Doorstop target DLL. So mods only load when they live under
//! `<game>/BepInEx`. Profiles are kept outside the game dir and their plugins
//! are copied in on launch (instant switch, vanilla stays clean when removed).

use regex::Regex;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Game-dir files from the pack root (the rest of the pack is dirs).
pub const BOOTSTRAP_FILES: &[&str] = &["winhttp.dll", "doorstop_config.ini", ".doorstop_version"];

/// The BepInEx IL2CPP preloader (verified against BepInEx 6.0.0-pre.2).
pub const IL2CPP_PRELOADER: &str = "BepInEx.Unity.IL2CPP.dll";

/// Among Us Steam app id, written so the game inits Steamworks on a direct launch.
pub const STEAM_APP_ID: &str = "945360";

const LOADER_MARKER: &str = ".perfectsync_loader";

/// True if a BepInEx loader installed by THIS app is present (winhttp proxy,
/// IL2CPP preloader, and our marker). A foreign/old install lacking the marker
/// reads false, so the app reinstalls the current build (auto-heals stale loaders).
pub fn has_loader(game_dir: &Path) -> bool {
    is_installed(game_dir) && game_dir.join("BepInEx").join(LOADER_MARKER).exists()
}

/// True if the recorded loader build is older than `latest` (so it should be
/// reinstalled). A missing/blank record counts as outdated.
pub fn is_outdated(installed: Option<&str>, latest: &str) -> bool {
    match installed {
        Some(cur) => crate::version::is_newer(latest, cur),
        None => true,
    }
}

/// The loader build id this app recorded (e.g. "be.764"), if any.
pub fn installed_version(game_dir: &Path) -> Option<String> {
    fs::read_to_string(game_dir.join("BepInEx").join(LOADER_MARKER))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Scrape the builds.bepinex.dev listing HTML for the NEWEST IL2CPP win-<arch>
/// build. Returns (build id like "be.764", absolute download url). This keeps the
/// app on the latest BepInEx forever without hardcoding a version.
pub fn parse_latest_build(html: &str, arch: &str) -> Option<(String, String)> {
    let pat = format!(
        r#"projects/bepinex_be/(\d+)/(BepInEx-Unity\.IL2CPP-win-{}[^"'<> ]+\.zip)"#,
        regex::escape(arch)
    );
    let re = Regex::new(&pat).ok()?;
    let mut best: Option<(u64, String)> = None;
    for c in re.captures_iter(html) {
        let n: u64 = c[1].parse().unwrap_or(0);
        let path = format!("projects/bepinex_be/{}/{}", &c[1], &c[2]);
        if best.as_ref().is_none_or(|(bn, _)| n > *bn) {
            best = Some((n, path));
        }
    }
    best.map(|(n, path)| (format!("be.{n}"), format!("https://builds.bepinex.dev/{path}")))
}

pub fn profile_bepinex_dir(profiles_root: &Path, profile_id: &str) -> PathBuf {
    profiles_root.join(profile_id).join("BepInEx")
}

pub fn profile_plugins_dir(profiles_root: &Path, profile_id: &str) -> PathBuf {
    profile_bepinex_dir(profiles_root, profile_id).join("plugins")
}

/// Create the per-profile BepInEx subdirs (profile is where mods are stored).
pub fn ensure_profile_layout(profiles_root: &Path, profile_id: &str) -> io::Result<()> {
    let bep = profile_bepinex_dir(profiles_root, profile_id);
    for sub in ["plugins", "config"] {
        fs::create_dir_all(bep.join(sub))?;
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Install the loader from an extracted `BepInExPack` directory entirely INTO the
/// game dir: Doorstop bootstrap, bundled `dotnet/` runtime, and `BepInEx/{core,config}`.
pub fn install_pack(pack_dir: &Path, game_dir: &Path, version: &str) -> io::Result<()> {
    for file in BOOTSTRAP_FILES {
        let src = pack_dir.join(file);
        if src.exists() {
            fs::copy(&src, game_dir.join(file))?;
        }
    }
    let dotnet = pack_dir.join("dotnet");
    if dotnet.is_dir() {
        copy_dir_recursive(&dotnet, &game_dir.join("dotnet"))?;
    }
    for sub in ["core", "config"] {
        let src = pack_dir.join("BepInEx").join(sub);
        if src.is_dir() {
            copy_dir_recursive(&src, &game_dir.join("BepInEx").join(sub))?;
        }
    }
    fs::create_dir_all(game_dir.join("BepInEx").join("plugins"))?;
    fs::write(game_dir.join("BepInEx").join(LOADER_MARKER), version)?;
    write_console_off(game_dir)?;
    ensure_steam_appid(game_dir)?;
    Ok(())
}

/// Force the BepInEx console window off (keep the on-disk log). BepInEx merges
/// missing keys on launch but preserves our explicit `Enabled = false`.
pub fn write_console_off(game_dir: &Path) -> io::Result<()> {
    let cfg_dir = game_dir.join("BepInEx").join("config");
    fs::create_dir_all(&cfg_dir)?;
    let cfg = "[Logging.Console]\nEnabled = false\n\n[Logging.Disk]\nEnabled = true\nWriteUnityLog = false\n";
    fs::write(cfg_dir.join("BepInEx.cfg"), cfg)
}

/// Write `steam_appid.txt` next to the exe so a direct launch passes Steam auth.
pub fn ensure_steam_appid(game_dir: &Path) -> io::Result<()> {
    fs::write(game_dir.join("steam_appid.txt"), STEAM_APP_ID)
}

/// Extract an entire zip archive into `dest` (used for the BepInEx pack).
pub fn extract_all(bytes: &[u8], dest: &Path) -> io::Result<()> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let name = file.name().replace('\\', "/");
        if name.contains("..") {
            continue;
        }
        let rel = Path::new(&name);
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            continue;
        }
        let out = dest.join(&name);
        if file.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut sink = fs::File::create(&out)?;
            io::copy(&mut file, &mut sink)?;
        }
    }
    Ok(())
}

/// Find the dir holding `winhttp.dll` in an extracted pack (checks dir + children).
pub fn locate_pack_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("winhttp.dll").exists() {
        return Some(dir.to_path_buf());
    }
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir() && p.join("winhttp.dll").exists() {
            return Some(p);
        }
    }
    None
}

/// Extract a downloaded BepInEx pack into `cache_dir` (once) and install it into
/// the game dir. Idempotent: re-extraction is skipped if cached.
pub fn install_pack_from_zip(
    bytes: &[u8],
    game_dir: &Path,
    cache_dir: &Path,
    version: &str,
) -> io::Result<()> {
    if locate_pack_root(cache_dir).is_none() {
        extract_all(bytes, cache_dir)?;
    }
    let root = locate_pack_root(cache_dir)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "winhttp.dll not found in pack"))?;
    install_pack(&root, game_dir, version)
}

/// True if the loader is installed in the game dir (proxy + preloader present).
pub fn is_installed(game_dir: &Path) -> bool {
    game_dir.join("winhttp.dll").exists()
        && game_dir
            .join("BepInEx")
            .join("core")
            .join(IL2CPP_PRELOADER)
            .exists()
}

/// Mirror the active profile's enabled plugins into the game's `BepInEx/plugins`,
/// wiping any plugin DLLs already there first so the game loads exactly the
/// profile, nothing else.
pub fn sync_profile_plugins(
    profiles_root: &Path,
    profile_id: &str,
    game_dir: &Path,
) -> io::Result<()> {
    let dst = game_dir.join("BepInEx").join("plugins");
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(&dst)? {
        let p = entry?.path();
        let lower = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        if lower.ends_with(".dll") || lower.ends_with(".dll.disabled") {
            let _ = fs::remove_file(&p);
        }
    }
    let src = profile_plugins_dir(profiles_root, profile_id);
    if src.is_dir() {
        for entry in fs::read_dir(&src)? {
            let p = entry?.path();
            if p.extension().and_then(|e| e.to_str()) == Some("dll") {
                if let Some(name) = p.file_name() {
                    fs::copy(&p, dst.join(name))?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pack(pack: &Path) {
        fs::create_dir_all(pack.join("dotnet")).unwrap();
        fs::create_dir_all(pack.join("BepInEx").join("core")).unwrap();
        fs::create_dir_all(pack.join("BepInEx").join("config")).unwrap();
        fs::write(pack.join("winhttp.dll"), b"proxy").unwrap();
        fs::write(pack.join("doorstop_config.ini"), b"[General]").unwrap();
        fs::write(pack.join("dotnet").join("coreclr.dll"), b"clr").unwrap();
        fs::write(pack.join("BepInEx").join("core").join(IL2CPP_PRELOADER), b"pre").unwrap();
        fs::write(pack.join("BepInEx").join("config").join("BepInEx.cfg"), b"cfg").unwrap();
    }

    #[test]
    fn installs_pack_into_game_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let pack = tmp.path().join("pack");
        let game = tmp.path().join("game");
        make_pack(&pack);
        fs::create_dir_all(&game).unwrap();

        install_pack(&pack, &game, "be.999").unwrap();

        assert!(game.join("winhttp.dll").exists());
        assert!(game.join("dotnet").join("coreclr.dll").exists());
        assert!(game.join("BepInEx").join("core").join(IL2CPP_PRELOADER).exists());
        assert!(game.join("BepInEx").join("config").join("BepInEx.cfg").exists());
        assert!(game.join("BepInEx").join("plugins").is_dir());
        assert!(game.join("steam_appid.txt").exists());
        assert!(is_installed(&game));
        assert!(has_loader(&game), "marker written so our loader is detected");
        assert_eq!(installed_version(&game).as_deref(), Some("be.999"));
    }

    #[test]
    fn parses_latest_build_from_listing_html() {
        let html = r#"
          <a href="projects/bepinex_be/762/BepInEx-Unity.IL2CPP-win-x86-6.0.0-be.762%2Bbd467c9.zip">x</a>
          <a href="projects/bepinex_be/764/BepInEx-Unity.IL2CPP-win-x86-6.0.0-be.764%2B5f39645.zip">x</a>
          <a href="projects/bepinex_be/763/BepInEx-Unity.IL2CPP-win-x64-6.0.0-be.763%2Bda64b22.zip">x</a>
        "#;
        let (id, url) = parse_latest_build(html, "x86").unwrap();
        assert_eq!(id, "be.764");
        assert_eq!(
            url,
            "https://builds.bepinex.dev/projects/bepinex_be/764/BepInEx-Unity.IL2CPP-win-x86-6.0.0-be.764%2B5f39645.zip"
        );
        // x64 picks the x64 asset
        assert_eq!(parse_latest_build(html, "x64").unwrap().0, "be.763");
    }

    #[test]
    fn console_disabled_in_config() {
        let tmp = tempfile::tempdir().unwrap();
        write_console_off(tmp.path()).unwrap();
        let cfg =
            fs::read_to_string(tmp.path().join("BepInEx").join("config").join("BepInEx.cfg")).unwrap();
        assert!(cfg.contains("[Logging.Console]"));
        assert!(cfg.contains("Enabled = false"));
    }

    #[test]
    fn writes_steam_appid_file() {
        let tmp = tempfile::tempdir().unwrap();
        ensure_steam_appid(tmp.path()).unwrap();
        assert_eq!(fs::read_to_string(tmp.path().join("steam_appid.txt")).unwrap(), "945360");
    }

    #[test]
    fn install_from_zip_then_sync_plugins() {
        let mut buf = Vec::new();
        {
            use std::io::Write;
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            for (path, body) in [
                ("BepInExPack/winhttp.dll", "proxy"),
                ("BepInExPack/doorstop_config.ini", "[General]"),
                ("BepInExPack/dotnet/coreclr.dll", "clr"),
                ("BepInExPack/BepInEx/core/BepInEx.Unity.IL2CPP.dll", "pre"),
            ] {
                zw.start_file(path, opts).unwrap();
                zw.write_all(body.as_bytes()).unwrap();
            }
            zw.finish().unwrap();
        }
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let cache = tmp.path().join("cache");
        let profiles = tmp.path().join("profiles");
        fs::create_dir_all(&game).unwrap();
        install_pack_from_zip(&buf, &game, &cache, "be.test").unwrap();
        assert!(has_loader(&game));

        // profile has a mod + a disabled mod
        let plugins = profile_plugins_dir(&profiles, "p1");
        fs::create_dir_all(&plugins).unwrap();
        fs::write(plugins.join("TheOtherRoles.dll"), b"mod").unwrap();
        fs::write(plugins.join("Off.dll.disabled"), b"off").unwrap();

        sync_profile_plugins(&profiles, "p1", &game).unwrap();
        let game_plugins = game.join("BepInEx").join("plugins");
        assert!(game_plugins.join("TheOtherRoles.dll").exists());
        assert!(!game_plugins.join("Off.dll.disabled").exists()); // disabled not copied

        // switching to an empty profile clears the old plugin
        let empty = profile_plugins_dir(&profiles, "p2");
        fs::create_dir_all(&empty).unwrap();
        sync_profile_plugins(&profiles, "p2", &game).unwrap();
        assert!(!game_plugins.join("TheOtherRoles.dll").exists());
    }

    #[test]
    fn extract_all_rejects_absolute_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&dest).unwrap();
        let escape = tmp.path().join("escaped.txt");
        let abs_name = escape.to_string_lossy().replace('\\', "/");

        let mut buf = Vec::new();
        {
            use std::io::Write;
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            zw.start_file(abs_name.as_str(), opts).unwrap();
            zw.write_all(b"EVIL").unwrap();
            zw.start_file("ok.txt", opts).unwrap();
            zw.write_all(b"good").unwrap();
            zw.finish().unwrap();
        }

        let mut ar = zip::ZipArchive::new(std::io::Cursor::new(&buf)).unwrap();
        let preserved = (0..ar.len())
            .any(|i| Path::new(&ar.by_index(i).unwrap().name().replace('\\', "/")).is_absolute());
        assert!(preserved, "zip writer did not preserve an absolute entry name");

        extract_all(&buf, &dest).unwrap();

        assert_eq!(fs::read(dest.join("ok.txt")).unwrap(), b"good");
        assert!(!escape.exists(), "absolute zip entry escaped dest");
    }

    #[test]
    fn sync_wipes_unmanaged_plugins() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let profiles = tmp.path().join("profiles");
        let game_plugins = game.join("BepInEx").join("plugins");
        fs::create_dir_all(&game_plugins).unwrap();
        fs::write(game_plugins.join("UserMod.dll"), b"user").unwrap();

        let p1 = profile_plugins_dir(&profiles, "p1");
        fs::create_dir_all(&p1).unwrap();
        fs::write(p1.join("AppMod.dll"), b"app").unwrap();

        sync_profile_plugins(&profiles, "p1", &game).unwrap();
        assert!(game_plugins.join("AppMod.dll").exists());
        assert!(!game_plugins.join("UserMod.dll").exists(), "only the profile loads");
    }

    #[test]
    fn is_outdated_compares_build_ids() {
        assert!(is_outdated(Some("be.764"), "be.770"));
        assert!(!is_outdated(Some("be.770"), "be.764"));
        assert!(!is_outdated(Some("be.764"), "be.764"));
        assert!(is_outdated(None, "be.764"));
    }
}
