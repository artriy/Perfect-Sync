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

    let spec = process::build_launch(Path::new("C:/Games/Among Us"), &profiles_root.join("live"));
    assert!(spec.program.ends_with("Among Us.exe"));
    assert!(spec
        .env
        .iter()
        .any(|(k, v)| k == "DOORSTOP_TARGET_ASSEMBLY" && v.contains("live")));
    println!("launch: {:?} env={:?}", spec.program, spec.env);
}
