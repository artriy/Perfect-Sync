use perfect_sync_core::compat;
use perfect_sync_core::loader;
use perfect_sync_core::process::GAME_EXE;
use perfect_sync_core::types::Runtime;
use std::fs;
use std::path::PathBuf;

#[test]
fn current_platform_synchronizes_a_selected_game_folder() {
    let temp = tempfile::tempdir().unwrap();
    let game = platform_game_path(temp.path());
    fs::create_dir_all(game.join("BepInEx/plugins")).unwrap();
    fs::write(game.join(GAME_EXE), b"MZ").unwrap();
    fs::write(game.join("BepInEx/plugins/stale.dll"), b"stale").unwrap();

    let profiles = temp.path().join("profiles");
    let profile_plugins = loader::profile_plugins_dir(&profiles, "crew");
    fs::create_dir_all(&profile_plugins).unwrap();
    fs::write(profile_plugins.join("Current.dll"), b"current").unwrap();

    loader::sync_profile_plugins(&profiles, "crew", &game).unwrap();
    assert_eq!(fs::read(game.join("BepInEx/plugins/Current.dll")).unwrap(), b"current");
    assert!(!game.join("BepInEx/plugins/stale.dll").exists());

    let ctx = compat::resolve_with_hint(&game, platform_runtime_hint());
    assert_eq!(ctx.runtime, expected_runtime());

    if let Some(prefix) = &ctx.prefix {
        fs::create_dir_all(prefix).unwrap();
        fs::write(prefix.join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
        compat::register_winhttp_override(prefix).unwrap();
        assert!(compat::has_winhttp_override(prefix));
    }

    let spec = compat::build_launch_spec(&game, &ctx);
    assert_eq!(spec.cwd, game);
    assert!(!spec.program.as_os_str().is_empty());
}

fn platform_game_path(root: &std::path::Path) -> PathBuf {
    if cfg!(target_os = "windows") {
        root.join("Among Us")
    } else if cfg!(target_os = "linux") {
        root.join("steamapps/common/Among Us")
    } else {
        root.join("Library/Application Support/CrossOver/Bottles/AU/drive_c/Games/Among Us")
    }
}

fn platform_runtime_hint() -> Option<Runtime> {
    if cfg!(target_os = "macos") {
        Some(Runtime::Crossover)
    } else {
        None
    }
}

fn expected_runtime() -> Runtime {
    if cfg!(target_os = "windows") {
        Runtime::Native
    } else if cfg!(target_os = "linux") {
        Runtime::Proton
    } else {
        Runtime::Crossover
    }
}
