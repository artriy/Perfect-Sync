use perfect_sync_core::compat;
use perfect_sync_core::loader;
use perfect_sync_core::process::{self, GAME_EXE};
use perfect_sync_core::types::Runtime;
use std::fs;
use std::path::PathBuf;

#[test]
fn current_platform_synchronizes_a_selected_game_folder() {
    let temp = tempfile::tempdir().unwrap();
    let game = platform_game_path(temp.path());
    fs::create_dir_all(game.join("BepInEx/plugins")).unwrap();
    fs::write(game.join(GAME_EXE), b"MZ").unwrap();
    fs::write(game.join("BepInEx/plugins/unmanaged.dll"), b"unmanaged").unwrap();
    fs::write(game.join("BepInEx/plugins/managed-stale.dll"), b"managed").unwrap();
    fs::write(
        game.join("BepInEx/plugins/.perfectsync-managed.json"),
        br#"{"names":["managed-stale.dll"]}"#,
    )
    .unwrap();

    let profiles = temp.path().join("profiles");
    let profile_plugins = loader::profile_plugins_dir(&profiles, "crew");
    fs::create_dir_all(&profile_plugins).unwrap();
    fs::write(profile_plugins.join("Current.dll"), b"current").unwrap();

    loader::sync_profile_plugins(&profiles, "crew", &game).unwrap();
    assert_eq!(
        fs::read(game.join("BepInEx/plugins/Current.dll")).unwrap(),
        b"current"
    );
    assert_eq!(
        fs::read(game.join("BepInEx/plugins/unmanaged.dll")).unwrap(),
        b"unmanaged"
    );
    assert!(!game.join("BepInEx/plugins/managed-stale.dll").exists());

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

#[test]
fn process_query_failures_are_representable() {
    let query: fn() -> std::io::Result<bool> = process::try_is_running;
    let _ = query;
}

#[test]
fn launch_specs_keep_spaces_and_unicode_structured() {
    let game = PathBuf::from("C:/Games/雪 Space/Among Us");
    let ctx = compat::RuntimeContext {
        host: compat::HostPlatform::Other,
        runtime: Runtime::Wine,
        prefix: None,
        launcher: Some(PathBuf::from("wine")),
        launcher_args: Vec::new(),
    };
    let spec = compat::build_launch_spec(&game, &ctx);
    assert_eq!(spec.args, vec![game.join(GAME_EXE).into_os_string()]);
    assert_eq!(spec.program, PathBuf::from("wine"));
}

#[cfg(unix)]
#[test]
fn launch_specs_preserve_non_utf8_paths_as_os_strings() {
    use perfect_sync_core::compat::{HostPlatform, RuntimeContext};
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let prefix = PathBuf::from(OsString::from_vec(b"/tmp/wine-\xff".to_vec()));
    let game = prefix.join("drive_c/Among Us");
    let ctx = RuntimeContext {
        host: HostPlatform::Linux,
        runtime: Runtime::Wine,
        prefix: Some(prefix.clone()),
        launcher: Some(PathBuf::from("/usr/bin/wine")),
        launcher_args: Vec::new(),
    };
    let spec = compat::build_launch_spec(&game, &ctx);

    assert_eq!(spec.args.last().unwrap(), game.join(GAME_EXE).as_os_str());
    assert!(spec
        .env
        .iter()
        .any(|(key, value)| key == "WINEPREFIX" && value == prefix.as_os_str()));
}
