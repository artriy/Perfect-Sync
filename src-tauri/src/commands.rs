//! Tauri commands: thin adapters over `perfect-sync-core`. Heavy logic lives in
//! the (tested) core crate; these wrap it for the frontend and map errors to
//! strings. The backend is authoritative for profile persistence on disk.
//!
//! Network/disk-heavy commands are `async` and run their blocking body on a
//! worker thread via `spawn_blocking`, so the UI thread never freezes.

use crate::settings::{self, Settings};
use perfect_sync_core::catalog::{parse, AssetArchRule, AssetRules, Catalog};
use perfect_sync_core::preview::{preview, Preview};
use perfect_sync_core::profile::{InstalledMod, ProfileRecord, ProfileStore};
use perfect_sync_core::resolver::{Http, Release, ResolvedDownload, UreqHttp};
use perfect_sync_core::types::{Arch, ModSource, ModTag};
use perfect_sync_core::{codec, deps, game, loader, process, profile, resolver};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

const BUNDLED_CATALOG: &str = include_str!("../../catalog/catalog.json");

const DEFAULT_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/artriy/Perfect-Sync/main/catalog/catalog.json";

/// Run a blocking closure off the UI thread and flatten the result.
async fn blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

fn catalog() -> Catalog {
    if let Ok(text) = fs::read_to_string(settings::catalog_cache_path()) {
        if let Ok(cat) = parse(&text) {
            return cat;
        }
    }
    parse(BUNDLED_CATALOG).expect("bundled catalog parses")
}

fn store() -> ProfileStore {
    ProfileStore::new(settings::profiles_root())
}

fn http() -> UreqHttp {
    UreqHttp::new(settings::load().github_token)
}

fn default_rules() -> AssetRules {
    AssetRules {
        per_arch: HashMap::<String, AssetArchRule>::new(),
        dll_name: None,
        bundles_loader: false,
    }
}

/// The game folder + arch to use: saved settings first, else autodetect.
fn current_game() -> Option<(String, String)> {
    let s = settings::load();
    if let Some(path) = s.game_path {
        return Some((path, s.arch.unwrap_or_else(|| "x86".to_string())));
    }
    let g = game::locate_all().into_iter().next()?;
    let arch = match g.arch {
        Arch::X86 => "x86",
        Arch::X64 => "x64",
    };
    Some((g.path.to_string_lossy().into_owned(), arch.to_string()))
}

/// Download a resolved asset and install it into the profile's plugins.
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

fn resolve_pack(http: &dyn Http, arch: &str) -> Result<ResolvedDownload, String> {
    let loader = catalog().loader.ok_or("catalog has no loader entry")?;
    match &loader.tag {
        Some(tag) => resolver::resolve_tag(http, &loader.repo, tag, &loader.asset_rules, arch),
        None => resolver::resolve_latest(http, &loader.repo, &loader.asset_rules, arch),
    }
    .map_err(|e| e.to_string())
}

/// Install the Doorstop/BepInEx loader for a profile (idempotent). Downloads +
/// caches the GitHub pack once per arch.
fn ensure_loader_impl(game_path: &str, profile_id: &str, arch: &str) -> Result<(), String> {
    let game_dir = Path::new(game_path);
    let root = settings::profiles_root();
    let profile_dir = root.join(profile_id);
    let preloader = profile_dir
        .join("BepInEx")
        .join("core")
        .join(loader::IL2CPP_PRELOADER);
    if loader::is_bootstrapped(game_dir) && preloader.exists() {
        return Ok(());
    }
    loader::ensure_profile_layout(&root, profile_id).map_err(|e| e.to_string())?;
    let cache = settings::cache_dir().join("bepinex").join(arch);
    if let Some(pack_root) = loader::locate_pack_root(&cache) {
        loader::install_pack(&pack_root, game_dir, &profile_dir).map_err(|e| e.to_string())?;
    } else {
        let h = http();
        let resolved = resolve_pack(&h, arch)?;
        let bytes = h.get_bytes(&resolved.url).map_err(|e| e.to_string())?;
        loader::install_pack_from_zip(&bytes, game_dir, &profile_dir, &cache)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------- settings + detection (fast, stay sync) ----------

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

// ---------- catalog ----------

#[derive(Serialize)]
pub struct CatalogListItem {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub summary: String,
    pub tags: Vec<ModTag>,
    pub latest: String,
}

#[tauri::command]
pub fn get_catalog() -> Vec<CatalogListItem> {
    catalog()
        .mods
        .into_iter()
        .map(|m| CatalogListItem {
            id: m.id.clone(),
            name: m.name,
            repo: m.repo.unwrap_or(m.id),
            summary: m.summary,
            tags: m.tags,
            latest: String::new(),
        })
        .collect()
}

#[tauri::command]
pub async fn refresh_catalog() -> Result<usize, String> {
    blocking(|| {
        let url = settings::load()
            .catalog_url
            .unwrap_or_else(|| DEFAULT_CATALOG_URL.to_string());
        let text = http().get_text(&url).map_err(|e| e.to_string())?;
        let cat = parse(&text).map_err(|e| format!("invalid catalog: {e}"))?;
        fs::create_dir_all(settings::app_data_dir()).map_err(|e| e.to_string())?;
        fs::write(settings::catalog_cache_path(), &text).map_err(|e| e.to_string())?;
        Ok(cat.mods.len())
    })
    .await
}

// ---------- profiles (fast) ----------

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

#[tauri::command]
pub fn preview_code(code: String, installed: Vec<(String, String)>) -> Result<Preview, String> {
    preview(&code, &catalog(), &installed).map_err(|e| e.to_string())
}

// ---------- release/file picker ----------

/// List a repo's recent releases (tags + asset files) for manual selection.
#[tauri::command]
pub async fn list_releases(repo: String) -> Result<Vec<Release>, String> {
    blocking(move || {
        let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
        resolver::fetch_releases(&http(), &repo, 20).map_err(|e| e.to_string())
    })
    .await
}

/// Install a specific release asset (chosen by the user) into a profile.
#[tauri::command]
pub async fn install_asset(
    profile_id: String,
    repo: String,
    tag: String,
    asset_name: String,
    arch: String,
) -> Result<ProfileRecord, String> {
    blocking(move || install_asset_impl(profile_id, repo, tag, asset_name, arch)).await
}

fn install_asset_impl(
    profile_id: String,
    repo: String,
    tag: String,
    asset_name: String,
    arch: String,
) -> Result<ProfileRecord, String> {
    let root = settings::profiles_root();
    let store = ProfileStore::new(&root);
    let mut rec = store.load(&profile_id).ok_or("profile not found")?;
    let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
    let h = http();
    let rel = resolver::fetch_release_by_tag(&h, &repo, &tag).map_err(|e| e.to_string())?;
    let asset = rel
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or("selected file not found in that release")?;
    let resolved = ResolvedDownload {
        url: asset.url.clone(),
        asset_name: asset.name.clone(),
        version: rel.tag.clone(),
        size: asset.size,
    };
    let cat = catalog();
    let entry = cat.get(&repo);
    let file = install_resolved(&root, &profile_id, &h, &resolved)?;

    if let Some(pos) = rec.mods.iter().position(|m| m.package_id == repo) {
        if let Some(old) = rec.mods[pos].file.clone() {
            if Some(&old) != file.as_ref() {
                let _ = profile::remove_plugin(&root, &profile_id, &old);
            }
        }
        rec.mods[pos].version = rel.tag.clone();
        rec.mods[pos].file = file;
        if !rec.mods[pos].versions.contains(&rel.tag) {
            rec.mods[pos].versions.insert(0, rel.tag.clone());
        }
        rec.mods[pos].update = None;
    } else {
        rec.mods.push(InstalledMod {
            package_id: repo.clone(),
            name: entry.map(|e| e.name.clone()).unwrap_or_else(|| repo.clone()),
            repo: Some(repo.clone()),
            version: rel.tag.clone(),
            versions: vec![rel.tag.clone()],
            enabled: true,
            source: ModSource::Github,
            tags: entry.map(|e| e.tags.clone()).unwrap_or_default(),
            managed: false,
            update: None,
            file,
        });
    }
    if let Some((gp, _)) = current_game() {
        let _ = ensure_loader_impl(&gp, &profile_id, &arch);
    }
    store.save(&rec).map_err(|e| e.to_string())?;
    Ok(rec)
}

// ---------- mod mutations ----------

#[tauri::command]
pub async fn add_mod(profile_id: String, repo: String, arch: String) -> Result<ProfileRecord, String> {
    blocking(move || add_mod_impl(profile_id, repo, arch)).await
}

fn add_mod_impl(profile_id: String, repo: String, arch: String) -> Result<ProfileRecord, String> {
    let root = settings::profiles_root();
    let store = ProfileStore::new(&root);
    let mut rec = store.load(&profile_id).ok_or("profile not found")?;
    let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
    let cat = catalog();
    let http = http();

    // enforce one role mod per profile (two role mods cannot coexist)
    let main_tags = cat.get(&repo).map(|e| e.tags.clone()).unwrap_or_default();
    if main_tags.contains(&ModTag::Role) {
        if let Some(existing) = rec
            .mods
            .iter()
            .find(|m| !m.managed && m.package_id != repo && m.tags.contains(&ModTag::Role))
        {
            return Err(format!(
                "Only one role mod per profile. Remove {} first.",
                existing.name
            ));
        }
    }

    rec.mods.retain(|m| m.package_id != repo);
    let ordered = deps::resolve(&cat, &[repo.clone()]).ordered;
    for id in ordered {
        if rec.mods.iter().any(|m| m.package_id == id) {
            continue;
        }
        let entry = cat.get(&id);
        let id_repo = entry
            .and_then(|e| e.repo.clone())
            .or_else(|| resolver::parse_repo(&id))
            .unwrap_or_else(|| id.clone());
        let rules = entry.map(|e| e.asset_rules.clone()).unwrap_or_else(default_rules);
        let tags = entry.map(|e| e.tags.clone()).unwrap_or_default();
        let name = entry.map(|e| e.name.clone()).unwrap_or_else(|| id.clone());
        let resolved =
            resolver::resolve_latest(&http, &id_repo, &rules, &arch).map_err(|e| e.to_string())?;
        let file = install_resolved(&root, &profile_id, &http, &resolved)?;
        let managed = id != repo && tags.iter().any(|t| matches!(t, ModTag::Library | ModTag::Loader));
        rec.mods.push(InstalledMod {
            package_id: id,
            name,
            repo: Some(id_repo),
            version: resolved.version.clone(),
            versions: vec![resolved.version],
            enabled: true,
            source: ModSource::Github,
            tags,
            managed,
            update: None,
            file,
        });
    }

    // auto-install the BepInEx loader using the detected/saved game (best-effort)
    if let Some((game_path, _)) = current_game() {
        let _ = ensure_loader_impl(&game_path, &profile_id, &arch);
    }
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
pub async fn set_mod_version(
    profile_id: String,
    package_id: String,
    version: String,
    arch: String,
) -> Result<ProfileRecord, String> {
    blocking(move || set_mod_version_impl(profile_id, package_id, version, arch)).await
}

fn set_mod_version_impl(
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

#[tauri::command]
pub async fn apply_lobby_code(code: String, arch: String) -> Result<ProfileRecord, String> {
    blocking(move || apply_lobby_code_impl(code, arch)).await
}

fn apply_lobby_code_impl(code: String, arch: String) -> Result<ProfileRecord, String> {
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
        let managed = tags.iter().any(|t| matches!(t, ModTag::Library | ModTag::Loader));
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
        id: id.clone(),
        name: format!("Lobby - {display}"),
        crew_color: "#ffd23f".to_string(),
        game_build: manifest.game_build.clone(),
        mods,
    };
    store.save(&record).map_err(|e| e.to_string())?;
    if let Some((game_path, _)) = current_game() {
        let _ = ensure_loader_impl(&game_path, &id, &arch);
    }
    Ok(record)
}

// ---------- loader + launch ----------

#[tauri::command]
pub async fn ensure_loader(
    game_path: String,
    profile_id: String,
    arch: String,
) -> Result<(), String> {
    blocking(move || ensure_loader_impl(&game_path, &profile_id, &arch)).await
}

#[tauri::command]
pub async fn launch_profile(game_path: String, profile_id: String) -> Result<(), String> {
    blocking(move || {
        if process::is_running() {
            return Err("Among Us is running. Close it before launching.".into());
        }
        let arch = settings::load().arch.unwrap_or_else(|| "x86".to_string());
        ensure_loader_impl(&game_path, &profile_id, &arch)?;
        let root = settings::profiles_root();
        let spec = process::build_launch(Path::new(&game_path), &root.join(&profile_id));
        process::launch(&spec).map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
}
