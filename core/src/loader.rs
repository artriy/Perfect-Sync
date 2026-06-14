//! LoaderManager: lay out per-profile BepInEx, install the Doorstop/BepInEx
//! bootstrap into the (otherwise pristine) game directory, build the launch
//! environment that redirects Doorstop at the active profile, and keep the
//! interop cache hygienic.
//!
//! r2modman model: the game dir only ever receives the small Doorstop proxy
//! (`winhttp.dll`); all mods live in a per-profile `BepInEx/` outside the game
//! dir, selected at launch via `DOORSTOP_TARGET_ASSEMBLY`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The doorstop proxy + runtime files that live next to `Among Us.exe`.
/// (`winhttp.dll` is the bootstrap; the rest are the bundled CoreCLR runtime
/// and the doorstop config/version markers from BepInExPack_AmongUs.)
pub const BOOTSTRAP_FILES: &[&str] = &["winhttp.dll", "doorstop_config.ini", ".doorstop_version"];

/// Path to a profile's BepInEx root: `<profiles_root>/<id>/BepInEx`.
pub fn profile_bepinex_dir(profiles_root: &Path, profile_id: &str) -> PathBuf {
    profiles_root.join(profile_id).join("BepInEx")
}

/// Path to a profile's plugins dir (where mod DLLs go).
pub fn profile_plugins_dir(profiles_root: &Path, profile_id: &str) -> PathBuf {
    profile_bepinex_dir(profiles_root, profile_id).join("plugins")
}

/// Create the standard per-profile BepInEx subdirectories.
pub fn ensure_profile_layout(profiles_root: &Path, profile_id: &str) -> io::Result<()> {
    let bep = profile_bepinex_dir(profiles_root, profile_id);
    for sub in ["plugins", "config", "core", "interop", "cache"] {
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

/// Install the loader from a downloaded `BepInExPack_AmongUs` directory:
/// - the Doorstop bootstrap + bundled `dotnet/` runtime go into the game dir,
/// - the framework `BepInEx/core` goes into the profile (so each profile owns
///   its loader and plugins; the game dir stays free of mods).
pub fn install_pack(pack_dir: &Path, game_dir: &Path, profile_dir: &Path) -> io::Result<()> {
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
            copy_dir_recursive(&src, &profile_dir.join("BepInEx").join(sub))?;
        }
    }
    ensure_steam_appid(game_dir)?;
    Ok(())
}

/// Among Us Steam app id, written so the game can init Steamworks when launched
/// directly (with Doorstop env vars) instead of through the Steam client.
pub const STEAM_APP_ID: &str = "945360";

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
            continue; // guard against path traversal
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

/// Find the directory that holds `winhttp.dll` within an extracted pack (the zip
/// nests everything under `BepInExPack_AmongUs/`). Checks `dir` then its children.
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
/// the game dir + profile. Idempotent: re-extraction is skipped if cached.
pub fn install_pack_from_zip(
    bytes: &[u8],
    game_dir: &Path,
    profile_dir: &Path,
    cache_dir: &Path,
) -> io::Result<()> {
    if locate_pack_root(cache_dir).is_none() {
        extract_all(bytes, cache_dir)?;
    }
    let root = locate_pack_root(cache_dir)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "winhttp.dll not found in pack"))?;
    install_pack(&root, game_dir, profile_dir)
}

/// True if the game dir already has the Doorstop bootstrap installed.
pub fn is_bootstrapped(game_dir: &Path) -> bool {
    game_dir.join("winhttp.dll").exists()
}

/// The BepInEx IL2CPP preloader Doorstop 4.x targets (verified against
/// BepInExPack_AmongUs 6.0.700 / Doorstop 4.3.0).
pub const IL2CPP_PRELOADER: &str = "BepInEx.Unity.IL2CPP.dll";

/// The Doorstop environment that launches the game against a specific profile's
/// BepInEx. `DOORSTOP_TARGET_ASSEMBLY` points at the profile's preloader (so
/// BepInEx resolves `plugins/`, `config/`, etc. from that profile), while the
/// CoreCLR runtime lives in the game dir's bundled `dotnet/`. Env vars override
/// the on-disk `doorstop_config.ini`, which is how profiles stay isolated.
pub fn launch_env(game_dir: &Path, profile_dir: &Path) -> Vec<(String, String)> {
    let preloader = profile_dir.join("BepInEx").join("core").join(IL2CPP_PRELOADER);
    let coreclr = game_dir.join("dotnet").join("coreclr.dll");
    let corlib = game_dir.join("dotnet");
    vec![
        ("DOORSTOP_ENABLED".to_string(), "1".to_string()),
        (
            "DOORSTOP_TARGET_ASSEMBLY".to_string(),
            preloader.to_string_lossy().into_owned(),
        ),
        (
            "DOORSTOP_CLR_RUNTIME_CORECLR_PATH".to_string(),
            coreclr.to_string_lossy().into_owned(),
        ),
        (
            "DOORSTOP_CLR_CORLIB_DIR".to_string(),
            corlib.to_string_lossy().into_owned(),
        ),
    ]
}

/// Clean a profile's generated interop assemblies (delete `*.dll`) while
/// preserving `assembly-hash.txt` so BepInEx does not force a full regenerate.
pub fn clean_interop(profile_dir: &Path) -> io::Result<()> {
    let interop = profile_dir.join("BepInEx").join("interop");
    if !interop.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&interop)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("dll") {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_profile_layout() {
        let tmp = tempfile::tempdir().unwrap();
        ensure_profile_layout(tmp.path(), "p1").unwrap();
        let bep = profile_bepinex_dir(tmp.path(), "p1");
        for sub in ["plugins", "config", "core", "interop", "cache"] {
            assert!(bep.join(sub).is_dir(), "missing {sub}");
        }
    }

    #[test]
    fn installs_pack_into_game_and_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let pack = tmp.path().join("pack");
        let game = tmp.path().join("game");
        let profile = tmp.path().join("profiles").join("p1");
        fs::create_dir_all(pack.join("dotnet")).unwrap();
        fs::create_dir_all(pack.join("BepInEx").join("core")).unwrap();
        fs::create_dir_all(&game).unwrap();
        fs::create_dir_all(&profile).unwrap();
        fs::write(pack.join("winhttp.dll"), b"proxy").unwrap();
        fs::write(pack.join("doorstop_config.ini"), b"[General]").unwrap();
        fs::write(pack.join("dotnet").join("coreclr.dll"), b"clr").unwrap();
        fs::write(
            pack.join("BepInEx").join("core").join("BepInEx.Preloader.IL2CPP.dll"),
            b"preloader",
        )
        .unwrap();

        install_pack(&pack, &game, &profile).unwrap();

        assert!(game.join("winhttp.dll").exists());
        assert!(game.join("doorstop_config.ini").exists());
        assert!(game.join("dotnet").join("coreclr.dll").exists());
        assert!(profile
            .join("BepInEx")
            .join("core")
            .join("BepInEx.Preloader.IL2CPP.dll")
            .exists());
        assert!(is_bootstrapped(&game));
    }

    #[test]
    fn writes_steam_appid_file() {
        let tmp = tempfile::tempdir().unwrap();
        ensure_steam_appid(tmp.path()).unwrap();
        assert_eq!(
            fs::read_to_string(tmp.path().join("steam_appid.txt")).unwrap(),
            "945360"
        );
    }

    #[test]
    fn launch_env_points_at_profile_preloader() {
        let env = launch_env(Path::new("/games/au"), Path::new("/profiles/p1"));
        assert!(env.iter().any(|(k, v)| k == "DOORSTOP_ENABLED" && v == "1"));
        let get = |key: &str| {
            env.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        let target = get("DOORSTOP_TARGET_ASSEMBLY");
        assert!(target.contains("p1"));
        assert!(target.ends_with("BepInEx.Unity.IL2CPP.dll"));
        assert!(get("DOORSTOP_CLR_RUNTIME_CORECLR_PATH").ends_with("coreclr.dll"));
        assert!(get("DOORSTOP_CLR_CORLIB_DIR").ends_with("dotnet"));
    }

    #[test]
    fn installs_pack_from_zip_mimicking_thunderstore_layout() {
        // craft a zip nesting everything under BepInExPack_AmongUs/ like the real pack
        let mut buf = Vec::new();
        {
            use std::io::Write;
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            for (path, body) in [
                ("BepInExPack_AmongUs/winhttp.dll", "proxy"),
                ("BepInExPack_AmongUs/doorstop_config.ini", "[General]"),
                ("BepInExPack_AmongUs/dotnet/coreclr.dll", "clr"),
                ("BepInExPack_AmongUs/BepInEx/core/BepInEx.Unity.IL2CPP.dll", "preloader"),
                ("BepInExPack_AmongUs/BepInEx/config/BepInEx.cfg", "cfg"),
            ] {
                zw.start_file(path, opts).unwrap();
                zw.write_all(body.as_bytes()).unwrap();
            }
            zw.finish().unwrap();
        }
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let profile = tmp.path().join("profiles").join("p1");
        let cache = tmp.path().join("cache");
        fs::create_dir_all(&game).unwrap();
        fs::create_dir_all(&profile).unwrap();

        install_pack_from_zip(&buf, &game, &profile, &cache).unwrap();

        assert!(game.join("winhttp.dll").exists());
        assert!(game.join("dotnet").join("coreclr.dll").exists());
        assert!(profile
            .join("BepInEx")
            .join("core")
            .join("BepInEx.Unity.IL2CPP.dll")
            .exists());
        assert!(profile.join("BepInEx").join("config").join("BepInEx.cfg").exists());
        assert!(is_bootstrapped(&game));
    }

    #[test]
    fn clean_interop_preserves_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let interop = tmp.path().join("BepInEx").join("interop");
        fs::create_dir_all(&interop).unwrap();
        fs::write(interop.join("Assembly-CSharp.dll"), b"x").unwrap();
        fs::write(interop.join("assembly-hash.txt"), b"hash").unwrap();

        clean_interop(tmp.path()).unwrap();

        assert!(!interop.join("Assembly-CSharp.dll").exists());
        assert!(interop.join("assembly-hash.txt").exists());
    }
}
