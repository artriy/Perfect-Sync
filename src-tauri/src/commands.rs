//! Tauri commands: thin adapters over `perfect-sync-core`. Heavy logic lives in
//! the (tested) core crate; these wrap it for the frontend and map errors to
//! strings. The backend is authoritative for profile persistence on disk.
//!
//! Network/disk-heavy commands are `async` and run their blocking body on a
//! worker thread via `spawn_blocking`, so the UI thread never freezes.

use crate::settings::{self, Settings, SettingsView, TokenAction};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use perfect_sync_core::catalog::{parse, AssetArchRule, AssetRules, Catalog};
use perfect_sync_core::deps;
use perfect_sync_core::preview::{preview, Preview};
use perfect_sync_core::profile::{InstalledMod, ProfileRecord, ProfileStore};
use perfect_sync_core::resolver::{download_resolved, Http, Release, ResolvedDownload, UreqHttp};
use perfect_sync_core::types::{
    valid_levelimposter_map_id, Arch, ModSource, ModTag, Runtime, Store, Trust,
};
use perfect_sync_core::{codec, compat, game, loader, process, profile, resolver, tou_cosmetics};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::ipc::Channel;

const BUNDLED_CATALOG: &str = include_str!("../../catalog/catalog.json");

const DEFAULT_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/artriy/Perfect-Sync/main/catalog/catalog.json";
const MAX_CATALOG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_USER_CATALOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_CATALOG_ENVELOPE_BYTES: u64 = MAX_CATALOG_BYTES * 2 + MAX_USER_CATALOG_BYTES + 64 * 1024;
const DOORSTOP_FIX_VERSION: &str = "4.5.1";
const DOORSTOP_FIX_URL: &str =
    "https://github.com/Pietrodjaowjao/UnityDoorstop/releases/download/v4.5.1/doorstop_win_release.zip";
const DOORSTOP_FIX_SIZE: usize = 34_391;
const DOORSTOP_FIX_SHA256: &str =
    "c729811c724395d871e97ff2f49be71963951147ed8b878cb0be5d2e439b55b7";
const PINNED_LOADER_VERSION: &str = "be.753+0d275a4";
const PINNED_LOADER_X86_URL: &str =
    "https://builds.bepinex.dev/projects/bepinex_be/753/BepInEx-Unity.IL2CPP-win-x86-6.0.0-be.753%2B0d275a4.zip";
const PINNED_LOADER_X64_URL: &str =
    "https://builds.bepinex.dev/projects/bepinex_be/753/BepInEx-Unity.IL2CPP-win-x64-6.0.0-be.753%2B0d275a4.zip";
const TOU_MIRA_ID: &str = "AU-Avengers/TOU-Mira";
const TOU_BUNDLED_DEPENDENCY_IDS: &[&str] = &[
    "All-Of-Us-Mods/MiraAPI",
    "NuclearPowered/Reactor",
    "miniduikboot/Mini.RegionInstall",
];
const TOU_BUNDLED_PLUGIN_FILES: &[&str] = &["Mini.RegionInstall.dll", "MiraAPI.dll", "Reactor.dll"];
const PRIORITY_CATALOG_IDS: &[&str] = &[
    "TheOtherRolesAU/TheOtherRoles",
    TOU_MIRA_ID,
    "EnhancedNetwork/TownofHost-Enhanced",
    "Mehzxzz/TownOfExtra",
    LEVELIMPOSTER_ID,
];
const MAX_TOU_PACKAGE_CACHE_BYTES: u64 = 512 * 1024 * 1024;
const LEVELIMPOSTER_API: &str = "https://api.levelimposter.net";
const LEVELIMPOSTER_ALGOLIA_URL: &str =
    "https://T5IVXJGKB9-dsn.algolia.net/1/indexes/LevelImposter-Maps";
const LEVELIMPOSTER_ALGOLIA_APP_ID: &str = "T5IVXJGKB9";
const LEVELIMPOSTER_ALGOLIA_SEARCH_KEY: &str = "14062d24b40e0b3689a899fc36abd756";
const LEVELIMPOSTER_ID: &str = "DigiWorm0/LevelImposter";
const MAX_LEVELIMPOSTER_MAPS_PER_BATCH: usize = 32;
const MAX_LEVELIMPOSTER_MAP_BYTES: u64 = 256 * 1024 * 1024;
const MAX_LEVELIMPOSTER_MAP_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_LEVELIMPOSTER_BANNER_BYTES: usize = 4 * 1024 * 1024;
const MAX_PROFILE_STAGE_FILES: usize = 8_192;
const MAX_PROFILE_STAGE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_PROFILE_RECOVERY_JOURNALS: usize = 64;
const MAX_PROFILE_RECOVERY_PARENT_ENTRIES: usize = 4_096;
const MAX_PROFILE_RECOVERY_JOURNAL_BYTES: u64 = 1_024;
const MAX_MANAGED_GAME_COPY_FILES: usize = 200_000;
const MAX_MANAGED_GAME_COPY_BYTES: u64 = 32 * 1024 * 1024 * 1024;
static MUTATION_LOCK: Mutex<()> = Mutex::new(());
static LAUNCH_PENDING: AtomicBool = AtomicBool::new(false);
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static INSPECTED_GAMES: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

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

fn lock_mutations() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    MUTATION_LOCK
        .lock()
        .map_err(|_| "backend mutation lock is poisoned".to_string())
}

fn validate_profile_id(id: &str) -> Result<(), String> {
    profile::validate_profile_id(id).map_err(|error| error.to_string())
}

fn game_is_stopped() -> Result<(), String> {
    if LAUNCH_PENDING.load(Ordering::Acquire) {
        return Err(
            "Among Us is still launching. Wait for startup to finish before changing files.".into(),
        );
    }
    match process::try_is_running() {
        Ok(false) => Ok(()),
        Ok(true) => Err("Among Us is running. Close it first.".into()),
        Err(error) => Err(format!(
            "Could not verify whether Among Us is running; refusing to modify game files: {error}"
        )),
    }
}

fn spawn_launch(operation: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    LAUNCH_PENDING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "Among Us is already launching.".to_string())?;
    if let Err(error) = operation() {
        LAUNCH_PENDING.store(false, Ordering::Release);
        return Err(error);
    }
    std::thread::spawn(|| {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if matches!(process::try_is_running(), Ok(true)) || Instant::now() >= deadline {
                LAUNCH_PENDING.store(false, Ordering::Release);
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
    Ok(())
}

fn unique_sibling(path: &Path, label: &str) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or("path has no parent directory")?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("path has no portable file name")?;
    for _ in 0..128 {
        let serial = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{name}.{label}.{}.{}", std::process::id(), serial));
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("could not allocate a unique temporary path".into())
}

#[cfg(windows)]
fn atomic_replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > limit {
        return Err(format!("{} exceeds its size limit", path.display()));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .map_err(|error| error.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > limit {
        return Err(format!("{} exceeds its size limit", path.display()));
    }
    Ok(Some(bytes))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("file has no parent directory")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = unique_sibling(path, "tmp")?;
    let result = (|| {
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        output.write_all(bytes).map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
        drop(output);
        atomic_replace_file(&temporary, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn copy_profile_tree(source: &Path, destination: &Path) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&source_metadata) || !source_metadata.is_dir() {
        return Err("profile is not a regular directory".into());
    }
    fs::create_dir(destination).map_err(|error| error.to_string())?;
    let mut pending = vec![(source.to_path_buf(), destination.to_path_buf())];
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    while let Some((from, to)) = pending.pop() {
        for entry in fs::read_dir(&from).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "profile contains a link or reparse point: {}",
                    entry.path().display()
                ));
            }
            let target = to.join(entry.file_name());
            if metadata.is_dir() {
                fs::create_dir(&target).map_err(|error| error.to_string())?;
                pending.push((entry.path(), target));
            } else if metadata.is_file() {
                files += 1;
                bytes = bytes
                    .checked_add(metadata.len())
                    .filter(|total| *total <= MAX_PROFILE_STAGE_BYTES)
                    .ok_or("profile exceeds the staging byte limit")?;
                if files > MAX_PROFILE_STAGE_FILES {
                    return Err("profile contains too many files".into());
                }
                let mut input = File::open(entry.path()).map_err(|error| error.to_string())?;
                let mut output = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&target)
                    .map_err(|error| error.to_string())?;
                let copied =
                    io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
                if copied != metadata.len() {
                    return Err("profile file changed while it was staged".into());
                }
                output.sync_all().map_err(|error| error.to_string())?;
            } else {
                return Err("profile contains an unsupported filesystem entry".into());
            }
        }
    }
    Ok(())
}

const DISABLED_DOORSTOP: &str = ".perfectsync-winhttp.disabled";
const APP_LOADER_MARKER: &str = ".perfectsync_loader";

fn copy_snapshot_path(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) {
        return Err(format!("{} is a link or reparse point", source.display()));
    }
    if metadata.is_dir() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        copy_profile_tree(source, destination)
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut input = File::open(source).map_err(|error| error.to_string())?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination)
            .map_err(|error| error.to_string())?;
        let copied = io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
        if copied != metadata.len() {
            return Err(format!(
                "{} changed while it was snapshotted",
                source.display()
            ));
        }
        output.sync_all().map_err(|error| error.to_string())
    } else {
        Err(format!(
            "{} is not a regular file or directory",
            source.display()
        ))
    }
}

fn remove_snapshot_target(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_reparse(&metadata) => Err(format!(
            "refusing to replace reparse point {}",
            path.display()
        )),
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path).map_err(|error| error.to_string())
        }
        Ok(metadata) if metadata.is_file() => {
            fs::remove_file(path).map_err(|error| error.to_string())
        }
        Ok(_) => Err(format!(
            "{} is an unsupported filesystem entry",
            path.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn game_artifact_transaction<T>(
    game_dir: &Path,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let relative_paths = [
        PathBuf::from("winhttp.dll"),
        PathBuf::from(DISABLED_DOORSTOP),
        PathBuf::from("doorstop_config.ini"),
        PathBuf::from(".doorstop_version"),
        PathBuf::from("changelog.txt"),
        PathBuf::from("steam_appid.txt"),
        PathBuf::from("dotnet"),
        PathBuf::from("BepInEx").join(APP_LOADER_MARKER),
        PathBuf::from("BepInEx").join(loader::DOORSTOP_PATCH_MARKER),
        PathBuf::from("BepInEx").join(loader::MANAGED_TOU_PACKAGE_MARKER),
        PathBuf::from(loader::DOORSTOP_PATCH_TRANSACTION),
        PathBuf::from("BepInEx/core"),
        PathBuf::from("BepInEx/config"),
        PathBuf::from("BepInEx/patchers"),
        PathBuf::from("BepInEx/interop"),
        PathBuf::from("BepInEx/unity-libs"),
        PathBuf::from("BepInEx/cache"),
        PathBuf::from("BepInEx/plugins"),
    ];
    let backup = unique_sibling(game_dir, "prepare-backup")?;
    fs::create_dir(&backup).map_err(|error| error.to_string())?;
    let bep_existed = game_dir.join("BepInEx").exists();
    for relative in &relative_paths {
        let source = game_dir.join(relative);
        match fs::symlink_metadata(&source) {
            Ok(_) => {
                if let Err(error) = copy_snapshot_path(&source, &backup.join(relative)) {
                    let _ = fs::remove_dir_all(&backup);
                    return Err(error);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                let _ = fs::remove_dir_all(&backup);
                return Err(error.to_string());
            }
        }
    }
    match operation() {
        Ok(value) => {
            let _ = fs::remove_dir_all(&backup);
            Ok(value)
        }
        Err(error) => {
            let mut rollback_errors = Vec::new();
            for relative in relative_paths.iter().rev() {
                let target = game_dir.join(relative);
                if let Err(rollback) = remove_snapshot_target(&target) {
                    rollback_errors.push(rollback);
                    continue;
                }
                let saved = backup.join(relative);
                if saved.exists() {
                    if let Some(parent) = target.parent() {
                        if let Err(rollback) = fs::create_dir_all(parent) {
                            rollback_errors.push(rollback.to_string());
                            continue;
                        }
                    }
                    if let Err(rollback) = fs::rename(&saved, &target) {
                        rollback_errors.push(rollback.to_string());
                    }
                }
            }
            if !bep_existed {
                match fs::remove_dir(game_dir.join("BepInEx")) {
                    Ok(()) => {}
                    Err(rollback) if rollback.kind() == io::ErrorKind::NotFound => {}
                    Err(rollback) => rollback_errors.push(rollback.to_string()),
                }
            }
            if rollback_errors.is_empty() {
                let _ = fs::remove_dir_all(&backup);
                Err(error)
            } else {
                Err(format!(
                    "{error}; additionally failed to restore game artifacts: {}. Recovery backup retained at {}",
                    rollback_errors.join("; "),
                    backup.display()
                ))
            }
        }
    }
}

fn with_profile_layout<T>(
    profiles_root: &Path,
    profile_id: &str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    recover_profile_transactions(profiles_root)?;
    let bep = profiles_root.join(profile_id).join("BepInEx");
    let plugins = bep.join("plugins");
    let config = bep.join("config");
    let bep_existed = bep.exists();
    let plugins_existed = plugins.exists();
    let config_existed = config.exists();
    let outcome = match loader::ensure_profile_layout(profiles_root, profile_id) {
        Ok(()) => operation(),
        Err(error) => Err(error.to_string()),
    };
    match outcome {
        Ok(value) => Ok(value),
        Err(error) => {
            let mut rollback_errors = Vec::new();
            for (path, existed) in [(&plugins, plugins_existed), (&config, config_existed)] {
                if !existed {
                    if let Err(rollback) = fs::remove_dir(path) {
                        if rollback.kind() != io::ErrorKind::NotFound {
                            rollback_errors.push(rollback.to_string());
                        }
                    }
                }
            }
            if !bep_existed {
                if let Err(rollback) = fs::remove_dir(&bep) {
                    if rollback.kind() != io::ErrorKind::NotFound {
                        rollback_errors.push(rollback.to_string());
                    }
                }
            }
            if rollback_errors.is_empty() {
                Err(error)
            } else {
                Err(format!(
                    "{error}; additionally could not remove the newly created profile layout: {}",
                    rollback_errors.join("; ")
                ))
            }
        }
    }
}

fn with_existing_profile_layout<T>(
    profiles_root: &Path,
    profile_id: &str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    validate_profile_id(profile_id)?;
    recovered_profile_store(profiles_root)?
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or("profile not found")?;
    with_profile_layout(profiles_root, profile_id, operation)
}

fn app_loader_owned(game_dir: &Path) -> Result<bool, String> {
    let marker = game_dir.join("BepInEx").join(APP_LOADER_MARKER);
    match fs::symlink_metadata(marker) {
        Ok(metadata) if !is_reparse(&metadata) && metadata.is_file() => Ok(true),
        Ok(_) => Err("Perfect-Sync loader ownership marker is not a regular file".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn restore_doorstop(game_dir: &Path) -> Result<(), String> {
    let disabled = game_dir.join(DISABLED_DOORSTOP);
    let destination = game_dir.join("winhttp.dll");
    let metadata = match fs::symlink_metadata(&disabled) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err("disabled Doorstop entry point is not a regular file".into());
    }
    if !app_loader_owned(game_dir)? {
        return Err("disabled Doorstop entry point has no Perfect-Sync ownership marker".into());
    }
    if destination.exists() {
        return Err(
            "Cannot restore the Perfect-Sync Doorstop entry point because winhttp.dll already exists."
                .into(),
        );
    }
    fs::rename(disabled, destination).map_err(|error| error.to_string())
}

fn disable_doorstop(game_dir: &Path) -> Result<bool, String> {
    let source = game_dir.join("winhttp.dll");
    let disabled = game_dir.join(DISABLED_DOORSTOP);
    let owned = app_loader_owned(game_dir)?;
    if disabled.exists() {
        if source.exists() {
            return Err(
                "Both active and disabled Perfect-Sync Doorstop entry points exist.".into(),
            );
        }
        let metadata = fs::symlink_metadata(&disabled).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) || !metadata.is_file() {
            return Err("disabled Doorstop entry point is not a regular file".into());
        }
        if !owned {
            return Err(
                "disabled Doorstop entry point has no Perfect-Sync ownership marker".into(),
            );
        }
        return Ok(false);
    }
    if !owned {
        return Ok(false);
    }
    if !source.exists() {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(&source).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err("Perfect-Sync Doorstop entry point is not a regular file".into());
    }
    fs::rename(&source, &disabled).map_err(|error| error.to_string())?;
    Ok(true)
}

fn launch_without_doorstop(
    game_dir: &Path,
    launch: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let disabled_here = disable_doorstop(game_dir)?;
    match fs::symlink_metadata(game_dir.join("winhttp.dll")) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err("Cannot launch vanilla while an unowned winhttp.dll remains active.".into())
        }
        Err(error) => return Err(error.to_string()),
    }
    match launch() {
        Ok(()) => Ok(()),
        Err(error) if !disabled_here => Err(error),
        Err(error) => match restore_doorstop(game_dir) {
            Ok(()) => Err(error),
            Err(rollback) => Err(format!(
                "{error}; additionally could not restore Doorstop after launch failure: {rollback}"
            )),
        },
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProfileRecoveryAction {
    Publish,
    Rollback,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProfileRecoveryJournal {
    version: u32,
    profile_id: String,
    action: ProfileRecoveryAction,
}

struct ProfileTransactionPaths {
    stage_root: PathBuf,
    backup_root: PathBuf,
    journal: PathBuf,
}

fn profile_sibling_prefix(root: &Path, label: &str) -> Result<String, String> {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("profile root has no portable file name")?;
    Ok(format!(".{name}.{label}."))
}

fn profile_transaction_paths(root: &Path, suffix: &str) -> Result<ProfileTransactionPaths, String> {
    let parent = root
        .parent()
        .ok_or("profile root has no parent directory")?;
    Ok(ProfileTransactionPaths {
        stage_root: parent.join(format!(
            "{}{suffix}",
            profile_sibling_prefix(root, "transaction")?
        )),
        backup_root: parent.join(format!(
            "{}{suffix}",
            profile_sibling_prefix(root, "backup")?
        )),
        journal: parent.join(format!(
            "{}{suffix}",
            profile_sibling_prefix(root, "recovery")?
        )),
    })
}

fn valid_profile_transaction_suffix(suffix: &str) -> bool {
    if suffix.is_empty() || suffix.len() > 48 {
        return false;
    }
    let mut parts = suffix.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(process), Some(serial), None)
            if !process.is_empty()
                && !serial.is_empty()
                && process.bytes().all(|byte| byte.is_ascii_digit())
                && serial.bytes().all(|byte| byte.is_ascii_digit())
    )
}

fn allocate_profile_transaction_paths(root: &Path) -> Result<ProfileTransactionPaths, String> {
    let stage_prefix = profile_sibling_prefix(root, "transaction")?;
    for _ in 0..128 {
        let stage_root = unique_sibling(root, "transaction")?;
        let stage_name = stage_root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("transaction path has no portable file name")?;
        let suffix = stage_name
            .strip_prefix(&stage_prefix)
            .ok_or("transaction path has an invalid name")?;
        let paths = profile_transaction_paths(root, suffix)?;
        let available = [&paths.backup_root, &paths.journal].iter().all(|path| {
            matches!(
                fs::symlink_metadata(path),
                Err(error) if error.kind() == io::ErrorKind::NotFound
            )
        });
        if available {
            return Ok(paths);
        }
    }
    Err("could not allocate profile transaction artifacts".into())
}

fn write_profile_recovery_journal(
    path: &Path,
    profile_id: &str,
    action: ProfileRecoveryAction,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(&ProfileRecoveryJournal {
        version: 1,
        profile_id: profile_id.to_string(),
        action,
    })
    .map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_PROFILE_RECOVERY_JOURNAL_BYTES {
        return Err("profile recovery journal exceeds its size limit".into());
    }
    atomic_write(path, &bytes)
}

fn validate_profile_tree(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("profile recovery artifact is not a regular directory".into());
    }
    let mut pending = vec![path.to_path_buf()];
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "profile recovery artifact contains a link or reparse point: {}",
                    entry.path().display()
                ));
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                files += 1;
                bytes = bytes
                    .checked_add(metadata.len())
                    .filter(|total| *total <= MAX_PROFILE_STAGE_BYTES)
                    .ok_or("profile recovery artifact exceeds the byte limit")?;
                if files > MAX_PROFILE_STAGE_FILES {
                    return Err("profile recovery artifact contains too many files".into());
                }
            } else {
                return Err("profile recovery artifact contains an unsupported entry".into());
            }
        }
    }
    Ok(())
}

fn validate_recovery_profile(container: &Path, id: &str) -> Result<bool, String> {
    let profile_dir = container.join(id);
    match fs::symlink_metadata(&profile_dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
        Ok(_) => {
            validate_profile_tree(&profile_dir)?;
            ProfileStore::new(container)
                .load(id)
                .map_err(|error| error.to_string())?
                .ok_or("profile recovery artifact has no manifest")?;
            Ok(true)
        }
    }
}

fn validate_recovery_container(container: &Path, id: &str) -> Result<(bool, bool), String> {
    let metadata = match fs::symlink_metadata(container) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((false, false)),
        Err(error) => return Err(error.to_string()),
        Ok(metadata) => metadata,
    };
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "{} is not a regular recovery directory",
            container.display()
        ));
    }
    let mut profile_present = false;
    for entry in fs::read_dir(container).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name() != std::ffi::OsStr::new(id) || profile_present {
            return Err(format!(
                "{} contains ambiguous recovery data",
                container.display()
            ));
        }
        profile_present = true;
    }
    if profile_present {
        validate_recovery_profile(container, id)?;
    }
    Ok((true, profile_present))
}

fn remove_profile_recovery_artifacts(paths: &ProfileTransactionPaths) -> Result<(), String> {
    for directory in [&paths.stage_root, &paths.backup_root] {
        match fs::remove_dir_all(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    fs::remove_file(&paths.journal).map_err(|error| error.to_string())
}

fn recover_profile_transaction(
    root: &Path,
    paths: &ProfileTransactionPaths,
    journal: &ProfileRecoveryJournal,
) -> Result<(), String> {
    for _ in 0..4 {
        let final_present = validate_recovery_profile(root, &journal.profile_id)?;
        let (_, stage_present) =
            validate_recovery_container(&paths.stage_root, &journal.profile_id)?;
        let (_, backup_present) =
            validate_recovery_container(&paths.backup_root, &journal.profile_id)?;
        match journal.action {
            ProfileRecoveryAction::Publish => {
                match (final_present, stage_present, backup_present) {
                    (true, true, true) => {
                        return Err("both final, staged, and backup profiles exist".into())
                    }
                    (true, true, false) => {
                        fs::rename(
                            root.join(&journal.profile_id),
                            paths.backup_root.join(&journal.profile_id),
                        )
                        .map_err(|error| error.to_string())?;
                    }
                    (false, true, true) => {
                        fs::rename(
                            paths.stage_root.join(&journal.profile_id),
                            root.join(&journal.profile_id),
                        )
                        .map_err(|error| error.to_string())?;
                    }
                    (true, false, true) | (true, false, false) => {
                        return remove_profile_recovery_artifacts(paths)
                    }
                    (false, false, true) => {
                        fs::rename(
                            paths.backup_root.join(&journal.profile_id),
                            root.join(&journal.profile_id),
                        )
                        .map_err(|error| error.to_string())?;
                    }
                    _ => return Err("profile recovery artifacts are incomplete".into()),
                }
            }
            ProfileRecoveryAction::Rollback => match (final_present, backup_present) {
                (false, true) => {
                    fs::rename(
                        paths.backup_root.join(&journal.profile_id),
                        root.join(&journal.profile_id),
                    )
                    .map_err(|error| error.to_string())?;
                }
                (true, false) => return remove_profile_recovery_artifacts(paths),
                (true, true) => return Err("both final and rollback profiles exist".into()),
                (false, false) => {
                    return Err("rollback profile recovery artifacts are incomplete".into())
                }
            },
        }
    }
    Err("profile recovery did not converge".into())
}

fn recover_profile_transactions(root: &Path) -> Result<(), String> {
    let parent = root
        .parent()
        .ok_or("profile root has no parent directory")?;
    let prefix = profile_sibling_prefix(root, "recovery")?;
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let mut journals = Vec::new();
    let mut parent_entries = 0_usize;
    for entry in entries {
        parent_entries += 1;
        if parent_entries > MAX_PROFILE_RECOVERY_PARENT_ENTRIES {
            return Err("profile recovery directory contains too many entries".into());
        }
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(suffix) = name.strip_prefix(&prefix).map(str::to_owned) else {
            continue;
        };
        if !valid_profile_transaction_suffix(&suffix) {
            return Err(format!(
                "invalid profile recovery marker {}",
                entry.path().display()
            ));
        }
        if journals.len() >= MAX_PROFILE_RECOVERY_JOURNALS {
            return Err("too many pending profile recovery journals".into());
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) || !metadata.is_file() {
            return Err(format!(
                "profile recovery marker is not a regular file: {}",
                entry.path().display()
            ));
        }
        let bytes = read_bounded(&entry.path(), MAX_PROFILE_RECOVERY_JOURNAL_BYTES)?
            .ok_or("profile recovery marker disappeared")?;
        let journal: ProfileRecoveryJournal = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid profile recovery marker: {error}"))?;
        if journal.version != 1 {
            return Err("unsupported profile recovery journal version".into());
        }
        validate_profile_id(&journal.profile_id)?;
        journals.push((name, profile_transaction_paths(root, &suffix)?, journal));
    }
    journals.sort_by(|left, right| left.0.cmp(&right.0));
    let mut ids = HashSet::new();
    let mut artifacts = HashSet::new();
    for (_, paths, journal) in &journals {
        if !ids.insert(journal.profile_id.to_ascii_lowercase())
            || !artifacts.insert(paths.stage_root.clone())
            || !artifacts.insert(paths.backup_root.clone())
        {
            return Err("ambiguous profile recovery journals were retained".into());
        }
    }
    for (_, paths, journal) in journals {
        if let Err(error) = recover_profile_transaction(root, &paths, &journal) {
            return Err(format!(
                "could not recover profile {}: {error}; recovery evidence was retained at {}",
                journal.profile_id,
                paths.journal.display()
            ));
        }
    }
    Ok(())
}

fn failed_profile_commit(
    backup: &Path,
    publish_error: &io::Error,
    rollback: Result<(), String>,
) -> String {
    match rollback {
        Ok(()) => format!("could not commit staged profile: {publish_error}"),
        Err(rollback_error) => format!(
            "could not commit staged profile ({publish_error}) or restore the old profile \
             ({rollback_error}); the intact backup and recovery journal were retained at {}",
            backup.display()
        ),
    }
}

fn profile_transaction<T>(
    root: &Path,
    id: &str,
    operation: impl FnOnce(&Path, &ProfileStore) -> Result<T, String>,
) -> Result<T, String> {
    validate_profile_id(id)?;
    recover_profile_transactions(root)?;
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let final_dir = root.join(id);
    let paths = allocate_profile_transaction_paths(root)?;
    fs::create_dir(&paths.stage_root).map_err(|error| error.to_string())?;
    let stage_dir = paths.stage_root.join(id);
    let old_exists = match fs::symlink_metadata(&final_dir) {
        Ok(metadata) if is_reparse(&metadata) || !metadata.is_dir() => {
            let _ = fs::remove_dir(&paths.stage_root);
            return Err("profile is not a regular directory".into());
        }
        Ok(_) => match copy_profile_tree(&final_dir, &stage_dir) {
            Ok(()) => true,
            Err(error) => {
                let _ = fs::remove_dir_all(&paths.stage_root);
                return Err(error);
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Err(error) = fs::create_dir(&stage_dir) {
                let _ = fs::remove_dir_all(&paths.stage_root);
                return Err(error.to_string());
            }
            false
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&paths.stage_root);
            return Err(error.to_string());
        }
    };
    let stage_store = ProfileStore::new(&paths.stage_root);
    if old_exists {
        match stage_store.load(id) {
            Ok(Some(_)) => {}
            Ok(None) => {
                let _ = fs::remove_dir_all(&paths.stage_root);
                return Err("existing profile has no manifest".into());
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&paths.stage_root);
                return Err(error.to_string());
            }
        }
    }
    let value = match operation(&paths.stage_root, &stage_store) {
        Ok(value) => value,
        Err(error) => {
            let _ = fs::remove_dir_all(&paths.stage_root);
            return Err(error);
        }
    };
    if let Err(error) = stage_store
        .load(id)
        .map_err(|error| error.to_string())
        .and_then(|record| record.ok_or("staged profile has no manifest".to_string()))
    {
        let _ = fs::remove_dir_all(&paths.stage_root);
        return Err(error);
    }
    let backup = paths.backup_root.join(id);
    if old_exists {
        fs::create_dir(&paths.backup_root).map_err(|error| {
            let _ = fs::remove_dir_all(&paths.stage_root);
            error.to_string()
        })?;
        if let Err(error) =
            write_profile_recovery_journal(&paths.journal, id, ProfileRecoveryAction::Publish)
        {
            let _ = fs::remove_dir_all(&paths.stage_root);
            let _ = fs::remove_dir(&paths.backup_root);
            return Err(error);
        }
        fs::rename(&final_dir, &backup).map_err(|error| {
            format!(
                "could not move the old profile into recovery storage ({error}); \
                 recovery evidence was retained at {}",
                paths.journal.display()
            )
        })?;
    }
    if let Err(error) = fs::rename(&stage_dir, &final_dir) {
        if !old_exists {
            let _ = fs::remove_dir_all(&paths.stage_root);
            return Err(format!("could not commit staged profile: {error}"));
        }
        if let Err(journal_error) =
            write_profile_recovery_journal(&paths.journal, id, ProfileRecoveryAction::Rollback)
        {
            return Err(format!(
                "could not commit staged profile ({error}) or record rollback intent \
                 ({journal_error}); recovery evidence was retained at {}",
                paths.journal.display()
            ));
        }
        let rollback = fs::rename(&backup, &final_dir).map_err(|rollback| rollback.to_string());
        if rollback.is_ok() {
            let _ = remove_profile_recovery_artifacts(&paths);
        }
        return Err(failed_profile_commit(&backup, &error, rollback));
    }
    if old_exists {
        remove_profile_recovery_artifacts(&paths)
            .map_err(|error| format!("profile committed but recovery cleanup failed: {error}"))?;
    } else {
        let _ = fs::remove_dir(&paths.stage_root);
    }
    Ok(value)
}

fn delete_profile_transaction(root: &Path, id: &str) -> Result<(), String> {
    validate_profile_id(id)?;
    recover_profile_transactions(root)?;
    let final_dir = root.join(id);
    let metadata = match fs::symlink_metadata(&final_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("profile is not a regular directory".into());
    }
    let trash_root = unique_sibling(root, "deleted")?;
    fs::create_dir(&trash_root).map_err(|error| error.to_string())?;
    let trash = trash_root.join(id);
    if let Err(error) = fs::rename(&final_dir, &trash) {
        let _ = fs::remove_dir(&trash_root);
        return Err(error.to_string());
    }
    let _ = fs::remove_dir_all(&trash_root);
    Ok(())
}

fn legacy_cached_catalog_text() -> Result<Option<String>, String> {
    let Some(bytes) = read_bounded(&settings::catalog_cache_path(), MAX_CATALOG_BYTES)? else {
        return Ok(None);
    };
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| format!("invalid catalog UTF-8: {error}"))
}

fn legacy_catalog() -> Result<Catalog, String> {
    match legacy_cached_catalog_text()? {
        Some(text) => parse(&text).map_err(|error| format!("invalid cached catalog: {error}")),
        None => Ok(bundled_catalog()),
    }
}

fn validate_persisted_catalog(catalog: Catalog) -> Result<Catalog, String> {
    let encoded = serde_json::to_string(&catalog).map_err(|error| error.to_string())?;
    parse(&encoded).map_err(|error| format!("invalid persisted hosted catalog: {error}"))
}

fn catalog() -> Result<Catalog, String> {
    let persisted = read_bounded(&settings::user_catalog_path(), MAX_CATALOG_ENVELOPE_BYTES)?
        .and_then(|bytes| serde_json::from_slice::<CatalogEnvelope>(&bytes).ok())
        .and_then(|envelope| envelope.hosted_catalog);
    let mut active = match persisted {
        Some(hosted) => validate_persisted_catalog(hosted)?,
        None => legacy_catalog()?,
    };
    apply_bundled_install_policy(&mut active);
    Ok(active)
}

/// The catalog compiled into this build (always current with the app). Used for
/// the loader source so a stale on-disk mod cache can't break BepInEx install.
fn bundled_catalog() -> Catalog {
    parse(BUNDLED_CATALOG).expect("bundled catalog parses")
}

/// Dependency graphs and asset selection are executable install policy. Bundled
/// entries therefore override stale hosted/cache policy while descriptive data stays live.
fn apply_bundled_install_policy(active: &mut Catalog) {
    let bundled = bundled_catalog();
    for entry in &mut active.mods {
        if let Some(authoritative) = bundled.get(&entry.id) {
            entry.asset_rules = authoritative.asset_rules.clone();
            entry.dependencies = authoritative.dependencies.clone();
            entry.dependency_versions = authoritative.dependency_versions.clone();
        }
    }
}

fn apply_bundled_display_policy(list: &mut [CatalogListItem]) {
    let bundled = bundled_catalog();
    for item in list {
        if let Some(authoritative) = bundled.get(&item.id) {
            item.dependencies = authoritative.dependencies.clone();
            item.included = catalog_item(authoritative.clone()).included;
        }
    }
}

fn catalog_display_rank(item: &CatalogListItem) -> usize {
    PRIORITY_CATALOG_IDS
        .iter()
        .position(|id| id.eq_ignore_ascii_case(&item.id))
        .unwrap_or_else(|| {
            if item.tags.contains(&ModTag::Library)
                || TOU_BUNDLED_DEPENDENCY_IDS
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&item.id))
            {
                usize::MAX
            } else {
                PRIORITY_CATALOG_IDS.len()
            }
        })
}

fn apply_default_catalog_order(list: &mut [CatalogListItem]) {
    list.sort_by_key(catalog_display_rank);
}

fn recovered_profile_store(root: &Path) -> Result<ProfileStore, String> {
    recover_profile_transactions(root)?;
    Ok(ProfileStore::new(root))
}

fn store() -> Result<ProfileStore, String> {
    recovered_profile_store(&settings::profiles_root())
}

fn http() -> Result<UreqHttp, String> {
    let token = settings::github_token().map_err(|error| error.to_string())?;
    let exposed = token
        .as_ref()
        .map(|secret| secret.expose_secret().to_owned());
    Ok(UreqHttp::new(exposed))
}

fn default_rules() -> AssetRules {
    AssetRules {
        per_arch: HashMap::<String, AssetArchRule>::new(),
        dll_name: None,
        bundles_loader: false,
    }
}

fn saved_game_arch(instance_id: Option<&str>) -> Result<String, String> {
    let saved = settings::load().map_err(|error| error.to_string())?;
    let instance = match instance_id {
        Some(id) => saved
            .game_instances
            .iter()
            .find(|instance| instance.id == id)
            .ok_or("unknown game instance")?,
        None => saved
            .game_instances
            .first()
            .ok_or("save a game instance before resolving mod assets")?,
    };
    let game_dir = canonical_game_path(Path::new(&instance.path))?;
    game::exe_arch(&game_dir.join(process::GAME_EXE))
        .map(arch_str)
        .ok_or("Among Us executable architecture is unsupported".to_string())
}

fn profile_arch(profile_id: &str) -> Result<String, String> {
    validate_profile_id(profile_id)?;
    let record = store()?
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or("profile not found")?;
    saved_game_arch(record.game_instance_id.as_deref())
}

fn profile_store_runtime(profile_id: &str) -> Result<(Store, Runtime), String> {
    validate_profile_id(profile_id)?;
    let record = store()?
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or("profile not found")?;
    let saved = settings::load().map_err(|error| error.to_string())?;
    let instance = match record.game_instance_id.as_deref() {
        Some(id) => saved
            .game_instances
            .iter()
            .find(|instance| instance.id == id)
            .ok_or("unknown game instance")?,
        None => saved
            .game_instances
            .first()
            .ok_or("save a game instance before resolving Town of Us assets")?,
    };
    Ok((instance.store, instance.runtime))
}

fn is_tou_mira(identity: &str) -> bool {
    identity.eq_ignore_ascii_case(TOU_MIRA_ID)
}

fn tou_package_key(version: &str, asset_name: &str) -> String {
    let mut identity = Vec::with_capacity(version.len() + asset_name.len() + 1);
    identity.extend_from_slice(version.as_bytes());
    identity.push(0);
    identity.extend_from_slice(asset_name.as_bytes());
    sha256_hex(&identity)
}

fn tou_package_cache_path(version: &str, asset_name: &str) -> PathBuf {
    settings::cache_dir()
        .join("tou-mira")
        .join(format!("{}.zip", tou_package_key(version, asset_name)))
}

fn cache_tou_package(version: &str, asset_name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_TOU_PACKAGE_CACHE_BYTES {
        return Err("Town of Us package has an invalid download size".into());
    }
    let path = tou_package_cache_path(version, asset_name);
    if let Some(existing) = read_bounded(&path, MAX_TOU_PACKAGE_CACHE_BYTES)? {
        if existing == bytes {
            return Ok(path);
        }
    }
    atomic_write(&path, bytes)?;
    Ok(path)
}

fn tou_asset_fragment(arch: &str, store: Store, runtime: Runtime) -> Result<&'static str, String> {
    match arch {
        "x64" => Ok("x64-epic-msstore.zip"),
        "x86" if runtime != Runtime::Native => Ok("x86-macos-linux.zip"),
        "x86" if matches!(store, Store::Steam | Store::Itch | Store::Manual) => {
            Ok("x86-steam-itch.zip")
        }
        "x86" => Err("Town of Us has no x86 Epic/MS Store package for native Windows".into()),
        _ => Err("Among Us executable architecture is unsupported".into()),
    }
}

fn pick_profile_asset<'a>(
    release: &'a Release,
    repo: &str,
    rules: &AssetRules,
    arch: &str,
    store: Store,
    runtime: Runtime,
) -> Result<Option<&'a resolver::Asset>, String> {
    if !is_tou_mira(repo) {
        return Ok(resolver::pick_asset(release, rules, arch));
    }
    let fragment = tou_asset_fragment(arch, store, runtime)?;
    let mut matches = release.assets.iter().filter(|asset| {
        let name = asset.name.to_ascii_lowercase();
        name.ends_with(".zip") && name.contains(fragment)
    });
    let selected = matches.next();
    if selected.is_some() && matches.next().is_some() {
        return Err(format!(
            "Town of Us release {} has multiple {fragment} packages",
            release.tag
        ));
    }
    Ok(selected)
}

fn resolve_profile_tag(
    http: &dyn Http,
    repo: &str,
    tag: &str,
    rules: &AssetRules,
    arch: &str,
    store: Store,
    runtime: Runtime,
) -> Result<ResolvedDownload, String> {
    if !is_tou_mira(repo) {
        return resolver::resolve_tag(http, repo, tag, rules, arch)
            .map_err(|error| error.to_string());
    }
    let release =
        resolver::fetch_release_by_tag(http, repo, tag).map_err(|error| error.to_string())?;
    let asset = pick_profile_asset(&release, repo, rules, arch, store, runtime)?
        .ok_or_else(|| format!("Town of Us {tag} has no compatible full package"))?;
    resolver::resolved_asset(http, &release, asset).map_err(|error| error.to_string())
}

fn resolve_profile_latest(
    http: &dyn Http,
    repo: &str,
    rules: &AssetRules,
    arch: &str,
    store: Store,
    runtime: Runtime,
) -> Result<ResolvedDownload, String> {
    if !is_tou_mira(repo) {
        return resolver::resolve_latest(http, repo, rules, arch)
            .map_err(|error| error.to_string());
    }
    let release = resolver::fetch_latest_release(http, repo).map_err(|error| error.to_string())?;
    let asset = pick_profile_asset(&release, repo, rules, arch, store, runtime)?
        .ok_or_else(|| format!("Town of Us {} has no compatible full package", release.tag))?;
    resolver::resolved_asset(http, &release, asset).map_err(|error| error.to_string())
}

fn arch_str(a: Arch) -> String {
    match a {
        Arch::X86 => "x86".to_string(),
        Arch::X64 => "x64".to_string(),
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    matches!(
        (fs::canonicalize(a), fs::canonicalize(b)),
        (Ok(a), Ok(b)) if a == b
    )
}

fn inferred_store(game_dir: &Path) -> Store {
    let path = game_dir.to_string_lossy().replace('\\', "/").to_lowercase();
    if path.contains("/steamapps/") {
        Store::Steam
    } else if game_dir.join(".egstore").is_dir() || path.contains("/epic games/") {
        Store::Epic
    } else if path.contains("/windowsapps/") || path.contains("/xboxgames/") {
        Store::Msstore
    } else {
        Store::Manual
    }
}

fn runtime_context(game_dir: &Path) -> Result<compat::RuntimeContext, String> {
    let saved = settings::load().map_err(|error| error.to_string())?;
    let hint = saved
        .game_instances
        .iter()
        .find(|instance| same_path(Path::new(&instance.path), game_dir))
        .map(|instance| instance.runtime);
    Ok(compat::resolve_with_hint(game_dir, hint))
}

fn validate_game_dir(game_dir: &Path) -> Result<(), String> {
    if !game_dir.is_dir() {
        return Err(format!("game folder not found: {}", game_dir.display()));
    }
    let exe = game_dir.join(process::GAME_EXE);
    if !exe.is_file() {
        return Err(format!(
            "This is not the Among Us folder: {} is missing",
            exe.display()
        ));
    }
    if let Some(hint) = protected_install_hint(game_dir) {
        return Err(hint);
    }
    let probe = game_dir.join(format!(".perfectsync-write-test-{}", std::process::id()));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|e| {
            format!(
                "Perfect-Sync cannot modify the Among Us folder {}: {e}",
                game_dir.display()
            )
        })?;
    fs::remove_file(&probe).map_err(|e| {
        format!(
            "Perfect-Sync could not clean up its write probe {}: {e}",
            probe.display()
        )
    })
}

fn copy_game_tree(
    source: &Path,
    destination: &Path,
    files: &mut usize,
    bytes: &mut u64,
) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&source_metadata) || !source_metadata.is_dir() {
        return Err(format!(
            "Managed game copies cannot follow links or reparse points: {}",
            source.display()
        ));
    }
    fs::create_dir(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(".perfectsync-") {
            continue;
        }
        let from = entry.path();
        let to = destination.join(name);
        let metadata = fs::symlink_metadata(&from).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) {
            return Err(format!(
                "Managed game copies cannot follow links or reparse points: {}",
                from.display()
            ));
        }
        if metadata.is_dir() {
            copy_game_tree(&from, &to, files, bytes)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(format!("Unsupported game file type: {}", from.display()));
        }
        *files = files
            .checked_add(1)
            .ok_or("Managed game copy file count overflow")?;
        *bytes = bytes
            .checked_add(metadata.len())
            .ok_or("Managed game copy size overflow")?;
        if *files > MAX_MANAGED_GAME_COPY_FILES || *bytes > MAX_MANAGED_GAME_COPY_BYTES {
            return Err("The selected game copy exceeds the managed-copy safety limit.".into());
        }
        let mut input = File::open(&from).map_err(|error| error.to_string())?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&to)
            .map_err(|error| error.to_string())?;
        io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn create_managed_game_copy(
    source_path: String,
    destination_parent: String,
) -> Result<game::GameInstall, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        game_is_stopped()?;
        let source = canonical_game_path(Path::new(&source_path))?;
        let parent = fs::canonicalize(Path::new(&destination_parent))
            .map_err(|error| format!("Could not open the managed-copy destination: {error}"))?;
        if !parent.is_dir() || !game::is_writable_game_dir(&parent) {
            return Err("Choose a writable destination folder for the managed game copy.".into());
        }
        if parent.starts_with(&source) {
            return Err(
                "Choose a managed-copy destination outside the original game folder.".into(),
            );
        }
        let destination = parent.join("Among Us - Perfect Sync");
        if destination.exists() {
            return Err(format!(
                "{} already exists. Choose a different destination or remove that prior copy.",
                destination.display()
            ));
        }
        let stage = parent.join(format!(
            ".perfectsync-game-copy-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut files = 0;
            let mut bytes = 0;
            copy_game_tree(&source, &stage, &mut files, &mut bytes)?;
            validate_game_dir(&stage)?;
            fs::rename(&stage, &destination).map_err(|error| error.to_string())?;
            Ok(game::GameInstall {
                path: destination.clone(),
                store: Store::Msstore,
                arch: game::exe_arch(&destination.join(process::GAME_EXE))
                    .ok_or("Managed Among Us copy has unsupported architecture")?,
                runtime: Runtime::Native,
                build: game::detect_build(&destination),
                writable: true,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&stage);
        }
        result
    })
    .await
}

fn canonical_game_path(game_dir: &Path) -> Result<PathBuf, String> {
    if !game_dir.is_dir() {
        return Err(format!("game folder not found: {}", game_dir.display()));
    }
    let canonical = fs::canonicalize(game_dir).map_err(|error| error.to_string())?;
    let executable = canonical.join(process::GAME_EXE);
    let metadata = fs::symlink_metadata(&executable).map_err(|_| {
        format!(
            "This is not the Among Us folder: {} is missing",
            executable.display()
        )
    })?;
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err("Among Us executable is not a regular file".into());
    }
    game::exe_arch(&executable).ok_or("Among Us executable architecture is unsupported")?;
    Ok(canonical)
}

fn validate_game_target(game_path: &str, profile_id: Option<&str>) -> Result<PathBuf, String> {
    let canonical = canonical_game_path(Path::new(game_path))?;
    let saved = settings::load().map_err(|error| error.to_string())?;
    if let Some(profile_id) = profile_id {
        validate_profile_id(profile_id)?;
        let record = store()?
            .load(profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let instance = match record.game_instance_id {
            Some(instance_id) => saved
                .game_instances
                .iter()
                .find(|instance| instance.id == instance_id)
                .ok_or("profile refers to an unknown game instance")?,
            None => saved
                .game_instances
                .first()
                .ok_or("profile has no saved game instance")?,
        };
        if !same_path(Path::new(&instance.path), &canonical) {
            return Err("game folder does not match the profile's saved instance".into());
        }
        return Ok(canonical);
    }
    if saved
        .game_instances
        .iter()
        .any(|instance| same_path(Path::new(&instance.path), &canonical))
    {
        return Ok(canonical);
    }
    if INSPECTED_GAMES
        .lock()
        .map_err(|_| "inspected game lock is poisoned")?
        .contains(&canonical)
    {
        return Ok(canonical);
    }
    Err("game folder must be saved or explicitly inspected before use".into())
}

fn configure_runtime_override(ctx: &compat::RuntimeContext) -> Result<(), String> {
    if ctx.runtime == Runtime::Native {
        return Ok(());
    }
    let prefix = ctx.prefix.as_ref().ok_or_else(|| {
        "No Wine prefix could be derived for this game folder. Select the real folder inside the Wine/CrossOver/Whisky/Bottles prefix.".to_string()
    })?;
    compat::register_winhttp_override(prefix).map_err(|e| {
        if ctx.runtime == Runtime::Proton && e.kind() == std::io::ErrorKind::NotFound {
            format!(
                "Steam has not created the Among Us Proton prefix yet. Launch Among Us once without mods, close it, then retry. Folder setup is already safe to run. ({e})"
            )
        } else {
            format!(
                "Could not configure BepInEx's winhttp override in {}: {e}",
                prefix.display()
            )
        }
    })
}

/// Friendly guidance when a non-native launcher cannot be started.
fn launch_err_msg(ctx: &compat::RuntimeContext, e: &std::io::Error) -> String {
    match ctx.runtime {
        Runtime::Proton => format!(
            "Couldn't start Steam or Flatpak Steam for Proton ({e}). The Among Us folder is already synchronized; launch the game from Steam."
        ),
        Runtime::Wine => format!(
            "Couldn't run Wine ({e}). The Among Us folder is already synchronized; launch the game from your Wine frontend."
        ),
        Runtime::Crossover => format!(
            "Couldn't run CrossOver's cxrun ({e}). The Among Us folder is already synchronized; launch it from CrossOver."
        ),
        Runtime::Whisky => format!(
            "Couldn't run Whisky's Wine ({e}). The Among Us folder is already synchronized; launch it from Whisky."
        ),
        Runtime::Bottles => format!(
            "Couldn't run bottles-cli ({e}). The Among Us folder is already synchronized; launch it from Bottles."
        ),
        Runtime::Native => format!("Failed to launch the game: {e}"),
    }
}

/// Download a verified release asset and install its declared plugin DLL.
fn install_resolved(
    profiles_root: &Path,
    profile_id: &str,
    http: &dyn Http,
    resolved: &ResolvedDownload,
    expected_dll: Option<&str>,
) -> Result<String, String> {
    let asset = resolved.asset_name.to_ascii_lowercase();
    let bytes = download_resolved(http, resolved).map_err(|error| error.to_string())?;
    if asset.ends_with(".dll") {
        profile::install_plugin_bytes(profiles_root, profile_id, &resolved.asset_name, &bytes)
            .map_err(|error| error.to_string())?;
        return Ok(resolved.asset_name.clone());
    }
    if asset.ends_with(".zip") {
        let dll_name = expected_dll.ok_or(
            "Catalog ZIP installs require an exact declared DLL name. Pick a DLL file manually.",
        )?;
        profile::install_plugin_zip_bytes(profiles_root, profile_id, dll_name, &bytes)
            .map_err(|error| error.to_string())?;
        return Ok(dll_name.to_string());
    }
    Err("Only .dll files and catalog-declared .zip packages can be installed.".into())
}

fn pinned_loader(arch: &str) -> Result<(String, String), String> {
    let url = match arch {
        "x86" => PINNED_LOADER_X86_URL,
        "x64" => PINNED_LOADER_X64_URL,
        _ => return Err("Among Us executable architecture is unsupported".into()),
    };
    Ok((PINNED_LOADER_VERSION.to_string(), url.to_string()))
}

/// Resolve the newest BepInEx loader for the explicit Advanced action.
/// The normal setup and reinstall paths use the pinned build instead.
fn resolve_loader(http: &dyn Http, arch: &str) -> Result<(String, String), String> {
    let loader = bundled_catalog()
        .loader
        .ok_or("catalog has no loader entry")?;
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

fn resolve_loader_for_ensure(
    complete_owned_loader: bool,
    resolve: impl FnOnce() -> Result<(String, String), String>,
) -> Result<Option<(String, String)>, String> {
    match resolve() {
        Ok(resolved) => Ok(Some(resolved)),
        Err(_) if complete_owned_loader => Ok(None),
        Err(error) => Err(error),
    }
}

fn download_loader_for_ensure(
    complete_owned_loader: bool,
    download: impl FnOnce() -> Result<Vec<u8>, String>,
) -> Result<Option<Vec<u8>>, String> {
    match download() {
        Ok(bytes) => Ok(Some(bytes)),
        Err(_) if complete_owned_loader => Ok(None),
        Err(error) => Err(error),
    }
}
fn download_doorstop_fix(http: &dyn Http) -> Result<Vec<u8>, String> {
    let bytes = http
        .get_bytes(DOORSTOP_FIX_URL)
        .map_err(|error| format!("could not download the BepInEx compatibility fix: {error}"))?;
    if bytes.len() != DOORSTOP_FIX_SIZE {
        return Err(format!(
            "BepInEx compatibility fix size mismatch: expected {DOORSTOP_FIX_SIZE} bytes, received {}",
            bytes.len()
        ));
    }
    if sha256_hex(&bytes) != DOORSTOP_FIX_SHA256 {
        return Err("BepInEx compatibility fix failed SHA-256 verification".into());
    }
    Ok(bytes)
}

fn install_loader_and_optional_fix(
    game_dir: &Path,
    pack: Option<(&Path, &str)>,
    fix: Option<&[u8]>,
) -> Result<(), String> {
    game_artifact_transaction(game_dir, || {
        if let Some((pack_root, version)) = pack {
            loader::install_pack(pack_root, game_dir, version)
                .map_err(|error| error.to_string())?;
        }
        if let Some(bytes) = fix {
            let arch = game::exe_arch(&game_dir.join(process::GAME_EXE))
                .map(arch_str)
                .ok_or("Among Us executable architecture is unsupported")?;
            loader::install_windows_doorstop_patch(bytes, game_dir, DOORSTOP_FIX_VERSION, &arch)
                .map_err(|error| error.to_string())?;
        } else if pack.is_some() {
            loader::clear_doorstop_patch_marker(game_dir).map_err(|error| error.to_string())?;
        }
        Ok(())
    })
}

fn doorstop_fix_is_current(game_dir: &Path, arch: &str) -> bool {
    loader::has_doorstop_patch(game_dir, DOORSTOP_FIX_VERSION, arch)
}

/// Install the Doorstop/BepInEx loader for a profile (idempotent). Downloads +
/// caches the GitHub pack once per arch.
fn ensure_loader_impl(
    game_path: &str,
    profile_id: &str,
    _arch: &str,
    apply_doorstop_fix: bool,
    http: &dyn Http,
    reporter: Option<&ProgressReporter>,
) -> Result<(), String> {
    if let Some(reporter) = reporter {
        reporter.stage(
            "preparing",
            "Checking the Among Us folder and active profile",
        );
    }
    game_is_stopped()?;
    let game_dir = validate_game_target(game_path, Some(profile_id))?;
    restore_doorstop(&game_dir)?;
    validate_game_dir(&game_dir)?;
    let root = settings::profiles_root();
    let profile = recovered_profile_store(&root)?
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "profile not found".to_string())?;
    if profile_uses_tou_mira(&profile) {
        if apply_doorstop_fix {
            return Err(
                "Town of Us includes its own fixed UnityDoorstop build; the separate compatibility fix is only available for BepInEx-only profiles."
                    .into(),
            );
        }
        return loader::has_loader(&game_dir)
            .then_some(())
            .ok_or_else(|| "The Town of Us BepInEx package is missing. Synchronize the profile to restore its complete release package.".into());
    }
    let arch = game::exe_arch(&game_dir.join(process::GAME_EXE))
        .map(arch_str)
        .ok_or("Among Us executable architecture is unsupported")?;
    if let Some(reporter) = reporter {
        reporter.stage(
            "resolving",
            "Checking the pinned BepInEx build and local cache",
        );
    }
    let have = loader::has_loader(&game_dir);
    let resolved = resolve_loader_for_ensure(have, || pinned_loader(&arch))?;
    let requested_install = resolved.filter(|(id, _)| {
        !have || loader::is_outdated(loader::installed_version(&game_dir).as_deref(), id)
    });

    let mut pack_install = None;
    if let Some((id, url)) = requested_install {
        let cache = loader::loader_cache_dir(&settings::cache_dir(), &id, &arch)
            .map_err(|error| error.to_string())?;
        let pack_root = if let Some(root) = loader::locate_pack_root(&cache) {
            Some(root)
        } else {
            download_loader_for_ensure(have, || {
                http.get_bytes(&url).map_err(|error| error.to_string())
            })?
            .map(|bytes| {
                loader::publish_pack_cache(&bytes, &cache).map_err(|error| error.to_string())
            })
            .transpose()?
        };
        if let Some(pack_root) = pack_root {
            pack_install = Some((pack_root, id));
        }
    }

    let needs_fix = apply_doorstop_fix && !doorstop_fix_is_current(&game_dir, &arch);
    if pack_install.is_none() && !needs_fix {
        return Ok(());
    }
    if let Some(reporter) = reporter {
        reporter.stage(
            "finalizing",
            "Publishing and configuring the BepInEx loader",
        );
    }
    let fix = if apply_doorstop_fix {
        Some(download_doorstop_fix(http)?)
    } else {
        None
    };
    game_is_stopped()?;
    install_loader_and_optional_fix(
        &game_dir,
        pack_install
            .as_ref()
            .map(|(root, version)| (root.as_path(), version.as_str())),
        fix.as_deref(),
    )
}

/// Force a fresh BepInEx download and rollback-safe replacement while keeping
/// profile plugins and the prior working loader intact on failure.
fn reinstall_loader_impl(
    game_path: &str,
    profile_id: &str,
    _arch: &str,
    apply_doorstop_fix: bool,
    use_latest_loader: bool,
) -> Result<(), String> {
    game_is_stopped()?;
    let game_dir = validate_game_target(game_path, Some(profile_id))?;
    restore_doorstop(&game_dir)?;
    validate_game_dir(&game_dir)?;
    let root = settings::profiles_root();
    let profile = recovered_profile_store(&root)?
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "profile not found".to_string())?;
    if profile_uses_tou_mira(&profile) {
        return Err(
            "Town of Us owns this profile's BepInEx build. Reinstall or change the Town of Us release instead."
                .into(),
        );
    }
    let arch = game::exe_arch(&game_dir.join(process::GAME_EXE))
        .map(arch_str)
        .ok_or("Among Us executable architecture is unsupported")?;
    let http = http()?;
    let (version, url) = if use_latest_loader {
        resolve_loader(&http, &arch)?
    } else {
        pinned_loader(&arch)?
    };
    let bytes = http.get_bytes(&url).map_err(|error| error.to_string())?;
    let cache = loader::loader_cache_dir(&settings::cache_dir(), &version, &arch)
        .map_err(|error| error.to_string())?;
    let pack_root =
        loader::publish_pack_cache(&bytes, &cache).map_err(|error| error.to_string())?;
    let fix = apply_doorstop_fix
        .then(|| download_doorstop_fix(&http))
        .transpose()?;
    game_is_stopped()?;
    install_loader_and_optional_fix(&game_dir, Some((&pack_root, &version)), fix.as_deref())
}

// ---------- settings + detection ----------

#[tauri::command]
pub async fn detect_games() -> Result<Vec<game::GameInstall>, String> {
    blocking(|| Ok(game::locate_all())).await
}

#[tauri::command]
pub async fn inspect_game(game_path: String) -> Result<game::GameInstall, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let canonical = canonical_game_path(Path::new(&game_path))?;
        let store = inferred_store(&canonical);
        let arch = game::exe_arch(&canonical.join(process::GAME_EXE))
            .ok_or("Among Us executable architecture is unsupported")?;
        let runtime = compat::resolve(&canonical).runtime;
        INSPECTED_GAMES
            .lock()
            .map_err(|_| "inspected game lock is poisoned")?
            .insert(canonical.clone());
        Ok(game::GameInstall {
            path: canonical.clone(),
            store,
            arch,
            runtime,
            build: game::detect_build(&canonical),
            writable: game::is_writable_game_dir(&canonical),
        })
    })
    .await
}

#[tauri::command]
pub async fn get_settings() -> Result<SettingsView, String> {
    blocking(|| settings::view().map_err(|error| error.to_string())).await
}

#[tauri::command]
pub async fn save_settings(
    mut settings: Settings,
    mut token_action: TokenAction,
) -> Result<SettingsView, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        if let TokenAction::Set { token } = &mut token_action {
            *token = token.trim().to_string();
            if token.is_empty() {
                return Err("GitHub token cannot be blank".into());
            }
        }
        let previous_settings = settings::load().map_err(|error| error.to_string())?;
        let mut ids = HashSet::new();
        let mut paths = Vec::<PathBuf>::new();
        for instance in &mut settings.game_instances {
            instance.id = instance.id.trim().to_string();
            instance.name = instance.name.trim().to_string();
            instance.path = instance.path.trim().to_string();
            if instance.id.is_empty() || !ids.insert(instance.id.clone()) {
                return Err("Every Among Us instance needs a unique id.".to_string());
            }
            if instance.name.is_empty() {
                instance.name = "Among Us".to_string();
            }
            let canonical = canonical_game_path(Path::new(&instance.path))?;
            validate_game_dir(&canonical)?;
            if paths.iter().any(|path| path == &canonical) {
                return Err("Every Among Us instance needs a unique folder.".to_string());
            }
            instance.path = canonical.to_string_lossy().into_owned();
            instance.arch = game::exe_arch(&canonical.join(process::GAME_EXE))
                .ok_or("Among Us executable architecture is unsupported")?;
            instance.runtime =
                compat::resolve_with_hint(&canonical, Some(instance.runtime)).runtime;
            let detected_store = inferred_store(&canonical);
            if detected_store != Store::Manual {
                instance.store = detected_store;
            }
            instance.build = game::detect_build(&canonical);
            instance.writable = game::is_writable_game_dir(&canonical);
            paths.push(canonical);
        }
        let mut personal_sources = HashSet::new();
        for personal in &mut settings.personal_mods {
            personal.repo = resolver::parse_repo(personal.repo.trim())
                .ok_or("Every personal mod needs a valid GitHub repository.")?;
            personal.tag = personal.tag.trim().to_string();
            personal.asset = personal.asset.trim().to_string();
            personal.name = personal
                .name
                .take()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty());
            if personal.tag.is_empty()
                || personal.tag.len() > 128
                || personal.asset.is_empty()
                || personal.asset.len() > 255
                || personal.asset.contains('/')
                || personal.asset.contains('\\')
                || personal.asset.chars().any(char::is_control)
            {
                return Err("Every personal mod needs a valid tag and release asset name.".into());
            }
            let identity = format!(
                "{}\0{}\0{}",
                personal.repo.to_ascii_lowercase(),
                personal.tag,
                personal.asset
            );
            if !personal_sources.insert(identity) {
                return Err("Personal mods cannot contain duplicate release assets.".into());
            }
        }
        let mut local_sources = HashSet::new();
        for local in &mut settings.personal_local_mods {
            local.path = local.path.trim().to_string();
            if !Path::new(&local.path).is_absolute() {
                return Err("Every local lobby default needs an absolute DLL path.".into());
            }
            let source_metadata =
                fs::symlink_metadata(&local.path).map_err(|error| error.to_string())?;
            if is_reparse(&source_metadata) || !source_metadata.is_file() {
                return Err("Every local lobby default must be a safe regular DLL file.".into());
            }
            let canonical = fs::canonicalize(&local.path)
                .map_err(|error| format!("Local lobby default is unavailable: {error}"))?;
            let metadata = fs::symlink_metadata(&canonical).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err("Every local lobby default must be a safe regular DLL file.".into());
            }
            if !metadata.is_file() {
                return Err("Every local lobby default must be a regular DLL file.".into());
            }
            let file_name = canonical
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("Local lobby default has no portable file name.")?;
            profile::validate_dll_name(file_name).map_err(|error| error.to_string())?;
            if !local_sources.insert(canonical.to_string_lossy().to_ascii_lowercase()) {
                return Err("Local lobby defaults cannot contain duplicate DLL files.".into());
            }
            local.path = canonical.to_string_lossy().into_owned();
            local.name = canonical
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or(file_name)
                .to_string();
        }
        for profile in store()?.list().map_err(|error| error.to_string())? {
            let (replacement, previous) = match profile.game_instance_id.as_deref() {
                Some(instance_id) => {
                    let replacement = settings
                        .game_instances
                        .iter()
                        .find(|instance| instance.id == instance_id)
                        .ok_or_else(|| {
                            format!(
                                "Game instance {instance_id} is still used by profile {}. Reassign or delete that profile first.",
                                profile.name
                            )
                        })?;
                    let previous = previous_settings
                        .game_instances
                        .iter()
                        .find(|instance| instance.id == instance_id);
                    (Some(replacement), previous)
                }
                None if profile.mods.is_empty() => continue,
                None => (
                    settings.game_instances.first(),
                    previous_settings.game_instances.first(),
                ),
            };
            let replacement = replacement.ok_or_else(|| {
                format!(
                    "Profile {} uses the default game instance. Save a compatible instance or reassign the profile first.",
                    profile.name
                )
            })?;
            if !profile.mods.is_empty() {
                let previous = previous.ok_or_else(|| {
                    format!(
                        "Profile {} has resolved mods but its prior game instance is missing. Re-create/re-resolve the profile before changing instances.",
                        profile.name
                    )
                })?;
                if previous.arch != replacement.arch || previous.store != replacement.store {
                    return Err(format!(
                        "Profile {} already contains resolved mods. Keep its game instance on the same architecture and store, or create/re-resolve a profile for the new installation.",
                        profile.name
                    ));
                }
            }
        }
        if let Some(active) = settings.active_profile.take() {
            let active = active.trim().to_string();
            validate_profile_id(&active)?;
            if store()?
                .load(&active)
                .map_err(|error| error.to_string())?
                .is_none()
            {
                return Err("Active profile does not exist.".into());
            }
            settings.active_profile = Some(active);
        }
        settings::apply_transaction(&settings, &token_action).map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
pub async fn game_running() -> Result<bool, String> {
    blocking(|| {
        if LAUNCH_PENDING.load(Ordering::Acquire) {
            Ok(true)
        } else {
            process::try_is_running().map_err(|error| error.to_string())
        }
    })
    .await
}

// ---------- catalog ----------

#[derive(Serialize, Deserialize, Clone)]
pub struct CatalogListItem {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub summary: String,
    pub tags: Vec<ModTag>,
    pub latest: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub included: Vec<String>,
    #[serde(default)]
    pub trust: Trust,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn catalog_item(entry: perfect_sync_core::catalog::CatalogEntry) -> CatalogListItem {
    let included = if is_tou_mira(&entry.id) {
        vec![
            "MiraAPI".into(),
            "Reactor".into(),
            "Mini.RegionInstall with the Town of Us server config".into(),
            "Town of Us cosmetics".into(),
        ]
    } else {
        Vec::new()
    };
    CatalogListItem {
        id: entry.id.clone(),
        name: entry.name,
        repo: entry.repo.unwrap_or(entry.id),
        summary: entry.summary,
        tags: entry.tags,
        dependencies: entry.dependencies,
        included,
        latest: String::new(),
        trust: Trust::Flagged,
        extra: HashMap::new(),
    }
}

fn apply_authoritative_trust(list: &mut [CatalogListItem]) {
    let bundled = bundled_catalog();
    for item in list {
        item.trust = bundled
            .get(&item.id)
            .filter(|entry| entry.repo.as_deref().unwrap_or(&entry.id) == item.repo)
            .map(|entry| entry.trust)
            .unwrap_or(Trust::Flagged);
    }
}

#[derive(Serialize, Deserialize)]
struct CatalogEnvelope {
    #[serde(default = "catalog_envelope_version")]
    version: u32,
    display: Vec<CatalogListItem>,
    #[serde(default)]
    order_policy_version: u32,
    #[serde(default)]
    hosted_ids: Vec<String>,
    #[serde(default)]
    hidden_hosted_ids: Vec<String>,
    #[serde(default)]
    hosted_catalog: Option<Catalog>,
}

fn catalog_envelope_version() -> u32 {
    1
}

const CATALOG_ORDER_POLICY_VERSION: u32 = 1;

fn validate_catalog_list(list: &mut [CatalogListItem]) -> Result<(), String> {
    if serde_json::to_vec(list)
        .map_err(|error| error.to_string())?
        .len() as u64
        > MAX_USER_CATALOG_BYTES
    {
        return Err("user catalog exceeds its size limit".into());
    }
    let mut ids = HashSet::new();
    for item in list.iter() {
        if item.id.trim().is_empty() || !ids.insert(item.id.to_ascii_lowercase()) {
            return Err("user catalog contains an invalid or duplicate id".into());
        }
        resolver::parse_repo(&item.repo)
            .ok_or_else(|| format!("user catalog contains invalid repository {}", item.repo))?;
    }
    apply_authoritative_trust(list);
    Ok(())
}

fn load_catalog_state() -> Result<CatalogEnvelope, String> {
    let path = settings::user_catalog_path();
    let stored = read_bounded(&path, MAX_CATALOG_ENVELOPE_BYTES)?;
    if let Some(bytes) = stored.as_deref() {
        if let Ok(mut envelope) = serde_json::from_slice::<CatalogEnvelope>(bytes) {
            if envelope.version != catalog_envelope_version() {
                return Err("unsupported user catalog envelope version".into());
            }
            if envelope.order_policy_version < CATALOG_ORDER_POLICY_VERSION {
                apply_default_catalog_order(&mut envelope.display);
                envelope.order_policy_version = CATALOG_ORDER_POLICY_VERSION;
            }
            validate_catalog_list(&mut envelope.display)?;
            if let Some(hosted) = envelope.hosted_catalog.take() {
                envelope.hosted_catalog = Some(validate_persisted_catalog(hosted)?);
            }
            return Ok(envelope);
        }
    }
    let legacy = legacy_catalog()?;
    let legacy_ids: Vec<String> = legacy.mods.iter().map(|entry| entry.id.clone()).collect();
    let mut display = match stored {
        Some(bytes) => serde_json::from_slice::<Vec<CatalogListItem>>(&bytes)
            .map_err(|error| format!("invalid user catalog: {error}"))?,
        None => legacy.mods.iter().cloned().map(catalog_item).collect(),
    };
    apply_default_catalog_order(&mut display);
    let mut state = CatalogEnvelope {
        version: catalog_envelope_version(),
        display,
        order_policy_version: CATALOG_ORDER_POLICY_VERSION,
        hosted_ids: legacy_ids,
        hidden_hosted_ids: Vec::new(),
        hosted_catalog: Some(legacy),
    };
    validate_catalog_list(&mut state.display)?;
    Ok(state)
}

fn display_catalog() -> Result<Vec<CatalogListItem>, String> {
    Ok(load_catalog_state()?.display)
}

fn serialized_catalog_state(state: &CatalogEnvelope) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(state).map_err(|error| error.to_string())?;
    if serde_json::to_vec(&state.display)
        .map_err(|error| error.to_string())?
        .len() as u64
        > MAX_USER_CATALOG_BYTES
    {
        return Err("user catalog exceeds its size limit".into());
    }
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_CATALOG_ENVELOPE_BYTES {
        return Err("catalog persistence envelope exceeds its size limit".into());
    }
    Ok(bytes)
}

fn save_catalog_state(state: &CatalogEnvelope) -> Result<(), String> {
    atomic_write(
        &settings::user_catalog_path(),
        &serialized_catalog_state(state)?,
    )
}

fn ensure_display_catalog_state(
    state: &mut CatalogEnvelope,
    requested_repo: &str,
    canonical_id: &str,
    effective_repo: &str,
    name: &str,
    summary: String,
    tags: Vec<ModTag>,
) {
    state
        .hidden_hosted_ids
        .retain(|hidden| !hidden.eq_ignore_ascii_case(canonical_id));
    let mut canonical = None;
    state.display.retain(|item| {
        if item.id.eq_ignore_ascii_case(canonical_id) {
            if canonical.is_none() {
                canonical = Some(item.clone());
            }
            return false;
        }
        !item.id.eq_ignore_ascii_case(requested_repo)
            && !item.repo.eq_ignore_ascii_case(effective_repo)
    });
    let mut item = canonical.unwrap_or_else(|| CatalogListItem {
        id: canonical_id.to_string(),
        name: name.to_string(),
        repo: effective_repo.to_string(),
        summary,
        tags,
        latest: String::new(),
        dependencies: Vec::new(),
        included: Vec::new(),
        trust: Trust::Flagged,
        extra: HashMap::new(),
    });
    item.id = canonical_id.to_string();
    item.repo = effective_repo.to_string();
    state.display.push(item);
}

fn ensure_display_catalog(
    requested_repo: &str,
    canonical_id: &str,
    effective_repo: &str,
    name: &str,
    summary: String,
    tags: Vec<ModTag>,
) -> Result<(), String> {
    let mut state = load_catalog_state()?;
    ensure_display_catalog_state(
        &mut state,
        requested_repo,
        canonical_id,
        effective_repo,
        name,
        summary,
        tags,
    );
    save_catalog_state(&state)
}

fn reconcile_hosted(mut state: CatalogEnvelope, hosted: &Catalog) -> CatalogEnvelope {
    let old_hosted: HashSet<String> = state
        .hosted_ids
        .iter()
        .map(|id| id.to_ascii_lowercase())
        .collect();
    let new_hosted: HashMap<String, _> = hosted
        .mods
        .iter()
        .map(|entry| (entry.id.to_ascii_lowercase(), entry))
        .collect();
    state
        .hidden_hosted_ids
        .retain(|id| new_hosted.contains_key(&id.to_ascii_lowercase()));
    let hidden: HashSet<String> = state
        .hidden_hosted_ids
        .iter()
        .map(|id| id.to_ascii_lowercase())
        .collect();
    let mut seen = HashSet::new();
    let mut output = Vec::with_capacity(state.display.len() + hosted.mods.len());
    for mut current in state.display {
        let folded = current.id.to_ascii_lowercase();
        if let Some(entry) = new_hosted.get(&folded) {
            seen.insert(folded.clone());
            if hidden.contains(&folded) {
                continue;
            }
            let mut replacement = catalog_item((*entry).clone());
            replacement.latest = std::mem::take(&mut current.latest);
            replacement.extra = std::mem::take(&mut current.extra);
            output.push(replacement);
        } else if !old_hosted.contains(&folded) {
            seen.insert(folded);
            output.push(current);
        }
    }
    for entry in &hosted.mods {
        let folded = entry.id.to_ascii_lowercase();
        if seen.insert(folded.clone()) && !hidden.contains(&folded) {
            output.push(catalog_item(entry.clone()));
        }
    }
    apply_bundled_display_policy(&mut output);
    apply_authoritative_trust(&mut output);
    apply_default_catalog_order(&mut output);
    state.display = output;
    state.hosted_ids = hosted.mods.iter().map(|entry| entry.id.clone()).collect();
    state.hosted_catalog = Some(hosted.clone());
    state
}

#[tauri::command]
pub async fn get_catalog() -> Result<Vec<CatalogListItem>, String> {
    blocking(|| {
        let _guard = lock_mutations()?;
        display_catalog()
    })
    .await
}

#[tauri::command]
pub async fn add_catalog_mod(
    repo: String,
    name: Option<String>,
) -> Result<Vec<CatalogListItem>, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
        let catalog = catalog()?;
        let entry = catalog_entry_for(&catalog, &repo).cloned();
        let canonical_id = entry
            .as_ref()
            .map(|entry| entry.id.clone())
            .unwrap_or_else(|| repo.clone());
        let effective_repo = entry
            .as_ref()
            .map(|entry| entry.repo.clone().unwrap_or_else(|| entry.id.clone()))
            .unwrap_or_else(|| repo.clone());
        let display_name = name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .or_else(|| entry.as_ref().map(|entry| entry.name.clone()))
            .unwrap_or_else(|| repo.clone());
        let summary = entry
            .as_ref()
            .map(|entry| entry.summary.clone())
            .unwrap_or_default();
        let tags = entry
            .as_ref()
            .map(|entry| entry.tags.clone())
            .unwrap_or_default();
        ensure_display_catalog(
            &repo,
            &canonical_id,
            &effective_repo,
            &display_name,
            summary,
            tags,
        )?;
        display_catalog()
    })
    .await
}

#[tauri::command]
pub async fn remove_catalog_mod(id: String) -> Result<Vec<CatalogListItem>, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let mut state = load_catalog_state()?;
        state
            .display
            .retain(|item| !item.id.eq_ignore_ascii_case(&id));
        if state
            .hosted_ids
            .iter()
            .any(|hosted| hosted.eq_ignore_ascii_case(&id))
            && !state
                .hidden_hosted_ids
                .iter()
                .any(|hidden| hidden.eq_ignore_ascii_case(&id))
        {
            state.hidden_hosted_ids.push(id);
        }
        save_catalog_state(&state)?;
        Ok(state.display)
    })
    .await
}

#[tauri::command]
pub async fn reorder_catalog(ids: Vec<String>) -> Result<Vec<CatalogListItem>, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let mut state = load_catalog_state()?;
        let current = &state.display;
        let mut seen = HashSet::new();
        let mut output = Vec::with_capacity(current.len());
        for id in ids {
            if !seen.insert(id.to_ascii_lowercase()) {
                return Err("catalog order contains duplicate ids".into());
            }
            let item = current
                .iter()
                .find(|item| item.id == id)
                .ok_or_else(|| format!("unknown catalog id {id}"))?;
            output.push(item.clone());
        }
        for item in current {
            if seen.insert(item.id.to_ascii_lowercase()) {
                output.push(item.clone());
            }
        }
        state.display = output;
        save_catalog_state(&state)?;
        Ok(state.display)
    })
    .await
}

#[tauri::command]
pub async fn refresh_catalog() -> Result<usize, String> {
    blocking(|| {
        let _guard = lock_mutations()?;
        let text = http()?
            .get_text(DEFAULT_CATALOG_URL)
            .map_err(|error| error.to_string())?;
        if text.len() as u64 > MAX_CATALOG_BYTES {
            return Err("hosted catalog exceeds its size limit".into());
        }
        let hosted = parse(&text).map_err(|error| format!("invalid catalog: {error}"))?;
        let state = reconcile_hosted(load_catalog_state()?, &hosted);
        save_catalog_state(&state)
            .map_err(|error| format!("could not publish hosted catalog: {error}"))?;
        Ok(hosted.mods.len())
    })
    .await
}

// ---------- profiles ----------

#[tauri::command]
pub async fn list_profiles() -> Result<Vec<ProfileRecord>, String> {
    blocking(|| {
        let _guard = lock_mutations()?;
        store()?.list().map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
pub async fn save_profile(mut profile: ProfileRecord) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        validate_profile_id(&profile.id)?;
        profile.name = profile.name.trim().to_string();
        profile.crew_color = profile.crew_color.trim().to_string();
        if profile.name.is_empty() || profile.crew_color.is_empty() {
            return Err("profile name and crew color are required".into());
        }
        let saved = settings::load().map_err(|error| error.to_string())?;
        let proposed_instance = match profile.game_instance_id.as_deref() {
            Some(instance_id) => Some(
                saved
                    .game_instances
                    .iter()
                    .find(|instance| instance.id == instance_id)
                    .ok_or("profile refers to an unknown game instance")?,
            ),
            None => saved.game_instances.first(),
        };
        if let Some(existing) = store()?
            .load(&profile.id)
            .map_err(|error| error.to_string())?
        {
            if !existing.mods.is_empty() {
                let proposed_instance = proposed_instance.ok_or(
                    "This populated profile needs a saved compatible Among Us instance before it can be changed.",
                )?;
                let existing_instance = match existing.game_instance_id.as_deref() {
                    Some(instance_id) => saved
                        .game_instances
                        .iter()
                        .find(|instance| instance.id == instance_id),
                    None => saved.game_instances.first(),
                };
                let existing_instance = existing_instance.ok_or(
                    "The populated profile's prior game instance is missing, so its asset architecture cannot be verified. Create a new profile and re-resolve its mods.",
                )?;
                if existing_instance.arch != proposed_instance.arch
                    || existing_instance.store != proposed_instance.store
                {
                    return Err(
                        "This profile already contains architecture/store-specific assets. Keep it on a compatible Among Us instance, or create a new profile and re-resolve its mods."
                            .into(),
                    );
                }
            }
            profile.game_build = existing.game_build;
            profile.mods = existing.mods;
        } else if !profile.mods.is_empty() {
            return Err("new profiles cannot contain backend-owned mods".into());
        }
        store()?.save(&profile).map_err(|error| error.to_string())?;
        Ok(profile)
    })
    .await
}

#[tauri::command]
pub async fn delete_profile(id: String) -> Result<(), String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        validate_profile_id(&id)?;
        let mut saved = settings::load().map_err(|error| error.to_string())?;
        let original = saved.clone();
        let clears_active = saved
            .active_profile
            .as_deref()
            .is_some_and(|active| active == id.as_str());
        if clears_active {
            saved.active_profile = None;
            settings::save(&saved).map_err(|error| error.to_string())?;
        }
        if let Err(error) = delete_profile_transaction(&settings::profiles_root(), &id) {
            if clears_active {
                return match settings::save(&original) {
                    Ok(()) => Err(error),
                    Err(rollback) => Err(format!(
                        "{error}; additionally could not restore the active-profile setting: {rollback}"
                    )),
                };
            }
            return Err(error);
        }
        Ok(())
    })
    .await
}

// ---------- lobby codes ----------

#[tauri::command]
pub async fn encode_lobby_code(profile: ProfileRecord) -> Result<String, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let profile_store = store()?;
        let mut authoritative = profile_store
            .load(&profile.id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let saved = settings::load().map_err(|error| error.to_string())?;
        let instance = authoritative
            .game_instance_id
            .as_deref()
            .and_then(|id| {
                saved
                    .game_instances
                    .iter()
                    .find(|instance| instance.id == id)
            })
            .or_else(|| saved.game_instances.first());
        authoritative.game_build =
            instance.and_then(|instance| game::detect_build(Path::new(&instance.path)));
        authoritative.levelimposter_maps = list_levelimposter_maps_impl(&authoritative.id)?;
        profile_store
            .save(&authoritative)
            .map_err(|error| error.to_string())?;
        codec::encode(&profile::to_manifest(&authoritative)).map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
pub fn preview_code(code: String, installed: Vec<(String, String)>) -> Result<Preview, String> {
    preview(&code, &bundled_catalog(), &installed).map_err(|error| error.to_string())
}

// ---------- release/file picker ----------

#[tauri::command]
pub async fn list_releases(repo: String) -> Result<Vec<Release>, String> {
    blocking(move || {
        let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
        resolver::fetch_releases(&http()?, &repo, 20).map_err(|error| error.to_string())
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModInstallOption {
    tag: String,
    asset_name: String,
    size: u64,
}

fn install_options(
    releases: Vec<Release>,
    rules: &AssetRules,
    arch: &str,
) -> Vec<ModInstallOption> {
    let mut options = Vec::new();
    for release in releases {
        let preferred =
            resolver::pick_asset(&release, rules, arch).map(|asset| asset.name.as_str());
        let mut assets: Vec<_> = release
            .assets
            .iter()
            .filter(|asset| {
                let lower = asset.name.to_ascii_lowercase();
                lower.ends_with(".dll")
                    || (lower.ends_with(".zip") && preferred == Some(asset.name.as_str()))
            })
            .collect();
        assets.sort_by_key(|asset| preferred != Some(asset.name.as_str()));
        options.extend(assets.into_iter().map(|asset| ModInstallOption {
            tag: release.tag.clone(),
            asset_name: asset.name.clone(),
            size: asset.size.bytes(),
        }));
    }
    options
}

fn install_options_for_profile(
    releases: Vec<Release>,
    repo: &str,
    rules: &AssetRules,
    arch: &str,
    store: Store,
    runtime: Runtime,
) -> Result<Vec<ModInstallOption>, String> {
    if !is_tou_mira(repo) {
        return Ok(install_options(releases, rules, arch));
    }
    let mut options = Vec::new();
    for release in releases {
        if let Some(asset) = pick_profile_asset(&release, repo, rules, arch, store, runtime)? {
            let asset_name = asset.name.clone();
            let size = asset.size.bytes();
            options.push(ModInstallOption {
                tag: release.tag,
                asset_name,
                size,
            });
        }
    }
    Ok(options)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModInstallSelection {
    id: String,
    repo: String,
    name: String,
    tag: String,
    asset_name: String,
    managed: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationProgress {
    phase: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes_received: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes_total: Option<u64>,
}

#[derive(Clone)]
struct ProgressReporter {
    channel: Channel<OperationProgress>,
}

impl ProgressReporter {
    fn new(channel: Channel<OperationProgress>) -> Self {
        Self { channel }
    }

    fn stage(&self, phase: &str, message: impl Into<String>) {
        let _ = self.channel.send(OperationProgress {
            phase: phase.to_string(),
            message: message.into(),
            bytes_received: None,
            bytes_total: None,
        });
    }

    fn download(&self, message: &str, bytes_received: u64, bytes_total: Option<u64>) {
        let _ = self.channel.send(OperationProgress {
            phase: "downloading".into(),
            message: message.to_string(),
            bytes_received: Some(bytes_received),
            bytes_total,
        });
    }
}

struct ProgressHttp {
    inner: UreqHttp,
    reporter: ProgressReporter,
}

impl ProgressHttp {
    fn new(inner: UreqHttp, reporter: ProgressReporter) -> Self {
        Self { inner, reporter }
    }
}

impl Http for ProgressHttp {
    fn get_text(&self, url: &str) -> Result<String, perfect_sync_core::resolver::ResolveError> {
        self.inner.get_text(url)
    }
    fn get_text_with_url(
        &self,
        url: &str,
    ) -> Result<perfect_sync_core::resolver::TextResponse, perfect_sync_core::resolver::ResolveError>
    {
        self.inner.get_text_with_url(url)
    }

    fn head(
        &self,
        url: &str,
    ) -> Result<perfect_sync_core::resolver::HeadResponse, perfect_sync_core::resolver::ResolveError>
    {
        self.inner.head(url)
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, perfect_sync_core::resolver::ResolveError> {
        let label = url::Url::parse(url)
            .ok()
            .and_then(|parsed| {
                parsed
                    .path_segments()
                    .and_then(|mut segments| segments.next_back())
                    .map(str::to_string)
            })
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "download".into())
            .replace("%20", " ");
        let message = format!("Downloading {label}");
        let mut last_emit = None;
        let mut last_received = 0_u64;
        let mut last_total = None;
        let result = self
            .inner
            .get_bytes_with_progress(url, &mut |received, total| {
                let now = Instant::now();
                let should_emit = received == 0
                    || total.is_some_and(|expected| received >= expected)
                    || last_emit.is_none_or(|last: Instant| {
                        now.duration_since(last) >= Duration::from_millis(100)
                    });
                last_received = received;
                last_total = total;
                if should_emit {
                    self.reporter.download(&message, received, total);
                    last_emit = Some(now);
                }
            });
        if let Ok(bytes) = &result {
            let received = bytes.len() as u64;
            if received != last_received {
                self.reporter.download(&message, received, last_total);
            }
        }
        result
    }
}

#[tauri::command]
pub async fn list_install_options(
    repo: String,
    profile_id: String,
) -> Result<Vec<ModInstallOption>, String> {
    blocking(move || {
        validate_profile_id(&profile_id)?;
        let arch = profile_arch(&profile_id)?;
        let (store, runtime) = profile_store_runtime(&profile_id)?;
        let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
        let catalog = catalog()?;
        let rules = catalog_entry_for(&catalog, &repo)
            .map(|entry| entry.asset_rules.clone())
            .unwrap_or_else(default_rules);
        let releases =
            resolver::fetch_releases(&http()?, &repo, 50).map_err(|error| error.to_string())?;
        install_options_for_profile(releases, &repo, &rules, &arch, store, runtime)
    })
    .await
}

#[tauri::command]
pub async fn list_tou_setup_options(
    arch: String,
    store: Store,
    runtime: Runtime,
) -> Result<Vec<ModInstallOption>, String> {
    blocking(move || {
        if !matches!(arch.as_str(), "x86" | "x64") {
            return Err("Among Us executable architecture is unsupported".into());
        }
        let catalog = catalog()?;
        let entry = catalog
            .get(TOU_MIRA_ID)
            .ok_or("Town of Us is missing from the trusted catalog")?;
        let repo = entry.repo.as_deref().unwrap_or(&entry.id);
        let releases =
            resolver::fetch_releases(&http()?, repo, 50).map_err(|error| error.to_string())?;
        install_options_for_profile(releases, repo, &entry.asset_rules, &arch, store, runtime)
    })
    .await
}

#[tauri::command]
pub async fn install_assets(
    profile_id: String,
    selections: Vec<ModInstallSelection>,
    confirmed: bool,
    on_progress: Channel<OperationProgress>,
) -> Result<ProfileRecord, String> {
    require_manual_install_confirmation(confirmed)?;
    blocking(move || {
        let _guard = lock_mutations()?;
        let reporter = ProgressReporter::new(on_progress);
        install_assets_impl(profile_id, selections, &reporter)
    })
    .await
}

#[tauri::command]
pub async fn install_local_mod(profile_id: String, path: String) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        install_local_mod_impl(&settings::profiles_root(), &profile_id, Path::new(&path))
    })
    .await
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelImposterMap {
    id: String,
    name: String,
    author_name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumbnail_url: Option<String>,
}

#[derive(Deserialize)]
struct LevelImposterCallback<T> {
    v: u32,
    #[serde(default)]
    error: String,
    data: Option<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LevelImposterMapMetadata {
    id: String,
    name: String,
    #[serde(default)]
    author_name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    is_public: bool,
    #[serde(default, rename = "downloadURL")]
    download_url: Option<String>,
    #[serde(default, rename = "thumbnailURL")]
    thumbnail_url: Option<String>,
}

#[derive(Deserialize)]
struct LevelImposterSearchResponse {
    hits: Vec<LevelImposterSearchHit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LevelImposterSearchHit {
    #[serde(rename = "objectID")]
    id: String,
    name: String,
    #[serde(default)]
    author_name: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "thumbnailURL")]
    thumbnail_url: Option<String>,
}

#[tauri::command]
pub async fn search_levelimposter_maps(query: String) -> Result<Vec<LevelImposterMap>, String> {
    blocking(move || search_levelimposter_maps_impl(&query)).await
}

#[tauri::command]
pub async fn fetch_levelimposter_banner(url: String) -> Result<String, String> {
    blocking(move || levelimposter_banner_data_url(&http()?, &url)).await
}

#[tauri::command]
pub async fn list_levelimposter_maps(profile_id: String) -> Result<Vec<String>, String> {
    blocking(move || list_levelimposter_maps_impl(&profile_id)).await
}

#[tauri::command]
pub async fn install_levelimposter_maps(
    profile_id: String,
    map_ids: Vec<String>,
    on_progress: Channel<OperationProgress>,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let reporter = ProgressReporter::new(on_progress);
        install_levelimposter_maps_impl(profile_id, map_ids, &reporter)
    })
    .await
}
#[tauri::command]
pub async fn remove_levelimposter_maps(
    profile_id: String,
    map_ids: Vec<String>,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        remove_levelimposter_maps_impl(profile_id, map_ids)
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
    confirmed: bool,
    on_progress: Channel<OperationProgress>,
) -> Result<ProfileRecord, String> {
    require_manual_install_confirmation(confirmed)?;
    blocking(move || {
        let _guard = lock_mutations()?;
        let reporter = ProgressReporter::new(on_progress);
        install_asset_impl(profile_id, repo, tag, asset_name, arch, &reporter)
    })
    .await
}

fn require_manual_install_confirmation(confirmed: bool) -> Result<(), String> {
    if confirmed {
        Ok(())
    } else {
        Err("Confirm the exact repository, release tag, and asset before installing.".into())
    }
}

fn catalog_entry_for<'a>(
    catalog: &'a Catalog,
    identity: &str,
) -> Option<&'a perfect_sync_core::catalog::CatalogEntry> {
    catalog.get(identity).or_else(|| {
        catalog.mods.iter().find(|entry| {
            entry
                .repo
                .as_deref()
                .is_some_and(|repo| repo.eq_ignore_ascii_case(identity))
        })
    })
}

fn is_managed_dependency(root_id: &str, candidate_id: &str) -> bool {
    !root_id.eq_ignore_ascii_case(candidate_id)
}

fn selected_dependencies(
    catalog: &Catalog,
    selected: &[String],
) -> Result<HashSet<String>, String> {
    let selected_folded: HashSet<String> =
        selected.iter().map(|id| id.to_ascii_lowercase()).collect();
    let mut dependencies = HashSet::new();
    for root in selected {
        for candidate in deps::resolve(catalog, std::slice::from_ref(root))
            .map_err(|error| error.to_string())?
            .ordered
        {
            if !candidate.eq_ignore_ascii_case(root)
                && selected_folded.contains(&candidate.to_ascii_lowercase())
            {
                dependencies.insert(candidate.to_ascii_lowercase());
            }
        }
    }
    Ok(dependencies)
}

fn validate_authoritative_dependencies(
    catalog: &Catalog,
    explicit_roots: &[String],
) -> Result<(), String> {
    let bundled = bundled_catalog();
    for root in explicit_roots {
        let resolved = deps::resolve(catalog, std::slice::from_ref(root))
            .map_err(|error| error.to_string())?;
        for candidate in &resolved.ordered {
            let is_root = candidate.eq_ignore_ascii_case(root);
            let hosted = catalog
                .get(candidate)
                .ok_or_else(|| format!("catalog dependency {candidate} is missing"))?;
            let Some(authoritative) = bundled.get(&hosted.id) else {
                if is_root {
                    // A custom explicit root was already manually confirmed by the user.
                    continue;
                }
                return Err(format!(
                    "Catalog dependency {} is not authorized by the bundled catalog.",
                    hosted.id
                ));
            };
            let hosted_repo = hosted.repo.as_deref().unwrap_or(&hosted.id);
            let authoritative_repo = authoritative.repo.as_deref().unwrap_or(&authoritative.id);
            if !hosted_repo.eq_ignore_ascii_case(authoritative_repo) {
                let role = if is_root { "root" } else { "dependency" };
                return Err(format!(
                    "Catalog {role} {} does not match its bundled authoritative identity.",
                    hosted.id
                ));
            }
        }
    }
    Ok(())
}

fn explicit_catalog_roots(record: &ProfileRecord, catalog: &Catalog) -> Vec<String> {
    record
        .mods
        .iter()
        .filter(|installed| !installed.managed)
        .filter_map(|installed| {
            catalog_entry_for(catalog, &installed.package_id).map(|entry| entry.id.clone())
        })
        .collect()
}

fn normalize_dependency_ownership(
    stage_root: &Path,
    profile_id: &str,
    record: &mut ProfileRecord,
    catalog: &Catalog,
) -> Result<(), String> {
    let explicit_roots: Vec<String> = record
        .mods
        .iter()
        .filter(|installed| !installed.managed)
        .filter_map(|installed| {
            catalog_entry_for(catalog, &installed.package_id).map(|entry| entry.id.clone())
        })
        .collect();
    let required: HashSet<String> = if explicit_roots.is_empty() {
        HashSet::new()
    } else {
        deps::resolve(catalog, &explicit_roots)
            .map_err(|error| error.to_string())?
            .ordered
            .into_iter()
            .map(|id| id.to_ascii_lowercase())
            .collect()
    };
    let roots: HashSet<String> = explicit_roots
        .into_iter()
        .map(|id| id.to_ascii_lowercase())
        .collect();
    let mut retained = Vec::with_capacity(record.mods.len());
    for mut installed in record.mods.drain(..) {
        let canonical = catalog_entry_for(catalog, &installed.package_id)
            .map(|entry| entry.id.to_ascii_lowercase());
        match canonical {
            Some(id) if roots.contains(&id) => {
                installed.managed = false;
                retained.push(installed);
            }
            Some(id) if required.contains(&id) => {
                installed.managed = true;
                retained.push(installed);
            }
            _ if installed.managed => {
                if let Some(file) = installed.file.as_deref() {
                    profile::remove_plugin(stage_root, profile_id, file)
                        .map_err(|error| error.to_string())?;
                }
            }
            _ => retained.push(installed),
        }
    }
    record.mods = retained;
    Ok(())
}

fn mod_position(record: &ProfileRecord, package_id: &str) -> Result<usize, String> {
    record
        .mods
        .iter()
        .position(|installed| installed.package_id.eq_ignore_ascii_case(package_id))
        .ok_or_else(|| "mod not found".to_string())
}

fn validate_mod_toggle(record: &ProfileRecord, package_id: &str) -> Result<usize, String> {
    if tou_bundle_dependency(package_id) && profile_has_tou_mira(record) {
        return Err(format!(
            "{package_id} is included in the Town of Us package and cannot be toggled separately."
        ));
    }
    mod_position(record, package_id)
}

fn remove_mod_from_record(
    stage_root: &Path,
    profile_id: &str,
    record: &mut ProfileRecord,
    catalog: &Catalog,
    package_id: &str,
) -> Result<(), String> {
    if tou_bundle_dependency(package_id) && profile_has_tou_mira(record) {
        return Err(format!(
            "{package_id} is included in the Town of Us package and cannot be removed separately."
        ));
    }
    let position = mod_position(record, package_id)?;
    let removed = record.mods.remove(position);
    if is_tou_mira(&removed.package_id) {
        profile::remove_tou_bundle(stage_root, profile_id).map_err(|error| error.to_string())?;
    }
    if let Some(file) = removed.file {
        profile::remove_plugin(stage_root, profile_id, &file).map_err(|error| error.to_string())?;
    }
    normalize_dependency_ownership(stage_root, profile_id, record, catalog)
}

#[derive(Clone, Copy)]
struct ReleaseAssetTarget<'a> {
    rules: Option<&'a AssetRules>,
    arch: &'a str,
    store: Store,
    runtime: Runtime,
}

fn selected_release_asset(
    http: &dyn Http,
    repo: &str,
    tag: &str,
    asset_name: &str,
    target: ReleaseAssetTarget<'_>,
) -> Result<ResolvedDownload, String> {
    let release =
        resolver::fetch_release_by_tag(http, repo, tag).map_err(|error| error.to_string())?;
    if is_tou_mira(repo) {
        let rules = target
            .rules
            .ok_or("Town of Us must use its authoritative catalog rules")?;
        let asset = pick_profile_asset(
            &release,
            repo,
            rules,
            target.arch,
            target.store,
            target.runtime,
        )?
        .ok_or_else(|| format!("Town of Us {tag} has no compatible full package"))?;
        return resolver::resolved_asset(http, &release, asset).map_err(|error| error.to_string());
    }

    let lower = asset_name.to_ascii_lowercase();
    if !lower.ends_with(".dll") && !lower.ends_with(".zip") {
        return Err("Only .dll files and catalog-selected .zip packages can be installed.".into());
    }
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .ok_or("selected file not found in that release")?;
    if lower.ends_with(".zip") {
        let selected = target
            .rules
            .and_then(|rules| resolver::pick_asset(&release, rules, target.arch))
            .is_some_and(|selected| selected.name == asset.name);
        if !selected {
            return Err(
                "Only the catalog-selected ZIP package for this game architecture can be installed."
                    .into(),
            );
        }
    }
    resolver::resolved_asset(http, &release, asset).map_err(|error| error.to_string())
}
fn staged_plugin_names(stage_root: &Path, profile_id: &str) -> Result<HashSet<String>, String> {
    let directory = stage_root.join(profile_id).join("BepInEx").join("plugins");
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut names = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "profile plugins contain a non-Unicode filename")?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) {
            return Err("profile plugins contain an unsupported filesystem entry".into());
        }
        if metadata.is_dir() && name.eq_ignore_ascii_case("LevelImposter") {
            continue;
        }
        if !metadata.is_file() {
            return Err("profile plugins contain an unsupported filesystem entry".into());
        }
        if !names.insert(name.to_ascii_lowercase()) {
            return Err("profile plugins contain case-colliding filenames".into());
        }
    }
    Ok(names)
}

struct InstallContext<'a> {
    stage_root: &'a Path,
    profile_id: &'a str,
    http: &'a dyn Http,
    catalog: &'a Catalog,
    arch: &'a str,
    store: Store,
    runtime: Runtime,
}

struct InstallRequest {
    package_id: String,
    name: String,
    repo: String,
    tags: Vec<ModTag>,
    managed: bool,
    resolved: ResolvedDownload,
}

fn tou_bundle_dependency(package_id: &str) -> bool {
    TOU_BUNDLED_DEPENDENCY_IDS
        .iter()
        .any(|dependency| dependency.eq_ignore_ascii_case(package_id))
}

fn profile_has_tou_mira(record: &ProfileRecord) -> bool {
    record
        .mods
        .iter()
        .any(|installed| is_tou_mira(&installed.package_id))
}

fn profile_uses_tou_mira(record: &ProfileRecord) -> bool {
    record
        .mods
        .iter()
        .any(|installed| installed.enabled && is_tou_mira(&installed.package_id))
}

fn safe_nonempty_plugin(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0 && !is_reparse(&metadata))
}

fn reuse_installed_dependency(
    context: &InstallContext<'_>,
    record: &mut ProfileRecord,
    id: &str,
    requirements: &[String],
) -> Result<bool, String> {
    let Some(position) = record
        .mods
        .iter()
        .position(|installed| installed.package_id.eq_ignore_ascii_case(id))
    else {
        return Ok(false);
    };
    let installed = record.mods[position].clone();
    if !requirements.is_empty()
        && !perfect_sync_core::version::satisfies_all(&installed.version, requirements)
    {
        if !installed.managed {
            let name = context
                .catalog
                .get(id)
                .map_or(id, |entry| entry.name.as_str());
            return Err(format!(
                "{} {} does not satisfy required version {}.",
                name,
                installed.version,
                requirements.join(", ")
            ));
        }
        return Ok(false);
    }
    let Some(file) = installed.file.as_deref() else {
        return Ok(false);
    };
    if profile::validate_dll_name(file).is_err() {
        return Ok(false);
    }
    if !installed.enabled {
        profile::set_plugin_enabled(context.stage_root, context.profile_id, file, true)
            .map_err(|error| error.to_string())?;
        record.mods[position].enabled = true;
    }
    let plugins = context
        .stage_root
        .join(context.profile_id)
        .join("BepInEx")
        .join("plugins");
    if !safe_nonempty_plugin(&plugins.join(file)) {
        return Ok(false);
    }
    if is_tou_mira(&installed.package_id) {
        let Ok(files) = profile::tou_bundle_files(context.stage_root, context.profile_id) else {
            return Ok(false);
        };
        return Ok(!files.is_empty()
            && files.iter().all(|relative| {
                safe_nonempty_plugin(
                    &context
                        .stage_root
                        .join(context.profile_id)
                        .join("BepInEx")
                        .join(relative),
                )
            }));
    }
    Ok(true)
}

fn tou_shadowed_plugin_files(record: &ProfileRecord) -> Vec<String> {
    if !profile_uses_tou_mira(record) {
        return Vec::new();
    }
    let mut names: Vec<String> = TOU_BUNDLED_PLUGIN_FILES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    for file in record
        .mods
        .iter()
        .filter(|installed| tou_bundle_dependency(&installed.package_id))
        .filter_map(|installed| installed.file.as_deref())
    {
        if !names.iter().any(|name| name.eq_ignore_ascii_case(file)) {
            names.push(file.to_string());
        }
    }
    names
}

fn install_tou_record(
    context: &InstallContext<'_>,
    record: &mut ProfileRecord,
    request: InstallRequest,
) -> Result<(), String> {
    if !request
        .resolved
        .asset_name
        .to_ascii_lowercase()
        .ends_with(".zip")
    {
        return Err("Town of Us must be installed from its complete release ZIP".into());
    }
    let bytes =
        download_resolved(context.http, &request.resolved).map_err(|error| error.to_string())?;
    let previous = record.mods.iter().position(|installed| {
        installed
            .package_id
            .eq_ignore_ascii_case(&request.package_id)
    });
    let previous_versions = previous
        .map(|position| record.mods[position].versions.clone())
        .unwrap_or_default();
    let previous_file = previous.and_then(|position| record.mods[position].file.clone());

    profile::remove_tou_bundle(context.stage_root, context.profile_id)
        .map_err(|error| error.to_string())?;
    if let Some(file) = previous_file {
        profile::remove_plugin(context.stage_root, context.profile_id, &file)
            .map_err(|error| error.to_string())?;
    }

    let file =
        profile::install_tou_bundle_zip_bytes(context.stage_root, context.profile_id, &bytes)
            .map_err(|error| error.to_string())?;
    cache_tou_package(
        &request.resolved.version,
        &request.resolved.asset_name,
        &bytes,
    )?;
    let previous = record.mods.iter().position(|installed| {
        installed
            .package_id
            .eq_ignore_ascii_case(&request.package_id)
    });
    let preserve_explicit_root =
        request.managed && previous.is_some_and(|position| !record.mods[position].managed);
    let mut versions = previous_versions;
    if !versions.contains(&request.resolved.version) {
        versions.insert(0, request.resolved.version.clone());
    }
    let installed = InstalledMod {
        package_id: request.package_id,
        name: request.name,
        repo: Some(request.repo),
        version: request.resolved.version.clone(),
        versions,
        enabled: true,
        source: ModSource::Github,
        tags: request.tags,
        managed: request.managed && !preserve_explicit_root,
        update: None,
        file: Some(file),
        asset: Some(request.resolved.asset_name),
    };
    if let Some(position) = previous {
        record.mods[position] = installed;
    } else {
        record.mods.push(installed);
    }
    Ok(())
}

fn install_record(
    context: &InstallContext<'_>,
    record: &mut ProfileRecord,
    request: InstallRequest,
) -> Result<(), String> {
    if is_tou_mira(&request.package_id) {
        return install_tou_record(context, record, request);
    }
    if tou_bundle_dependency(&request.package_id) && profile_has_tou_mira(record) {
        return Err(format!(
            "{} is auto-included at the release-matched version by Town of Us - Mira",
            request.name
        ));
    }
    let InstallRequest {
        package_id,
        name,
        repo,
        tags,
        managed,
        resolved,
    } = request;
    let preserve_explicit_root = managed
        && record.mods.iter().any(|installed| {
            installed.package_id.eq_ignore_ascii_case(&package_id) && !installed.managed
        });
    if let Some(existing) = record
        .mods
        .iter()
        .find(|installed| installed.package_id.eq_ignore_ascii_case(&package_id))
        .and_then(|installed| installed.file.clone())
    {
        profile::remove_plugin(context.stage_root, context.profile_id, &existing)
            .map_err(|error| error.to_string())?;
    }
    let preexisting = staged_plugin_names(context.stage_root, context.profile_id)?;
    let expected_dll = context
        .catalog
        .get(&package_id)
        .and_then(|entry| entry.asset_rules.dll_name.as_deref());
    let file = install_resolved(
        context.stage_root,
        context.profile_id,
        context.http,
        &resolved,
        expected_dll,
    )?;
    let lower = file.to_ascii_lowercase();
    if preexisting.contains(&lower) || preexisting.contains(&format!("{lower}.disabled")) {
        return Err(format!(
            "plugin file {file} would overwrite a file not owned by this package"
        ));
    }
    if record.mods.iter().any(|installed| {
        !installed.package_id.eq_ignore_ascii_case(&package_id)
            && installed
                .file
                .as_deref()
                .is_some_and(|owned| owned.eq_ignore_ascii_case(&file))
    }) {
        return Err(format!(
            "plugin file {file} is already owned by another installed package"
        ));
    }
    let previous = record
        .mods
        .iter()
        .position(|installed| installed.package_id.eq_ignore_ascii_case(&package_id));
    let mut versions = previous
        .map(|position| record.mods[position].versions.clone())
        .unwrap_or_default();
    if !versions.contains(&resolved.version) {
        versions.insert(0, resolved.version.clone());
    }
    let installed = InstalledMod {
        package_id,
        name,
        repo: Some(repo),
        version: resolved.version.clone(),
        versions,
        enabled: true,
        source: ModSource::Github,
        tags,
        managed: managed && !preserve_explicit_root,
        update: None,
        file: Some(file),
        asset: Some(resolved.asset_name),
    };
    if let Some(position) = previous {
        record.mods[position] = installed;
    } else {
        record.mods.push(installed);
    }
    Ok(())
}

fn install_catalog_latest(
    context: &InstallContext<'_>,
    record: &mut ProfileRecord,
    id: &str,
    managed: bool,
    requirements: &[String],
) -> Result<(), String> {
    let entry = context
        .catalog
        .get(id)
        .ok_or("catalog dependency is missing")?;
    if managed && reuse_installed_dependency(context, record, &entry.id, requirements)? {
        return Ok(());
    }
    let repo = entry
        .repo
        .clone()
        .or_else(|| resolver::parse_repo(&entry.id))
        .ok_or_else(|| format!("cannot resolve source for {}", entry.id))?;
    let resolved = if requirements.is_empty() {
        resolve_profile_latest(
            context.http,
            &repo,
            &entry.asset_rules,
            context.arch,
            context.store,
            context.runtime,
        )?
    } else {
        let releases =
            resolver::fetch_releases(context.http, &repo, 20).map_err(|error| error.to_string())?;
        let mut selected = None;
        for release in releases {
            if !perfect_sync_core::version::satisfies_all(&release.tag, requirements) {
                continue;
            }
            let Some(asset) = pick_profile_asset(
                &release,
                &repo,
                &entry.asset_rules,
                context.arch,
                context.store,
                context.runtime,
            )?
            else {
                continue;
            };
            selected = Some(
                resolver::resolved_asset(context.http, &release, asset)
                    .map_err(|error| error.to_string())?,
            );
            break;
        }
        selected.ok_or_else(|| {
            format!(
                "{} has no recent release satisfying {}.",
                entry.name,
                requirements.join(", ")
            )
        })?
    };
    install_record(
        context,
        record,
        InstallRequest {
            package_id: entry.id.clone(),
            name: entry.name.clone(),
            repo,
            tags: entry.tags.clone(),
            managed,
            resolved,
        },
    )
}

fn install_local_mod_into_record(
    profiles_root: &Path,
    profile_id: &str,
    record: &mut ProfileRecord,
    source: &Path,
) -> Result<(), String> {
    if !source.is_absolute() {
        return Err("Choose an absolute path to a local .dll file.".into());
    }
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Local mod path has no portable file name.")?;
    profile::validate_dll_name(file_name).map_err(|error| error.to_string())?;
    let display_name = source
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("Local mod path has no portable file name.")?
        .to_string();
    let package_id = format!("local/{}", file_name.to_ascii_lowercase());
    if record.mods.iter().any(|installed| {
        !installed.package_id.eq_ignore_ascii_case(&package_id)
            && installed
                .file
                .as_deref()
                .is_some_and(|owned| owned.eq_ignore_ascii_case(file_name))
    }) {
        return Err(format!(
            "plugin file {file_name} is already owned by another installed package"
        ));
    }

    let previous = record
        .mods
        .iter()
        .position(|installed| installed.package_id.eq_ignore_ascii_case(&package_id));
    if let Some(existing) = previous.and_then(|position| record.mods[position].file.as_deref()) {
        profile::remove_plugin(profiles_root, profile_id, existing)
            .map_err(|error| error.to_string())?;
    }
    let preexisting = staged_plugin_names(profiles_root, profile_id)?;
    let folded_file = file_name.to_ascii_lowercase();
    if preexisting.contains(&folded_file)
        || preexisting.contains(&format!("{folded_file}.disabled"))
    {
        return Err(format!(
            "plugin file {file_name} would overwrite a file not owned by this package"
        ));
    }

    profile::install_plugin_dll(profiles_root, profile_id, source)
        .map_err(|error| error.to_string())?;
    let installed = InstalledMod {
        package_id,
        name: display_name,
        repo: None,
        version: "local".into(),
        versions: vec!["local".into()],
        enabled: true,
        source: ModSource::File,
        tags: Vec::new(),
        managed: false,
        update: None,
        file: Some(file_name.to_string()),
        asset: Some(file_name.to_string()),
    };
    if let Some(position) = previous {
        record.mods[position] = installed;
    } else {
        record.mods.push(installed);
    }
    Ok(())
}

fn install_local_mod_impl(
    profiles_root: &Path,
    profile_id: &str,
    source: &Path,
) -> Result<ProfileRecord, String> {
    validate_profile_id(profile_id)?;
    profile_transaction(profiles_root, profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        install_local_mod_into_record(stage_root, profile_id, &mut record, source)?;
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}

fn install_asset_impl(
    profile_id: String,
    repo: String,
    tag: String,
    asset_name: String,
    arch: String,
    reporter: &ProgressReporter,
) -> Result<ProfileRecord, String> {
    reporter.stage("preparing", "Checking profile and dependencies");
    validate_profile_id(&profile_id)?;
    let _ = arch;
    let arch = profile_arch(&profile_id)?;
    let (store, runtime) = profile_store_runtime(&profile_id)?;
    let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
    let catalog = catalog()?;
    let root_entry = catalog_entry_for(&catalog, &repo).cloned();
    let (ordered, requirements) = match root_entry.as_ref() {
        Some(entry) => {
            let plan = deps::resolve(&catalog, std::slice::from_ref(&entry.id))
                .map_err(|error| error.to_string())?;
            (plan.ordered, plan.requirements)
        }
        None => (Vec::new(), HashMap::new()),
    };
    let root = settings::profiles_root();
    profile_transaction(&root, &profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let mut explicit_roots = explicit_catalog_roots(&record, &catalog);
        if let Some(entry) = root_entry.as_ref() {
            if !explicit_roots
                .iter()
                .any(|root| root.eq_ignore_ascii_case(&entry.id))
            {
                explicit_roots.push(entry.id.clone());
            }
        }
        validate_authoritative_dependencies(&catalog, &explicit_roots)?;
        reporter.stage("resolving", "Resolving exact release files");
        let http = ProgressHttp::new(http()?, reporter.clone());
        let install = InstallContext {
            stage_root,
            profile_id: &profile_id,
            http: &http,
            catalog: &catalog,
            arch: &arch,
            store,
            runtime,
        };
        for dependency in ordered.iter().filter(|id| {
            root_entry
                .as_ref()
                .is_none_or(|root| is_managed_dependency(&root.id, id))
        }) {
            install_catalog_latest(
                &install,
                &mut record,
                dependency,
                true,
                requirements.get(dependency).map_or(&[][..], Vec::as_slice),
            )?;
        }
        let resolved = selected_release_asset(
            &http,
            &repo,
            &tag,
            &asset_name,
            ReleaseAssetTarget {
                rules: root_entry.as_ref().map(|entry| &entry.asset_rules),
                arch: &arch,
                store,
                runtime,
            },
        )?;
        install_record(
            &install,
            &mut record,
            InstallRequest {
                package_id: root_entry
                    .as_ref()
                    .map(|entry| entry.id.clone())
                    .unwrap_or_else(|| repo.clone()),
                name: root_entry
                    .as_ref()
                    .map(|entry| entry.name.clone())
                    .unwrap_or_else(|| repo.clone()),
                repo: repo.clone(),
                tags: root_entry
                    .as_ref()
                    .map(|entry| entry.tags.clone())
                    .unwrap_or_default(),
                managed: false,
                resolved,
            },
        )?;
        reporter.stage("finalizing", "Verifying and saving the profile");
        normalize_dependency_ownership(stage_root, &profile_id, &mut record, &catalog)?;
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}

fn install_assets_impl(
    profile_id: String,
    selections: Vec<ModInstallSelection>,
    reporter: &ProgressReporter,
) -> Result<ProfileRecord, String> {
    reporter.stage(
        "preparing",
        format!(
            "Checking {} selected file{}",
            selections.len(),
            if selections.len() == 1 { "" } else { "s" }
        ),
    );
    validate_profile_id(&profile_id)?;
    if selections.is_empty() || selections.len() > 64 {
        return Err("Select between 1 and 64 mods and dependencies.".into());
    }
    let arch = profile_arch(&profile_id)?;
    let (store, runtime) = profile_store_runtime(&profile_id)?;
    let catalog = catalog()?;
    let mut seen = HashSet::with_capacity(selections.len());
    let mut prepared = Vec::with_capacity(selections.len());
    let mut selected_catalog_roots = Vec::new();
    for selection in selections {
        let repo = resolver::parse_repo(&selection.repo).ok_or("invalid repo or URL")?;
        if !seen.insert(repo.to_ascii_lowercase()) {
            return Err(format!("Repository {repo} was selected more than once."));
        }
        let name = selection.name.trim();
        if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
            return Err("Mod names must be 1..=128 non-control characters.".into());
        }
        let catalog_entry = catalog_entry_for(&catalog, &selection.id)
            .or_else(|| catalog_entry_for(&catalog, &repo))
            .cloned();
        if let Some(entry) = catalog_entry.as_ref() {
            let authoritative_repo = entry.repo.as_deref().unwrap_or(&entry.id);
            if !authoritative_repo.eq_ignore_ascii_case(&repo) {
                return Err(format!(
                    "Catalog entry {} does not match repository {repo}.",
                    entry.id
                ));
            }
            if !selection.managed {
                selected_catalog_roots.push(entry.id.clone());
            }
        } else if selection.managed {
            return Err("Only bundled catalog entries can be auto-managed dependencies.".into());
        }
        prepared.push((selection, repo, catalog_entry));
    }

    let plan = if selected_catalog_roots.is_empty() {
        deps::Resolved {
            ordered: Vec::new(),
            requirements: HashMap::new(),
        }
    } else {
        deps::resolve(&catalog, &selected_catalog_roots).map_err(|error| error.to_string())?
    };
    let ordered = &plan.ordered;
    let selected_root_ids: HashSet<String> = selected_catalog_roots
        .iter()
        .map(|id| id.to_ascii_lowercase())
        .collect();
    let allowed_ids: HashSet<String> = ordered.iter().map(|id| id.to_ascii_lowercase()).collect();
    let selected_catalog_ids: HashSet<String> = prepared
        .iter()
        .filter_map(|(_, _, entry)| entry.as_ref())
        .map(|entry| entry.id.to_ascii_lowercase())
        .collect();
    for (selection, _, entry) in &prepared {
        if selection.managed {
            let id = entry.as_ref().unwrap().id.to_ascii_lowercase();
            if selected_root_ids.contains(&id) || !allowed_ids.contains(&id) {
                return Err(format!(
                    "{} is not a dependency of the selected mods.",
                    entry.as_ref().unwrap().name
                ));
            }
        }
    }
    for (selection, _, entry) in &prepared {
        let Some(entry) = entry else {
            continue;
        };
        let Some(requirements) = plan.requirements.get(&entry.id) else {
            continue;
        };
        if !perfect_sync_core::version::satisfies_all(&selection.tag, requirements) {
            return Err(format!(
                "{} {} does not satisfy required version {}.",
                entry.name,
                selection.tag,
                requirements.join(", ")
            ));
        }
    }
    let omitted_dependencies: Vec<String> = ordered
        .iter()
        .filter(|id| {
            let folded = id.to_ascii_lowercase();
            !selected_root_ids.contains(&folded) && !selected_catalog_ids.contains(&folded)
        })
        .cloned()
        .collect();
    let order: HashMap<String, usize> = ordered
        .iter()
        .enumerate()
        .map(|(position, id)| (id.to_ascii_lowercase(), position))
        .collect();
    prepared.sort_by_key(|(_, _, entry)| {
        entry
            .as_ref()
            .and_then(|entry| order.get(&entry.id.to_ascii_lowercase()))
            .copied()
            .unwrap_or(usize::MAX)
    });

    let root = settings::profiles_root();
    profile_transaction(&root, &profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let previous_roots = explicit_catalog_roots(&record, &catalog);
        let other_roots: Vec<String> = previous_roots
            .iter()
            .filter(|id| !selected_root_ids.contains(&id.to_ascii_lowercase()))
            .cloned()
            .collect();
        let other_required: HashSet<String> = if other_roots.is_empty() {
            HashSet::new()
        } else {
            deps::resolve(&catalog, &other_roots)
                .map_err(|error| error.to_string())?
                .ordered
                .into_iter()
                .map(|id| id.to_ascii_lowercase())
                .collect()
        };
        let mut explicit_roots = previous_roots;
        for id in &selected_catalog_roots {
            if !explicit_roots
                .iter()
                .any(|root| root.eq_ignore_ascii_case(id))
            {
                explicit_roots.push(id.clone());
            }
        }
        validate_authoritative_dependencies(&catalog, &explicit_roots)?;
        reporter.stage("resolving", "Resolving exact releases and dependencies");
        let http = ProgressHttp::new(http()?, reporter.clone());
        let install = InstallContext {
            stage_root,
            profile_id: &profile_id,
            http: &http,
            catalog: &catalog,
            arch: &arch,
            store,
            runtime,
        };
        for (selection, repo, catalog_entry) in &prepared {
            if selection.managed {
                let entry = catalog_entry.as_ref().unwrap();
                let requirements = plan
                    .requirements
                    .get(&entry.id)
                    .map_or(&[][..], Vec::as_slice);
                if reuse_installed_dependency(&install, &mut record, &entry.id, requirements)? {
                    continue;
                }
            }
            let resolved = selected_release_asset(
                &http,
                repo,
                &selection.tag,
                &selection.asset_name,
                ReleaseAssetTarget {
                    rules: catalog_entry.as_ref().map(|entry| &entry.asset_rules),
                    arch: &arch,
                    store,
                    runtime,
                },
            )?;
            install_record(
                &install,
                &mut record,
                InstallRequest {
                    package_id: catalog_entry
                        .as_ref()
                        .map(|entry| entry.id.clone())
                        .unwrap_or_else(|| repo.clone()),
                    name: catalog_entry
                        .as_ref()
                        .map(|entry| entry.name.clone())
                        .unwrap_or_else(|| selection.name.trim().to_string()),
                    repo: repo.clone(),
                    tags: catalog_entry
                        .as_ref()
                        .map(|entry| entry.tags.clone())
                        .unwrap_or_default(),
                    managed: selection.managed,
                    resolved,
                },
            )?;
        }
        reporter.stage("finalizing", "Verifying files and saving the profile");
        normalize_dependency_ownership(stage_root, &profile_id, &mut record, &catalog)?;
        for omitted in &omitted_dependencies {
            if other_required.contains(&omitted.to_ascii_lowercase()) {
                continue;
            }
            let position = record.mods.iter().position(|installed| {
                installed.managed
                    && catalog_entry_for(&catalog, &installed.package_id)
                        .is_some_and(|entry| entry.id.eq_ignore_ascii_case(omitted))
            });
            if let Some(position) = position {
                let removed = record.mods.remove(position);
                if let Some(file) = removed.file {
                    profile::remove_plugin(stage_root, &profile_id, &file)
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}

fn levelimposter_callback<T>(text: &str) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let response: LevelImposterCallback<T> = serde_json::from_str(text)
        .map_err(|error| format!("Invalid LevelImposter response: {error}"))?;
    if response.v != 1 {
        return Err(format!(
            "Unsupported LevelImposter API version {}.",
            response.v
        ));
    }
    if !response.error.is_empty() {
        return Err(response.error);
    }
    response
        .data
        .ok_or_else(|| "LevelImposter returned no map data.".to_string())
}

fn levelimposter_banner_data_url(http: &dyn Http, raw_url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(raw_url).map_err(|_| "Invalid LevelImposter banner URL.")?;
    let official_path = parsed
        .path()
        .starts_with("/v0/b/levelimposter-347807.appspot.com/o/maps%2F");
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("firebasestorage.googleapis.com")
        || !official_path
    {
        return Err("LevelImposter banner URL is not from the official map bucket.".into());
    }
    let bytes = http
        .get_bytes(parsed.as_str())
        .map_err(|error| error.to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_LEVELIMPOSTER_BANNER_BYTES {
        return Err("LevelImposter banner exceeds the size limit.".into());
    }
    let media_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return Err("LevelImposter banner is not a supported image.".into());
    };
    Ok(format!(
        "data:{media_type};base64,{}",
        BASE64_STANDARD.encode(bytes)
    ))
}

fn search_levelimposter_maps_impl(query: &str) -> Result<Vec<LevelImposterMap>, String> {
    let query = query.trim();
    if query.len() > 128 || query.chars().any(char::is_control) {
        return Err("Map searches must be at most 128 non-control characters.".into());
    }
    let http = http()?;
    if query.is_empty() {
        let text = http
            .get_text(&format!("{LEVELIMPOSTER_API}/maps/top"))
            .map_err(|error| error.to_string())?;
        let maps: Vec<LevelImposterMapMetadata> = levelimposter_callback(&text)?;
        return Ok(maps
            .into_iter()
            .filter(|map| valid_levelimposter_map_id(&map.id))
            .take(40)
            .map(|map| LevelImposterMap {
                id: map.id,
                name: map.name,
                author_name: map.author_name,
                description: map.description,
                thumbnail_url: map.thumbnail_url,
            })
            .collect());
    }

    let mut url = url::Url::parse(LEVELIMPOSTER_ALGOLIA_URL).map_err(|error| error.to_string())?;
    url.query_pairs_mut()
        .append_pair("query", query)
        .append_pair("hitsPerPage", "40")
        .append_pair("x-algolia-application-id", LEVELIMPOSTER_ALGOLIA_APP_ID)
        .append_pair("x-algolia-api-key", LEVELIMPOSTER_ALGOLIA_SEARCH_KEY);
    let text = http
        .get_text(url.as_str())
        .map_err(|error| error.to_string())?;
    let response: LevelImposterSearchResponse = serde_json::from_str(&text)
        .map_err(|error| format!("Invalid LevelImposter search response: {error}"))?;
    Ok(response
        .hits
        .into_iter()
        .filter(|hit| valid_levelimposter_map_id(&hit.id))
        .take(40)
        .map(|hit| LevelImposterMap {
            id: hit.id,
            name: hit.name,
            author_name: hit.author_name,
            description: hit.description,
            thumbnail_url: hit.thumbnail_url,
        })
        .collect())
}

fn list_levelimposter_maps_impl(profile_id: &str) -> Result<Vec<String>, String> {
    validate_profile_id(profile_id)?;
    recovered_profile_store(&settings::profiles_root())?
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or("profile not found")?;
    let directory = settings::profiles_root()
        .join(profile_id)
        .join("BepInEx")
        .join("plugins")
        .join("LevelImposter");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) {
            return Err("LevelImposter map folder contains a link or reparse point.".into());
        }
        if !metadata.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(id) = name.strip_suffix(".lim2") else {
            continue;
        };
        if valid_levelimposter_map_id(id) {
            ids.push(id.to_ascii_lowercase());
        }
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn valid_levelimposter_map_download_url(parsed: &url::Url, id: &str) -> bool {
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("storage.googleapis.com")
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return false;
    }

    let Some(segments) = parsed.path_segments() else {
        return false;
    };
    let segments: Vec<_> = segments.collect();
    if segments.len() < 3
        || !matches!(
            segments[0].to_ascii_lowercase().as_str(),
            "levelimposter" | "levelimposter-347807.appspot.com"
        )
        || !segments[1].eq_ignore_ascii_case("maps")
    {
        return false;
    }

    let Some(file_name) = segments.last() else {
        return false;
    };
    file_name.eq_ignore_ascii_case(&format!("{id}.lim"))
        || file_name.eq_ignore_ascii_case(&format!("{id}.lim2"))
}

fn levelimposter_map_download(http: &dyn Http, id: &str) -> Result<(String, Vec<u8>), String> {
    let text = http
        .get_text(&format!("{LEVELIMPOSTER_API}/map/{id}"))
        .map_err(|error| error.to_string())?;
    let metadata: LevelImposterMapMetadata = levelimposter_callback(&text)?;
    if !metadata.id.eq_ignore_ascii_case(id) || !metadata.is_public {
        return Err(format!("LevelImposter map {id} is not public."));
    }
    let download_url = metadata
        .download_url
        .ok_or_else(|| format!("LevelImposter map {} has no download.", metadata.name))?;
    let parsed = url::Url::parse(&download_url).map_err(|_| "Invalid map download URL.")?;
    if !valid_levelimposter_map_download_url(&parsed, id) {
        return Err("LevelImposter returned an untrusted map download URL.".into());
    }
    let bytes = http
        .get_bytes(parsed.as_str())
        .map_err(|error| error.to_string())?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_LEVELIMPOSTER_MAP_BYTES {
        return Err(format!(
            "LevelImposter map {} has an invalid download size.",
            metadata.name
        ));
    }
    Ok((id.to_ascii_lowercase(), bytes))
}

fn download_levelimposter_maps(
    http: &dyn Http,
    ids: &[String],
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut downloads = Vec::with_capacity(ids.len());
    let mut total_bytes = 0_u64;
    for id in ids {
        let download = levelimposter_map_download(http, id)?;
        total_bytes = total_bytes
            .checked_add(download.1.len() as u64)
            .filter(|total| *total <= MAX_LEVELIMPOSTER_MAP_TOTAL_BYTES)
            .ok_or("Selected LevelImposter maps exceed the batch size limit.")?;
        downloads.push(download);
    }
    Ok(downloads)
}

fn replace_profile_levelimposter_maps(
    profiles_root: &Path,
    profile_id: &str,
    previously_owned: &[String],
    downloads: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let directory = profiles_root
        .join(profile_id)
        .join("BepInEx")
        .join("plugins")
        .join("LevelImposter");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if is_reparse(&metadata) || !metadata.is_dir() => {
            return Err("LevelImposter map path is not a regular directory.".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound && downloads.is_empty() => {
            return Ok(());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }

    let selected = downloads
        .iter()
        .map(|(id, _)| id.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for id in previously_owned {
        if selected.contains(&id.to_ascii_lowercase()) {
            continue;
        }
        let path = directory.join(format!("{id}.lim2"));
        match fs::symlink_metadata(&path) {
            Ok(metadata) if is_reparse(&metadata) || !metadata.is_file() => {
                return Err("Managed LevelImposter map is not a regular file.".into());
            }
            Ok(_) => fs::remove_file(path).map_err(|error| error.to_string())?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    for (id, bytes) in downloads {
        atomic_write(&directory.join(format!("{id}.lim2")), bytes)?;
    }
    Ok(())
}

fn normalize_levelimposter_map_ids(map_ids: Vec<String>) -> Result<Vec<String>, String> {
    if map_ids.is_empty() || map_ids.len() > MAX_LEVELIMPOSTER_MAPS_PER_BATCH {
        return Err(format!(
            "Select between 1 and {MAX_LEVELIMPOSTER_MAPS_PER_BATCH} maps."
        ));
    }
    let mut seen = HashSet::with_capacity(map_ids.len());
    let mut normalized = Vec::with_capacity(map_ids.len());
    for id in map_ids {
        let id = id.trim().to_ascii_lowercase();
        if !valid_levelimposter_map_id(&id) {
            return Err("LevelImposter map IDs must be UUIDs.".into());
        }
        if !seen.insert(id.clone()) {
            return Err(format!(
                "LevelImposter map {id} was selected more than once."
            ));
        }
        normalized.push(id);
    }
    Ok(normalized)
}

fn install_levelimposter_maps_impl(
    profile_id: String,
    map_ids: Vec<String>,
    reporter: &ProgressReporter,
) -> Result<ProfileRecord, String> {
    reporter.stage(
        "preparing",
        format!(
            "Preparing {} map download{}",
            map_ids.len(),
            if map_ids.len() == 1 { "" } else { "s" }
        ),
    );
    let normalized = normalize_levelimposter_map_ids(map_ids)?;

    let http = ProgressHttp::new(http()?, reporter.clone());
    let downloads = download_levelimposter_maps(&http, &normalized)?;

    let arch = profile_arch(&profile_id)?;
    let (store, runtime) = profile_store_runtime(&profile_id)?;
    let catalog = catalog()?;
    let levelimposter = catalog
        .get(LEVELIMPOSTER_ID)
        .cloned()
        .ok_or("LevelImposter is missing from the trusted catalog.")?;
    let bundled = bundled_catalog();
    let authoritative = bundled
        .get(LEVELIMPOSTER_ID)
        .ok_or("LevelImposter is missing from the bundled trusted catalog.")?;
    let hosted_repo = levelimposter.repo.as_deref().unwrap_or(&levelimposter.id);
    let authoritative_repo = authoritative.repo.as_deref().unwrap_or(&authoritative.id);
    if levelimposter.trust != Trust::Trusted
        || !hosted_repo.eq_ignore_ascii_case(authoritative_repo)
    {
        return Err("LevelImposter catalog metadata is not trusted.".into());
    }
    let plan =
        deps::resolve(&catalog, &[levelimposter.id.clone()]).map_err(|error| error.to_string())?;
    let ordered = &plan.ordered;
    validate_authoritative_dependencies(&catalog, &[levelimposter.id.clone()])?;

    let root = settings::profiles_root();
    profile_transaction(&root, &profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let already_installed = record
            .mods
            .iter()
            .any(|installed| installed.package_id.eq_ignore_ascii_case(LEVELIMPOSTER_ID));
        if !already_installed {
            let install = InstallContext {
                stage_root,
                profile_id: &profile_id,
                http: &http,
                catalog: &catalog,
                arch: &arch,
                store,
                runtime,
            };
            for id in ordered {
                install_catalog_latest(
                    &install,
                    &mut record,
                    id,
                    !id.eq_ignore_ascii_case(LEVELIMPOSTER_ID),
                    plan.requirements.get(id).map_or(&[][..], Vec::as_slice),
                )?;
            }
            normalize_dependency_ownership(stage_root, &profile_id, &mut record, &catalog)?;
        }

        reporter.stage("finalizing", "Writing maps and saving the profile");
        replace_profile_levelimposter_maps(stage_root, &profile_id, &[], &downloads)?;
        for (id, _) in &downloads {
            if !record
                .levelimposter_maps
                .iter()
                .any(|installed| installed.eq_ignore_ascii_case(id))
            {
                record.levelimposter_maps.push(id.clone());
            }
        }
        record.levelimposter_maps.sort();
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}

fn remove_levelimposter_maps_impl(
    profile_id: String,
    map_ids: Vec<String>,
) -> Result<ProfileRecord, String> {
    validate_profile_id(&profile_id)?;
    let normalized = normalize_levelimposter_map_ids(map_ids)?;
    let removed = normalized
        .iter()
        .map(|id| id.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let root = settings::profiles_root();
    profile_transaction(&root, &profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        replace_profile_levelimposter_maps(stage_root, &profile_id, &normalized, &[])?;
        record
            .levelimposter_maps
            .retain(|id| !removed.contains(&id.to_ascii_lowercase()));
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}
// ---------- mod mutations ----------

#[tauri::command]
pub async fn add_mod(
    profile_id: String,
    repo: String,
    arch: String,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        add_mod_impl(profile_id, repo, arch)
    })
    .await
}

fn add_mod_impl(profile_id: String, repo: String, arch: String) -> Result<ProfileRecord, String> {
    validate_profile_id(&profile_id)?;
    let _ = arch;
    let arch = profile_arch(&profile_id)?;
    let (store, runtime) = profile_store_runtime(&profile_id)?;
    let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
    let catalog = catalog()?;
    let root_entry = catalog_entry_for(&catalog, &repo).cloned();
    let (ordered, requirements) = match root_entry.as_ref() {
        Some(entry) => {
            let plan = deps::resolve(&catalog, std::slice::from_ref(&entry.id))
                .map_err(|error| error.to_string())?;
            (plan.ordered, plan.requirements)
        }
        None => (Vec::new(), HashMap::new()),
    };
    let root = settings::profiles_root();
    profile_transaction(&root, &profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let mut explicit_roots = explicit_catalog_roots(&record, &catalog);
        if let Some(entry) = root_entry.as_ref() {
            if !explicit_roots
                .iter()
                .any(|root| root.eq_ignore_ascii_case(&entry.id))
            {
                explicit_roots.push(entry.id.clone());
            }
        }
        validate_authoritative_dependencies(&catalog, &explicit_roots)?;
        let http = http()?;
        let install = InstallContext {
            stage_root,
            profile_id: &profile_id,
            http: &http,
            catalog: &catalog,
            arch: &arch,
            store,
            runtime,
        };
        for id in &ordered {
            let managed = root_entry
                .as_ref()
                .is_some_and(|root| is_managed_dependency(&root.id, id));
            install_catalog_latest(
                &install,
                &mut record,
                id,
                managed,
                requirements.get(id).map_or(&[][..], Vec::as_slice),
            )?;
        }
        if root_entry.is_none() {
            let rules = default_rules();
            let resolved = resolve_profile_latest(&http, &repo, &rules, &arch, store, runtime)?;
            install_record(
                &install,
                &mut record,
                InstallRequest {
                    package_id: repo.clone(),
                    name: repo.clone(),
                    repo: repo.clone(),
                    tags: Vec::new(),
                    managed: false,
                    resolved,
                },
            )?;
        }
        normalize_dependency_ownership(stage_root, &profile_id, &mut record, &catalog)?;
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}

#[tauri::command]
pub async fn set_mod_enabled(
    profile_id: String,
    package_id: String,
    enabled: bool,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        validate_profile_id(&profile_id)?;
        let root = settings::profiles_root();
        profile_transaction(&root, &profile_id, |stage_root, stage_store| {
            let mut record = stage_store
                .load(&profile_id)
                .map_err(|error| error.to_string())?
                .ok_or("profile not found")?;
            let position = validate_mod_toggle(&record, &package_id)?;
            if let Some(file) = record.mods[position].file.as_deref() {
                profile::set_plugin_enabled(stage_root, &profile_id, file, enabled)
                    .map_err(|error| error.to_string())?;
            }
            record.mods[position].enabled = enabled;
            stage_store
                .save(&record)
                .map_err(|error| error.to_string())?;
            Ok(record)
        })
    })
    .await
}

#[tauri::command]
pub async fn set_mod_version(
    profile_id: String,
    package_id: String,
    version: String,
    arch: String,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        set_mod_version_impl(profile_id, package_id, version, arch)
    })
    .await
}

fn set_mod_version_impl(
    profile_id: String,
    package_id: String,
    version: String,
    arch: String,
) -> Result<ProfileRecord, String> {
    validate_profile_id(&profile_id)?;
    let _ = arch;
    let arch = profile_arch(&profile_id)?;
    let (store, runtime) = profile_store_runtime(&profile_id)?;
    let catalog = catalog()?;
    let root = settings::profiles_root();
    profile_transaction(&root, &profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let position = record
            .mods
            .iter()
            .position(|installed| installed.package_id == package_id)
            .ok_or("mod not found")?;
        let existing = record.mods[position].clone();
        if existing.managed {
            return Err(
                "Managed dependency versions are selected by their root mods. Change the root mod version instead."
                    .into(),
            );
        }
        let repo = existing
            .repo
            .as_deref()
            .and_then(resolver::parse_repo)
            .or_else(|| resolver::parse_repo(&package_id))
            .ok_or("cannot resolve source")?;
        let rules = catalog_entry_for(&catalog, &package_id)
            .map(|entry| entry.asset_rules.clone())
            .unwrap_or_else(default_rules);
        let http = http()?;
        let install = InstallContext {
            stage_root,
            profile_id: &profile_id,
            http: &http,
            catalog: &catalog,
            arch: &arch,
            store,
            runtime,
        };
        let resolved = resolve_profile_tag(&http, &repo, &version, &rules, &arch, store, runtime)?;
        install_record(
            &install,
            &mut record,
            InstallRequest {
                package_id: existing.package_id,
                name: existing.name,
                repo,
                tags: existing.tags,
                managed: existing.managed,
                resolved,
            },
        )?;
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}

#[tauri::command]
pub async fn remove_mod(profile_id: String, package_id: String) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        validate_profile_id(&profile_id)?;
        let root = settings::profiles_root();
        profile_transaction(&root, &profile_id, |stage_root, stage_store| {
            let mut record = stage_store
                .load(&profile_id)
                .map_err(|error| error.to_string())?
                .ok_or("profile not found")?;
            let catalog = catalog()?;
            remove_mod_from_record(stage_root, &profile_id, &mut record, &catalog, &package_id)?;
            stage_store
                .save(&record)
                .map_err(|error| error.to_string())?;
            Ok(record)
        })
    })
    .await
}

#[tauri::command]
pub async fn check_mod_updates(profile_id: String, arch: String) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        validate_profile_id(&profile_id)?;
        let _ = arch;
        let arch = profile_arch(&profile_id)?;
        let (store, runtime) = profile_store_runtime(&profile_id)?;
        let catalog = catalog()?;
        let root = settings::profiles_root();
        profile_transaction(&root, &profile_id, |_stage_root, stage_store| {
            let mut record = stage_store
                .load(&profile_id)
                .map_err(|error| error.to_string())?
                .ok_or("profile not found")?;
            let http = http()?;
            let has_tou_mira = profile_has_tou_mira(&record);
            for installed in &mut record.mods {
                if has_tou_mira && tou_bundle_dependency(&installed.package_id) {
                    installed.update = None;
                    continue;
                }
                let Some(repo) = installed
                    .repo
                    .as_deref()
                    .and_then(resolver::parse_repo)
                    .or_else(|| resolver::parse_repo(&installed.package_id))
                else {
                    continue;
                };
                let rules = catalog_entry_for(&catalog, &installed.package_id)
                    .map(|entry| entry.asset_rules.clone())
                    .unwrap_or_else(default_rules);
                let latest = resolve_profile_latest(&http, &repo, &rules, &arch, store, runtime)?;
                installed.update =
                    perfect_sync_core::version::is_newer(&latest.version, &installed.version)
                        .then_some(latest.version);
            }
            stage_store
                .save(&record)
                .map_err(|error| error.to_string())?;
            Ok(record)
        })
    })
    .await
}

#[tauri::command]
pub async fn apply_mod_updates(
    profile_id: String,
    package_ids: Vec<String>,
    arch: String,
    on_progress: Channel<OperationProgress>,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        validate_profile_id(&profile_id)?;
        if package_ids.is_empty() || package_ids.len() > 64 {
            return Err("Choose between 1 and 64 reviewed mod updates.".into());
        }
        let mut selected = HashSet::new();
        for package_id in &package_ids {
            if package_id.is_empty() || !selected.insert(package_id.to_ascii_lowercase()) {
                return Err("Reviewed mod updates contain an invalid or duplicate id.".into());
            }
        }
        let _ = arch;
        let arch = profile_arch(&profile_id)?;
        let (store, runtime) = profile_store_runtime(&profile_id)?;
        let catalog = catalog()?;
        let root = settings::profiles_root();
        let reporter = ProgressReporter::new(on_progress);
        profile_transaction(&root, &profile_id, |stage_root, stage_store| {
            let mut record = stage_store
                .load(&profile_id)
                .map_err(|error| error.to_string())?
                .ok_or("profile not found")?;
            let progress_http = ProgressHttp::new(http()?, reporter.clone());
            for (index, package_id) in package_ids.iter().enumerate() {
                let position = record
                    .mods
                    .iter()
                    .position(|installed| installed.package_id.eq_ignore_ascii_case(package_id))
                    .ok_or_else(|| format!("Reviewed mod is no longer installed: {package_id}"))?;
                let existing = record.mods[position].clone();
                if existing.managed {
                    return Err(format!(
                        "{} is an automatic dependency. Update its root mod instead.",
                        existing.name
                    ));
                }
                let version = existing
                    .update
                    .as_deref()
                    .filter(|version| {
                        perfect_sync_core::version::is_newer(version, &existing.version)
                    })
                    .ok_or_else(|| {
                        format!("{} no longer has a newer reviewed version.", existing.name)
                    })?
                    .to_string();
                let repo = existing
                    .repo
                    .as_deref()
                    .and_then(resolver::parse_repo)
                    .or_else(|| resolver::parse_repo(&existing.package_id))
                    .ok_or_else(|| format!("Cannot resolve the source for {}.", existing.name))?;
                let rules = catalog_entry_for(&catalog, &existing.package_id)
                    .map(|entry| entry.asset_rules.clone())
                    .unwrap_or_else(default_rules);
                reporter.stage(
                    "resolving",
                    format!(
                        "Resolving {} {} ({}/{})",
                        existing.name,
                        version,
                        index + 1,
                        package_ids.len()
                    ),
                );
                let resolved = resolve_profile_tag(
                    &progress_http,
                    &repo,
                    &version,
                    &rules,
                    &arch,
                    store,
                    runtime,
                )?;
                let install = InstallContext {
                    stage_root,
                    profile_id: &profile_id,
                    http: &progress_http,
                    catalog: &catalog,
                    arch: &arch,
                    store,
                    runtime,
                };
                install_record(
                    &install,
                    &mut record,
                    InstallRequest {
                        package_id: existing.package_id,
                        name: existing.name,
                        repo,
                        tags: existing.tags,
                        managed: false,
                        resolved,
                    },
                )?;
            }
            normalize_dependency_ownership(stage_root, &profile_id, &mut record, &catalog)?;
            reporter.stage("finalizing", "Saving the reviewed mod update batch");
            stage_store
                .save(&record)
                .map_err(|error| error.to_string())?;
            Ok(record)
        })
    })
    .await
}

#[tauri::command]
pub async fn apply_lobby_code(
    code: String,
    arch: String,
    game_instance_id: Option<String>,
    on_progress: Channel<OperationProgress>,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let reporter = ProgressReporter::new(on_progress);
        apply_lobby_code_impl(code, arch, game_instance_id, &reporter)
    })
    .await
}

fn lobby_digest(code: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(code.as_bytes());
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn apply_lobby_code_impl(
    code: String,
    arch: String,
    game_instance_id: Option<String>,
    reporter: &ProgressReporter,
) -> Result<ProfileRecord, String> {
    reporter.stage("preparing", "Reading and validating the lobby code");
    let manifest = codec::decode(&code).map_err(|error| error.to_string())?;
    reporter.stage(
        "resolving",
        format!(
            "Preparing {} mod{} and {} map{}",
            manifest.mods.len(),
            if manifest.mods.len() == 1 { "" } else { "s" },
            manifest.levelimposter_maps.len(),
            if manifest.levelimposter_maps.len() == 1 {
                ""
            } else {
                "s"
            }
        ),
    );
    let settings = settings::load().map_err(|error| error.to_string())?;
    if let Some(instance_id) = game_instance_id.as_deref() {
        if !settings
            .game_instances
            .iter()
            .any(|instance| instance.id == instance_id)
        {
            return Err("lobby profile refers to an unknown game instance".into());
        }
    }
    let _ = arch;
    let arch = saved_game_arch(game_instance_id.as_deref())?;
    let target_instance = match game_instance_id.as_deref() {
        Some(instance_id) => settings
            .game_instances
            .iter()
            .find(|instance| instance.id == instance_id)
            .ok_or("lobby profile refers to an unknown game instance")?,
        None => settings
            .game_instances
            .first()
            .ok_or("save a game instance before applying a lobby")?,
    };
    let store = target_instance.store;
    let runtime = target_instance.runtime;
    let display = manifest
        .name
        .clone()
        .unwrap_or_else(|| "Imported lobby".to_string());
    let slug: String = display
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .take(16)
        .collect();
    let digest = lobby_digest(&code);
    let slug = slug.trim_matches('-');
    let id = if slug.is_empty() {
        format!("lobby-{}", &digest[..40])
    } else {
        format!("lobby-{slug}-{}", &digest[..40])
    };
    validate_profile_id(&id)?;
    let catalog = catalog()?;
    let selected: Vec<String> = manifest
        .mods
        .iter()
        .filter_map(|manifest_mod| {
            catalog_entry_for(&catalog, &manifest_mod.id).map(|entry| entry.id.clone())
        })
        .collect();
    validate_authoritative_dependencies(&catalog, &selected)?;
    let included_dependencies = selected_dependencies(&catalog, &selected)?;
    let http = ProgressHttp::new(http()?, reporter.clone());
    let root = settings::profiles_root();
    profile_transaction(&root, &id, |stage_root, stage_store| {
        let marker = stage_store
            .profile_dir(&id)
            .map_err(|error| error.to_string())?
            .join(".perfectsync-lobby-source");
        let existing_record = stage_store.load(&id).map_err(|error| error.to_string())?;
        match (existing_record.is_some(), read_bounded(&marker, 128)?) {
            (_, Some(existing)) if existing != digest.as_bytes() => {
                return Err("lobby profile id belongs to a different lobby source".into());
            }
            (true, None) => {
                return Err("refusing to overwrite a profile with unknown lobby source".into());
            }
            _ => {}
        }
        let map_downloads = download_levelimposter_maps(&http, &manifest.levelimposter_maps)?;
        let mut record = existing_record.unwrap_or(ProfileRecord {
            id: id.clone(),
            name: display.clone(),
            crew_color: "#ffd23f".to_string(),
            game_build: None,
            game_instance_id: game_instance_id.clone(),
            mods: Vec::new(),
            levelimposter_maps: Vec::new(),
        });
        let previously_owned_maps = record.levelimposter_maps.clone();
        if profile_has_tou_mira(&record) {
            profile::remove_tou_bundle(stage_root, &id).map_err(|error| error.to_string())?;
        }
        for old in &record.mods {
            if let Some(file) = old.file.as_deref() {
                profile::remove_plugin(stage_root, &id, file).map_err(|error| error.to_string())?;
            }
        }
        record.mods.clear();
        record.name = display.clone();
        record.game_build = None;
        record.game_instance_id = game_instance_id.clone();
        let install = InstallContext {
            stage_root,
            profile_id: &id,
            http: &http,
            catalog: &catalog,
            arch: &arch,
            store,
            runtime,
        };
        for manifest_mod in &manifest.mods {
            let entry = catalog_entry_for(&catalog, &manifest_mod.id);
            let repo = entry
                .and_then(|entry| entry.repo.clone())
                .or_else(|| resolver::parse_repo(&manifest_mod.id))
                .ok_or_else(|| format!("cannot resolve source for {}", manifest_mod.id))?;
            let rules = entry
                .map(|entry| entry.asset_rules.clone())
                .unwrap_or_else(default_rules);
            let resolved = if let Some(asset) = manifest_mod.asset.as_deref() {
                selected_release_asset(
                    &http,
                    &repo,
                    &manifest_mod.v,
                    asset,
                    ReleaseAssetTarget {
                        rules: Some(&rules),
                        arch: &arch,
                        store,
                        runtime,
                    },
                )?
            } else {
                resolve_profile_tag(&http, &repo, &manifest_mod.v, &rules, &arch, store, runtime)?
            };
            install_record(
                &install,
                &mut record,
                InstallRequest {
                    package_id: entry
                        .map(|entry| entry.id.clone())
                        .unwrap_or_else(|| manifest_mod.id.clone()),
                    name: entry
                        .map(|entry| entry.name.clone())
                        .unwrap_or_else(|| repo.clone()),
                    repo,
                    tags: entry.map(|entry| entry.tags.clone()).unwrap_or_default(),
                    managed: entry.is_some_and(|entry| {
                        included_dependencies.contains(&entry.id.to_ascii_lowercase())
                    }),
                    resolved,
                },
            )?;
        }
        for personal in settings
            .personal_mods
            .iter()
            .filter(|personal| personal.enabled)
        {
            let repo = resolver::parse_repo(&personal.repo)
                .ok_or_else(|| format!("invalid personal mod source {}", personal.repo))?;
            if let Some(installed) = record.mods.iter_mut().find(|installed| {
                installed.package_id.eq_ignore_ascii_case(&repo)
                    || installed
                        .repo
                        .as_deref()
                        .is_some_and(|source| source.eq_ignore_ascii_case(&repo))
            }) {
                installed.managed = false;
                continue;
            }
            let entry = catalog_entry_for(&catalog, &repo);
            let resolved = selected_release_asset(
                &http,
                &repo,
                &personal.tag,
                &personal.asset,
                ReleaseAssetTarget {
                    rules: entry.map(|entry| &entry.asset_rules),
                    arch: &arch,
                    store,
                    runtime,
                },
            )?;
            install_record(
                &install,
                &mut record,
                InstallRequest {
                    package_id: repo.clone(),
                    name: personal
                        .name
                        .clone()
                        .or_else(|| entry.map(|entry| entry.name.clone()))
                        .unwrap_or_else(|| repo.clone()),
                    repo,
                    tags: entry.map(|entry| entry.tags.clone()).unwrap_or_default(),
                    managed: false,
                    resolved,
                },
            )?;
        }
        for local in settings
            .personal_local_mods
            .iter()
            .filter(|local| local.enabled)
        {
            install_local_mod_into_record(stage_root, &id, &mut record, Path::new(&local.path))?;
        }
        reporter.stage(
            "finalizing",
            "Verifying files and publishing the lobby profile",
        );
        replace_profile_levelimposter_maps(
            stage_root,
            &id,
            &previously_owned_maps,
            &map_downloads,
        )?;
        record.levelimposter_maps = map_downloads
            .iter()
            .map(|(map_id, _)| map_id.clone())
            .collect();
        normalize_dependency_ownership(stage_root, &id, &mut record, &catalog)?;
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        atomic_write(&marker, digest.as_bytes())?;
        Ok(record)
    })
}

// ---------- loader + launch ----------

#[tauri::command]
pub async fn ensure_loader(
    game_path: String,
    profile_id: String,
    arch: String,
    apply_doorstop_fix: bool,
    on_progress: Channel<OperationProgress>,
) -> Result<Option<String>, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let reporter = ProgressReporter::new(on_progress);
        let http = ProgressHttp::new(http()?, reporter.clone());
        with_existing_profile_layout(&settings::profiles_root(), &profile_id, || {
            ensure_loader_impl(
                &game_path,
                &profile_id,
                &arch,
                apply_doorstop_fix,
                &http,
                Some(&reporter),
            )?;
            reporter.stage("finalizing", "Configuring the game runtime");
            let game_dir = validate_game_target(&game_path, Some(&profile_id))?;
            let context = runtime_context(&game_dir)?;
            game_is_stopped()?;
            Ok(configure_runtime_override(&context).err())
        })
    })
    .await
}

#[tauri::command]
pub async fn reinstall_loader(
    game_path: String,
    profile_id: String,
    arch: String,
    apply_doorstop_fix: bool,
    use_latest_loader: bool,
) -> Result<Option<String>, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        with_existing_profile_layout(&settings::profiles_root(), &profile_id, || {
            reinstall_loader_impl(
                &game_path,
                &profile_id,
                &arch,
                apply_doorstop_fix,
                use_latest_loader,
            )?;
            let game_dir = validate_game_target(&game_path, Some(&profile_id))?;
            let context = runtime_context(&game_dir)?;
            game_is_stopped()?;
            Ok(configure_runtime_override(&context).err())
        })
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderStatus {
    pub game_found: bool,
    pub winhttp: bool,
    pub preloader: bool,
    pub current: bool,
    pub installed_version: Option<String>,
    pub doorstop_fix: bool,
    pub dotnet: bool,
    pub steam_appid: bool,
    pub profile_plugins: usize,
    pub game_plugins: usize,
    pub runtime: Runtime,
    pub runtime_ready: bool,
}

#[tauri::command]
pub async fn loader_status(game_path: String, profile_id: String) -> Result<LoaderStatus, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        validate_profile_id(&profile_id)?;
        let game = validate_game_target(&game_path, Some(&profile_id))?;
        let root = settings::profiles_root();
        let store = recovered_profile_store(&root)?;
        store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "profile not found".to_string())?;
        let profile_plugins = store
            .profile_dir(&profile_id)
            .map_err(|error| error.to_string())?
            .join("BepInEx")
            .join("plugins");
        let count_dll = |directory: PathBuf| -> Result<usize, String> {
            let entries = match fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
                Err(error) => return Err(error.to_string()),
            };
            let mut count = 0;
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                let metadata = entry.metadata().map_err(|error| error.to_string())?;
                if metadata.is_file()
                    && entry
                        .path()
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
                {
                    count += 1;
                }
            }
            Ok(count)
        };
        let context = runtime_context(&game)?;
        let runtime_ready = context.runtime == Runtime::Native
            || context
                .prefix
                .as_deref()
                .is_some_and(compat::has_winhttp_override);
        let arch = game::exe_arch(&game.join(process::GAME_EXE))
            .map(arch_str)
            .ok_or("Among Us executable architecture is unsupported")?;
        Ok(LoaderStatus {
            game_found: true,
            winhttp: game.join("winhttp.dll").is_file(),
            preloader: game
                .join("BepInEx")
                .join("core")
                .join(loader::IL2CPP_PRELOADER)
                .is_file(),
            current: loader::has_loader(&game),
            installed_version: loader::installed_version(&game),
            doorstop_fix: doorstop_fix_is_current(&game, &arch),
            dotnet: game.join("dotnet").join("coreclr.dll").is_file(),
            steam_appid: game.join("steam_appid.txt").is_file(),
            profile_plugins: count_dll(profile_plugins)?,
            game_plugins: count_dll(game.join("BepInEx").join("plugins"))?,
            runtime: context.runtime,
            runtime_ready,
        })
    })
    .await
}

/// A game copy in a location apps can't write (Microsoft Store / Game Pass lives
/// under the ACL-locked WindowsApps). Returns guidance instead of letting the
/// install fail later with a raw permission error.
fn protected_install_hint(game_dir: &Path) -> Option<String> {
    let path = game_dir.to_string_lossy().replace('\\', "/").to_lowercase();
    if path.contains("/windowsapps/") || path.ends_with("/windowsapps") {
        Some("This Among Us copy is in the protected WindowsApps folder (Microsoft Store / Game Pass), which apps can't modify. Copy the \"Among Us\" folder to a normal location (e.g. your Documents), then point Perfect-Sync at that copy.".to_string())
    } else {
        None
    }
}

fn load_tou_package_bytes(
    installed: &InstalledMod,
    arch: &str,
    store: Store,
    runtime: Runtime,
) -> Result<Vec<u8>, String> {
    let asset_name = installed
        .asset
        .as_deref()
        .ok_or("Town of Us profile is missing its exact release asset")?;
    let cache_path = tou_package_cache_path(&installed.version, asset_name);
    if let Some(bytes) = read_bounded(&cache_path, MAX_TOU_PACKAGE_CACHE_BYTES)? {
        return Ok(bytes);
    }
    let repo = installed
        .repo
        .as_deref()
        .and_then(resolver::parse_repo)
        .or_else(|| resolver::parse_repo(&installed.package_id))
        .ok_or("Town of Us profile has no valid release source")?;
    let catalog = catalog()?;
    let rules = catalog_entry_for(&catalog, &repo)
        .map(|entry| entry.asset_rules.clone())
        .ok_or("Town of Us is missing its authoritative catalog rules")?;
    let http = http()?;
    let resolved = selected_release_asset(
        &http,
        &repo,
        &installed.version,
        asset_name,
        ReleaseAssetTarget {
            rules: Some(&rules),
            arch,
            store,
            runtime,
        },
    )?;
    if !resolved.asset_name.eq_ignore_ascii_case(asset_name) {
        return Err(format!(
            "Town of Us {} no longer resolves to the profile's exact asset {}",
            installed.version, asset_name
        ));
    }
    let bytes = download_resolved(&http, &resolved).map_err(|error| error.to_string())?;
    cache_tou_package(&installed.version, asset_name, &bytes)?;
    Ok(bytes)
}

fn prepare_profile(game_path: &str, profile_id: &str) -> Result<Option<String>, String> {
    game_is_stopped()?;
    let game_dir = validate_game_target(game_path, Some(profile_id))?;
    let arch = game::exe_arch(&game_dir.join(process::GAME_EXE))
        .map(arch_str)
        .ok_or("Among Us executable architecture is unsupported")?;
    let preserve_doorstop_fix = doorstop_fix_is_current(&game_dir, &arch);
    let profiles_root = settings::profiles_root();
    let profile = recovered_profile_store(&profiles_root)?
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or("profile not found")?;
    let active_tou = profile
        .mods
        .iter()
        .find(|installed| installed.enabled && is_tou_mira(&installed.package_id));
    let tou_key = active_tou
        .map(|installed| {
            installed
                .asset
                .as_deref()
                .map(|asset| tou_package_key(&installed.version, asset))
                .ok_or("Town of Us profile is missing its exact release asset")
        })
        .transpose()?;
    let tou_current = !preserve_doorstop_fix
        && tou_key
            .as_deref()
            .map(|key| loader::tou_package_is_current(&game_dir, key))
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or(false);
    let tou_package = if let Some(installed) = active_tou.filter(|_| !tou_current) {
        let (store, runtime) = profile_store_runtime(profile_id)?;
        Some(load_tou_package_bytes(installed, &arch, store, runtime)?)
    } else {
        None
    };
    let shadowed_plugin_files = tou_shadowed_plugin_files(&profile);

    with_profile_layout(&profiles_root, profile_id, || {
        game_artifact_transaction(&game_dir, || {
            restore_doorstop(&game_dir)?;
            game_is_stopped()?;
            if active_tou.is_some() {
                if let Some(bytes) = tou_package.as_deref() {
                    tou_cosmetics::remove_managed_files(&game_dir.join("BepInEx").join("plugins"))
                        .map_err(|error| error.to_string())?;
                    loader::install_tou_package(
                        bytes,
                        &game_dir,
                        tou_key
                            .as_deref()
                            .ok_or("Town of Us package key is missing")?,
                        PINNED_LOADER_VERSION,
                    )
                    .map_err(|error| error.to_string())?;
                }
            } else {
                tou_cosmetics::remove_managed_files(&game_dir.join("BepInEx").join("plugins"))
                    .map_err(|error| error.to_string())?;
                loader::remove_tou_package(&game_dir).map_err(|error| error.to_string())?;
            }
            loader::sync_profile_plugins_shadowing(
                &profiles_root,
                profile_id,
                &game_dir,
                &shadowed_plugin_files,
            )
            .map_err(|error| error.to_string())?;
            loader::sync_levelimposter_maps(&profiles_root, profile_id, &game_dir)
                .map_err(|error| error.to_string())?;
            let loader_http = http()?;
            ensure_loader_impl(
                game_path,
                profile_id,
                &arch,
                active_tou.is_none() && preserve_doorstop_fix,
                &loader_http,
                None,
            )?;
            game_is_stopped()?;
            loader::ensure_steam_appid(&game_dir).map_err(|error| error.to_string())?;
            game_is_stopped()?;
            loader::write_console_off(&game_dir).map_err(|error| error.to_string())?;
            let context = runtime_context(&game_dir)?;
            game_is_stopped()?;
            Ok(configure_runtime_override(&context).err())
        })
    })
}

fn require_launch_ready(guidance: Option<String>) -> Result<(), String> {
    match guidance {
        Some(guidance) => Err(guidance),
        None => Ok(()),
    }
}

#[tauri::command]
pub async fn sync_profile(game_path: String, profile_id: String) -> Result<Option<String>, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        prepare_profile(&game_path, &profile_id)
    })
    .await
}

fn launch_store(game_path: &Path) -> Result<Store, String> {
    Ok(settings::load()
        .map_err(|error| error.to_string())?
        .game_instances
        .into_iter()
        .find(|instance| same_path(Path::new(&instance.path), game_path))
        .map(|instance| instance.store)
        .unwrap_or_else(|| inferred_store(game_path)))
}

fn registered_steam_client(game_dir: &Path) -> Option<PathBuf> {
    let client = game::native_steam_client_for_install(game_dir)?;
    let metadata = fs::symlink_metadata(&client).ok()?;
    (!is_reparse(&metadata) && metadata.is_file()).then_some(client)
}

fn launch_steam_app(client: &Path) -> Result<(), String> {
    spawn_launch(|| {
        process::command(client)
            .args(["-applaunch", game::STEAM_APP_ID])
            .stdin(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("couldn't launch the registered Steam install: {error}"))
    })
}

const EPIC_STARTER_URL: &str =
    "https://github.com/whichtwix/EpicGamesStarter/releases/download/1.1.0/EpicGamesStarter.exe.zip";
const EPIC_STARTER_SIZE: u64 = 6_865_606;
const EPIC_STARTER_SHA256: &str =
    "15fb526b39b90a6e571397ec9981faded67e140a6dd3f42c011c7f4060188bc8";
const MAX_EPIC_STARTER_BYTES: u64 = 64 * 1024 * 1024;
const EPIC_EXECUTABLE_SIZE: u64 = 15_316_174;
const EPIC_EXECUTABLE_SHA256: &str =
    "7e1d7e1d2d96aca2ae3a4229f0c2902b1997d94f166f70ce2acd4e7b5bcb8c42";

fn pinned_epic_download() -> Result<ResolvedDownload, String> {
    let release = resolver::parse_release(&format!(
        r#"{{"tag_name":"1.1.0","assets":[{{"name":"EpicGamesStarter.exe.zip","browser_download_url":"{EPIC_STARTER_URL}","size":{EPIC_STARTER_SIZE},"digest":"sha256:{EPIC_STARTER_SHA256}"}}]}}"#
    ))
    .map_err(|error| error.to_string())?;
    let asset = release
        .assets
        .into_iter()
        .next()
        .ok_or("missing Epic pin")?;
    Ok(ResolvedDownload {
        url: asset.url,
        asset_name: asset.name,
        version: release.tag,
        size: asset.size,
    })
}

fn validated_epic_executable(archive: &[u8]) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|error| format!("invalid EpicGamesStarter ZIP: {error}"))?;
    if zip.len() != 1 {
        return Err("EpicGamesStarter ZIP must contain exactly one entry".into());
    }
    let mut entry = zip
        .by_index(0)
        .map_err(|error| format!("invalid EpicGamesStarter ZIP entry: {error}"))?;
    if entry.is_dir()
        || entry.name() != "EpicGamesStarter.exe"
        || entry.size() == 0
        || entry.size() > MAX_EPIC_STARTER_BYTES
        || entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 != 0o100000)
    {
        return Err(
            "EpicGamesStarter ZIP must contain one nonempty regular EpicGamesStarter.exe".into(),
        );
    }
    let expected = entry.size();
    let mut bytes = Vec::with_capacity(expected as usize);
    entry
        .by_ref()
        .take(MAX_EPIC_STARTER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 != expected {
        return Err("EpicGamesStarter expanded size did not match ZIP metadata".into());
    }
    Ok(bytes)
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn epic_executable_matches_pin(path: &Path) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "{} is not a regular non-link executable",
            path.display()
        ));
    }
    if metadata.len() != EPIC_EXECUTABLE_SIZE {
        return Ok(false);
    }
    let bytes = read_bounded(path, EPIC_EXECUTABLE_SIZE)?
        .ok_or("EpicGamesStarter executable disappeared during verification")?;
    Ok(sha256_hex(&bytes) == EPIC_EXECUTABLE_SHA256)
}

fn ensure_epic_starter(http: &dyn Http, game_dir: &Path) -> Result<PathBuf, String> {
    let executable = game_dir.join("EpicGamesStarter.exe");
    if epic_executable_matches_pin(&executable)? {
        return Ok(executable);
    }
    let archive =
        download_resolved(http, &pinned_epic_download()?).map_err(|error| error.to_string())?;
    let delivered = validated_epic_executable(&archive)?;
    if delivered.len() as u64 != EPIC_EXECUTABLE_SIZE
        || sha256_hex(&delivered) != EPIC_EXECUTABLE_SHA256
    {
        return Err("downloaded EpicGamesStarter executable failed the extracted-file pin".into());
    }
    game_is_stopped()?;
    atomic_write(&executable, &delivered)?;
    if !epic_executable_matches_pin(&executable)? {
        return Err("published EpicGamesStarter failed pin verification".into());
    }
    Ok(executable)
}

/// Create a neutral token cache without replacing an existing Legendary login.
/// This keeps EpicGamesStarter away from Among Us's incompatible `EGSAuth.json`.
fn ensure_epic_auth_file(token_store: &Path) -> Result<PathBuf, String> {
    match fs::symlink_metadata(token_store) {
        Ok(metadata) if !is_reparse(&metadata) && metadata.is_file() => {
            return Ok(token_store.to_path_buf());
        }
        Ok(_) => {
            return Err(format!(
                "{} is not a regular non-link file",
                token_store.display()
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }

    atomic_write(token_store, b"null\n")?;
    Ok(token_store.to_path_buf())
}

fn ensure_epic_auth_store(user_profile: &Path) -> Result<PathBuf, String> {
    ensure_epic_auth_file(
        &user_profile
            .join(".config")
            .join("legendary")
            .join("user.json"),
    )
}

fn prepare_epic_auth_stores(
    game_dir: &Path,
    context: &compat::RuntimeContext,
) -> Result<(), String> {
    if context.host == compat::HostPlatform::Windows {
        let user_profile = std::env::var_os("USERPROFILE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or("Windows USERPROFILE is unavailable; cannot prepare Epic authentication")?;
        ensure_epic_auth_store(&user_profile)?;
        return Ok(());
    }

    let mut prepared_profiles = 0_usize;
    if let Some(prefix) = &context.prefix {
        let users = prefix.join("drive_c").join("users");
        let entries = match fs::read_dir(&users) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "couldn't inspect Wine user profiles at {}: {error}",
                    users.display()
                ));
            }
        };
        if let Some(entries) = entries {
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                let metadata =
                    fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
                if is_reparse(&metadata) || !metadata.is_dir() {
                    continue;
                }
                let among_us_data = entry
                    .path()
                    .join("AppData")
                    .join("LocalLow")
                    .join("Innersloth")
                    .join("Among Us");
                let has_among_us_data = fs::symlink_metadata(&among_us_data)
                    .is_ok_and(|metadata| !is_reparse(&metadata) && metadata.is_dir());
                if has_among_us_data {
                    ensure_epic_auth_store(&entry.path())?;
                    prepared_profiles += 1;
                }
            }
        }
    }

    if prepared_profiles == 0 {
        ensure_epic_auth_file(&game_dir.join("EGSAuth.json"))?;
    }
    Ok(())
}

fn launch_prepared_game(game_dir: &Path) -> Result<(), String> {
    let store = launch_store(game_dir)?;
    if store == Store::Steam {
        if let Some(client) = registered_steam_client(game_dir) {
            return launch_steam_app(&client);
        }
    }
    let context = runtime_context(game_dir)?;
    if store == Store::Epic {
        let starter = ensure_epic_starter(&http()?, game_dir)?;
        prepare_epic_auth_stores(game_dir, &context)?;
        if cfg!(windows) {
            return spawn_launch(|| {
                let helper_pid = process::launch_console_interactive(&starter, game_dir)
                    .map_err(|error| format!("couldn't run EpicGamesStarter: {error}"))?;
                crate::console_monitor::start(helper_pid)
                    .map_err(|error| format!("couldn't monitor EpicGamesStarter: {error}"))
            });
        }
        let specification = compat::build_program_spec(&starter, game_dir, &context);
        return spawn_launch(|| {
            process::launch_interactive(&specification)
                .map(|_| ())
                .map_err(|error| launch_err_msg(&context, &error))
        });
    }
    if store == Store::Msstore {
        if !game::is_writable_game_dir(game_dir) {
            return Err(
                "Microsoft Store/Game Pass installs must be launched from a writable managed copy. Create one in Settings first."
                    .into(),
            );
        }
        if game::exe_arch(&game_dir.join(process::GAME_EXE)) != Some(Arch::X64) {
            return Err(
                "This saved Microsoft Store instance is not the expected x64 Among Us build. Re-detect the game or create a fresh managed copy."
                    .into(),
            );
        }
    }
    let specification = compat::build_launch_spec(game_dir, &context);
    spawn_launch(|| {
        process::launch(&specification)
            .map(|_| ())
            .map_err(|error| launch_err_msg(&context, &error))
    })
}

#[tauri::command]
pub async fn launch_profile(game_path: String, profile_id: String) -> Result<(), String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        require_launch_ready(prepare_profile(&game_path, &profile_id)?)?;
        game_is_stopped()?;
        let game_dir = validate_game_target(&game_path, Some(&profile_id))?;
        launch_prepared_game(&game_dir)
    })
    .await
}

#[tauri::command]
pub async fn launch_vanilla(game_path: String) -> Result<(), String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        game_is_stopped()?;
        let game_dir = validate_game_target(&game_path, None)?;
        launch_without_doorstop(&game_dir, || launch_prepared_game(&game_dir))
    })
    .await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveBackupInfo {
    pub id: String,
    pub created_at: u64,
    pub files: usize,
    pub bytes: u64,
}

fn unix_millis() -> Result<u64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    u64::try_from(millis).map_err(|_| "system timestamp is too large".to_string())
}

fn innersloth_save_dir() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or("Windows LOCALAPPDATA is unavailable; save data cannot be located.")?;
    let local_low = local
        .parent()
        .ok_or("Windows LOCALAPPDATA has no parent directory.")?
        .join("LocalLow");
    Ok(local_low.join("Innersloth").join("Among Us"))
}

fn save_backups_root() -> PathBuf {
    settings::app_data_dir().join("backups").join("save-data")
}

fn create_save_backup_impl() -> Result<SaveBackupInfo, String> {
    let source = innersloth_save_dir()?;
    let metadata = fs::symlink_metadata(&source).map_err(|error| {
        format!(
            "Among Us save data was not found at {}: {error}",
            source.display()
        )
    })?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("The Among Us save-data folder is not a safe regular directory.".into());
    }
    let created_at = unix_millis()?;
    let id = format!(
        "{created_at}-{}",
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let root = save_backups_root();
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let destination = root.join(&id);
    let stage = root.join(format!(".stage-{id}"));
    let result = (|| {
        fs::create_dir(&stage).map_err(|error| error.to_string())?;
        let mut files = 0;
        let mut bytes = 0;
        copy_game_tree(&source, &stage.join("data"), &mut files, &mut bytes)?;
        let info = SaveBackupInfo {
            id: id.clone(),
            created_at,
            files,
            bytes,
        };
        let manifest = serde_json::to_vec_pretty(&info).map_err(|error| error.to_string())?;
        atomic_write(&stage.join("manifest.json"), &manifest)?;
        fs::rename(&stage, &destination).map_err(|error| error.to_string())?;
        Ok(info)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

#[tauri::command]
pub async fn backup_save_data() -> Result<SaveBackupInfo, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        game_is_stopped()?;
        let backup = create_save_backup_impl()?;
        if let Err(error) = prune_save_backups() {
            log::warn!("created save backup but could not prune old backups: {error}");
        }
        Ok(backup)
    })
    .await
}

fn prune_save_backups() -> Result<(), String> {
    let root = save_backups_root();
    let mut backups = list_save_backups_impl()?;
    if backups.len() <= 25 {
        return Ok(());
    }
    backups.sort_by_key(|backup| backup.created_at);
    let excess = backups.len().saturating_sub(25);
    for backup in backups.into_iter().take(excess) {
        let path = root.join(backup.id);
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) || !metadata.is_dir() {
            return Err("A save-backup path is not a safe regular directory.".into());
        }
        fs::remove_dir_all(path).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn list_save_backups_impl() -> Result<Vec<SaveBackupInfo>, String> {
    let root = save_backups_root();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut backups = Vec::new();
    for entry in entries.take(256) {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) || !metadata.is_dir() {
            continue;
        }
        let Some(bytes) = read_bounded(&entry.path().join("manifest.json"), 64 * 1024)? else {
            continue;
        };
        let info: SaveBackupInfo =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if info.id == entry.file_name().to_string_lossy() {
            backups.push(info);
        }
    }
    backups.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(backups)
}

#[tauri::command]
pub async fn list_save_backups() -> Result<Vec<SaveBackupInfo>, String> {
    blocking(list_save_backups_impl).await
}

#[tauri::command]
pub async fn restore_save_data(backup_id: String) -> Result<SaveBackupInfo, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        game_is_stopped()?;
        if backup_id.is_empty()
            || backup_id.len() > 64
            || !backup_id
                .chars()
                .all(|character| character.is_ascii_digit() || character == '-')
        {
            return Err("Invalid save backup id.".into());
        }
        let backup = list_save_backups_impl()?
            .into_iter()
            .find(|backup| backup.id == backup_id)
            .ok_or("Save backup was not found.")?;
        let backup_data = save_backups_root().join(&backup.id).join("data");
        let backup_metadata =
            fs::symlink_metadata(&backup_data).map_err(|error| error.to_string())?;
        if is_reparse(&backup_metadata) || !backup_metadata.is_dir() {
            return Err("Save backup data is not a safe regular directory.".into());
        }

        let target = innersloth_save_dir()?;
        let parent = target
            .parent()
            .ok_or("Among Us save-data folder has no parent directory.")?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let safety_backup = if target.is_dir() {
            Some(create_save_backup_impl()?)
        } else {
            None
        };
        let stage = unique_sibling(&target, "restore")?;
        let rollback = unique_sibling(&target, "rollback")?;
        let mut files = 0;
        let mut bytes = 0;
        if let Err(error) = copy_game_tree(&backup_data, &stage, &mut files, &mut bytes) {
            let _ = fs::remove_dir_all(&stage);
            return Err(error);
        }
        let had_target = target.exists();
        if had_target {
            let metadata = fs::symlink_metadata(&target).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) || !metadata.is_dir() {
                let _ = fs::remove_dir_all(&stage);
                return Err("Current save data is not a safe regular directory.".into());
            }
            fs::rename(&target, &rollback).map_err(|error| error.to_string())?;
        }
        if let Err(error) = fs::rename(&stage, &target) {
            if had_target {
                let _ = fs::rename(&rollback, &target);
            }
            let _ = fs::remove_dir_all(&stage);
            return Err(format!("Could not publish restored save data: {error}"));
        }
        if had_target {
            if let Err(error) = fs::remove_dir_all(&rollback) {
                log::warn!("restored save data but could not remove rollback folder: {error}");
            }
        }
        if let Err(error) = prune_save_backups() {
            log::warn!("restored save data but could not prune old backups: {error}");
        }
        Ok(safety_backup.unwrap_or(backup))
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticGame {
    pub name: String,
    pub store: Store,
    pub arch: Arch,
    pub runtime: Runtime,
    pub build: Option<String>,
    pub writable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticLoader {
    pub current: bool,
    pub installed_version: Option<String>,
    pub winhttp: bool,
    pub preloader: bool,
    pub dotnet: bool,
    pub profile_plugins: usize,
    pub game_plugins: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticAsset {
    pub name: String,
    pub version: String,
    pub file: Option<String>,
    pub enabled: bool,
    pub source: ModSource,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    pub generated_at: u64,
    pub app_version: String,
    pub profile_name: Option<String>,
    pub game: Option<DiagnosticGame>,
    pub loader: Option<DiagnosticLoader>,
    pub assets: Vec<DiagnosticAsset>,
    pub log_errors: Vec<String>,
    pub game_running: Option<bool>,
    pub warnings: Vec<String>,
}

fn count_dll_files(directory: &Path) -> Result<usize, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.to_string()),
    };
    let mut count = 0;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if !is_reparse(&metadata)
            && metadata.is_file()
            && entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
        {
            count += 1;
        }
    }
    Ok(count)
}

fn redact_sensitive(mut text: String, saved: &Settings) -> String {
    for path in saved
        .game_instances
        .iter()
        .map(|instance| instance.path.as_str())
        .chain(
            saved
                .personal_local_mods
                .iter()
                .map(|local| local.path.as_str()),
        )
    {
        if !path.is_empty() {
            text = text.replace(path, "<redacted-path>");
        }
    }
    for variable in ["USERPROFILE", "HOME", "APPDATA", "LOCALAPPDATA"] {
        if let Some(path) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
            text = text.replace(&path.to_string_lossy().to_string(), "<redacted-user-path>");
        }
    }
    let token =
        regex::Regex::new(r"(?i)\b(?:github_pat_[A-Za-z0-9_]{20,}|gh[pousr]_[A-Za-z0-9_]{20,})\b")
            .expect("static token regex");
    token.replace_all(&text, "<redacted-token>").into_owned()
}

fn recent_log_errors(game_dir: &Path, saved: &Settings) -> Result<Vec<String>, String> {
    const MAX_LOG_TAIL: u64 = 2 * 1024 * 1024;
    let path = game_dir.join("BepInEx").join("LogOutput.log");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err("BepInEx LogOutput.log is not a safe regular file.".into());
    }
    let mut file = File::open(&path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_LOG_TAIL {
        file.seek(SeekFrom::End(-(MAX_LOG_TAIL as i64)))
            .map_err(|error| error.to_string())?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let mut errors: Vec<String> = text
        .lines()
        .filter(|line| {
            let folded = line.to_ascii_lowercase();
            folded.contains("[error") || folded.contains("exception")
        })
        .map(|line| redact_sensitive(line.trim().to_string(), saved))
        .collect();
    if errors.len() > 50 {
        errors.drain(..errors.len() - 50);
    }
    Ok(errors)
}

fn diagnostics_report_impl(profile_id: Option<&str>) -> Result<DiagnosticsReport, String> {
    if let Some(profile_id) = profile_id {
        validate_profile_id(profile_id)?;
    }
    let saved = settings::load().map_err(|error| error.to_string())?;
    let profile_store = recovered_profile_store(&settings::profiles_root())?;
    let profile = match profile_id {
        Some(profile_id) => profile_store
            .load(profile_id)
            .map_err(|error| error.to_string())?,
        None => None,
    };
    let instance = profile
        .as_ref()
        .and_then(|profile| profile.game_instance_id.as_deref())
        .and_then(|id| {
            saved
                .game_instances
                .iter()
                .find(|instance| instance.id == id)
        })
        .or_else(|| saved.game_instances.first());
    let mut warnings = Vec::new();
    let mut loader_status = None;
    let mut log_errors = Vec::new();
    let game_status = if let Some(instance) = instance {
        match canonical_game_path(Path::new(&instance.path)) {
            Ok(game_dir) => {
                let build = game::detect_build(&game_dir);
                let writable = game::is_writable_game_dir(&game_dir);
                if !writable {
                    warnings.push(
                        "This game folder is not writable; create a managed Microsoft Store copy."
                            .to_string(),
                    );
                }
                if let Some(profile) = profile.as_ref() {
                    let profile_dir = profile_store
                        .profile_dir(&profile.id)
                        .map_err(|error| error.to_string())?;
                    loader_status = Some(DiagnosticLoader {
                        current: loader::has_loader(&game_dir),
                        installed_version: loader::installed_version(&game_dir),
                        winhttp: game_dir.join("winhttp.dll").is_file(),
                        preloader: game_dir
                            .join("BepInEx")
                            .join("core")
                            .join(loader::IL2CPP_PRELOADER)
                            .is_file(),
                        dotnet: game_dir.join("dotnet").join("coreclr.dll").is_file(),
                        profile_plugins: count_dll_files(
                            &profile_dir.join("BepInEx").join("plugins"),
                        )?,
                        game_plugins: count_dll_files(&game_dir.join("BepInEx").join("plugins"))?,
                    });
                }
                log_errors = recent_log_errors(&game_dir, &saved)?;
                Some(DiagnosticGame {
                    name: instance.name.clone(),
                    store: instance.store,
                    arch: instance.arch,
                    runtime: instance.runtime,
                    build,
                    writable,
                })
            }
            Err(error) => {
                warnings.push(error);
                None
            }
        }
    } else {
        warnings.push("No Among Us instance is configured.".to_string());
        None
    };
    if let Some(loader) = loader_status.as_ref() {
        if !loader.current || !loader.winhttp || !loader.preloader || !loader.dotnet {
            warnings.push("BepInEx is incomplete or not current for this game folder.".to_string());
        }
    }
    let game_running = match process::try_is_running() {
        Ok(running) => Some(running),
        Err(error) => {
            warnings.push(format!("Could not check the Among Us process: {error}"));
            None
        }
    };
    let assets = profile
        .as_ref()
        .map(|profile| {
            profile
                .mods
                .iter()
                .map(|asset| DiagnosticAsset {
                    name: asset.name.clone(),
                    version: asset.version.clone(),
                    file: asset.file.clone(),
                    enabled: asset.enabled,
                    source: asset.source,
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(DiagnosticsReport {
        generated_at: unix_millis()?,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        profile_name: profile.as_ref().map(|profile| profile.name.clone()),
        game: game_status,
        loader: loader_status,
        assets,
        log_errors,
        game_running,
        warnings,
    })
}

#[tauri::command]
pub async fn collect_diagnostics(profile_id: Option<String>) -> Result<DiagnosticsReport, String> {
    blocking(move || diagnostics_report_impl(profile_id.as_deref())).await
}

#[tauri::command]
pub async fn export_support_bundle(
    destination: String,
    profile_id: Option<String>,
) -> Result<String, String> {
    blocking(move || {
        let path = PathBuf::from(&destination);
        if !path.is_absolute()
            || path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("zip"))
        {
            return Err("Choose an absolute .zip destination for the support bundle.".into());
        }
        let report = diagnostics_report_impl(profile_id.as_deref())?;
        let saved = settings::load().map_err(|error| error.to_string())?;
        let mut redacted_settings =
            serde_json::to_value(&saved).map_err(|error| error.to_string())?;
        if let Some(instances) = redacted_settings
            .get_mut("gameInstances")
            .and_then(serde_json::Value::as_array_mut)
        {
            for instance in instances {
                if let Some(object) = instance.as_object_mut() {
                    object.insert(
                        "path".to_string(),
                        serde_json::Value::String("<redacted-game-path>".to_string()),
                    );
                }
            }
        }
        if let Some(locals) = redacted_settings
            .get_mut("personalLocalMods")
            .and_then(serde_json::Value::as_array_mut)
        {
            for local in locals {
                if let Some(object) = local.as_object_mut() {
                    object.insert(
                        "path".to_string(),
                        serde_json::Value::String("<redacted-local-mod-path>".to_string()),
                    );
                }
            }
        }
        let profile = match profile_id.as_deref() {
            Some(profile_id) => recovered_profile_store(&settings::profiles_root())?
                .load(profile_id)
                .map_err(|error| error.to_string())?,
            None => None,
        };
        let temporary = unique_sibling(&path, "support")?;
        let result = (|| {
            let output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            let mut archive = zip::ZipWriter::new(output);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            archive
                .start_file("diagnostics.json", options)
                .map_err(|error| error.to_string())?;
            archive
                .write_all(&serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
            archive
                .start_file("settings-redacted.json", options)
                .map_err(|error| error.to_string())?;
            archive
                .write_all(
                    &serde_json::to_vec_pretty(&redacted_settings)
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
            if let Some(profile) = profile {
                archive
                    .start_file("profile.json", options)
                    .map_err(|error| error.to_string())?;
                archive
                    .write_all(
                        &serde_json::to_vec_pretty(&profile).map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())?;
            }
            let output = archive.finish().map_err(|error| error.to_string())?;
            output.sync_all().map_err(|error| error.to_string())?;
            atomic_replace_file(&temporary, &path).map_err(|error| error.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        Ok(path.to_string_lossy().into_owned())
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
}

const REPO_SLUG: &str = "artriy/Perfect-Sync";

#[tauri::command]
pub async fn check_update() -> Result<Option<UpdateInfo>, String> {
    blocking(|| {
        let release = resolver::fetch_latest_release(&http()?, REPO_SLUG)
            .map_err(|error| error.to_string())?;
        let tag = release.tag;
        if !perfect_sync_core::version::is_newer(&tag, env!("CARGO_PKG_VERSION")) {
            return Ok(None);
        }
        let mut release_url = url::Url::parse(&format!("https://github.com/{REPO_SLUG}"))
            .map_err(|error| error.to_string())?;
        release_url
            .path_segments_mut()
            .map_err(|_| "invalid release base URL")?
            .extend(["releases", "tag", tag.as_str()]);
        let url = release_url.to_string();
        validate_release_url(&url)?;
        Ok(Some(UpdateInfo {
            version: tag.trim_start_matches('v').to_string(),
            url,
        }))
    })
    .await
}

fn validate_release_url(value: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(value).map_err(|error| format!("invalid release URL: {error}"))?;
    let segments: Vec<&str> = parsed
        .path_segments()
        .ok_or("release URL cannot be a base URL")?
        .filter(|segment| !segment.is_empty())
        .collect();
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || segments.len() < 3
        || segments[0] != "artriy"
        || segments[1] != "Perfect-Sync"
        || segments[2] != "releases"
    {
        return Err(
            "only artriy/Perfect-Sync canonical GitHub Releases HTTPS links are allowed".into(),
        );
    }
    Ok(parsed)
}

#[cfg(windows)]
fn open_release_url_native(url: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            window: *mut std::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> isize;
    }
    let operation: Vec<u16> = std::ffi::OsStr::new("open")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let file: Vec<u16> = std::ffi::OsStr::new(url)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    if result <= 32 {
        Err(format!("native URL opener failed with status {result}"))
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn open_release_url_native(url: &str) -> Result<(), String> {
    process::command("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn open_release_url_native(url: &str) -> Result<(), String> {
    process::command("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn open_url(url: String) -> Result<(), String> {
    blocking(move || {
        let parsed = validate_release_url(&url)?;
        open_release_url_native(parsed.as_str())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DownloadHttp(&'static [u8]);

    impl Http for DownloadHttp {
        fn get_text(
            &self,
            _url: &str,
        ) -> Result<String, perfect_sync_core::resolver::ResolveError> {
            unreachable!()
        }

        fn get_bytes(
            &self,
            _url: &str,
        ) -> Result<Vec<u8>, perfect_sync_core::resolver::ResolveError> {
            Ok(self.0.to_vec())
        }
    }

    struct MapHttp {
        metadata: String,
        bytes: &'static [u8],
    }

    impl Http for MapHttp {
        fn get_text(
            &self,
            _url: &str,
        ) -> Result<String, perfect_sync_core::resolver::ResolveError> {
            Ok(self.metadata.clone())
        }

        fn get_bytes(
            &self,
            _url: &str,
        ) -> Result<Vec<u8>, perfect_sync_core::resolver::ResolveError> {
            Ok(self.bytes.to_vec())
        }
    }

    #[test]
    fn levelimposter_banner_proxy_accepts_only_official_bounded_images() {
        let official = "https://firebasestorage.googleapis.com/v0/b/levelimposter-347807.appspot.com/o/maps%2Fowner%2Fmap.png?alt=media&token=test";
        let data =
            levelimposter_banner_data_url(&DownloadHttp(b"\x89PNG\r\n\x1a\nimage"), official)
                .unwrap();
        assert!(data.starts_with("data:image/png;base64,"));
        assert!(levelimposter_banner_data_url(
            &DownloadHttp(b"\x89PNG\r\n\x1a\nimage"),
            "https://attacker.example/map.png",
        )
        .is_err());
        assert!(levelimposter_banner_data_url(&DownloadHttp(b"not an image"), official).is_err());
    }

    #[test]
    #[ignore]
    fn live_levelimposter_banner_proxy_fetches_current_map_image() {
        let http = UreqHttp::new(None);
        let text = http
            .get_text(&format!("{LEVELIMPOSTER_API}/maps/top"))
            .unwrap();
        let maps: Vec<LevelImposterMapMetadata> = levelimposter_callback(&text).unwrap();
        let banner = maps
            .into_iter()
            .find_map(|map| map.thumbnail_url)
            .expect("current map has a banner");

        let data = levelimposter_banner_data_url(&http, &banner).unwrap();

        assert!(data.starts_with("data:image/"));
        assert!(data.len() > 100);
    }

    #[test]
    fn levelimposter_map_download_requires_public_matching_storage_asset() {
        let id = "0ed1f569-eaf5-4ef6-b91c-f41ad78d4018";
        let metadata = |map_id: &str, url: &str, public: bool| {
            format!(
                r#"{{"v":1,"error":"","data":{{"id":"{map_id}","name":"Farm","isPublic":{public},"downloadURL":"{url}"}}}}"#
            )
        };
        let current = format!(
            "https://storage.googleapis.com/levelimposter-347807.appspot.com/maps/author/{id}.lim?signature=trusted"
        );
        let legacy = format!(
            "https://storage.googleapis.com/levelimposter/maps/{id}.lim2?signature=trusted"
        );
        for trusted in [&current, &legacy] {
            let http = MapHttp {
                metadata: metadata(id, trusted, true),
                bytes: b"map",
            };
            assert_eq!(
                levelimposter_map_download(&http, id).unwrap(),
                (id.to_string(), b"map".to_vec())
            );
        }

        let untrusted_bucket = format!("https://storage.googleapis.com/attacker/maps/{id}.lim");
        for (map_id, url, public) in [
            (
                "ef1d13ec-64ce-4c2c-a45d-816fc4ff46da",
                current.as_str(),
                true,
            ),
            (id, "https://example.invalid/map.lim2", true),
            (id, untrusted_bucket.as_str(), true),
            (id, current.as_str(), false),
        ] {
            let http = MapHttp {
                metadata: metadata(map_id, url, public),
                bytes: b"map",
            };
            assert!(levelimposter_map_download(&http, id).is_err());
        }
    }

    #[test]
    #[ignore]
    fn live_levelimposter_map_download_accepts_current_lim_asset() {
        let id = "c4ab53b1-1cc2-4080-9648-5f3d4ceab3d5";
        let (downloaded_id, bytes) = levelimposter_map_download(&UreqHttp::new(None), id).unwrap();

        assert_eq!(downloaded_id, id);
        assert!(!bytes.is_empty());
    }

    #[test]
    fn replacing_profile_maps_removes_only_previously_owned_maps() {
        let temp = tempfile::tempdir().unwrap();
        let profile_id = "maps";
        let directory = temp
            .path()
            .join(profile_id)
            .join("BepInEx/plugins/LevelImposter");
        fs::create_dir_all(&directory).unwrap();
        let old = "ef1d13ec-64ce-4c2c-a45d-816fc4ff46da";
        let unmanaged = "33eaaab4-b5fb-90f7-fb39-a41291409f93";
        let selected = "0ed1f569-eaf5-4ef6-b91c-f41ad78d4018";
        fs::write(directory.join(format!("{old}.lim2")), b"old").unwrap();
        fs::write(directory.join(format!("{unmanaged}.lim2")), b"user").unwrap();

        replace_profile_levelimposter_maps(
            temp.path(),
            profile_id,
            &[old.into()],
            &[(selected.into(), b"selected".to_vec())],
        )
        .unwrap();

        assert!(!directory.join(format!("{old}.lim2")).exists());
        assert_eq!(
            fs::read(directory.join(format!("{unmanaged}.lim2"))).unwrap(),
            b"user"
        );
        assert_eq!(
            fs::read(directory.join(format!("{selected}.lim2"))).unwrap(),
            b"selected"
        );
    }

    fn resolved_download(size: u64, digest: Option<&str>) -> ResolvedDownload {
        let digest = digest
            .map(|value| format!(r#","digest":"{value}""#))
            .unwrap_or_default();
        let release = resolver::parse_release(&format!(
            r#"{{"tag_name":"v1","assets":[{{"name":"mod.dll","browser_download_url":"https://example.invalid/mod.dll","size":{size}{digest}}}]}}"#
        ))
        .unwrap();
        let asset = &release.assets[0];
        ResolvedDownload {
            url: asset.url.clone(),
            asset_name: asset.name.clone(),
            version: release.tag.clone(),
            size: asset.size,
        }
    }

    #[test]
    fn resolved_install_verifies_metadata_before_publishing() {
        let temp = tempfile::tempdir().unwrap();
        let profile = "checked-download";

        let size_error = install_resolved(
            temp.path(),
            profile,
            &DownloadHttp(b"short"),
            &resolved_download(6, None),
            None,
        )
        .unwrap_err();
        assert!(size_error.contains("download size mismatch"));

        let digest_error = install_resolved(
            temp.path(),
            profile,
            &DownloadHttp(b"evil"),
            &resolved_download(
                4,
                Some("sha256:770e607624d689265ca6c44884d0807d9b054d23c473c106c72be9de08b7376c"),
            ),
            None,
        )
        .unwrap_err();
        assert!(digest_error.contains("SHA-256 digest"));
        assert!(!temp
            .path()
            .join(profile)
            .join("BepInEx")
            .join("plugins")
            .join("mod.dll")
            .exists());

        let mut zip = resolved_download(4, None);
        zip.asset_name = "mod.zip".into();
        assert_eq!(
            install_resolved(temp.path(), profile, &DownloadHttp(b"data"), &zip, None),
            Err("Catalog ZIP installs require an exact declared DLL name. Pick a DLL file manually.".into())
        );
    }

    #[test]
    fn local_mod_import_copies_only_a_bare_dll_and_records_file_source() {
        let temp = tempfile::tempdir().unwrap();
        let profiles_root = temp.path().join("profiles");
        let profile_id = "local-mod";
        ProfileStore::new(&profiles_root)
            .save(&ProfileRecord {
                id: profile_id.into(),
                name: "Local".into(),
                crew_color: "#fff".into(),
                game_build: None,
                game_instance_id: None,
                mods: Vec::new(),
                levelimposter_maps: Vec::new(),
            })
            .unwrap();
        let source = temp.path().join("CustomRoles.dll");
        fs::write(&source, b"local-dll").unwrap();

        let installed = install_local_mod_impl(&profiles_root, profile_id, &source).unwrap();

        assert_eq!(installed.mods.len(), 1);
        assert_eq!(installed.mods[0].package_id, "local/customroles.dll");
        assert_eq!(installed.mods[0].source, ModSource::File);
        assert_eq!(installed.mods[0].file.as_deref(), Some("CustomRoles.dll"));
        assert_eq!(
            fs::read(
                profiles_root
                    .join(profile_id)
                    .join("BepInEx/plugins/CustomRoles.dll")
            )
            .unwrap(),
            b"local-dll"
        );
        assert!(profile::to_manifest(&installed).mods.is_empty());
        let mut disabled = installed.clone();
        disabled.mods[0].enabled = false;
        assert!(profile::to_manifest(&disabled).mods.is_empty());

        let archive = temp.path().join("mod.zip");
        fs::write(&archive, b"not-a-dll").unwrap();
        assert!(install_local_mod_impl(&profiles_root, profile_id, &archive)
            .unwrap_err()
            .contains("DLL"));
    }

    #[test]
    fn lobby_digest_is_collision_resistant_and_stable() {
        assert_eq!(lobby_digest("abc"), lobby_digest("abc"));
        assert_ne!(lobby_digest("abc"), lobby_digest("abd"));
        assert_eq!(lobby_digest("anything").len(), 64);
    }

    #[test]
    fn town_of_us_dependencies_are_owned_by_the_complete_zip() {
        let catalog = bundled_catalog();
        let root = "AU-Avengers/TOU-Mira".to_string();
        let ordered = deps::resolve(&catalog, std::slice::from_ref(&root))
            .unwrap()
            .ordered;
        assert_eq!(ordered, [root]);
        assert!(tou_bundle_dependency("NuclearPowered/Reactor"));
        assert!(tou_bundle_dependency("All-Of-Us-Mods/MiraAPI"));
        assert!(tou_bundle_dependency("miniduikboot/Mini.RegionInstall"));
    }

    #[test]
    fn bundled_catalog_allows_user_selected_role_mod_combinations() {
        let catalog = bundled_catalog();
        validate_authoritative_dependencies(
            &catalog,
            &[
                "AU-Avengers/TOU-Mira".into(),
                "TheOtherRolesAU/TheOtherRoles".into(),
            ],
        )
        .unwrap();
    }

    #[test]
    fn bundled_catalog_exposes_the_expanded_verified_mod_set() {
        let catalog = bundled_catalog();
        assert_eq!(catalog.mods.len(), 30);
        for id in [
            "D1GQ/BetterAmongUs",
            "xChipseq/VanillaEnhancements",
            "Mr-Fluuff/StellarRolesAU",
            "SuperNewRoles/SuperNewRoles",
            "CallOfCreator/NewMod",
            "RaresHonour/HostGuard",
            "AtomicTyler1/MinimumLevel",
        ] {
            let entry = catalog.get(id).unwrap_or_else(|| panic!("missing {id}"));
            assert!(entry.repo.is_some());
            assert!(entry.asset_rules.dll_name.is_some());
        }
    }

    #[test]
    fn lobby_rows_only_mark_catalog_dependencies_as_managed() {
        let catalog = bundled_catalog();
        let selected = vec![
            "NuclearPowered/Reactor".to_string(),
            "All-Of-Us-Mods/MiraAPI".to_string(),
            "AU-Avengers/TOU-Mira".to_string(),
        ];
        let managed = selected_dependencies(&catalog, &selected).unwrap();
        assert!(managed.contains("nuclearpowered/reactor"));
        assert!(!managed.contains("all-of-us-mods/miraapi"));
        assert!(!managed.contains("au-avengers/tou-mira"));
    }

    #[test]
    fn dependency_normalization_prunes_orphans_but_keeps_explicit_roots() {
        let temp = tempfile::tempdir().unwrap();
        let profile_id = "deps";
        let plugin = |id: &str, file: &str, managed: bool| InstalledMod {
            package_id: id.into(),
            name: id.into(),
            repo: Some(id.into()),
            version: "v1".into(),
            versions: vec!["v1".into()],
            enabled: true,
            source: ModSource::Github,
            tags: Vec::new(),
            managed,
            update: None,
            file: Some(file.into()),
            asset: Some(file.into()),
        };
        for file in ["Tou.dll", "Reactor.dll", "Mira.dll"] {
            profile::install_plugin_bytes(temp.path(), profile_id, file, file.as_bytes()).unwrap();
        }
        let mut record = ProfileRecord {
            id: profile_id.into(),
            name: "Dependencies".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: vec![
                plugin("AU-Avengers/TOU-Mira", "Tou.dll", false),
                plugin("NuclearPowered/Reactor", "Reactor.dll", false),
                plugin("All-Of-Us-Mods/MiraAPI", "Mira.dll", true),
            ],
            levelimposter_maps: Vec::new(),
        };
        let catalog = bundled_catalog();
        normalize_dependency_ownership(temp.path(), profile_id, &mut record, &catalog).unwrap();
        assert!(record
            .mods
            .iter()
            .find(|item| item.package_id == "NuclearPowered/Reactor")
            .is_some_and(|item| !item.managed));
        record
            .mods
            .retain(|item| item.package_id != "AU-Avengers/TOU-Mira");
        profile::remove_plugin(temp.path(), profile_id, "Tou.dll").unwrap();
        normalize_dependency_ownership(temp.path(), profile_id, &mut record, &catalog).unwrap();
        assert_eq!(record.mods.len(), 1);
        assert_eq!(record.mods[0].package_id, "NuclearPowered/Reactor");
        assert!(!temp
            .path()
            .join(profile_id)
            .join("BepInEx/plugins/Mira.dll")
            .exists());
    }

    #[test]
    fn town_of_us_takeover_keeps_other_mod_dependencies_for_restoration() {
        let temp = tempfile::tempdir().unwrap();
        let profile_id = "tou-takeover";
        let catalog = parse(
            r#"{"schema":1,"mods":[
                {"id":"AU-Avengers/TOU-Mira","name":"Town of Us","summary":"","repo":"AU-Avengers/TOU-Mira","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}},
                {"id":"Owner/OtherMod","name":"Other Mod","summary":"","repo":"Owner/OtherMod","tags":[],"trust":"trusted","dependencies":["All-Of-Us-Mods/MiraAPI","miniduikboot/Mini.RegionInstall"],"assetRules":{}},
                {"id":"All-Of-Us-Mods/MiraAPI","name":"MiraAPI","summary":"","repo":"All-Of-Us-Mods/MiraAPI","tags":[],"trust":"trusted","dependencies":["NuclearPowered/Reactor"],"assetRules":{}},
                {"id":"NuclearPowered/Reactor","name":"Reactor","summary":"","repo":"NuclearPowered/Reactor","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}},
                {"id":"miniduikboot/Mini.RegionInstall","name":"Mini","summary":"","repo":"miniduikboot/Mini.RegionInstall","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}}
            ]}"#,
        )
        .unwrap();
        let installed = |id: &str, file: &str, managed: bool| InstalledMod {
            package_id: id.into(),
            name: id.into(),
            repo: Some(id.into()),
            version: "v1".into(),
            versions: vec!["v1".into()],
            enabled: true,
            source: ModSource::Github,
            tags: Vec::new(),
            managed,
            update: None,
            file: Some(file.into()),
            asset: Some(file.into()),
        };
        let mut record = ProfileRecord {
            id: profile_id.into(),
            name: "Takeover".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: vec![
                installed("Owner/OtherMod", "Other.dll", false),
                installed("All-Of-Us-Mods/MiraAPI", "MiraAPI.dll", true),
                installed("NuclearPowered/Reactor", "Reactor.dll", true),
                installed(
                    "miniduikboot/Mini.RegionInstall",
                    "Mini.RegionInstall.dll",
                    true,
                ),
                installed("AU-Avengers/TOU-Mira", "TownOfUsMira.dll", false),
            ],
            levelimposter_maps: Vec::new(),
        };
        for file in [
            "Other.dll",
            "MiraAPI.dll",
            "Reactor.dll",
            "Mini.RegionInstall.dll",
            "TownOfUsMira.dll",
        ] {
            profile::install_plugin_bytes(temp.path(), profile_id, file, file.as_bytes()).unwrap();
        }

        normalize_dependency_ownership(temp.path(), profile_id, &mut record, &catalog).unwrap();
        assert_eq!(record.mods.len(), 5);
        record
            .mods
            .retain(|installed| !is_tou_mira(&installed.package_id));
        profile::remove_plugin(temp.path(), profile_id, "TownOfUsMira.dll").unwrap();
        normalize_dependency_ownership(temp.path(), profile_id, &mut record, &catalog).unwrap();

        assert_eq!(record.mods.len(), 4);
        for file in ["MiraAPI.dll", "Reactor.dll", "Mini.RegionInstall.dll"] {
            assert!(temp
                .path()
                .join(profile_id)
                .join("BepInEx/plugins")
                .join(file)
                .is_file());
        }
    }

    #[test]
    fn github_release_url_allowlist_rejects_unsafe_open_targets() {
        assert!(
            validate_release_url("https://github.com/artriy/Perfect-Sync/releases/tag/v1").is_ok()
        );
        for unsafe_url in [
            "http://github.com/artriy/Perfect-Sync/releases",
            "https://github.com.evil.invalid/artriy/Perfect-Sync/releases",
            "https://github.com/artriy/Perfect-Sync/issues",
            "https://user@github.com/artriy/Perfect-Sync/releases",
            "https://github.com/artriy/Perfect-Sync/releases?next=evil",
            "https://github.com/attacker/payload/releases/tag/v1",
            "https://github.com/artriy/Perfect-Sync/releases#foreign",
        ] {
            assert!(validate_release_url(unsafe_url).is_err(), "{unsafe_url}");
        }
    }

    #[test]
    fn profile_transaction_failure_preserves_record_and_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let store = ProfileStore::new(&root);
        let record = ProfileRecord {
            id: "stable".into(),
            name: "Old".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: Vec::new(),
            levelimposter_maps: Vec::new(),
        };
        store.save(&record).unwrap();
        profile::install_plugin_bytes(&root, "stable", "Owned.dll", b"old").unwrap();
        let original_manifest = fs::read(root.join("stable/profile.json")).unwrap();
        let original_plugin = fs::read(root.join("stable/BepInEx/plugins/Owned.dll")).unwrap();

        let error = profile_transaction(&root, "stable", |stage_root, _| {
            profile::remove_plugin(stage_root, "stable", "Owned.dll").unwrap();
            Err::<(), _>("injected failure".into())
        })
        .unwrap_err();

        assert_eq!(error, "injected failure");
        assert_eq!(
            fs::read(root.join("stable/profile.json")).unwrap(),
            original_manifest
        );
        assert_eq!(
            fs::read(root.join("stable/BepInEx/plugins/Owned.dll")).unwrap(),
            original_plugin
        );
    }

    #[test]
    fn profile_transaction_artifacts_never_enter_the_profile_store() {
        use std::cell::RefCell;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let observed_stage = RefCell::new(None);
        profile_transaction(&root, "new-profile", |stage_root, stage_store| {
            observed_stage.replace(Some(stage_root.to_path_buf()));
            stage_store
                .save(&ProfileRecord {
                    id: "new-profile".into(),
                    name: "New".into(),
                    crew_color: "#fff".into(),
                    game_build: None,
                    game_instance_id: None,
                    mods: Vec::new(),
                    levelimposter_maps: Vec::new(),
                })
                .map_err(|error| error.to_string())
        })
        .unwrap();

        let stage = observed_stage.into_inner().unwrap();
        assert_eq!(stage.parent(), root.parent());
        assert_ne!(stage.parent(), Some(root.as_path()));
        let backup_candidate = unique_sibling(&root, "backup").unwrap();
        assert_eq!(backup_candidate.parent(), root.parent());
        assert_ne!(backup_candidate.parent(), Some(root.as_path()));
        assert!(!stage.exists());
        assert_eq!(ProfileStore::new(&root).list().unwrap().len(), 1);
        assert_eq!(
            fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>(),
            vec![std::ffi::OsString::from("new-profile")]
        );
    }

    #[test]
    fn interrupted_profile_swap_recovers_before_list_and_load() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let id = "recoverable";
        let original = ProfileRecord {
            id: id.into(),
            name: "Original".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: Vec::new(),
            levelimposter_maps: Vec::new(),
        };
        ProfileStore::new(&root).save(&original).unwrap();
        let paths = allocate_profile_transaction_paths(&root).unwrap();
        fs::create_dir(&paths.stage_root).unwrap();
        fs::create_dir(&paths.backup_root).unwrap();
        copy_profile_tree(&root.join(id), &paths.stage_root.join(id)).unwrap();
        let stage_store = ProfileStore::new(&paths.stage_root);
        let mut committed = stage_store.load(id).unwrap().unwrap();
        committed.name = "Committed".into();
        stage_store.save(&committed).unwrap();
        write_profile_recovery_journal(&paths.journal, id, ProfileRecoveryAction::Publish).unwrap();
        fs::rename(root.join(id), paths.backup_root.join(id)).unwrap();

        let recovered = recovered_profile_store(&root).unwrap();
        let listed = recovered.list().unwrap();

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Committed");
        assert_eq!(recovered.load(id).unwrap().unwrap().name, "Committed");
        assert!(!paths.stage_root.exists());
        assert!(!paths.backup_root.exists());
        assert!(!paths.journal.exists());
    }

    #[test]
    fn interrupted_profile_swap_restores_backup_when_publish_stage_is_gone() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let id = "restore-original";
        let original = ProfileRecord {
            id: id.into(),
            name: "Original".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: Vec::new(),
            levelimposter_maps: Vec::new(),
        };
        ProfileStore::new(&root).save(&original).unwrap();
        let paths = allocate_profile_transaction_paths(&root).unwrap();
        fs::create_dir(&paths.stage_root).unwrap();
        fs::create_dir(&paths.backup_root).unwrap();
        write_profile_recovery_journal(&paths.journal, id, ProfileRecoveryAction::Publish).unwrap();
        fs::rename(root.join(id), paths.backup_root.join(id)).unwrap();

        let recovered = recovered_profile_store(&root).unwrap();

        assert_eq!(recovered.load(id).unwrap().unwrap().name, "Original");
        assert!(!paths.stage_root.exists());
        assert!(!paths.backup_root.exists());
        assert!(!paths.journal.exists());
    }

    #[test]
    fn ambiguous_profile_recovery_fails_closed_and_retains_every_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let id = "ambiguous";
        let record = ProfileRecord {
            id: id.into(),
            name: "Original".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: Vec::new(),
            levelimposter_maps: Vec::new(),
        };
        ProfileStore::new(&root).save(&record).unwrap();
        let paths = allocate_profile_transaction_paths(&root).unwrap();
        fs::create_dir(&paths.stage_root).unwrap();
        fs::create_dir(&paths.backup_root).unwrap();
        copy_profile_tree(&root.join(id), &paths.stage_root.join(id)).unwrap();
        write_profile_recovery_journal(&paths.journal, id, ProfileRecoveryAction::Publish).unwrap();
        fs::rename(root.join(id), paths.backup_root.join(id)).unwrap();
        copy_profile_tree(&paths.backup_root.join(id), &root.join(id)).unwrap();

        let error = recovered_profile_store(&root).err().unwrap();

        assert!(error.contains("recovery evidence was retained"));
        assert!(root.join(id).exists());
        assert!(paths.stage_root.join(id).exists());
        assert!(paths.backup_root.join(id).exists());
        assert!(paths.journal.exists());
    }

    #[test]
    fn failed_commit_retains_backup_when_rollback_fails() {
        let temp = tempfile::tempdir().unwrap();
        let stage_root = temp.path().join("stage");
        let backup_root = temp.path().join("backup");
        let backup = backup_root.join("stable");
        fs::create_dir_all(&stage_root).unwrap();
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("profile.json"), b"intact").unwrap();

        let error = failed_profile_commit(
            &backup,
            &io::Error::other("publish blocked"),
            Err("rollback blocked".into()),
        );

        assert!(stage_root.exists());
        assert_eq!(fs::read(backup.join("profile.json")).unwrap(), b"intact");
        assert!(error.contains(&backup.display().to_string()));
    }

    #[test]
    fn vanilla_doorstop_move_is_reversible_and_owned() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("BepInEx")).unwrap();
        fs::write(
            temp.path().join("BepInEx").join(APP_LOADER_MARKER),
            b"owned",
        )
        .unwrap();
        fs::write(temp.path().join("winhttp.dll"), b"doorstop").unwrap();
        assert!(disable_doorstop(temp.path()).unwrap());
        assert!(!temp.path().join("winhttp.dll").exists());
        assert_eq!(
            fs::read(temp.path().join(DISABLED_DOORSTOP)).unwrap(),
            b"doorstop"
        );
        restore_doorstop(temp.path()).unwrap();
        assert_eq!(
            fs::read(temp.path().join("winhttp.dll")).unwrap(),
            b"doorstop"
        );
    }

    #[test]
    fn vanilla_launch_blocks_unowned_loader_and_only_rolls_back_its_own_move() {
        use std::cell::Cell;

        let unowned = tempfile::tempdir().unwrap();
        fs::write(unowned.path().join("winhttp.dll"), b"foreign").unwrap();
        let launched = Cell::new(false);
        let error = launch_without_doorstop(unowned.path(), || {
            launched.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(error.contains("unowned winhttp.dll"));
        assert!(!launched.get());

        let already_disabled = tempfile::tempdir().unwrap();
        fs::create_dir_all(already_disabled.path().join("BepInEx")).unwrap();
        fs::write(
            already_disabled
                .path()
                .join("BepInEx")
                .join(APP_LOADER_MARKER),
            b"owned",
        )
        .unwrap();
        fs::write(already_disabled.path().join(DISABLED_DOORSTOP), b"disabled").unwrap();
        assert_eq!(
            launch_without_doorstop(already_disabled.path(), || Err("spawn failed".into()))
                .unwrap_err(),
            "spawn failed"
        );
        assert!(already_disabled.path().join(DISABLED_DOORSTOP).exists());
        assert!(!already_disabled.path().join("winhttp.dll").exists());

        let disabled_here = tempfile::tempdir().unwrap();
        fs::create_dir_all(disabled_here.path().join("BepInEx")).unwrap();
        fs::write(
            disabled_here.path().join("BepInEx").join(APP_LOADER_MARKER),
            b"owned",
        )
        .unwrap();
        fs::write(disabled_here.path().join("winhttp.dll"), b"owned").unwrap();
        assert_eq!(
            launch_without_doorstop(disabled_here.path(), || Err("spawn failed".into()))
                .unwrap_err(),
            "spawn failed"
        );
        assert_eq!(
            fs::read(disabled_here.path().join("winhttp.dll")).unwrap(),
            b"owned"
        );
        assert!(!disabled_here.path().join(DISABLED_DOORSTOP).exists());
    }

    #[test]
    fn failed_game_prepare_restores_all_touched_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("BepInEx/config")).unwrap();
        fs::write(temp.path().join("winhttp.dll"), b"old-loader").unwrap();
        fs::write(
            temp.path().join("BepInEx/config/BepInEx.cfg"),
            b"old-config",
        )
        .unwrap();
        let error = game_artifact_transaction(temp.path(), || {
            fs::write(temp.path().join("winhttp.dll"), b"new-loader").unwrap();
            fs::create_dir_all(temp.path().join("BepInEx/plugins")).unwrap();
            fs::write(temp.path().join("BepInEx/plugins/New.dll"), b"new").unwrap();
            Err::<(), _>("injected prepare failure".into())
        })
        .unwrap_err();
        assert_eq!(error, "injected prepare failure");
        assert_eq!(
            fs::read(temp.path().join("winhttp.dll")).unwrap(),
            b"old-loader"
        );
        assert_eq!(
            fs::read(temp.path().join("BepInEx/config/BepInEx.cfg")).unwrap(),
            b"old-config"
        );
        assert!(!temp.path().join("BepInEx/plugins/New.dll").exists());
    }

    fn epic_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn epic_helper_zip_requires_one_exact_nonempty_executable() {
        assert_eq!(
            validated_epic_executable(&epic_zip(&[("EpicGamesStarter.exe", b"verified")])).unwrap(),
            b"verified"
        );
        for invalid in [
            epic_zip(&[("other.exe", b"x")]),
            epic_zip(&[("EpicGamesStarter.exe", b"")]),
        ] {
            assert!(validated_epic_executable(&invalid).is_err());
        }
        let pin = pinned_epic_download().unwrap();
        assert_eq!(pin.url, EPIC_STARTER_URL);
        assert_eq!(pin.size.bytes(), EPIC_STARTER_SIZE);
    }

    #[test]
    fn bundled_install_policy_overrides_stale_town_of_us_policy() {
        let mut stale = parse(
            r#"{"schema":1,"mods":[{"id":"NuclearPowered/Reactor","name":"Reactor","summary":"","repo":"NuclearPowered/Reactor","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}},{"id":"AU-Avengers/TOU-Mira","name":"Town of Us - Mira","summary":"stale","repo":"AU-Avengers/TOU-Mira","tags":[],"trust":"trusted","dependencies":["NuclearPowered/Reactor"],"assetRules":{"perArch":{},"dllName":"Wrong.dll","bundlesLoader":false}}]}"#,
        )
        .unwrap();

        apply_bundled_install_policy(&mut stale);

        let entry = stale.get("AU-Avengers/TOU-Mira").unwrap();
        assert_eq!(entry.asset_rules.per_arch.len(), 2);
        assert_eq!(
            entry.asset_rules.dll_name.as_deref(),
            Some("TownOfUsMira.dll")
        );
        assert!(entry.asset_rules.bundles_loader);
        assert!(entry.dependencies.is_empty());
    }

    #[test]
    fn bundled_display_policy_exposes_current_town_of_us_bundle_contents() {
        let mut item = catalog_item(
            bundled_catalog()
                .get("AU-Avengers/TOU-Mira")
                .unwrap()
                .clone(),
        );
        item.dependencies.push("stale/dependency".into());
        item.included.clear();

        apply_bundled_display_policy(std::slice::from_mut(&mut item));

        assert!(item.dependencies.is_empty());
        assert_eq!(
            item.included,
            [
                "MiraAPI",
                "Reactor",
                "Mini.RegionInstall with the Town of Us server config",
                "Town of Us cosmetics"
            ]
        );
    }

    #[test]
    fn town_of_us_options_expose_only_the_target_specific_complete_zip() {
        let catalog = bundled_catalog();
        let rules = &catalog.get("AU-Avengers/TOU-Mira").unwrap().asset_rules;
        let release = resolver::parse_release(
            r#"{"tag_name":"1.6.3-beta2","assets":[{"name":"MiraAPI.dll","browser_download_url":"https://example.invalid/MiraAPI.dll","size":10},{"name":"TouMirav1.6.3b2-x64-epic-msstore.zip","browser_download_url":"https://example.invalid/x64.zip","size":30},{"name":"TouMirav1.6.3b2-x86-steam-itch.zip","browser_download_url":"https://example.invalid/x86.zip","size":31},{"name":"TouMirav1.6.3b2-x86-macOS-linux.zip","browser_download_url":"https://example.invalid/unix.zip","size":32},{"name":"TownOfUsMira.dll","browser_download_url":"https://example.invalid/TownOfUsMira.dll","size":20}]}"#,
        )
        .unwrap();

        for (arch, store, runtime, expected) in [
            (
                "x64",
                Store::Epic,
                Runtime::Native,
                "TouMirav1.6.3b2-x64-epic-msstore.zip",
            ),
            (
                "x86",
                Store::Steam,
                Runtime::Native,
                "TouMirav1.6.3b2-x86-steam-itch.zip",
            ),
            (
                "x86",
                Store::Steam,
                Runtime::Proton,
                "TouMirav1.6.3b2-x86-macOS-linux.zip",
            ),
        ] {
            let options = install_options_for_profile(
                vec![release.clone()],
                TOU_MIRA_ID,
                rules,
                arch,
                store,
                runtime,
            )
            .unwrap();
            assert_eq!(options.len(), 1);
            assert_eq!(options[0].asset_name, expected);
        }
        assert!(install_options_for_profile(
            vec![release],
            TOU_MIRA_ID,
            rules,
            "x86",
            Store::Epic,
            Runtime::Native,
        )
        .is_err());
    }

    #[test]
    fn catalog_repo_alias_reuses_and_unhides_canonical_entry() {
        let catalog = parse(
            r#"{"schema":1,"mods":[{"id":"Canonical/Mod","name":"Canonical","summary":"hosted","repo":"Alias/Repository","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}}]}"#,
        )
        .unwrap();
        let entry = catalog_entry_for(&catalog, "Alias/Repository")
            .unwrap()
            .clone();
        let mut state = CatalogEnvelope {
            version: catalog_envelope_version(),
            order_policy_version: CATALOG_ORDER_POLICY_VERSION,
            display: vec![CatalogListItem {
                id: "Alias/Repository".into(),
                name: "Duplicate".into(),
                repo: "Alias/Repository".into(),
                summary: String::new(),
                tags: Vec::new(),
                dependencies: Vec::new(),
                included: Vec::new(),
                latest: String::new(),
                trust: Trust::Flagged,
                extra: HashMap::new(),
            }],
            hosted_ids: vec![entry.id.clone()],
            hidden_hosted_ids: vec![entry.id.clone()],
            hosted_catalog: Some(catalog),
        };
        let effective_repo = entry.repo.clone().unwrap_or_else(|| entry.id.clone());

        ensure_display_catalog_state(
            &mut state,
            "Alias/Repository",
            &entry.id,
            &effective_repo,
            &entry.name,
            entry.summary,
            entry.tags,
        );

        assert!(state.hidden_hosted_ids.is_empty());
        assert_eq!(state.display.len(), 1);
        assert_eq!(state.display[0].id, "Canonical/Mod");
        assert_eq!(state.display[0].repo, "Alias/Repository");
    }

    #[test]
    fn hosted_reconciliation_keeps_custom_order_and_updates_hosted_entries() {
        let hosted = parse(
            r#"{"schema":1,"mods":[{"id":"Owner/Hosted","name":"New","summary":"fresh","repo":"Owner/Hosted","tags":[],"trust":"trusted","dependencies":[],"assetRules":{"perArch":{},"bundlesLoader":false}}]}"#,
        )
        .unwrap();
        let custom = CatalogListItem {
            id: "User/Custom".into(),
            name: "Custom".into(),
            repo: "User/Custom".into(),
            summary: String::new(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            included: Vec::new(),
            latest: String::new(),
            trust: Trust::Flagged,
            extra: HashMap::new(),
        };
        let old_hosted = CatalogListItem {
            id: "Owner/Hosted".into(),
            name: "Old".into(),
            repo: "Owner/Hosted".into(),
            summary: String::new(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            included: Vec::new(),
            latest: String::new(),
            trust: Trust::Flagged,
            extra: HashMap::new(),
        };
        let state = CatalogEnvelope {
            version: catalog_envelope_version(),
            order_policy_version: CATALOG_ORDER_POLICY_VERSION,
            display: vec![custom, old_hosted],
            hosted_ids: vec!["Owner/Hosted".into()],
            hidden_hosted_ids: Vec::new(),
            hosted_catalog: None,
        };
        let reconciled = reconcile_hosted(state, &hosted);
        assert_eq!(reconciled.display[0].id, "User/Custom");
        assert_eq!(reconciled.display[1].name, "New");
        assert_eq!(
            reconciled.hosted_catalog.as_ref().unwrap().mods[0].id,
            "Owner/Hosted"
        );
    }

    #[test]
    fn hosted_reconciliation_honors_tombstones_and_prunes_removed_provenance() {
        let hosted = parse(
            r#"{"schema":1,"mods":[{"id":"Owner/Hidden","name":"Hidden","summary":"","repo":"Owner/Hidden","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}}]}"#,
        )
        .unwrap();
        let item = |id: &str| CatalogListItem {
            id: id.into(),
            name: id.into(),
            repo: id.into(),
            summary: String::new(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            included: Vec::new(),
            latest: String::new(),
            trust: Trust::Flagged,
            extra: HashMap::new(),
        };
        let state = CatalogEnvelope {
            version: catalog_envelope_version(),
            order_policy_version: CATALOG_ORDER_POLICY_VERSION,
            display: vec![
                item("User/Custom"),
                item("Owner/Hidden"),
                item("Owner/Gone"),
            ],
            hosted_ids: vec!["Owner/Hidden".into(), "Owner/Gone".into()],
            hidden_hosted_ids: vec!["Owner/Hidden".into(), "Owner/Gone".into()],
            hosted_catalog: None,
        };
        let reconciled = reconcile_hosted(state, &hosted);
        assert_eq!(
            reconciled
                .display
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["User/Custom"]
        );
        assert_eq!(reconciled.hidden_hosted_ids, vec!["Owner/Hidden"]);
        assert_eq!(reconciled.hosted_ids, vec!["Owner/Hidden"]);
    }

    #[test]
    fn selected_game_folder_must_contain_the_executable() {
        let temp = tempfile::tempdir().unwrap();
        let error = validate_game_dir(temp.path()).unwrap_err();
        assert!(error.contains(process::GAME_EXE));
    }

    #[test]
    fn writable_game_folder_validation_leaves_no_probe() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(process::GAME_EXE), b"MZ").unwrap();
        validate_game_dir(temp.path()).unwrap();
        assert!(fs::read_dir(temp.path()).unwrap().flatten().all(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".perfectsync-write-test-")
        }));
    }

    #[test]
    fn managed_game_copy_preserves_tree_and_skips_internal_work_files() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("managed");
        fs::create_dir_all(source.join("Among Us_Data")).unwrap();
        fs::write(source.join(process::GAME_EXE), b"MZ").unwrap();
        fs::write(
            source.join("Among Us_Data/globalgamemanagers"),
            b"game-data",
        )
        .unwrap();
        fs::write(source.join(".perfectsync-stage-stale"), b"stale").unwrap();
        let mut files = 0;
        let mut bytes = 0;

        copy_game_tree(&source, &destination, &mut files, &mut bytes).unwrap();

        assert_eq!(files, 2);
        assert_eq!(bytes, 11);
        assert_eq!(
            fs::read(destination.join(process::GAME_EXE)).unwrap(),
            b"MZ"
        );
        assert_eq!(
            fs::read(destination.join("Among Us_Data/globalgamemanagers")).unwrap(),
            b"game-data"
        );
        assert!(!destination.join(".perfectsync-stage-stale").exists());
    }
    #[test]
    fn nonexistent_profile_loader_preflight_creates_no_layout_or_record() {
        use std::cell::Cell;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let ran = Cell::new(false);
        let result = with_existing_profile_layout(&root, "missing", || {
            ran.set(true);
            Ok(())
        });

        assert_eq!(result.unwrap_err(), "profile not found");
        assert!(!ran.get());
        assert!(ProfileStore::new(&root).list().unwrap().is_empty());
        assert!(!root.join("missing").exists());
    }

    #[test]
    fn launch_readiness_rejects_runtime_guidance_while_sync_can_return_it() {
        assert!(require_launch_ready(None).is_ok());
        assert_eq!(
            require_launch_ready(Some("configure the runtime override first".into())).unwrap_err(),
            "configure the runtime override first"
        );
    }

    #[test]
    fn complete_owned_loader_survives_offline_acquisition() {
        assert_eq!(
            resolve_loader_for_ensure(true, || Err("offline".into())).unwrap(),
            None
        );
        assert_eq!(
            download_loader_for_ensure(true, || Err("offline".into())).unwrap(),
            None
        );
        assert_eq!(
            resolve_loader_for_ensure(false, || Err("offline".into())).unwrap_err(),
            "offline"
        );
        assert_eq!(
            download_loader_for_ensure(false, || Err("offline".into())).unwrap_err(),
            "offline"
        );
    }

    #[test]
    fn pinned_loader_selects_exact_build_753_archive_for_each_arch() {
        assert_eq!(
            pinned_loader("x86").unwrap(),
            (
                PINNED_LOADER_VERSION.to_string(),
                PINNED_LOADER_X86_URL.to_string()
            )
        );
        assert_eq!(
            pinned_loader("x64").unwrap(),
            (
                PINNED_LOADER_VERSION.to_string(),
                PINNED_LOADER_X64_URL.to_string()
            )
        );
        assert!(pinned_loader("arm64").is_err());
    }

    #[test]
    fn hosted_only_dependency_is_not_authoritative() {
        let hosted = parse(
            r#"{"schema":1,"mods":[
                {"id":"Owner/Root","name":"Root","summary":"","repo":"Owner/Root","tags":[],"trust":"trusted","dependencies":["Attacker/Dependency"],"assetRules":{}},
                {"id":"Attacker/Dependency","name":"Dependency","summary":"","repo":"Attacker/Dependency","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}}
            ]}"#,
        )
        .unwrap();

        let error =
            validate_authoritative_dependencies(&hosted, &["Owner/Root".into()]).unwrap_err();
        assert!(error.contains("not authorized by the bundled catalog"));
    }

    #[test]
    fn bundled_identity_cannot_be_reused_by_an_explicit_root_with_another_repo() {
        let custom = parse(
            r#"{"schema":1,"mods":[
                {"id":"Owner/Manual","name":"Manual","summary":"","repo":"Owner/Manual","tags":[],"trust":"flagged","dependencies":[],"assetRules":{}}
            ]}"#,
        )
        .unwrap();
        validate_authoritative_dependencies(&custom, &["Owner/Manual".into()]).unwrap();

        let impersonated = parse(
            r#"{"schema":1,"mods":[
                {"id":"NuclearPowered/Reactor","name":"Reactor","summary":"","repo":"Attacker/Reactor","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}}
            ]}"#,
        )
        .unwrap();
        let error =
            validate_authoritative_dependencies(&impersonated, &["NuclearPowered/Reactor".into()])
                .unwrap_err();
        assert!(error.contains("Catalog root NuclearPowered/Reactor"));
        assert!(error.contains("bundled authoritative identity"));
    }

    #[test]
    fn complete_installed_town_of_us_dependency_skips_network_resolution() {
        struct NoNetwork;

        impl Http for NoNetwork {
            fn get_text(
                &self,
                _url: &str,
            ) -> Result<String, perfect_sync_core::resolver::ResolveError> {
                panic!("a complete installed dependency must not fetch release metadata")
            }

            fn get_bytes(
                &self,
                _url: &str,
            ) -> Result<Vec<u8>, perfect_sync_core::resolver::ResolveError> {
                panic!("a complete installed dependency must not be downloaded again")
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let profile_id = "cached-tou";
        let package = epic_zip(&[
            ("BepInEx/plugins/Mini.RegionInstall.dll", b"mini"),
            ("BepInEx/plugins/MiraAPI.dll", b"mira"),
            ("BepInEx/plugins/Reactor.dll", b"reactor"),
            ("BepInEx/plugins/touhats.bundle", b"bundle"),
            ("BepInEx/plugins/touhats.catalog", b"catalog"),
            ("BepInEx/plugins/TownOfUsMira.dll", b"tou"),
            ("BepInEx/config/at.duikbo.regioninstall.cfg", b"config"),
        ]);
        profile::install_tou_bundle_zip_bytes(temp.path(), profile_id, &package).unwrap();
        let mut record = ProfileRecord {
            id: profile_id.into(),
            name: "Cached Town of Us".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: vec![InstalledMod {
                package_id: TOU_MIRA_ID.into(),
                name: "Town of Us - Mira".into(),
                repo: Some(TOU_MIRA_ID.into()),
                version: "1.6.3-beta2".into(),
                versions: vec!["1.6.3-beta2".into()],
                enabled: true,
                source: ModSource::Github,
                tags: vec![ModTag::Role, ModTag::AllClient],
                managed: false,
                update: None,
                file: Some("TownOfUsMira.dll".into()),
                asset: Some("TouMirav1.6.3b2-x86-steam-itch.zip".into()),
            }],
            levelimposter_maps: Vec::new(),
        };
        let catalog = bundled_catalog();
        let context = InstallContext {
            stage_root: temp.path(),
            profile_id,
            http: &NoNetwork,
            catalog: &catalog,
            arch: "x86",
            store: Store::Steam,
            runtime: Runtime::Native,
        };

        install_catalog_latest(&context, &mut record, TOU_MIRA_ID, true, &[]).unwrap();

        assert_eq!(record.mods.len(), 1);
        assert!(!record.mods[0].managed);
    }

    #[test]
    fn default_catalog_order_prioritizes_major_mods_and_puts_dependencies_last() {
        let catalog = bundled_catalog();
        let mut display: Vec<_> = catalog.mods.into_iter().map(catalog_item).collect();
        apply_default_catalog_order(&mut display);

        assert_eq!(
            display
                .iter()
                .take(PRIORITY_CATALOG_IDS.len())
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            PRIORITY_CATALOG_IDS
        );
        let dependency_start = display
            .iter()
            .position(|item| catalog_display_rank(item) == usize::MAX)
            .unwrap();
        assert!(display[dependency_start..]
            .iter()
            .all(|item| catalog_display_rank(item) == usize::MAX));
    }

    #[test]
    fn town_of_us_extensions_declare_town_of_us_as_their_dependency() {
        let catalog = bundled_catalog();
        for id in [
            "DivaniNL/TownOfUsMiraDivaniModsAddOn",
            "Mehzxzz/TownOfExtra",
            "rewalo/TownOfUsMiraRolesExtension",
        ] {
            assert_eq!(
                catalog.get(id).unwrap().dependencies,
                vec![TOU_MIRA_ID.to_string()],
                "{id}"
            );
        }
    }

    #[test]
    fn bundled_dependency_cannot_be_removed_while_town_of_us_is_installed() {
        let temp = tempfile::tempdir().unwrap();
        let profile_id = "managed-tou-dependency";
        let installed = |id: &str, file: &str| InstalledMod {
            package_id: id.into(),
            name: id.into(),
            repo: Some(id.into()),
            version: "v1".into(),
            versions: vec!["v1".into()],
            enabled: true,
            source: ModSource::Github,
            tags: Vec::new(),
            managed: false,
            update: None,
            file: Some(file.into()),
            asset: Some(file.into()),
        };
        profile::install_plugin_bytes(temp.path(), profile_id, "Reactor.dll", b"reactor").unwrap();
        let mut record = ProfileRecord {
            id: profile_id.into(),
            name: "Managed Town of Us dependency".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: vec![
                installed("AU-Avengers/TOU-Mira", "Tou.dll"),
                InstalledMod {
                    managed: true,
                    ..installed("NuclearPowered/Reactor", "Reactor.dll")
                },
            ],
            levelimposter_maps: Vec::new(),
        };
        let catalog = bundled_catalog();

        let error = remove_mod_from_record(
            temp.path(),
            profile_id,
            &mut record,
            &catalog,
            "NuclearPowered/Reactor",
        )
        .unwrap_err();

        assert!(error.contains("included in the Town of Us package"));
        assert!(record
            .mods
            .iter()
            .any(|installed| installed.package_id == "NuclearPowered/Reactor"));
        assert!(temp
            .path()
            .join(profile_id)
            .join("BepInEx/plugins/Reactor.dll")
            .is_file());
    }

    #[test]
    fn operation_progress_channel_uses_frontend_contract() {
        let (sender, receiver) = std::sync::mpsc::channel::<serde_json::Value>();
        let channel = Channel::new(move |body| {
            sender
                .send(body.deserialize().unwrap())
                .expect("progress receiver must remain connected");
            Ok(())
        });
        let reporter = ProgressReporter::new(channel);

        reporter.stage("preparing", "Checking profile");
        reporter.download("Downloading Reactor.dll", 64, Some(128));

        let events: Vec<_> = receiver.try_iter().collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["phase"], "preparing");
        assert_eq!(events[0]["message"], "Checking profile");
        assert!(events[0].get("bytesReceived").is_none());
        assert_eq!(events[1]["phase"], "downloading");
        assert_eq!(events[1]["bytesReceived"], 64);
        assert_eq!(events[1]["bytesTotal"], 128);
    }

    #[test]
    fn epic_auth_store_seeds_login_without_overwriting_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let token_store = ensure_epic_auth_store(temp.path()).unwrap();
        assert_eq!(token_store, temp.path().join(".config/legendary/user.json"));
        assert_eq!(fs::read(&token_store).unwrap(), b"null\n");

        fs::write(&token_store, b"existing session").unwrap();
        assert_eq!(ensure_epic_auth_store(temp.path()).unwrap(), token_store);
        assert_eq!(fs::read(token_store).unwrap(), b"existing session");
    }

    #[test]
    fn epic_auth_store_covers_linux_and_macos_wine_profiles() {
        for (name, host, runtime) in [
            ("linux", compat::HostPlatform::Linux, Runtime::Wine),
            ("macos", compat::HostPlatform::Macos, Runtime::Whisky),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let game_dir = temp.path().join(format!("{name}-game"));
            let prefix = temp.path().join(format!("{name}-prefix"));
            let user_profile = prefix.join("drive_c/users/steamuser");
            fs::create_dir_all(user_profile.join("AppData/LocalLow/Innersloth/Among Us")).unwrap();
            fs::create_dir(&game_dir).unwrap();
            let context = compat::RuntimeContext {
                host,
                runtime,
                prefix: Some(prefix),
                launcher: None,
                launcher_args: Vec::new(),
            };

            prepare_epic_auth_stores(&game_dir, &context).unwrap();

            assert_eq!(
                fs::read(user_profile.join(".config/legendary/user.json")).unwrap(),
                b"null\n"
            );
            assert!(!game_dir.join("EGSAuth.json").exists());
        }
    }

    #[test]
    fn epic_auth_store_falls_back_to_game_folder_without_a_wine_profile() {
        let temp = tempfile::tempdir().unwrap();
        let game_dir = temp.path().join("game");
        fs::create_dir(&game_dir).unwrap();
        let context = compat::RuntimeContext {
            host: compat::HostPlatform::Other,
            runtime: Runtime::Wine,
            prefix: None,
            launcher: None,
            launcher_args: Vec::new(),
        };

        prepare_epic_auth_stores(&game_dir, &context).unwrap();

        assert_eq!(fs::read(game_dir.join("EGSAuth.json")).unwrap(), b"null\n");
    }

    #[test]
    fn manual_install_requires_confirmation_before_work_starts() {
        assert!(require_manual_install_confirmation(true).is_ok());
        let error = require_manual_install_confirmation(false).unwrap_err();
        assert!(error.contains("repository, release tag, and asset"));
    }
}
