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
    bundled_catalog()
}

/// The catalog compiled into this build (always current with the app). Used for
/// the loader source so a stale on-disk mod cache can't break BepInEx install.
fn bundled_catalog() -> Catalog {
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

/// Resolve the newest BepInEx loader (id + download url) for `arch`.
/// Preferred: scrape the latest build from builds.bepinex.dev (always current,
/// never hardcoded). Fallbacks: BepInEx Among Us pack API, then a fixed url.
fn resolve_loader(http: &dyn Http, arch: &str) -> Result<(String, String), String> {
    let loader = bundled_catalog().loader.ok_or("catalog has no loader entry")?;
    if let Some(builds) = &loader.builds_url {
        if let Ok(html) = http.get_text(builds) {
            if let Some(pair) = loader::parse_latest_build(&html, arch) {
                return Ok(pair);
            }
        }
    }
    if let Some(api) = &loader.thunderstore_api {
        if let Ok(text) = http.get_text(api) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let (Some(ver), Some(url)) = (
                    v["latest"]["version_number"].as_str(),
                    v["latest"]["download_url"].as_str(),
                ) {
                    return Ok((format!("au-{ver}"), url.to_string()));
                }
            }
        }
    }
    if let Some(u) = &loader.pack_url {
        if !u.is_empty() {
            return Ok(("pinned".to_string(), u.clone()));
        }
    }
    Err("could not resolve a BepInEx loader source (check your internet)".to_string())
}

/// Install the Doorstop/BepInEx loader for a profile (idempotent). Downloads +
/// caches the GitHub pack once per arch.
fn ensure_loader_impl(game_path: &str, profile_id: &str, arch: &str) -> Result<(), String> {
    let game_dir = Path::new(game_path);
    if !game_dir.is_dir() {
        return Err(format!("game folder not found: {game_path}"));
    }
    let root = settings::profiles_root();
    loader::ensure_profile_layout(&root, profile_id).map_err(|e| e.to_string())?;
    // a loader we installed is already present? leave it (no per-launch network).
    if loader::has_loader(game_dir) {
        return Ok(());
    }
    // resolve newest BEFORE wiping, so an offline failure doesn't break a working install
    let h = http();
    let (id, url) = resolve_loader(&h, arch)?;
    // remove any old/foreign loader bits (keep plugins) then install the current build
    let bep = game_dir.join("BepInEx");
    let _ = fs::remove_file(game_dir.join("winhttp.dll"));
    for d in ["core", "interop", "cache"] {
        let _ = fs::remove_dir_all(bep.join(d));
    }
    let cache = settings::cache_dir().join("bepinex").join(&id).join(arch);
    if let Some(pack_root) = loader::locate_pack_root(&cache) {
        loader::install_pack(&pack_root, game_dir, &id).map_err(|e| e.to_string())?;
    } else {
        let bytes = h.get_bytes(&url).map_err(|e| e.to_string())?;
        loader::install_pack_from_zip(&bytes, game_dir, &cache, &id).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Force a clean BepInEx reinstall: wipe the cached pack + the game's loader
/// (keeping plugins), then reinstall the current pack. Use when the installed
/// BepInEx is stale (e.g. Cpp2IL metadata version mismatch).
fn reinstall_loader_impl(game_path: &str, profile_id: &str, arch: &str) -> Result<(), String> {
    let game = Path::new(game_path);
    let _ = fs::remove_dir_all(settings::cache_dir().join("bepinex"));
    let _ = fs::remove_file(game.join("winhttp.dll"));
    let _ = fs::remove_file(game.join("BepInEx").join(".perfectsync_loader"));
    let _ = fs::remove_dir_all(game.join("BepInEx").join("core"));
    let _ = fs::remove_dir_all(game.join("BepInEx").join("interop"));
    let _ = fs::remove_dir_all(game.join("BepInEx").join("cache"));
    ensure_loader_impl(game_path, profile_id, arch)
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

    // auto-install this mod's catalog dependencies (Reactor/MiraAPI/etc.)
    for dep in deps::resolve(&cat, &[repo.clone()]).ordered {
        if dep == repo || rec.mods.iter().any(|m| m.package_id == dep) {
            continue;
        }
        let dentry = cat.get(&dep);
        let dep_repo = dentry
            .and_then(|e| e.repo.clone())
            .or_else(|| resolver::parse_repo(&dep))
            .unwrap_or_else(|| dep.clone());
        let rules = dentry.map(|e| e.asset_rules.clone()).unwrap_or_else(default_rules);
        let tags = dentry.map(|e| e.tags.clone()).unwrap_or_default();
        let name = dentry.map(|e| e.name.clone()).unwrap_or_else(|| dep.clone());
        let Ok(resolved) = resolver::resolve_latest(&h, &dep_repo, &rules, &arch) else {
            continue;
        };
        let dfile = install_resolved(&root, &profile_id, &h, &resolved)?;
        rec.mods.push(InstalledMod {
            package_id: dep,
            name,
            repo: Some(dep_repo),
            version: resolved.version.clone(),
            versions: vec![resolved.version],
            enabled: true,
            source: ModSource::Github,
            tags,
            managed: true,
            update: None,
            file: dfile,
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
        let repo = resolver::parse_repo(&mm.id)
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

    // re-resolve dependencies (kept out of the code to keep it short)
    let chosen: Vec<String> = manifest.mods.iter().map(|m| m.id.clone()).collect();
    for dep in deps::resolve(&cat, &chosen).ordered {
        if mods.iter().any(|m| m.package_id == dep) {
            continue;
        }
        let dentry = cat.get(&dep);
        let dep_repo = dentry
            .and_then(|e| e.repo.clone())
            .or_else(|| resolver::parse_repo(&dep))
            .unwrap_or_else(|| dep.clone());
        let rules = dentry.map(|e| e.asset_rules.clone()).unwrap_or_else(default_rules);
        let tags = dentry.map(|e| e.tags.clone()).unwrap_or_default();
        let dname = dentry.map(|e| e.name.clone()).unwrap_or_else(|| dep.clone());
        let Ok(resolved) = resolver::resolve_latest(&http, &dep_repo, &rules, &arch) else {
            continue;
        };
        let file = install_resolved(&root, &id, &http, &resolved)?;
        mods.push(InstalledMod {
            package_id: dep,
            name: dname,
            repo: Some(dep_repo),
            version: resolved.version.clone(),
            versions: vec![resolved.version],
            enabled: true,
            source: ModSource::Github,
            tags,
            managed: true,
            update: None,
            file,
        });
    }

    // merge the user's personal "always-include" mods (if not already in the code)
    for pm in settings::load().personal_mods {
        let prepo = resolver::parse_repo(&pm.repo).unwrap_or_else(|| pm.repo.clone());
        if mods.iter().any(|m| m.package_id == prepo) {
            continue;
        }
        let Ok(rel) = resolver::fetch_release_by_tag(&http, &prepo, &pm.tag) else {
            continue;
        };
        let Some(asset) = rel.assets.iter().find(|a| a.name == pm.asset) else {
            continue;
        };
        let resolved = ResolvedDownload {
            url: asset.url.clone(),
            asset_name: asset.name.clone(),
            version: rel.tag.clone(),
            size: asset.size,
        };
        let file = install_resolved(&root, &id, &http, &resolved)?;
        let entry = cat.get(&prepo);
        mods.push(InstalledMod {
            package_id: prepo.clone(),
            name: pm.name.clone().or_else(|| entry.map(|e| e.name.clone())).unwrap_or_else(|| prepo.clone()),
            repo: Some(prepo),
            version: rel.tag.clone(),
            versions: vec![rel.tag],
            enabled: true,
            source: ModSource::Github,
            tags: entry.map(|e| e.tags.clone()).unwrap_or_default(),
            managed: false,
            update: None,
            file,
        });
    }

    let record = ProfileRecord {
        id: id.clone(),
        name: display.clone(),
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
pub async fn reinstall_loader(
    game_path: String,
    profile_id: String,
    arch: String,
) -> Result<(), String> {
    blocking(move || reinstall_loader_impl(&game_path, &profile_id, &arch)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderStatus {
    pub game_found: bool,
    pub winhttp: bool,
    pub preloader: bool,
    pub current: bool,
    pub installed_version: Option<String>,
    pub dotnet: bool,
    pub steam_appid: bool,
    pub profile_plugins: usize,
    pub game_plugins: usize,
}

#[tauri::command]
pub fn loader_status(game_path: String, profile_id: String) -> LoaderStatus {
    let game = Path::new(&game_path);
    let root = settings::profiles_root();
    let count_dll = |dir: std::path::PathBuf| {
        fs::read_dir(dir)
            .map(|it| {
                it.flatten()
                    .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("dll"))
                    .count()
            })
            .unwrap_or(0)
    };
    LoaderStatus {
        game_found: game.is_dir(),
        winhttp: game.join("winhttp.dll").exists(),
        preloader: game.join("BepInEx").join("core").join(loader::IL2CPP_PRELOADER).exists(),
        current: loader::has_loader(game),
        installed_version: loader::installed_version(game),
        dotnet: game.join("dotnet").join("coreclr.dll").exists(),
        steam_appid: game.join("steam_appid.txt").exists(),
        profile_plugins: count_dll(loader::profile_plugins_dir(&root, &profile_id)),
        game_plugins: count_dll(game.join("BepInEx").join("plugins")),
    }
}

#[tauri::command]
pub async fn launch_profile(game_path: String, profile_id: String) -> Result<(), String> {
    blocking(move || {
        if process::is_running() {
            return Err("Among Us is running. Close it before launching.".into());
        }
        let arch = settings::load().arch.unwrap_or_else(|| "x86".to_string());
        ensure_loader_impl(&game_path, &profile_id, &arch)?;
        let _ = loader::ensure_steam_appid(Path::new(&game_path));
        let _ = loader::write_console_off(Path::new(&game_path));
        // copy this profile's enabled plugins into the game's BepInEx/plugins
        loader::sync_profile_plugins(&settings::profiles_root(), &profile_id, Path::new(&game_path))
            .map_err(|e| e.to_string())?;
        let spec = process::build_launch(Path::new(&game_path));
        process::launch(&spec).map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
}
