//! End-to-end LIVE integration test for the full mod pipeline.
//!
//! Hits the real GitHub API + CDN, so it is `#[ignore]`d by default. Run with:
//!   cargo test -p perfect-sync-core --test e2e_live -- --ignored --nocapture
//!
//! It resolves Reactor's latest release, downloads the actual asset, installs it
//! into a temp profile's BepInEx/plugins, and builds the Doorstop launch spec.

use perfect_sync_core::resolver::Http;
use perfect_sync_core::{catalog, loader, process, profile, resolver};
use std::path::Path;

const CATALOG: &str = include_str!("../fixtures/catalog.sample.json");

/// LIVE: resolve + download the BepInEx IL2CPP pack from GitHub (no Thunderstore)
/// and install the Doorstop bootstrap + framework into a temp game + profile.
#[test]
#[ignore]
fn live_install_latest_bepinex_from_build_server() {
    // scrape the newest build from builds.bepinex.dev (the "always latest" path)
    let http = resolver::UreqHttp::new(None);
    let html = http
        .get_text("https://builds.bepinex.dev/projects/bepinex_be")
        .expect("fetch build listing");
    let (id, url) = loader::parse_latest_build(&html, "x86").expect("parse latest build");
    println!("latest loader: {id} -> {url}");
    assert!(id.starts_with("be."));

    let bytes = http.get_bytes(&url).expect("download loader pack");
    let tmp = tempfile::tempdir().unwrap();
    let game = tmp.path().join("game");
    let cache = tmp.path().join("cache");
    std::fs::create_dir_all(&game).unwrap();

    loader::install_pack_from_zip(&bytes, &game, &cache, &id).unwrap();

    assert!(game.join("winhttp.dll").exists(), "winhttp installed to game dir");
    assert!(game.join("dotnet").join("coreclr.dll").exists(), "dotnet runtime installed");
    assert!(
        game.join("BepInEx").join("core").join("BepInEx.Unity.IL2CPP.dll").exists(),
        "preloader installed to game BepInEx/core"
    );
    assert!(loader::has_loader(&game));
    assert_eq!(loader::installed_version(&game).as_deref(), Some(id.as_str()));
}

#[test]
#[ignore]
fn live_end_to_end_reactor_install() {
    // 1. Resolve the latest Reactor release for x86 (Steam/Epic/itch).
    let cat = catalog::parse(CATALOG).unwrap();
    let rules = &cat.get("NuclearPowered/Reactor").unwrap().asset_rules;
    let http = resolver::UreqHttp::new(None);
    let resolved = resolver::resolve_latest(&http, "NuclearPowered/Reactor", rules, "x86")
        .expect("resolve Reactor latest");
    println!(
        "resolved: {} {} ({} bytes) -> {}",
        resolved.asset_name, resolved.version, resolved.size, resolved.url
    );
    assert!(resolved.asset_name.to_lowercase().contains("reactor"));

    // 2. Download the real asset bytes.
    let bytes = http.get_bytes(&resolved.url).expect("download asset");
    assert!(!bytes.is_empty(), "downloaded asset should not be empty");
    println!("downloaded {} bytes", bytes.len());

    // 3. Install into a temp profile (bare-dll or zip path).
    let tmp = tempfile::tempdir().unwrap();
    let profiles_root = tmp.path();
    let dest = if resolved.asset_name.to_lowercase().ends_with(".dll") {
        let dl = tmp.path().join(&resolved.asset_name);
        std::fs::write(&dl, &bytes).unwrap();
        profile::install_plugin_dll(profiles_root, "live", &dl).unwrap()
    } else {
        let installed = profile::install_from_zip(profiles_root, "live", &bytes).unwrap();
        assert!(!installed.is_empty(), "zip should contain a plugin dll");
        loader::profile_plugins_dir(profiles_root, "live").join(&installed[0])
    };
    assert!(dest.exists(), "installed plugin should exist");
    assert!(std::fs::metadata(&dest).unwrap().len() > 0);
    println!("installed plugin at {}", dest.display());

    // 4. Persist a profile record and build the launch spec.
    let store = profile::ProfileStore::new(profiles_root);
    store
        .save(&profile::ProfileRecord {
            id: "live".into(),
            name: "Live test".into(),
            crew_color: "#5be3b0".into(),
            game_build: None,
            mods: vec![profile::InstalledMod {
                package_id: "NuclearPowered/Reactor".into(),
                name: "Reactor".into(),
                repo: Some("NuclearPowered/Reactor".into()),
                version: resolved.version.clone(),
                versions: vec![resolved.version.clone()],
                enabled: true,
                source: perfect_sync_core::types::ModSource::Github,
                tags: vec![perfect_sync_core::types::ModTag::Library],
                managed: true,
                update: None,
                file: Some("Reactor.dll".into()),
            }],
        })
        .unwrap();
    assert!(store.load("live").is_some());

    let spec = process::build_launch(Path::new("C:/Games/Among Us"));
    assert!(spec.program.ends_with("Among Us.exe"));
    println!("launch: {:?}", spec.program);
}
