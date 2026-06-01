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
    let core = pack_dir.join("BepInEx").join("core");
    if core.is_dir() {
        copy_dir_recursive(&core, &profile_dir.join("BepInEx").join("core"))?;
    }
    Ok(())
}

/// True if the game dir already has the Doorstop bootstrap installed.
pub fn is_bootstrapped(game_dir: &Path) -> bool {
    game_dir.join("winhttp.dll").exists()
}

/// The Doorstop environment that launches the game against a specific profile's
/// BepInEx. Setting `DOORSTOP_TARGET_ASSEMBLY` to the profile's preloader makes
/// BepInEx resolve `plugins/`, `config/`, etc. relative to that profile.
pub fn launch_env(profile_dir: &Path) -> Vec<(String, String)> {
    let preloader = profile_dir
        .join("BepInEx")
        .join("core")
        .join("BepInEx.Preloader.IL2CPP.dll");
    vec![
        ("DOORSTOP_ENABLED".to_string(), "1".to_string()),
        (
            "DOORSTOP_TARGET_ASSEMBLY".to_string(),
            preloader.to_string_lossy().into_owned(),
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
    fn launch_env_points_at_profile_preloader() {
        let env = launch_env(Path::new("/profiles/p1"));
        assert!(env.iter().any(|(k, v)| k == "DOORSTOP_ENABLED" && v == "1"));
        let target = env
            .iter()
            .find(|(k, _)| k == "DOORSTOP_TARGET_ASSEMBLY")
            .map(|(_, v)| v.clone())
            .unwrap();
        assert!(target.contains("p1"));
        assert!(target.ends_with("BepInEx.Preloader.IL2CPP.dll"));
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
