//! End-to-end LIVE integration test for the full mod pipeline.
//!
//! Hits real release pages and CDNs, so it is `#[ignore]`d by default. Run with:
//!   cargo test -p perfect-sync-core --test e2e_live -- --ignored --nocapture
//!
//! It resolves Reactor's latest release, downloads the actual asset, installs it
//! into a temp profile's BepInEx/plugins, and builds the Doorstop launch spec.

use perfect_sync_core::resolver::Http;
use perfect_sync_core::{catalog, loader, profile, resolver, tou_cosmetics};
use std::path::Path;

const CATALOG: &str = include_str!("../fixtures/catalog.sample.json");
const BUNDLED_CATALOG: &str = include_str!("../../catalog/catalog.json");

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

    assert!(
        game.join("winhttp.dll").exists(),
        "winhttp installed to game dir"
    );
    assert!(
        game.join("dotnet").join("coreclr.dll").exists(),
        "dotnet runtime installed"
    );
    assert!(
        game.join("BepInEx")
            .join("core")
            .join("BepInEx.Unity.IL2CPP.dll")
            .exists(),
        "preloader installed to game BepInEx/core"
    );
    assert!(loader::has_loader(&game));
    assert_eq!(
        loader::installed_version(&game).as_deref(),
        Some(id.as_str())
    );
}

#[test]
#[ignore]
fn live_town_of_us_release_includes_version_matched_cosmetics() {
    let cat = catalog::parse(CATALOG).unwrap();
    let rules = &cat.get("AU-Avengers/TOU-Mira").unwrap().asset_rules;
    let http = resolver::UreqHttp::new(None);

    let selected_version = std::env::var("TOU_VERSION").ok();
    let plugin = match selected_version.as_deref() {
        Some(version) => {
            resolver::resolve_tag(&http, "AU-Avengers/TOU-Mira", version, rules, "x86")
        }
        None => resolver::resolve_latest(&http, "AU-Avengers/TOU-Mira", rules, "x86"),
    }
    .expect("resolve selected Town of Us - Mira version");
    assert_eq!(plugin.asset_name, "TownOfUsMira.dll");

    let release = resolver::fetch_release_by_tag(&http, tou_cosmetics::PACKAGE_ID, &plugin.version)
        .expect("resolve the exact selected Town of Us release");
    let asset = release
        .assets
        .iter()
        .find(|asset| {
            let name = asset.name.to_ascii_lowercase();
            name.ends_with(".zip") && name.contains("x86") && name.contains("steam")
        })
        .expect("select the matching Steam cosmetics pack");
    let pack = resolver::resolved_asset(&http, &release, asset)
        .expect("resolve the selected pack's download metadata");
    assert_eq!(pack.version, plugin.version);
    assert!(pack.asset_name.to_ascii_lowercase().contains("x86"));
    assert!(pack.asset_name.to_ascii_lowercase().contains("steam"));

    let bytes = resolver::download_resolved(&http, &pack).expect("download verified release pack");
    let cosmetics = tou_cosmetics::extract_release_pack(&bytes, &plugin.version, &pack.asset_name)
        .expect("extract Town of Us cosmetics");
    assert!(cosmetics.bundle.starts_with(b"UnityFS\0"));
    assert!(cosmetics.catalog.starts_with(b"{"));
    println!(
        "{} -> {} (bundle {} bytes, catalog {} bytes)",
        plugin.version,
        pack.asset_name,
        cosmetics.bundle.len(),
        cosmetics.catalog.len()
    );
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

    // 3. Install the bare DLL into a temp profile.
    assert!(
        resolved.asset_name.to_ascii_lowercase().ends_with(".dll"),
        "mod resolution must never select an archive"
    );
    let tmp = tempfile::tempdir().unwrap();
    let profiles_root = tmp.path();
    let dest =
        profile::install_plugin_bytes(profiles_root, "live", &resolved.asset_name, &bytes).unwrap();
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
            game_instance_id: None,
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
                asset: Some("Reactor.dll".into()),
            }],
            levelimposter_maps: Vec::new(),
        })
        .unwrap();
    assert!(store.load("live").unwrap().is_some());

    let game = Path::new("C:/Games/Among Us");
    let spec = perfect_sync_core::compat::build_launch_spec(
        game,
        &perfect_sync_core::compat::resolve(game),
    );
    assert!(spec.program.ends_with("Among Us.exe"));
    println!("launch: {:?}", spec.program);
}

#[test]
#[ignore]
fn live_end_to_end_catalog_zip_install() {
    let catalog = catalog::parse(BUNDLED_CATALOG).unwrap();
    let entry = catalog
        .get("TheOtherRolesAU/TheOtherRoles")
        .expect("The Other Roles catalog entry");
    let mut archive_rules = entry.asset_rules.clone();
    archive_rules.dll_name = None;
    let http = resolver::UreqHttp::new(None);
    let resolved = resolver::resolve_latest(
        &http,
        entry.repo.as_deref().unwrap_or(&entry.id),
        &archive_rules,
        "x86",
    )
    .expect("resolve latest catalog ZIP");
    assert!(resolved.asset_name.to_ascii_lowercase().ends_with(".zip"));

    let bytes = resolver::download_resolved(&http, &resolved).expect("download verified ZIP");
    let temporary = tempfile::tempdir().unwrap();
    let dll_name = entry
        .asset_rules
        .dll_name
        .as_deref()
        .expect("declared plugin DLL");
    let installed =
        profile::install_plugin_zip_bytes(temporary.path(), "archive", dll_name, &bytes)
            .expect("extract only the declared plugin DLL");
    assert_eq!(
        installed.file_name().and_then(|name| name.to_str()),
        Some(dll_name)
    );
    assert!(std::fs::metadata(&installed).unwrap().len() > 0);
    println!(
        "resolved {} {} and installed {}",
        resolved.asset_name,
        resolved.version,
        installed.display()
    );
}
