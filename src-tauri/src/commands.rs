//! Tauri commands: thin adapters over `perfect-sync-core`. Heavy logic lives in
//! the (tested) core crate; these wrap it for the frontend and map errors to
//! strings. The backend is authoritative for profile persistence on disk.

use crate::settings::{self, Settings};
use perfect_sync_core::catalog::{parse, AssetArchRule, AssetRules, Catalog};
use perfect_sync_core::preview::{preview, Preview};
use perfect_sync_core::profile::{InstalledMod, ProfileRecord, ProfileStore};
use perfect_sync_core::resolver::{Http, ResolvedDownload, UreqHttp};
use perfect_sync_core::types::{ModSource, ModTag};
use perfect_sync_core::{codec, game, process, profile, resolver};
use std::collections::HashMap;
use std::path::Path;

const CATALOG_JSON: &str = include_str!("../../core/fixtures/catalog.sample.json");

fn catalog() -> Catalog {
    parse(CATALOG_JSON).expect("bundled catalog parses")
}

fn store() -> ProfileStore {
    ProfileStore::new(settings::profiles_root())
}

fn http() -> UreqHttp {
    UreqHttp::new(settings::load().github_token)
}

/// Asset rules for an unknown repo: no per-arch rules, so the resolver falls
/// back to the single `.dll` asset.
fn default_rules() -> AssetRules {
    AssetRules {
        per_arch: HashMap::<String, AssetArchRule>::new(),
        dll_name: None,
        bundles_loader: false,
    }
}

/// Download a resolved asset and install it into the profile's plugins;
/// return the installed plugin file name (if any).
fn install_resolved(
    profiles_root: &Path,
    profile_id: &str,
    http: &dyn Http,
    resolved: &ResolvedDownload,
) -> Result<Option<String>, String> {
    let bytes = http.get_bytes(&resolved.url).map_err(|e| e.to_string())?;
    if resolved.asset_name.to_lowercase().ends_with(".dll") {
        profile::install_plugin_bytes(profiles_root, profile_id, &resolved.asset_name, &bytes)
            .map_err(|e| e.to_string())?;
        Ok(Some(resolved.asset_name.clone()))
    } else {
        let installed = profile::install_from_zip(profiles_root, profile_id, &bytes)
            .map_err(|e| e.to_string())?;
        Ok(installed.into_iter().next())
    }
}

// ---------- settings + detection ----------

#[tauri::command]
pub fn detect_games() -> Vec<game::GameInstall> {
    game::locate_all()
}

#[tauri::command]
pub fn get_settings() -> Settings {
    settings::load()
}

#[tauri::command]
pub fn save_settings(settings: Settings) -> Result<(), String> {
    settings::save(&settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn game_running() -> bool {
    process::is_running()
}

// ---------- profiles ----------

#[tauri::command]
pub fn list_profiles() -> Vec<ProfileRecord> {
    store().list()
}

#[tauri::command]
pub fn save_profile(profile: ProfileRecord) -> Result<(), String> {
    store().save(&profile).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_profile(id: String) -> Result<(), String> {
    store().delete(&id).map_err(|e| e.to_string())
}

// ---------- lobby codes ----------

#[tauri::command]
pub fn encode_lobby_code(profile: ProfileRecord) -> String {
    codec::encode(&profile::to_manifest(&profile))
}

/// Decode a PERFECT- code into a UI preview (diff vs the installed set).
#[tauri::command]
pub fn preview_code(code: String, installed: Vec<(String, String)>) -> Result<Preview, String> {
    preview(&code, &catalog(), &installed).map_err(|e| e.to_string())
}

// ---------- mod mutations (backend-authoritative) ----------

#[tauri::command]
pub fn add_mod(profile_id: String, repo: String, arch: String) -> Result<ProfileRecord, String> {
    let root = settings::profiles_root();
    let store = ProfileStore::new(&root);
    let mut rec = store.load(&profile_id).ok_or("profile not found")?;
    let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;

    let cat = catalog();
    let entry = cat.get(&repo);
    let rules = entry.map(|e| e.asset_rules.clone()).unwrap_or_else(default_rules);
    let tags = entry.map(|e| e.tags.clone()).unwrap_or_default();
    let name = entry.map(|e| e.name.clone()).unwrap_or_else(|| repo.clone());

    let http = http();
    let resolved = resolver::resolve_latest(&http, &repo, &rules, &arch).map_err(|e| e.to_string())?;
    let file = install_resolved(&root, &profile_id, &http, &resolved)?;

    rec.mods.retain(|m| m.package_id != repo);
    rec.mods.push(InstalledMod {
        package_id: repo.clone(),
        name,
        repo: Some(repo),
        version: resolved.version.clone(),
        versions: vec![resolved.version],
        enabled: true,
        source: ModSource::Github,
        tags,
        managed: false,
        update: None,
        file,
    });
    store.save(&rec).map_err(|e| e.to_string())?;
    Ok(rec)
}

#[tauri::command]
pub fn set_mod_enabled(
    profile_id: String,
    package_id: String,
    enabled: bool,
) -> Result<ProfileRecord, String> {
    let root = settings::profiles_root();
    let store = ProfileStore::new(&root);
    let mut rec = store.load(&profile_id).ok_or("profile not found")?;

    let pos = rec
        .mods
        .iter()
        .position(|m| m.package_id == package_id)
        .ok_or("mod not found")?;
    rec.mods[pos].enabled = enabled;
    if let Some(file) = rec.mods[pos].file.clone() {
        profile::set_plugin_enabled(&root, &profile_id, &file, enabled).map_err(|e| e.to_string())?;
    }
    store.save(&rec).map_err(|e| e.to_string())?;
    Ok(rec)
}

#[tauri::command]
pub fn set_mod_version(
    profile_id: String,
    package_id: String,
    version: String,
    arch: String,
) -> Result<ProfileRecord, String> {
    let root = settings::profiles_root();
    let store = ProfileStore::new(&root);
    let mut rec = store.load(&profile_id).ok_or("profile not found")?;
    let pos = rec
        .mods
        .iter()
        .position(|m| m.package_id == package_id)
        .ok_or("mod not found")?;
    let repo = rec.mods[pos]
        .repo
        .clone()
        .and_then(|r| resolver::parse_repo(&r))
        .or_else(|| resolver::parse_repo(&package_id))
        .ok_or("cannot resolve source")?;
    let cat = catalog();
    let rules = cat
        .get(&package_id)
        .or_else(|| cat.get(&repo))
        .map(|e| e.asset_rules.clone())
        .unwrap_or_else(default_rules);
    let http = http();
    let resolved =
        resolver::resolve_tag(&http, &repo, &version, &rules, &arch).map_err(|e| e.to_string())?;
    if let Some(old) = rec.mods[pos].file.clone() {
        let _ = profile::remove_plugin(&root, &profile_id, &old);
    }
    let file = install_resolved(&root, &profile_id, &http, &resolved)?;
    rec.mods[pos].version = resolved.version.clone();
    rec.mods[pos].file = file;
    if !rec.mods[pos].versions.contains(&resolved.version) {
        rec.mods[pos].versions.push(resolved.version.clone());
    }
    rec.mods[pos].update = None;
    store.save(&rec).map_err(|e| e.to_string())?;
    Ok(rec)
}

#[tauri::command]
pub fn remove_mod(profile_id: String, package_id: String) -> Result<ProfileRecord, String> {
    let root = settings::profiles_root();
    let store = ProfileStore::new(&root);
    let mut rec = store.load(&profile_id).ok_or("profile not found")?;

    let pos = rec
        .mods
        .iter()
        .position(|m| m.package_id == package_id)
        .ok_or("mod not found")?;
    let removed = rec.mods.remove(pos);
    if let Some(file) = removed.file {
        let _ = profile::remove_plugin(&root, &profile_id, &file);
    }
    store.save(&rec).map_err(|e| e.to_string())?;
    Ok(rec)
}

/// Build (or refresh) a profile from a PERFECT- code, downloading each mod at
/// its exact pinned version so the recipient is handshake-compatible.
#[tauri::command]
pub fn apply_lobby_code(code: String, arch: String) -> Result<ProfileRecord, String> {
    let manifest = codec::decode(&code).map_err(|e| e.to_string())?;
    let root = settings::profiles_root();
    let store = ProfileStore::new(&root);
    let cat = catalog();
    let http = http();

    let display = manifest.name.clone().unwrap_or_else(|| "Imported lobby".to_string());
    let slug = display.to_lowercase().replace(|c: char| !c.is_alphanumeric(), "-");
    let id = format!("lobby-{slug}");

    let mut mods = Vec::new();
    for mm in &manifest.mods {
        let repo = mm
            .r#ref
            .as_deref()
            .and_then(resolver::parse_repo)
            .or_else(|| resolver::parse_repo(&mm.id))
            .ok_or_else(|| format!("cannot resolve source for {}", mm.id))?;
        let entry = cat.get(&mm.id).or_else(|| cat.get(&repo));
        let rules = entry.map(|e| e.asset_rules.clone()).unwrap_or_else(default_rules);
        let tags = entry.map(|e| e.tags.clone()).unwrap_or_default();
        let name = entry.map(|e| e.name.clone()).unwrap_or_else(|| repo.clone());

        let resolved =
            resolver::resolve_tag(&http, &repo, &mm.v, &rules, &arch).map_err(|e| e.to_string())?;
        let file = install_resolved(&root, &id, &http, &resolved)?;
        let managed = tags
            .iter()
            .any(|t| matches!(t, ModTag::Library | ModTag::Loader));
        mods.push(InstalledMod {
            package_id: mm.id.clone(),
            name,
            repo: Some(repo),
            version: mm.v.clone(),
            versions: vec![mm.v.clone()],
            enabled: true,
            source: ModSource::Github,
            tags,
            managed,
            update: None,
            file,
        });
    }

    let record = ProfileRecord {
        id,
        name: format!("Lobby - {display}"),
        crew_color: "#ffd23f".to_string(),
        game_build: manifest.game_build.clone(),
        mods,
    };
    store.save(&record).map_err(|e| e.to_string())?;
    Ok(record)
}

// ---------- launch ----------

#[tauri::command]
pub fn launch_profile(game_path: String, profile_id: String) -> Result<(), String> {
    if process::is_running() {
        return Err("Among Us is running. Close it before launching.".into());
    }
    let root = settings::profiles_root();
    let spec = process::build_launch(Path::new(&game_path), &root.join(&profile_id));
    process::launch(&spec).map_err(|e| e.to_string())?;
    Ok(())
}
