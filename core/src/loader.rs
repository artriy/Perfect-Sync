//! LoaderManager: install the Doorstop + BepInEx loader directly into the game
//! folder (the layout every manual install uses, so BepInEx finds everything),
//! and sync the active profile's plugins into it at launch.
//!
//! Why game-dir, not per-profile env redirect: BepInEx-IL2CPP derives its
//! `plugins/`, `config/`, `interop/` paths from the GAME executable directory,
//! not from the Doorstop target DLL. So mods only load when they live under
//! `<game>/BepInEx`. Profiles are kept outside the game dir and their plugins
//! are copied in on launch (instant switch, vanilla stays clean when removed).

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};
use std::time::{SystemTime, UNIX_EPOCH};

/// Game-dir files from the pack root (the rest of the pack is dirs).
pub const BOOTSTRAP_FILES: &[&str] = &["winhttp.dll", "doorstop_config.ini", ".doorstop_version"];

/// The BepInEx IL2CPP preloader (verified against BepInEx 6.0.0-pre.2).
pub const IL2CPP_PRELOADER: &str = "BepInEx.Unity.IL2CPP.dll";

/// Among Us Steam app id, written so a direct launch passes Steam auth.
pub const STEAM_APP_ID: &str = "945360";

const LOADER_MARKER: &str = ".perfectsync_loader";
pub const DOORSTOP_PATCH_MARKER: &str = ".perfectsync_doorstop";
pub const DOORSTOP_PATCH_TRANSACTION: &str = ".doorstop-fix.perfectsync-txn";
pub const MANAGED_TOU_PACKAGE_MARKER: &str = ".perfectsync-tou-package.json";
const DOORSTOP_GC_VARIABLE: &str = "GC_DISABLE_INCREMENTAL";
const MAX_DOORSTOP_CONFIG_BYTES: u64 = 1024 * 1024;
const MANAGED_PLUGINS_MARKER: &str = ".perfectsync-managed.json";
const PLUGIN_SYNC_TRANSACTION: &str = ".plugins.perfectsync-sync";
const PLUGIN_SYNC_STAGE: &str = "sync-stage";
const PLUGIN_SYNC_BACKUP: &str = "sync-old";
const PLUGIN_SYNC_JOURNAL: &str = "journal.json";
const PLUGIN_SYNC_JOURNAL_PENDING: &str = "journal.pending";
const PLUGIN_SYNC_COMMITTED: &str = "committed";
const PLUGIN_SYNC_COMMITTED_PENDING: &str = "committed.pending";
pub const UNMANAGED_QUARANTINE_DIR: &str = ".perfectsync-quarantine";
const QUARANTINE_MANIFEST_PENDING: &str = "manifest.pending.json";
const QUARANTINE_MANIFEST: &str = "manifest.json";
const MAX_PLUGIN_SYNC_JOURNAL_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 8_192;
const MAX_ZIP_PATH_BYTES: usize = 1_024;
const MAX_ZIP_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ZIP_EXPANDED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_MANAGED_MARKER_BYTES: u64 = 1024 * 1024;
const MAX_MANAGED_PLUGINS: usize = 4_096;
const MAX_MANAGED_PLUGIN_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MANAGED_PLUGIN_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const LEVELIMPOSTER_MAP_DIRECTORY: &str = "LevelImposter";
const MANAGED_LEVELIMPOSTER_MAPS_MARKER: &str = ".perfectsync-maps.json";
const SPLASH_SCREEN_ORIGINAL: &str =
    "BepInEx/patchers/BepInEx.SplashScreen/BepInEx.SplashScreen.GUI.exe";
const SPLASH_SCREEN_RUNTIME_NAME: &str =
    "BepInEx/patchers/BepInEx.SplashScreen/Among Us.SplashScreen.GUI.exe";
const MAX_MANAGED_LEVELIMPOSTER_MAPS: usize = 4_096;
const MAX_MANAGED_LEVELIMPOSTER_MAP_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MANAGED_LEVELIMPOSTER_MAP_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
static QUARANTINE_COUNTER: AtomicU64 = AtomicU64::new(0);
static PACK_CACHE_LOCK: Mutex<()> = Mutex::new(());
static LOADER_LOCK: Mutex<()> = Mutex::new(());
static SYNC_LOCK: Mutex<()> = Mutex::new(());

/// True if a BepInEx loader installed by THIS app is present (winhttp proxy,
/// IL2CPP preloader, and our marker). A foreign/old install lacking the marker
/// reads false, so the app reinstalls the current build (auto-heals stale loaders).
pub fn has_loader(game_dir: &Path) -> bool {
    is_installed(game_dir) && game_dir.join("BepInEx").join(LOADER_MARKER).is_file()
}

fn doorstop_patch_record(version: &str, arch: &str) -> io::Result<String> {
    cache_component(version)?;
    if !matches!(arch, "x86" | "x64") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported Doorstop architecture",
        ));
    }
    Ok(format!("{version}:win:{arch}"))
}

/// True when the pinned Windows Doorstop patch was applied for this game architecture.
///
/// Among Us remains a Windows executable on every supported host. Linux uses
/// Proton and macOS uses Wine, so both must load the Windows proxy selected
/// from the actual PE bitness rather than a host-native Doorstop library.
pub fn has_doorstop_patch(game_dir: &Path, version: &str, arch: &str) -> bool {
    let Ok(expected) = doorstop_patch_record(version, arch) else {
        return false;
    };
    fs::read_to_string(game_dir.join("BepInEx").join(DOORSTOP_PATCH_MARKER))
        .ok()
        .is_some_and(|record| record.trim() == expected)
}

pub fn has_doorstop_patch_marker(game_dir: &Path) -> bool {
    game_dir
        .join("BepInEx")
        .join(DOORSTOP_PATCH_MARKER)
        .is_file()
}

/// Remove only Perfect Sync's patch marker after an unchecked full reinstall.
pub fn clear_doorstop_patch_marker(game_dir: &Path) -> io::Result<()> {
    let bepinex = game_dir.join("BepInEx");
    let marker = bepinex.join(DOORSTOP_PATCH_MARKER);
    crate::profile::reject_reparse(game_dir)?;
    crate::profile::reject_reparse(&bepinex)?;
    match fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(marker)
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Doorstop patch marker is not a regular file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// True if the recorded loader build is older than `latest` (so it should be
/// reinstalled). A missing/blank record counts as outdated.
pub fn is_outdated(installed: Option<&str>, latest: &str) -> bool {
    let Some(current) = installed else {
        return true;
    };
    if current == latest {
        return false;
    }
    match crate::version::cmp(latest, current) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(_) => false,
        None => true,
    }
}

/// The loader build id this app recorded (e.g. "be.764"), if any.
pub fn installed_version(game_dir: &Path) -> Option<String> {
    fs::read_to_string(game_dir.join("BepInEx").join(LOADER_MARKER))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Scrape the builds.bepinex.dev listing HTML for the NEWEST IL2CPP win-<arch>
/// build.
pub fn parse_latest_build(html: &str, arch: &str) -> Option<(String, String)> {
    let pat = format!(
        r#"projects/bepinex_be/(\d+)/(BepInEx-Unity\.IL2CPP-win-{}[^"'<> ]+\.zip)"#,
        regex::escape(arch)
    );
    let re = Regex::new(&pat).ok()?;
    let mut best: Option<(u64, String)> = None;
    for captures in re.captures_iter(html) {
        let number: u64 = captures[1].parse().unwrap_or(0);
        let path = format!("projects/bepinex_be/{}/{}", &captures[1], &captures[2]);
        if best.as_ref().is_none_or(|(current, _)| number > *current) {
            best = Some((number, path));
        }
    }
    best.map(|(number, path)| {
        (
            format!("be.{number}"),
            format!("https://builds.bepinex.dev/{path}"),
        )
    })
}

pub fn profile_bepinex_dir(profiles_root: &Path, profile_id: &str) -> PathBuf {
    profiles_root.join(profile_id).join("BepInEx")
}

pub fn profile_plugins_dir(profiles_root: &Path, profile_id: &str) -> PathBuf {
    profile_bepinex_dir(profiles_root, profile_id).join("plugins")
}

fn checked_profile_bepinex_dir(profiles_root: &Path, profile_id: &str) -> io::Result<PathBuf> {
    crate::profile::validate_profile_id(profile_id)?;
    crate::profile::reject_reparse(profiles_root)?;
    let profile = profiles_root.join(profile_id);
    let bep = profile.join("BepInEx");
    for path in [&profile, &bep] {
        crate::profile::reject_reparse(path)?;
    }
    Ok(bep)
}

/// Create the per-profile BepInEx subdirs (profile is where mods are stored).
pub fn ensure_profile_layout(profiles_root: &Path, profile_id: &str) -> io::Result<()> {
    let bep = checked_profile_bepinex_dir(profiles_root, profile_id)?;
    for sub in ["plugins", "config"] {
        let path = bep.join(sub);
        crate::profile::reject_reparse(&path)?;
        fs::create_dir_all(&path)?;
        crate::profile::reject_reparse(&path)?;
    }
    Ok(())
}

fn windows_device_name(value: &str) -> bool {
    let stem = value
        .trim_end_matches(['.', ' '])
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

fn cache_component(value: &str) -> io::Result<()> {
    let mut components = Path::new(value).components();
    let single =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if value.is_empty()
        || value.len() > 128
        || !value.is_ascii()
        || value.ends_with('.')
        || value.ends_with(' ')
        || windows_device_name(value)
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
        || !single
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe loader cache key",
        ));
    }
    Ok(())
}

pub fn loader_cache_dir(cache_root: &Path, version: &str, arch: &str) -> io::Result<PathBuf> {
    cache_component(version)?;
    if !matches!(arch, "x86" | "x64") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported loader architecture",
        ));
    }
    crate::profile::reject_reparse(cache_root)?;
    let bepinex = cache_root.join("bepinex");
    let version_dir = bepinex.join(version);
    let destination = version_dir.join(arch);
    for path in [&bepinex, &version_dir, &destination] {
        crate::profile::reject_reparse(path)?;
    }
    if bepinex.is_dir() {
        for entry in fs::read_dir(&bepinex)? {
            let entry = entry?;
            let other = entry.file_name().into_string().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "non-Unicode loader cache key")
            })?;
            if other != version && other.eq_ignore_ascii_case(version) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "loader cache key collides case-insensitively",
                ));
            }
        }
    }
    Ok(destination)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    crate::profile::reject_reparse(src)?;
    crate::profile::reject_reparse(dst)?;
    let metadata = fs::metadata(src)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source is not a directory",
        ));
    }
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "source tree contains a symlink",
            ));
        }
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            let mut input = File::open(&from)?;
            let mut output = OpenOptions::new().create_new(true).write(true).open(&to)?;
            io::copy(&mut input, &mut output)?;
            output.sync_all()?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "source tree contains a special file",
            ));
        }
    }
    Ok(())
}

fn overlay_dir(src: &Path, dst: &Path) -> io::Result<()> {
    crate::profile::reject_reparse(src)?;
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "loader pack contains a symlink",
            ));
        }
        if file_type.is_dir() {
            overlay_dir(&from, &to)?;
        } else if file_type.is_file() {
            crate::profile::reject_reparse(&to)?;
            let tmp = crate::profile::unique_sibling(&to, "overlay")?;
            let mut input = File::open(&from)?;
            let mut output = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
            io::copy(&mut input, &mut output)?;
            output.sync_all()?;
            drop(output);
            drop(input);
            crate::profile::atomic_replace(&tmp, &to)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "loader pack contains a special file",
            ));
        }
    }
    Ok(())
}

fn regular_nonempty(path: &Path) -> bool {
    crate::profile::reject_reparse(path).is_ok()
        && fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

fn validate_pack_root(root: &Path) -> io::Result<()> {
    let mandatory = [
        PathBuf::from("winhttp.dll"),
        PathBuf::from("doorstop_config.ini"),
        PathBuf::from("dotnet").join("coreclr.dll"),
        PathBuf::from("BepInEx").join("core").join(IL2CPP_PRELOADER),
    ];
    for relative in &mandatory {
        if !regular_nonempty(&root.join(relative)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "loader pack is missing mandatory file {}",
                    relative.display()
                ),
            ));
        }
    }
    Ok(())
}

fn remove_any(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to remove a symlink",
        )),
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn rollback_targets(
    stage: &Path,
    destination_root: &Path,
    backup: &Path,
    installed: &[PathBuf],
    moved: &[PathBuf],
    created_dirs: &[PathBuf],
) -> io::Result<()> {
    let mut failure = None;
    for relative in installed.iter().rev() {
        let source = destination_root.join(relative);
        let destination = stage.join(relative);
        match fs::symlink_metadata(&destination) {
            Ok(_) => {
                failure = failure.or(Some(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "rollback stage target unexpectedly exists",
                )));
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                failure = failure.or(Some(error));
                continue;
            }
        }
        if let Some(parent) = destination.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                failure = failure.or(Some(error));
                continue;
            }
        }
        if let Err(error) = fs::rename(source, destination) {
            failure = failure.or(Some(error));
        }
    }
    for relative in moved.iter().rev() {
        let source = backup.join(relative);
        let destination = destination_root.join(relative);
        match fs::symlink_metadata(&destination) {
            Ok(_) => {
                failure = failure.or(Some(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "rollback destination target unexpectedly exists",
                )));
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                failure = failure.or(Some(error));
                continue;
            }
        }
        if let Some(parent) = destination.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                failure = failure.or(Some(error));
                continue;
            }
        }
        if let Err(error) = fs::rename(source, destination) {
            failure = failure.or(Some(error));
        }
    }
    for relative in created_dirs.iter().rev() {
        match fs::remove_dir(destination_root.join(relative)) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => {
                failure = failure.or(Some(error));
            }
        }
    }
    if failure.is_none() {
        if let Err(error) = remove_any(backup) {
            failure = Some(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

fn commit_error(
    error: io::Error,
    stage: &Path,
    destination_root: &Path,
    backup: &Path,
    installed: &[PathBuf],
    moved: &[PathBuf],
    created_dirs: &[PathBuf],
) -> io::Error {
    match rollback_targets(
        stage,
        destination_root,
        backup,
        installed,
        moved,
        created_dirs,
    ) {
        Ok(()) => error,
        Err(rollback) => io::Error::new(
            error.kind(),
            format!("{error}; loader transaction rollback also failed: {rollback}"),
        ),
    }
}

struct CommitPolicy<'a> {
    sentinel: Option<&'a Path>,
    cleanup_backup: bool,
}

fn commit_staged_files_with_sentinel_impl<F>(
    stage: &Path,
    destination_root: &Path,
    backup: &Path,
    targets: &[PathBuf],
    required_dirs: &[PathBuf],
    policy: CommitPolicy<'_>,
    mut rename: F,
) -> io::Result<()>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    for relative in targets {
        reject_relative_reparse(destination_root, relative)?;
        let source = stage.join(relative);
        match fs::symlink_metadata(&source) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "staged target is not a regular file",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let destination = destination_root.join(relative);
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "transaction target collides with a non-file",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    let mut directories = required_dirs.to_vec();
    directories.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
    directories.dedup();
    for relative in &directories {
        let destination = destination_root.join(relative);
        crate::profile::reject_reparse(&destination)?;
        match fs::metadata(destination) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "transaction directory collides with a file",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let mut created_dirs = Vec::new();
    for relative in directories {
        let destination = destination_root.join(&relative);
        if !destination.exists() {
            if let Err(error) = fs::create_dir(&destination) {
                return Err(commit_error(
                    error,
                    stage,
                    destination_root,
                    backup,
                    &[],
                    &[],
                    &created_dirs,
                ));
            }
            created_dirs.push(relative);
        }
    }

    let mut moved = Vec::new();
    let mut installed = Vec::new();
    if let Some(relative) = policy.sentinel {
        let destination = destination_root.join(relative);
        if destination.exists() {
            let saved = backup.join(relative);
            if let Some(parent) = saved.parent() {
                if let Err(error) = fs::create_dir_all(parent) {
                    return Err(commit_error(
                        error,
                        stage,
                        destination_root,
                        backup,
                        &installed,
                        &moved,
                        &created_dirs,
                    ));
                }
            }
            if let Err(error) = rename(&destination, &saved) {
                return Err(commit_error(
                    error,
                    stage,
                    destination_root,
                    backup,
                    &installed,
                    &moved,
                    &created_dirs,
                ));
            }
            moved.push(relative.to_path_buf());
        }
    }
    for relative in targets {
        if policy.sentinel.is_some_and(|sentinel| relative == sentinel) {
            continue;
        }
        let destination = destination_root.join(relative);
        if destination.exists() {
            let saved = backup.join(relative);
            if let Some(parent) = saved.parent() {
                if let Err(error) = fs::create_dir_all(parent) {
                    return Err(commit_error(
                        error,
                        stage,
                        destination_root,
                        backup,
                        &installed,
                        &moved,
                        &created_dirs,
                    ));
                }
            }
            if let Err(error) = rename(&destination, &saved) {
                return Err(commit_error(
                    error,
                    stage,
                    destination_root,
                    backup,
                    &installed,
                    &moved,
                    &created_dirs,
                ));
            }
            moved.push(relative.clone());
        }

        let source = stage.join(relative);
        if source.exists() {
            if let Some(parent) = destination.parent() {
                if let Err(error) = fs::create_dir_all(parent) {
                    return Err(commit_error(
                        error,
                        stage,
                        destination_root,
                        backup,
                        &installed,
                        &moved,
                        &created_dirs,
                    ));
                }
            }
            if let Err(error) = rename(&source, &destination) {
                return Err(commit_error(
                    error,
                    stage,
                    destination_root,
                    backup,
                    &installed,
                    &moved,
                    &created_dirs,
                ));
            }
            installed.push(relative.clone());
        }
    }
    if let Some(relative) = policy.sentinel {
        let source = stage.join(relative);
        if source.exists() {
            let destination = destination_root.join(relative);
            if let Err(error) = rename(&source, &destination) {
                return Err(commit_error(
                    error,
                    stage,
                    destination_root,
                    backup,
                    &installed,
                    &moved,
                    &created_dirs,
                ));
            }
            installed.push(relative.to_path_buf());
        }
    }
    if policy.cleanup_backup {
        let _ = remove_any(backup);
    }
    Ok(())
}

fn commit_staged_files_with_sentinel<F>(
    stage: &Path,
    destination_root: &Path,
    backup: &Path,
    targets: &[PathBuf],
    required_dirs: &[PathBuf],
    sentinel: Option<&Path>,
    rename: F,
) -> io::Result<()>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    commit_staged_files_with_sentinel_impl(
        stage,
        destination_root,
        backup,
        targets,
        required_dirs,
        CommitPolicy {
            sentinel,
            cleanup_backup: true,
        },
        rename,
    )
}

fn commit_staged_files_with<F>(
    stage: &Path,
    destination_root: &Path,
    backup: &Path,
    targets: &[PathBuf],
    required_dirs: &[PathBuf],
    rename: F,
) -> io::Result<()>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    commit_staged_files_with_sentinel(
        stage,
        destination_root,
        backup,
        targets,
        required_dirs,
        None,
        rename,
    )
}
fn commit_staged_files(
    stage: &Path,
    destination_root: &Path,
    backup: &Path,
    targets: &[PathBuf],
    required_dirs: &[PathBuf],
) -> io::Result<()> {
    commit_staged_files_with(
        stage,
        destination_root,
        backup,
        targets,
        required_dirs,
        |source, destination| fs::rename(source, destination),
    )
}

fn commit_staged_files_retaining_backup(
    stage: &Path,
    destination_root: &Path,
    backup: &Path,
    targets: &[PathBuf],
    required_dirs: &[PathBuf],
) -> io::Result<()> {
    commit_staged_files_with_sentinel_impl(
        stage,
        destination_root,
        backup,
        targets,
        required_dirs,
        CommitPolicy {
            sentinel: None,
            cleanup_backup: false,
        },
        |source, destination| fs::rename(source, destination),
    )
}

/// Install a validated pack as a rollback-safe replacement. The marker is the final target.
pub fn install_pack(pack_dir: &Path, game_dir: &Path, version: &str) -> io::Result<()> {
    cache_component(version)?;
    validate_pack_root(pack_dir)?;
    crate::profile::reject_reparse(game_dir)?;
    crate::profile::reject_reparse(&game_dir.join("BepInEx"))?;
    crate::profile::reject_reparse(&game_dir.join("dotnet"))?;
    let _guard = LOADER_LOCK
        .lock()
        .map_err(|_| io::Error::other("loader install lock is poisoned"))?;
    let transaction =
        crate::profile::unique_sibling(&game_dir.join(".perfectsync-loader"), "stage")?;
    let stage = transaction.join("new");
    let backup = transaction.join("old");
    fs::create_dir_all(&stage)?;
    fs::create_dir_all(&backup)?;

    let result = (|| {
        for file in BOOTSTRAP_FILES {
            let source = pack_dir.join(file);
            if source.exists() {
                let mut input = File::open(&source)?;
                let mut output = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(stage.join(file))?;
                io::copy(&mut input, &mut output)?;
                output.sync_all()?;
                drop(output);
                drop(input);
            }
        }
        let existing_dotnet = game_dir.join("dotnet");
        if existing_dotnet.is_dir() {
            copy_dir_recursive(&existing_dotnet, &stage.join("dotnet"))?;
        }
        overlay_dir(&pack_dir.join("dotnet"), &stage.join("dotnet"))?;
        copy_dir_recursive(
            &pack_dir.join("BepInEx").join("core"),
            &stage.join("BepInEx").join("core"),
        )?;
        let existing_config = game_dir.join("BepInEx").join("config");
        if existing_config.is_dir() {
            copy_dir_recursive(&existing_config, &stage.join("BepInEx").join("config"))?;
        }
        let pack_config = pack_dir.join("BepInEx").join("config");
        if pack_config.is_dir() {
            overlay_dir(&pack_config, &stage.join("BepInEx").join("config"))?;
        }
        write_console_off(&stage)?;
        ensure_steam_appid(&stage)?;
        fs::create_dir_all(stage.join("BepInEx"))?;
        fs::create_dir_all(stage.join("BepInEx").join("interop"))?;
        fs::create_dir_all(stage.join("BepInEx").join("cache"))?;
        let mut marker = File::create(stage.join("BepInEx").join(LOADER_MARKER))?;
        let existing_plugins = game_dir.join("BepInEx").join("plugins");
        crate::profile::reject_reparse(&existing_plugins)?;
        if !existing_plugins.exists() {
            fs::create_dir_all(stage.join("BepInEx").join("plugins"))?;
        }
        marker.write_all(version.as_bytes())?;
        marker.sync_all()?;
        drop(marker);

        let replacement_dirs = [
            PathBuf::from("dotnet"),
            PathBuf::from("BepInEx").join("core"),
            PathBuf::from("BepInEx").join("config"),
            PathBuf::from("BepInEx").join("interop"),
            PathBuf::from("BepInEx").join("cache"),
        ];
        let mut target_set = HashSet::new();
        let mut staged_files = Vec::new();
        collect_regular_files(&stage, &stage, &mut staged_files)?;
        target_set.extend(staged_files);
        for relative in &replacement_dirs {
            let current = game_dir.join(relative);
            if current.is_dir() {
                let mut existing_files = Vec::new();
                collect_regular_files(game_dir, &current, &mut existing_files)?;
                target_set.extend(existing_files);
            }
        }
        for relative in BOOTSTRAP_FILES
            .iter()
            .map(|file| PathBuf::from(*file))
            .chain([PathBuf::from("steam_appid.txt")])
        {
            if stage.join(&relative).exists() {
                target_set.insert(relative);
            }
        }
        let marker_relative = PathBuf::from("BepInEx").join(LOADER_MARKER);
        target_set.insert(marker_relative.clone());
        let mut targets: Vec<_> = target_set.into_iter().collect();
        targets.sort();
        targets.retain(|relative| relative != &marker_relative);
        targets.push(marker_relative.clone());

        let mut required_dirs = vec![PathBuf::new()];
        collect_directories(&stage, &stage, &mut required_dirs)?;
        commit_staged_files_with_sentinel(
            &stage,
            game_dir,
            &backup,
            &targets,
            &required_dirs,
            Some(&marker_relative),
            |source, destination| fs::rename(source, destination),
        )
    })();
    let backup_has_entries = fs::read_dir(&backup)
        .ok()
        .is_some_and(|mut entries| entries.next().is_some());
    if result.is_err() && backup_has_entries {
        let _ = fs::remove_dir_all(&stage);
    } else {
        let _ = fs::remove_dir_all(&transaction);
    }
    result
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file has no parent"))?;
    crate::profile::reject_reparse(parent)?;
    fs::create_dir_all(parent)?;
    crate::profile::reject_reparse(path)?;
    let temporary = crate::profile::unique_sibling(path, "write")?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        crate::profile::atomic_replace(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn configured_doorstop_ini(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let source = std::str::from_utf8(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Doorstop configuration is not UTF-8",
        )
    })?;
    let (bom, body) = source
        .strip_prefix('\u{feff}')
        .map_or(("", source), |body| ("\u{feff}", body));
    let lower = body.to_ascii_lowercase();
    if !lower.contains("target_assembly") || !lower.contains("bepinex") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Doorstop configuration does not target BepInEx",
        ));
    }

    let newline = if body.contains("\r\n") { "\r\n" } else { "\n" };
    let had_trailing_newline = body.ends_with('\n') || body.ends_with('\r');
    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
    let mut environment_header = None;
    let mut in_environment = false;
    let mut variable_lines = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(section) = trimmed
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        {
            in_environment = section.trim().eq_ignore_ascii_case("Environment");
            if in_environment && environment_header.is_none() {
                environment_header = Some(index);
            }
            continue;
        }
        if in_environment
            && !trimmed.starts_with('#')
            && !trimmed.starts_with(';')
            && trimmed
                .split_once('=')
                .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case(DOORSTOP_GC_VARIABLE))
        {
            variable_lines.push(index);
        }
    }

    if let Some(first) = variable_lines.first().copied() {
        lines[first] = format!("{DOORSTOP_GC_VARIABLE}=1");
        for duplicate in variable_lines.into_iter().skip(1).rev() {
            lines.remove(duplicate);
        }
    } else if let Some(header) = environment_header {
        lines.insert(header + 1, format!("{DOORSTOP_GC_VARIABLE}=1"));
    } else {
        if lines.last().is_some_and(|line| !line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("[Environment]".to_string());
        lines.push(format!("{DOORSTOP_GC_VARIABLE}=1"));
    }

    let mut configured = String::with_capacity(source.len() + 64);
    configured.push_str(bom);
    configured.push_str(&lines.join(newline));
    if had_trailing_newline {
        configured.push_str(newline);
    }
    Ok(configured.into_bytes())
}

fn copy_new_synced(source: &Path, destination: &Path) -> io::Result<()> {
    if !regular_nonempty(source) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Doorstop patch file is missing or empty: {}",
                source.display()
            ),
        ));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()
}

fn write_new_synced(destination: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    output.write_all(bytes)?;
    output.sync_all()
}

fn validate_windows_proxy_arch(path: &Path, arch: &str) -> io::Result<()> {
    let bytes = fs::read(path)?;
    let pe_offset = bytes
        .get(0x3c..0x40)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .map(|value| value as usize)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Doorstop proxy is not a PE file",
            )
        })?;
    if bytes.get(pe_offset..pe_offset + 4) != Some(b"PE\0\0") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Doorstop proxy is not a PE file",
        ));
    }
    let machine = bytes
        .get(pe_offset + 4..pe_offset + 6)
        .and_then(|value| value.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Doorstop proxy has no COFF header",
            )
        })?;
    let expected = match arch {
        "x86" => 0x014c,
        "x64" => 0x8664,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported Doorstop architecture",
            ));
        }
    };
    if machine != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Doorstop proxy architecture does not match {arch} Among Us"),
        ));
    }
    Ok(())
}

/// Force the BepInEx console window off (keep the on-disk log).
pub fn write_console_off(game_dir: &Path) -> io::Result<()> {
    let cfg_dir = game_dir.join("BepInEx").join("config");
    crate::profile::reject_reparse(&game_dir.join("BepInEx"))?;
    crate::profile::reject_reparse(&cfg_dir)?;
    fs::create_dir_all(&cfg_dir)?;
    let cfg = "[Logging.Console]\nEnabled = false\n\n[Logging.Disk]\nEnabled = true\nWriteUnityLog = false\n";
    atomic_write(&cfg_dir.join("BepInEx.cfg"), cfg.as_bytes())
}

/// Write `steam_appid.txt` next to the exe so a direct launch passes Steam auth.
pub fn ensure_steam_appid(game_dir: &Path) -> io::Result<()> {
    crate::profile::reject_reparse(game_dir)?;
    atomic_write(&game_dir.join("steam_appid.txt"), STEAM_APP_ID.as_bytes())
}

fn normalized_zip_path(name: &str) -> io::Result<(PathBuf, bool)> {
    if name.len() > MAX_ZIP_PATH_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ZIP entry path is too long",
        ));
    }
    let normalized = name.replace('\\', "/");
    let is_dir = normalized.ends_with('/');
    let body = normalized.trim_end_matches('/');
    if body.is_empty()
        || normalized.starts_with('/')
        || normalized.starts_with("//")
        || body
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.contains(':'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsafe ZIP entry path",
        ));
    }
    Ok((PathBuf::from(body), is_dir))
}

fn extract_zip_to_empty(bytes: &[u8], dest: &Path) -> io::Result<()> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ZIP has too many entries",
        ));
    }
    let mut expanded = 0_u64;
    let mut paths = HashSet::new();
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let (relative, is_dir) = normalized_zip_path(file.name())?;
        let key = relative
            .to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase();
        if !paths.insert(key) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ZIP contains duplicate or case-colliding paths",
            ));
        }
        if is_dir || file.is_dir() {
            continue;
        }
        if file.size() > MAX_ZIP_ENTRY_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ZIP entry is too large",
            ));
        }
        expanded = expanded
            .checked_add(file.size())
            .filter(|size| *size <= MAX_ZIP_EXPANDED_BYTES)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "ZIP expanded size is too large")
            })?;
    }
    fs::create_dir_all(dest)?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let (relative, is_dir) = normalized_zip_path(file.name())?;
        let output_path = dest.join(relative);
        if is_dir || file.is_dir() {
            fs::create_dir_all(output_path)?;
            continue;
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let expected = file.size();
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&output_path)?;
        let written = io::copy(
            &mut file.by_ref().take(MAX_ZIP_ENTRY_BYTES + 1),
            &mut output,
        )?;
        if written != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ZIP entry size mismatch",
            ));
        }
        output.sync_all()?;
    }
    Ok(())
}

fn collect_regular_files(root: &Path, current: &Path, output: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staged tree contains a symlink",
            ));
        }
        if file_type.is_dir() {
            collect_regular_files(root, &path, output)?;
        } else if file_type.is_file() {
            output.push(
                path.strip_prefix(root)
                    .map_err(io::Error::other)?
                    .to_path_buf(),
            );
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staged tree contains a special file",
            ));
        }
    }
    Ok(())
}

fn collect_directories(root: &Path, current: &Path, output: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staged tree contains a symlink",
            ));
        }
        if file_type.is_dir() {
            output.push(
                path.strip_prefix(root)
                    .map_err(io::Error::other)?
                    .to_path_buf(),
            );
            collect_directories(root, &path, output)?;
        } else if !file_type.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staged tree contains a special file",
            ));
        }
    }
    Ok(())
}

fn reject_relative_reparse(base: &Path, relative: &Path) -> io::Result<()> {
    let mut current = base.to_path_buf();
    crate::profile::reject_reparse(&current)?;
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsafe relative path",
            ));
        };
        current.push(component);
        crate::profile::reject_reparse(&current)?;
    }
    Ok(())
}

/// Extract a bounded archive, fully validating it before publishing any entry.
pub fn extract_all(bytes: &[u8], dest: &Path) -> io::Result<()> {
    crate::profile::reject_reparse(dest)?;
    if let Some(parent) = dest.parent() {
        crate::profile::reject_reparse(parent)?;
    }
    let stage = crate::profile::unique_sibling(dest, "extract")?;
    let backup = crate::profile::unique_sibling(dest, "extract-backup")?;
    let result = (|| {
        extract_zip_to_empty(bytes, &stage)?;
        let mut files = Vec::new();
        collect_regular_files(&stage, &stage, &mut files)?;
        let mut directories = vec![PathBuf::new()];
        collect_directories(&stage, &stage, &mut directories)?;
        commit_staged_files(&stage, dest, &backup, &files, &directories)
    })();
    let _ = fs::remove_dir_all(&stage);
    if result.is_ok() || !backup.exists() {
        let _ = fs::remove_dir_all(&backup);
    }
    result
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedTouPackage {
    key: String,
    files: Vec<String>,
}

fn valid_tou_package_relative(relative: &Path) -> bool {
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return false;
    }
    let names: Vec<_> = components
        .iter()
        .map(|component| component.as_os_str().to_str())
        .collect();
    if names.iter().any(|name| {
        name.is_none_or(|name| {
            name.is_empty() || !name.is_ascii() || name.to_ascii_lowercase().contains("perfectsync")
        })
    }) {
        return false;
    }
    let first = names[0].unwrap_or_default();
    if first.eq_ignore_ascii_case("BepInEx") || first.eq_ignore_ascii_case("dotnet") {
        return components.len() >= 2;
    }
    components.len() == 1
        && (BOOTSTRAP_FILES
            .iter()
            .any(|allowed| first.eq_ignore_ascii_case(allowed))
            || first.eq_ignore_ascii_case("changelog.txt")
            || first.eq_ignore_ascii_case("steam_appid.txt"))
}

fn read_managed_tou_package(game_dir: &Path) -> io::Result<Option<ManagedTouPackage>> {
    let marker = game_dir.join("BepInEx").join(MANAGED_TOU_PACKAGE_MARKER);
    crate::profile::reject_reparse(&marker)?;
    let metadata = match fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed Town of Us package marker is not a regular file",
            ));
        }
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.len() == 0 || metadata.len() > MAX_MANAGED_MARKER_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed Town of Us package marker has an invalid size",
        ));
    }
    let managed: ManagedTouPackage = serde_json::from_slice(&fs::read(marker)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if managed.key.is_empty()
        || managed.key.len() > 128
        || !managed.key.bytes().all(|byte| byte.is_ascii_hexdigit())
        || managed.files.is_empty()
        || managed.files.len() > MAX_ZIP_ENTRIES
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed Town of Us package marker is invalid",
        ));
    }
    let mut seen = HashSet::with_capacity(managed.files.len());
    for name in &managed.files {
        let relative = PathBuf::from(name);
        if !valid_tou_package_relative(&relative)
            || !seen.insert(name.replace('\\', "/").to_ascii_lowercase())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed Town of Us package marker contains an invalid path",
            ));
        }
    }
    Ok(Some(managed))
}

pub fn managed_tou_package_key(game_dir: &Path) -> io::Result<Option<String>> {
    read_managed_tou_package(game_dir).map(|managed| managed.map(|record| record.key))
}

/// Whether the complete release-matched Town of Us package is already present.
pub fn tou_package_is_current(game_dir: &Path, key: &str) -> io::Result<bool> {
    let Some(managed) = read_managed_tou_package(game_dir)? else {
        return Ok(false);
    };
    if managed.key != key {
        return Ok(false);
    }
    for name in &managed.files {
        let original = game_dir.join(name);
        crate::profile::reject_reparse(&original)?;
        let metadata = match fs::metadata(&original) {
            Ok(metadata) => Some(metadata),
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && name.eq_ignore_ascii_case(SPLASH_SCREEN_ORIGINAL) =>
            {
                let runtime = game_dir.join(SPLASH_SCREEN_RUNTIME_NAME);
                crate::profile::reject_reparse(&runtime)?;
                match fs::metadata(runtime) {
                    Ok(metadata) => Some(metadata),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if !metadata.is_some_and(|metadata| metadata.is_file() && metadata.len() > 0) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Remove every game-root file owned by the previously applied Town of Us ZIP.
/// The caller wraps this in the broader game artifact transaction.
pub fn remove_tou_package(game_dir: &Path) -> io::Result<()> {
    let Some(mut managed) = read_managed_tou_package(game_dir)? else {
        return Ok(());
    };
    managed
        .files
        .sort_by_key(|name| std::cmp::Reverse(name.matches('/').count()));
    for name in managed.files {
        let target = game_dir.join(name);
        crate::profile::reject_reparse(&target)?;
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "managed Town of Us package target changed type",
                ));
            }
            Ok(_) => fs::remove_file(target)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let marker = game_dir.join("BepInEx").join(MANAGED_TOU_PACKAGE_MARKER);
    match fs::remove_file(marker) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    clear_doorstop_patch_marker(game_dir)
}

/// Apply every safe game-root file from the exact Town of Us release ZIP.
/// The bundled BepInEx pack is installed first, then the full archive is
/// overlaid. The caller provides rollback through the game artifact transaction.
pub fn install_tou_package(
    bytes: &[u8],
    game_dir: &Path,
    key: &str,
    loader_version: &str,
) -> io::Result<()> {
    if key.is_empty() || key.len() > 128 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Town of Us package key",
        ));
    }
    crate::profile::reject_reparse(game_dir)?;
    let extraction = crate::profile::unique_sibling(game_dir, "tou-package")?;
    let result = (|| {
        extract_zip_to_empty(bytes, &extraction)?;
        let package_root = locate_pack_root(&extraction).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Town of Us ZIP has no unique complete BepInEx package root",
            )
        })?;
        validate_pack_root(&package_root)?;
        for required in crate::profile::TOU_REQUIRED_FILES {
            if !regular_nonempty(&package_root.join("BepInEx").join(required)) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("Town of Us ZIP is missing required file {required}"),
                ));
            }
        }

        let mut files = Vec::new();
        collect_regular_files(&package_root, &package_root, &mut files)?;
        if files.is_empty() || files.len() > MAX_ZIP_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Town of Us ZIP has an invalid file count",
            ));
        }
        let mut names = Vec::with_capacity(files.len());
        let mut seen = HashSet::with_capacity(files.len());
        for relative in &files {
            if !valid_tou_package_relative(relative) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Town of Us ZIP contains an unsupported game-root path {}",
                        relative.display()
                    ),
                ));
            }
            let name = relative.to_string_lossy().replace('\\', "/");
            if !seen.insert(name.to_ascii_lowercase()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Town of Us ZIP contains case-colliding game-root paths",
                ));
            }
            names.push(name);
        }

        remove_tou_package(game_dir)?;
        install_pack(&package_root, game_dir, loader_version)?;
        overlay_dir(&package_root, game_dir)?;
        clear_doorstop_patch_marker(game_dir)?;
        names.sort_by_key(|name| name.to_ascii_lowercase());
        let marker_bytes = serde_json::to_vec(&ManagedTouPackage {
            key: key.to_string(),
            files: names,
        })
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if marker_bytes.len() as u64 > MAX_MANAGED_MARKER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed Town of Us package marker would be too large",
            ));
        }
        atomic_write(
            &game_dir.join("BepInEx").join(MANAGED_TOU_PACKAGE_MARKER),
            &marker_bytes,
        )
    })();
    let _ = fs::remove_dir_all(&extraction);
    result
}

/// Apply the patched Windows UnityDoorstop proxy without replacing BepInEx's
/// target assembly configuration. The patch is transactionally installed after
/// BepInEx, and its marker is committed last.
pub fn install_windows_doorstop_patch(
    bytes: &[u8],
    game_dir: &Path,
    version: &str,
    arch: &str,
) -> io::Result<()> {
    let marker_record = doorstop_patch_record(version, arch)?;
    crate::profile::reject_reparse(game_dir)?;
    crate::profile::reject_reparse(&game_dir.join("BepInEx"))?;
    if !has_loader(game_dir) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "BepInEx must be installed by Perfect Sync before applying the Doorstop patch",
        ));
    }

    let _guard = LOADER_LOCK
        .lock()
        .map_err(|_| io::Error::other("loader install lock is poisoned"))?;
    let transaction = game_dir.join(DOORSTOP_PATCH_TRANSACTION);
    crate::profile::reject_reparse(&transaction)?;
    remove_any(&transaction)?;
    let archive = transaction.join("archive");
    let stage = transaction.join("new");
    let backup = transaction.join("old");
    fs::create_dir_all(&stage)?;
    fs::create_dir_all(&backup)?;

    let result = (|| {
        extract_zip_to_empty(bytes, &archive)?;
        let payload = archive.join(arch);
        crate::profile::reject_reparse(&payload)?;
        if !payload.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Doorstop archive does not contain its {arch} payload"),
            ));
        }

        let proxy = payload.join("winhttp.dll");
        let released_version = payload.join(".doorstop_version");
        let released_config = payload.join("doorstop_config.ini");
        for required in [&proxy, &released_version, &released_config] {
            if !regular_nonempty(required) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Doorstop archive is missing mandatory file {}",
                        required
                            .strip_prefix(&archive)
                            .unwrap_or(required)
                            .display()
                    ),
                ));
            }
        }
        validate_windows_proxy_arch(&proxy, arch)?;
        if fs::read_to_string(&released_version)
            .ok()
            .is_none_or(|value| value.trim() != version)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Doorstop archive version does not match the pinned release",
            ));
        }
        let released_config_bytes = fs::read(&released_config)?;
        if !std::str::from_utf8(&released_config_bytes)
            .ok()
            .is_some_and(|value| value.to_ascii_lowercase().contains("[environment]"))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Doorstop archive does not support process environment configuration",
            ));
        }

        let installed_config = game_dir.join("doorstop_config.ini");
        let config_metadata = fs::metadata(&installed_config)?;
        if !config_metadata.is_file()
            || config_metadata.len() == 0
            || config_metadata.len() > MAX_DOORSTOP_CONFIG_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "installed BepInEx Doorstop configuration is invalid",
            ));
        }
        let configured = configured_doorstop_ini(&fs::read(&installed_config)?)?;

        copy_new_synced(&proxy, &stage.join("winhttp.dll"))?;
        copy_new_synced(&released_version, &stage.join(".doorstop_version"))?;
        write_new_synced(&stage.join("doorstop_config.ini"), &configured)?;
        let marker_relative = PathBuf::from("BepInEx").join(DOORSTOP_PATCH_MARKER);
        write_new_synced(&stage.join(&marker_relative), marker_record.as_bytes())?;

        let targets = [
            PathBuf::from("winhttp.dll"),
            PathBuf::from(".doorstop_version"),
            PathBuf::from("doorstop_config.ini"),
            marker_relative.clone(),
        ];
        let required_dirs = [PathBuf::new(), PathBuf::from("BepInEx")];
        commit_staged_files_with_sentinel(
            &stage,
            game_dir,
            &backup,
            &targets,
            &required_dirs,
            Some(&marker_relative),
            |source, destination| fs::rename(source, destination),
        )
    })();
    remove_any(&archive).ok();
    let backup_empty = fs::read_dir(&backup)
        .ok()
        .is_some_and(|mut entries| entries.next().is_none());
    if result.is_ok() || !backup.exists() || backup_empty {
        remove_any(&transaction).ok();
    }
    result
}

/// Find the unique complete pack root (the cache itself or exactly one child).
pub fn locate_pack_root(dir: &Path) -> Option<PathBuf> {
    crate::profile::reject_reparse(dir).ok()?;
    let mut candidates = Vec::new();
    if dir.join("winhttp.dll").exists() {
        candidates.push(dir.to_path_buf());
    }
    for entry in fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        if entry.file_type().ok()?.is_symlink() {
            return None;
        }
        let path = entry.path();
        if path.is_dir() && path.join("winhttp.dll").exists() {
            candidates.push(path);
        }
    }
    if candidates.len() != 1 || validate_pack_root(&candidates[0]).is_err() {
        return None;
    }
    candidates.pop()
}

fn publish_pack_cache_with<F>(bytes: &[u8], cache_dir: &Path, mut rename: F) -> io::Result<PathBuf>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    let _guard = PACK_CACHE_LOCK
        .lock()
        .map_err(|_| io::Error::other("pack cache lock is poisoned"))?;
    let parent = cache_dir
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "loader cache has no parent"))?;
    crate::profile::reject_reparse(parent)?;
    crate::profile::reject_reparse(cache_dir)?;
    let stage = crate::profile::unique_sibling(cache_dir, "cache-stage")?;
    let backup = crate::profile::unique_sibling(cache_dir, "cache-old")?;
    let result = (|| {
        extract_zip_to_empty(bytes, &stage)?;
        let staged_root = locate_pack_root(&stage).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "archive does not contain exactly one complete loader pack",
            )
        })?;
        let relative_root = staged_root
            .strip_prefix(&stage)
            .map_err(io::Error::other)?
            .to_path_buf();

        let had_live = match fs::symlink_metadata(cache_dir) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => true,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "loader cache target is not a regular directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        if had_live {
            rename(cache_dir, &backup)?;
        }
        if let Err(error) = rename(&stage, cache_dir) {
            if had_live {
                if let Err(rollback) = rename(&backup, cache_dir) {
                    return Err(io::Error::new(
                        error.kind(),
                        format!(
                            "{error}; loader cache publication rollback also failed: {rollback}"
                        ),
                    ));
                }
            }
            return Err(error);
        }
        if had_live {
            let _ = remove_any(&backup);
        }
        Ok(cache_dir.join(relative_root))
    })();
    let _ = remove_any(&stage);
    result
}

/// Publish a complete loader cache using a serialized whole-directory swap.
pub fn publish_pack_cache(bytes: &[u8], cache_dir: &Path) -> io::Result<PathBuf> {
    publish_pack_cache_with(bytes, cache_dir, |source, destination| {
        fs::rename(source, destination)
    })
}

/// Extract a downloaded pack into a validated cache and install it.
pub fn install_pack_from_zip(
    bytes: &[u8],
    game_dir: &Path,
    cache_dir: &Path,
    version: &str,
) -> io::Result<()> {
    let root = publish_pack_cache(bytes, cache_dir)?;
    install_pack(&root, game_dir, version)
}

/// True if the loader is installed in the game dir (proxy + preloader present).
pub fn is_installed(game_dir: &Path) -> bool {
    regular_nonempty(&game_dir.join("winhttp.dll"))
        && regular_nonempty(&game_dir.join("BepInEx").join("core").join(IL2CPP_PRELOADER))
}

#[derive(Debug, Serialize, Deserialize)]
struct ManagedPlugins {
    names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedPlugin {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub importable: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QuarantinedPlugin {
    original_path: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginQuarantineManifest {
    created_at: u128,
    files: Vec<QuarantinedPlugin>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginSyncJournal {
    targets: Vec<PluginSyncTarget>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginSyncTarget {
    name: String,
    old_sha256: Option<[u8; 32]>,
    new_sha256: Option<[u8; 32]>,
}

fn plugin_sync_transaction(destination: &Path) -> io::Result<PathBuf> {
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "plugin destination has no parent directory",
        )
    })?;
    Ok(parent.join(PLUGIN_SYNC_TRANSACTION))
}

fn regular_file_digest(path: &Path) -> io::Result<Option<[u8; 32]>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "transaction target is not a regular file: {}",
                    path.display()
                ),
            ));
        }
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transaction target changed while it was opened",
        ));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut read_bytes = 0_u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        read_bytes = read_bytes
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "file size overflow"))?;
    }
    if read_bytes != metadata.len() || file.metadata()?.len() != metadata.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transaction target changed while it was hashed",
        ));
    }
    Ok(Some(digest.finalize().into()))
}

fn valid_sync_artifact_name(name: &str) -> bool {
    name == MANAGED_PLUGINS_MARKER || crate::profile::validate_dll_name(name).is_ok()
}

fn validate_sync_artifact_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin sync artifact directory is not a regular directory",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin sync artifact has a non-Unicode name",
            )
        })?;
        let file_type = entry.file_type()?;
        if !valid_sync_artifact_name(&name) || file_type.is_symlink() || !file_type.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin sync artifact contains an ambiguous entry",
            ));
        }
    }
    Ok(())
}

fn validate_sync_transaction_tree(transaction: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(transaction)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "plugin sync transaction is not a regular directory",
        ));
    }
    for entry in fs::read_dir(transaction)? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin sync transaction has a non-Unicode entry",
            )
        })?;
        let file_type = entry.file_type()?;
        match name.as_str() {
            PLUGIN_SYNC_STAGE | PLUGIN_SYNC_BACKUP
                if file_type.is_dir() && !file_type.is_symlink() =>
            {
                validate_sync_artifact_dir(&entry.path())?;
            }
            PLUGIN_SYNC_JOURNAL
            | PLUGIN_SYNC_JOURNAL_PENDING
            | PLUGIN_SYNC_COMMITTED
            | PLUGIN_SYNC_COMMITTED_PENDING
                if file_type.is_file() && !file_type.is_symlink() => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "plugin sync transaction contains an ambiguous entry",
                ));
            }
        }
    }
    Ok(())
}

fn sync_artifact_dir_is_empty(path: &Path) -> io::Result<bool> {
    match fs::read_dir(path) {
        Ok(mut entries) => Ok(entries.next().transpose()?.is_none()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

fn write_sync_record(
    transaction: &Path,
    pending_name: &str,
    final_name: &str,
    bytes: &[u8],
) -> io::Result<()> {
    let pending = transaction.join(pending_name);
    let final_path = transaction.join(final_name);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&pending)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(pending, final_path)
}

fn read_plugin_sync_journal(path: &Path) -> io::Result<PluginSyncJournal> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_PLUGIN_SYNC_JOURNAL_BYTES
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "plugin sync journal is invalid or too large",
        ));
    }
    let journal: PluginSyncJournal = serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if journal.targets.is_empty() || journal.targets.len() > MAX_MANAGED_PLUGINS + 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "plugin sync journal has an invalid target count",
        ));
    }
    let mut names = HashSet::with_capacity(journal.targets.len());
    for (index, target) in journal.targets.iter().enumerate() {
        if !valid_sync_artifact_name(&target.name)
            || !names.insert(target.name.to_ascii_lowercase())
            || (target.old_sha256.is_none() && target.new_sha256.is_none())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin sync journal contains an invalid target",
            ));
        }
        if target.name == MANAGED_PLUGINS_MARKER
            && (index + 1 != journal.targets.len() || target.new_sha256.is_none())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin sync marker is not the final journal target",
            ));
        }
    }
    if journal
        .targets
        .last()
        .is_none_or(|target| target.name != MANAGED_PLUGINS_MARKER)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "plugin sync journal is missing its ownership marker",
        ));
    }
    for directory in [
        path.parent().unwrap().join(PLUGIN_SYNC_STAGE),
        path.parent().unwrap().join(PLUGIN_SYNC_BACKUP),
    ] {
        if directory.exists() {
            for entry in fs::read_dir(directory)? {
                let name = entry?
                    .file_name()
                    .into_string()
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid artifact"))?;
                if !names.contains(&name.to_ascii_lowercase()) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "plugin sync artifact is absent from its journal",
                    ));
                }
            }
        }
    }
    Ok(journal)
}

fn interrupted_sync_ambiguity(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("interrupted plugin sync state is ambiguous for {name}"),
    )
}

fn recover_interrupted_plugin_sync(destination: &Path) -> io::Result<()> {
    let transaction = plugin_sync_transaction(destination)?;
    match fs::symlink_metadata(&transaction) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    validate_sync_transaction_tree(&transaction)?;
    let journal_path = transaction.join(PLUGIN_SYNC_JOURNAL);
    if !journal_path.exists() {
        if transaction.join(PLUGIN_SYNC_COMMITTED).exists()
            || !sync_artifact_dir_is_empty(&transaction.join(PLUGIN_SYNC_BACKUP))?
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "journal-less plugin sync transaction is ambiguous",
            ));
        }
        return remove_any(&transaction);
    }
    let journal = read_plugin_sync_journal(&journal_path)?;
    if transaction.join(PLUGIN_SYNC_COMMITTED).exists() {
        return remove_any(&transaction);
    }
    crate::profile::reject_reparse(destination)?;
    if !destination.exists() {
        if journal
            .targets
            .iter()
            .any(|target| target.old_sha256.is_some())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "interrupted plugin sync lost its destination",
            ));
        }
        fs::create_dir(destination)?;
    }
    let stage = transaction.join(PLUGIN_SYNC_STAGE);
    let backup = transaction.join(PLUGIN_SYNC_BACKUP);
    for target in &journal.targets {
        let destination_path = destination.join(&target.name);
        let stage_path = stage.join(&target.name);
        let backup_path = backup.join(&target.name);
        let destination_digest = regular_file_digest(&destination_path)?;
        let stage_digest = regular_file_digest(&stage_path)?;
        let backup_digest = regular_file_digest(&backup_path)?;
        if stage_digest.is_some() && stage_digest != target.new_sha256 {
            return Err(interrupted_sync_ambiguity(&target.name));
        }
        if backup_digest.is_some() && backup_digest != target.old_sha256 {
            return Err(interrupted_sync_ambiguity(&target.name));
        }
        if let Some(new_digest) = target.new_sha256 {
            if stage_digest.is_some() {
                if backup_digest.is_none() {
                    match target.old_sha256 {
                        Some(old_digest) if destination_digest == Some(old_digest) => {
                            fs::create_dir_all(&backup)?;
                            fs::rename(&destination_path, &backup_path)?;
                        }
                        None if destination_digest.is_none() => {}
                        _ => return Err(interrupted_sync_ambiguity(&target.name)),
                    }
                } else if destination_digest.is_some() {
                    return Err(interrupted_sync_ambiguity(&target.name));
                }
                fs::rename(&stage_path, &destination_path)?;
            } else if destination_digest != Some(new_digest) {
                return Err(interrupted_sync_ambiguity(&target.name));
            }
        } else {
            if stage_digest.is_some() {
                return Err(interrupted_sync_ambiguity(&target.name));
            }
            if backup_digest.is_none() {
                if destination_digest != target.old_sha256 {
                    return Err(interrupted_sync_ambiguity(&target.name));
                }
                fs::create_dir_all(&backup)?;
                fs::rename(&destination_path, &backup_path)?;
            } else if destination_digest.is_some() {
                return Err(interrupted_sync_ambiguity(&target.name));
            }
        }
    }
    let committed_pending = transaction.join(PLUGIN_SYNC_COMMITTED_PENDING);
    match fs::remove_file(&committed_pending) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    write_sync_record(
        &transaction,
        PLUGIN_SYNC_COMMITTED_PENDING,
        PLUGIN_SYNC_COMMITTED,
        b"committed",
    )?;
    remove_any(&transaction)
}

fn read_managed_plugins(marker: &Path) -> io::Result<HashSet<String>> {
    crate::profile::reject_reparse(marker)?;
    match fs::metadata(marker) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_MANAGED_MARKER_BYTES => {}
        Ok(metadata) if metadata.len() > MAX_MANAGED_MARKER_BYTES => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed plugin marker is too large",
            ));
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed plugin marker is not a regular file",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => return Err(error),
    }
    let text = fs::read_to_string(marker)?;
    let managed: ManagedPlugins =
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if managed.names.len() > MAX_MANAGED_PLUGINS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed plugin marker has too many names",
        ));
    }
    let mut names = HashSet::new();
    for name in managed.names {
        crate::profile::validate_dll_name(&name)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        if !names.insert(name.to_ascii_lowercase()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed plugin marker has duplicate names",
            ));
        }
    }
    Ok(names)
}
fn plugin_path_key(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

fn managed_tou_plugin_paths(game_dir: &Path) -> io::Result<HashSet<String>> {
    let Some(managed) = read_managed_tou_package(game_dir)? else {
        return Ok(HashSet::new());
    };
    let mut paths = HashSet::new();
    for file in managed.files {
        let normalized = plugin_path_key(&file);
        if let Some(relative) = normalized.strip_prefix("bepinex/plugins/") {
            paths.insert(relative.to_string());
        }
    }
    Ok(paths)
}

fn selected_profile_plugins(source: &Path) -> io::Result<HashMap<String, PathBuf>> {
    let mut selected = HashMap::new();
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile plugins contain a symlink",
            ));
        }
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().into_string().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "non-Unicode profile plugin filename",
            )
        })?;
        if !name.to_ascii_lowercase().ends_with(".dll") {
            continue;
        }
        crate::profile::validate_dll_name(&name)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        if selected
            .insert(name.to_ascii_lowercase(), entry.path())
            .is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile has case-colliding DLLs",
            ));
        }
    }
    Ok(selected)
}

fn collect_plugin_dlls(
    root: &Path,
    directory: &Path,
    output: &mut Vec<UnmanagedPlugin>,
) -> io::Result<()> {
    crate::profile::reject_reparse(directory)?;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "game plugins contain a link or reparse point: {}",
                    path.display()
                ),
            ));
        }
        if metadata.is_dir() {
            collect_plugin_dlls(root, &path, output)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("game plugins contain a special file: {}", path.display()),
            ));
        }
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
        {
            continue;
        }
        if metadata.len() == 0 || metadata.len() > MAX_MANAGED_PLUGIN_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("plugin DLL has an invalid size: {}", path.display()),
            ));
        }
        if output.len() >= MAX_MANAGED_PLUGINS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "game folder has too many plugin DLLs",
            ));
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "plugin path escaped its root")
        })?;
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "game plugin has an unsafe relative path",
            ));
        }
        let portable = relative
            .to_str()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "non-Unicode game plugin path")
            })?
            .replace('\\', "/");
        if portable.len() > MAX_ZIP_PATH_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "game plugin path is too long",
            ));
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "non-Unicode game plugin filename",
                )
            })?
            .to_string();
        let importable =
            relative.components().count() == 1 && crate::profile::validate_dll_name(&name).is_ok();
        output.push(UnmanagedPlugin {
            path: portable,
            name,
            size: metadata.len(),
            importable,
        });
    }
    Ok(())
}

fn unmanaged_plugins_unlocked(
    profiles_root: &Path,
    profile_id: &str,
    game_dir: &Path,
) -> io::Result<Vec<UnmanagedPlugin>> {
    let source = checked_profile_bepinex_dir(profiles_root, profile_id)?.join("plugins");
    crate::profile::reject_reparse(&source)?;
    if !fs::metadata(&source).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "profile plugins directory not found",
        ));
    }
    let destination = game_dir.join("BepInEx").join("plugins");
    crate::profile::reject_reparse(game_dir)?;
    crate::profile::reject_reparse(&game_dir.join("BepInEx"))?;
    crate::profile::reject_reparse(&destination)?;
    if !destination.is_dir() {
        return Ok(Vec::new());
    }
    recover_interrupted_plugin_sync(&destination)?;
    let previously_owned = read_managed_plugins(&destination.join(MANAGED_PLUGINS_MARKER))?;
    let package_owned = managed_tou_plugin_paths(game_dir)?;
    let selected = selected_profile_plugins(&source)?;
    let mut candidates = Vec::new();
    collect_plugin_dlls(&destination, &destination, &mut candidates)?;

    let mut unmanaged = Vec::new();
    for plugin in candidates {
        let key = plugin_path_key(&plugin.path);
        let root_name = (!plugin.path.contains('/')).then(|| plugin.name.to_ascii_lowercase());
        if root_name
            .as_ref()
            .is_some_and(|name| previously_owned.contains(name))
            || package_owned.contains(&key)
        {
            continue;
        }
        if let Some(source) = root_name.as_ref().and_then(|name| selected.get(name)) {
            let destination_file = destination.join(&plugin.path);
            if regular_file_digest(&destination_file)? == regular_file_digest(source)? {
                continue;
            }
        }
        unmanaged.push(plugin);
    }
    unmanaged.sort_by(|left, right| {
        left.path
            .to_ascii_lowercase()
            .cmp(&right.path.to_ascii_lowercase())
    });
    Ok(unmanaged)
}

/// List every loadable DLL that is not owned by Perfect Sync or identical to
/// the selected profile's copy.
pub fn unmanaged_plugins(
    profiles_root: &Path,
    profile_id: &str,
    game_dir: &Path,
) -> io::Result<Vec<UnmanagedPlugin>> {
    let _guard = SYNC_LOCK
        .lock()
        .map_err(|_| io::Error::other("plugin sync lock is poisoned"))?;
    unmanaged_plugins_unlocked(profiles_root, profile_id, game_dir)
}

fn resolve_unmanaged_selection(
    plugins: Vec<UnmanagedPlugin>,
    selected_paths: &[String],
) -> io::Result<Vec<UnmanagedPlugin>> {
    if selected_paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "select at least one unmanaged plugin",
        ));
    }
    if selected_paths.len() > MAX_MANAGED_PLUGINS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many unmanaged plugins selected",
        ));
    }
    let mut requested = HashSet::with_capacity(selected_paths.len());
    for path in selected_paths {
        if path.is_empty() || path.len() > MAX_ZIP_PATH_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid unmanaged plugin selection",
            ));
        }
        if !requested.insert(plugin_path_key(path)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unmanaged plugin selection contains duplicates",
            ));
        }
    }
    let selected = plugins
        .into_iter()
        .filter(|plugin| requested.remove(&plugin_path_key(&plugin.path)))
        .collect::<Vec<_>>();
    if !requested.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unmanaged plugin selection is stale; review the folder again",
        ));
    }
    Ok(selected)
}

/// Re-read the game folder and return exactly the selected unmanaged DLLs.
pub fn selected_unmanaged_plugins(
    profiles_root: &Path,
    profile_id: &str,
    game_dir: &Path,
    selected_paths: &[String],
) -> io::Result<Vec<UnmanagedPlugin>> {
    let _guard = SYNC_LOCK
        .lock()
        .map_err(|_| io::Error::other("plugin sync lock is poisoned"))?;
    resolve_unmanaged_selection(
        unmanaged_plugins_unlocked(profiles_root, profile_id, game_dir)?,
        selected_paths,
    )
}

fn digest_hex(digest: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

/// Move unmanaged DLLs outside BepInEx's plugin scan without deleting them.
/// The caller supplies the broader game transaction used for rollback.
pub fn quarantine_unmanaged_plugins(
    profiles_root: &Path,
    profile_id: &str,
    game_dir: &Path,
    selected_paths: &[String],
) -> io::Result<Vec<UnmanagedPlugin>> {
    let _guard = SYNC_LOCK
        .lock()
        .map_err(|_| io::Error::other("plugin sync lock is poisoned"))?;
    let plugins = resolve_unmanaged_selection(
        unmanaged_plugins_unlocked(profiles_root, profile_id, game_dir)?,
        selected_paths,
    )?;

    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| io::Error::other("system clock is before the Unix epoch"))?
        .as_millis();
    let batch_name = format!(
        "{created_at}-{}-{}",
        std::process::id(),
        QUARANTINE_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let destination_root = game_dir
        .join("BepInEx")
        .join(UNMANAGED_QUARANTINE_DIR)
        .join(batch_name);
    fs::create_dir_all(destination_root.join("plugins"))?;
    crate::profile::reject_reparse(&destination_root)?;

    let source_root = game_dir.join("BepInEx").join("plugins");
    let mut manifest_files = Vec::with_capacity(plugins.len());
    for plugin in &plugins {
        let source = source_root.join(&plugin.path);
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != plugin.size
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("plugin changed before quarantine: {}", plugin.path),
            ));
        }
        let digest = regular_file_digest(&source)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "plugin disappeared"))?;
        manifest_files.push(QuarantinedPlugin {
            original_path: plugin.path.clone(),
            size: plugin.size,
            sha256: digest_hex(digest),
        });
    }
    let manifest = PluginQuarantineManifest {
        created_at,
        files: manifest_files,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if manifest_bytes.len() as u64 > MAX_MANAGED_MARKER_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "plugin quarantine manifest is too large",
        ));
    }
    let pending = destination_root.join(QUARANTINE_MANIFEST_PENDING);
    let mut manifest_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&pending)?;
    manifest_file.write_all(&manifest_bytes)?;
    manifest_file.sync_all()?;
    drop(manifest_file);

    let mut moved = Vec::with_capacity(plugins.len());
    let result = (|| {
        for plugin in &plugins {
            let source = source_root.join(&plugin.path);
            let destination = destination_root.join("plugins").join(&plugin.path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
                crate::profile::reject_reparse(parent)?;
            }
            crate::profile::reject_reparse(&destination)?;
            fs::rename(&source, &destination)?;
            moved.push((source, destination));
        }
        fs::rename(&pending, destination_root.join(QUARANTINE_MANIFEST))
    })();
    if let Err(error) = result {
        for (source, destination) in moved.into_iter().rev() {
            if let Some(parent) = source.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::rename(destination, source);
        }
        let _ = fs::remove_dir_all(&destination_root);
        return Err(error);
    }
    Ok(plugins)
}

/// Permanently remove selected unmanaged DLLs. The caller must provide rollback.
pub fn delete_unmanaged_plugins(
    profiles_root: &Path,
    profile_id: &str,
    game_dir: &Path,
    selected_paths: &[String],
) -> io::Result<Vec<UnmanagedPlugin>> {
    let _guard = SYNC_LOCK
        .lock()
        .map_err(|_| io::Error::other("plugin sync lock is poisoned"))?;
    let plugins = resolve_unmanaged_selection(
        unmanaged_plugins_unlocked(profiles_root, profile_id, game_dir)?,
        selected_paths,
    )?;
    let source_root = game_dir.join("BepInEx").join("plugins");
    for plugin in &plugins {
        let source = source_root.join(&plugin.path);
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != plugin.size
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("plugin changed before deletion: {}", plugin.path),
            ));
        }
    }
    for plugin in &plugins {
        fs::remove_file(source_root.join(&plugin.path))?;
    }
    Ok(plugins)
}

fn copy_bounded_profile_dll(
    source: &Path,
    destination: &Path,
    expected_len: u64,
) -> io::Result<()> {
    let mut input = File::open(source)?;
    let opened = input.metadata()?;
    if !opened.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "profile DLL is not a regular file",
        ));
    }
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let copied = {
        let mut limited = Read::take(&mut input, MAX_MANAGED_PLUGIN_BYTES + 1);
        io::copy(&mut limited, &mut output)?
    };
    if copied != expected_len {
        drop(output);
        drop(input);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "profile DLL changed while it was being copied",
        ));
    }
    output.sync_all()?;
    drop(output);
    drop(input);
    Ok(())
}

/// Synchronize only Perfect Sync-managed DLLs while preserving unmanaged plugins.
pub fn sync_profile_plugins(
    profiles_root: &Path,
    profile_id: &str,
    game_dir: &Path,
) -> io::Result<()> {
    sync_profile_plugins_shadowing(profiles_root, profile_id, game_dir, &[])
}

/// Synchronize profile DLLs while leaving package-owned versions of the named
/// plugins untouched. Shadowed profile files remain available for later use.
pub fn sync_profile_plugins_shadowing(
    profiles_root: &Path,
    profile_id: &str,
    game_dir: &Path,
    shadowed_names: &[String],
) -> io::Result<()> {
    let _guard = SYNC_LOCK
        .lock()
        .map_err(|_| io::Error::other("plugin sync lock is poisoned"))?;
    let source_bep = checked_profile_bepinex_dir(profiles_root, profile_id)?;
    let source = source_bep.join("plugins");
    crate::profile::reject_reparse(&source)?;
    if !fs::metadata(&source).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "profile plugins directory not found",
        ));
    }

    let destination = game_dir.join("BepInEx").join("plugins");
    crate::profile::reject_reparse(game_dir)?;
    crate::profile::reject_reparse(&game_dir.join("BepInEx"))?;
    crate::profile::reject_reparse(&destination)?;
    recover_interrupted_plugin_sync(&destination)?;
    let marker = destination.join(MANAGED_PLUGINS_MARKER);
    let mut previously_owned = read_managed_plugins(&marker)?;
    if shadowed_names.len() > MAX_MANAGED_PLUGINS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many shadowed plugin names",
        ));
    }
    let mut shadowed = HashSet::with_capacity(shadowed_names.len());
    for name in shadowed_names {
        crate::profile::validate_dll_name(name)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        if !shadowed.insert(name.to_ascii_lowercase()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "shadowed plugin names collide case-insensitively",
            ));
        }
    }
    previously_owned.retain(|name| !shadowed.contains(name));

    let mut selected = HashMap::<String, (String, PathBuf, u64)>::new();
    let mut total_source_bytes = 0_u64;
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile plugins contain a symlink",
            ));
        }
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().into_string().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "non-Unicode plugin filename")
        })?;
        if !name.to_ascii_lowercase().ends_with(".dll") {
            continue;
        }
        crate::profile::validate_dll_name(&name)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let key = name.to_ascii_lowercase();
        if shadowed.contains(&key) {
            continue;
        }
        if selected.contains_key(&key) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile has case-colliding DLLs",
            ));
        }
        if selected.len() >= MAX_MANAGED_PLUGINS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile has too many managed plugins",
            ));
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile DLL is empty or not regular",
            ));
        }
        let length = metadata.len();
        if length > MAX_MANAGED_PLUGIN_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile DLL exceeds the managed plugin size limit",
            ));
        }
        total_source_bytes = total_source_bytes.checked_add(length).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "managed plugin aggregate size overflow",
            )
        })?;
        if total_source_bytes > MAX_MANAGED_PLUGIN_TOTAL_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile DLLs exceed the aggregate size limit",
            ));
        }
        selected.insert(key, (name, path, length));
    }

    let mut owned_targets = HashMap::new();
    if destination.is_dir() {
        for entry in fs::read_dir(&destination)? {
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "non-Unicode game plugin filename",
                )
            })?;
            if name == MANAGED_PLUGINS_MARKER {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "game plugins contain a symlink",
                ));
            }
            let key = name.to_ascii_lowercase();
            if !previously_owned.contains(&key) {
                if let Some((_, source, _)) = selected.get(&key) {
                    let same_file = file_type.is_file()
                        && regular_file_digest(&entry.path())? == regular_file_digest(source)?;
                    if same_file {
                        previously_owned.insert(key.clone());
                    } else {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            format!(
                                "{name} already exists outside Perfect Sync and differs from the selected profile version. Remove it from the game's BepInEx/plugins folder, or exclude that dependency in Review selection."
                            ),
                        ));
                    }
                }
            }
            if previously_owned.contains(&key) {
                if !file_type.is_file() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "managed plugin target is not a regular file",
                    ));
                }
                if owned_targets.insert(key, name).is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "managed game plugins collide case-insensitively",
                    ));
                }
            } else if !file_type.is_file() && !file_type.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "game plugins contain a special file",
                ));
            }
        }
    }

    let transaction = plugin_sync_transaction(&destination)?;
    let transaction_parent = transaction.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "plugin sync transaction has no parent",
        )
    })?;
    fs::create_dir_all(transaction_parent)?;
    crate::profile::reject_reparse(transaction_parent)?;
    fs::create_dir(&transaction)?;
    let stage = transaction.join(PLUGIN_SYNC_STAGE);
    let backup = transaction.join(PLUGIN_SYNC_BACKUP);
    let journal_path = transaction.join(PLUGIN_SYNC_JOURNAL);
    let result = (|| {
        fs::create_dir(&stage)?;
        let mut managed_names = Vec::new();
        for (key, (name, path, expected_len)) in &selected {
            let staged_name = owned_targets.get(key).map_or(name.as_str(), String::as_str);
            copy_bounded_profile_dll(path, &stage.join(staged_name), *expected_len)?;
            managed_names.push(
                owned_targets
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| name.clone()),
            );
        }
        managed_names.sort_by_key(|name| name.to_ascii_lowercase());
        let mut target_set: HashSet<PathBuf> = owned_targets
            .values()
            .map(|name| PathBuf::from(name.as_str()))
            .collect();
        target_set.extend(
            managed_names
                .iter()
                .map(|name| PathBuf::from(name.as_str())),
        );
        let marker_json = serde_json::to_vec(&ManagedPlugins {
            names: managed_names,
        })
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if marker_json.len() as u64 > MAX_MANAGED_MARKER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed plugin marker would be too large",
            ));
        }
        let mut marker_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(stage.join(MANAGED_PLUGINS_MARKER))?;
        marker_file.write_all(&marker_json)?;
        marker_file.sync_all()?;
        drop(marker_file);

        let marker_relative = PathBuf::from(MANAGED_PLUGINS_MARKER);
        target_set.insert(marker_relative.clone());
        let mut targets: Vec<_> = target_set.into_iter().collect();
        targets.sort();
        targets.retain(|relative| relative != &marker_relative);
        targets.push(marker_relative);
        let journal = PluginSyncJournal {
            targets: targets
                .iter()
                .map(|relative| {
                    let name = relative
                        .to_str()
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "plugin transaction target is not Unicode",
                            )
                        })?
                        .to_string();
                    Ok(PluginSyncTarget {
                        old_sha256: regular_file_digest(&destination.join(relative))?,
                        new_sha256: regular_file_digest(&stage.join(relative))?,
                        name,
                    })
                })
                .collect::<io::Result<Vec<_>>>()?,
        };
        let journal_json = serde_json::to_vec(&journal)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if journal_json.len() as u64 > MAX_PLUGIN_SYNC_JOURNAL_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin sync journal would be too large",
            ));
        }
        write_sync_record(
            &transaction,
            PLUGIN_SYNC_JOURNAL_PENDING,
            PLUGIN_SYNC_JOURNAL,
            &journal_json,
        )?;
        commit_staged_files_retaining_backup(
            &stage,
            &destination,
            &backup,
            &targets,
            &[PathBuf::new()],
        )?;
        write_sync_record(
            &transaction,
            PLUGIN_SYNC_COMMITTED_PENDING,
            PLUGIN_SYNC_COMMITTED,
            b"committed",
        )
    })();
    if (result.is_ok() || !journal_path.exists())
        && validate_sync_transaction_tree(&transaction).is_ok()
    {
        let _ = remove_any(&transaction);
    }
    result
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedLevelImposterMaps {
    names: Vec<String>,
}

fn valid_levelimposter_map_name(name: &str) -> bool {
    let Some(id) = name.strip_suffix(".lim2") else {
        return false;
    };
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

fn read_managed_levelimposter_maps(marker: &Path) -> io::Result<HashSet<String>> {
    let metadata = match fs::symlink_metadata(marker) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed LevelImposter map marker is not a regular file",
            ));
        }
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => return Err(error),
    };
    if metadata.len() > MAX_MANAGED_MARKER_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed LevelImposter map marker is too large",
        ));
    }
    let managed: ManagedLevelImposterMaps = serde_json::from_slice(&fs::read(marker)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if managed.names.len() > MAX_MANAGED_LEVELIMPOSTER_MAPS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed LevelImposter map marker has too many names",
        ));
    }
    let mut names = HashSet::with_capacity(managed.names.len());
    for name in managed.names {
        if !valid_levelimposter_map_name(&name) || !names.insert(name.to_ascii_lowercase()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed LevelImposter map marker has invalid names",
            ));
        }
    }
    Ok(names)
}

pub fn levelimposter_maps_are_current(game_dir: &Path, map_ids: &[String]) -> io::Result<bool> {
    let marker = game_dir
        .join("BepInEx")
        .join("plugins")
        .join(LEVELIMPOSTER_MAP_DIRECTORY)
        .join(MANAGED_LEVELIMPOSTER_MAPS_MARKER);
    let managed = read_managed_levelimposter_maps(&marker)?;
    let mut expected = HashSet::with_capacity(map_ids.len());
    for id in map_ids {
        let name = format!("{id}.lim2").to_ascii_lowercase();
        if !valid_levelimposter_map_name(&name) || !expected.insert(name) {
            return Ok(false);
        }
    }
    Ok(managed == expected)
}

fn copy_bounded_levelimposter_map(
    source: &Path,
    destination: &Path,
    expected_len: u64,
) -> io::Result<()> {
    let mut input = File::open(source)?;
    if !input.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "profile LevelImposter map is not a regular file",
        ));
    }
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let copied = {
        let mut limited = Read::take(&mut input, MAX_MANAGED_LEVELIMPOSTER_MAP_BYTES + 1);
        io::copy(&mut limited, &mut output)?
    };
    if copied != expected_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "profile LevelImposter map changed while it was copied",
        ));
    }
    output.sync_all()
}

/// Synchronize profile-owned LevelImposter `.lim2` maps while preserving maps
/// added outside Perfect Sync. The caller includes this in the game artifact
/// transaction so any filesystem failure restores the prior map set.
pub fn sync_levelimposter_maps(
    profiles_root: &Path,
    profile_id: &str,
    game_dir: &Path,
) -> io::Result<()> {
    let _guard = SYNC_LOCK
        .lock()
        .map_err(|_| io::Error::other("map sync lock is poisoned"))?;
    let source = checked_profile_bepinex_dir(profiles_root, profile_id)?
        .join("plugins")
        .join(LEVELIMPOSTER_MAP_DIRECTORY);
    let destination = game_dir
        .join("BepInEx")
        .join("plugins")
        .join(LEVELIMPOSTER_MAP_DIRECTORY);
    crate::profile::reject_reparse(game_dir)?;
    crate::profile::reject_reparse(&game_dir.join("BepInEx"))?;
    crate::profile::reject_reparse(&game_dir.join("BepInEx").join("plugins"))?;
    crate::profile::reject_reparse(&destination)?;

    let mut selected = HashMap::<String, (String, PathBuf, u64)>::new();
    let mut total_bytes = 0_u64;
    match fs::read_dir(&source) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let file_type = entry.file_type()?;
                if file_type.is_symlink() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "profile LevelImposter maps contain a symlink",
                    ));
                }
                if !file_type.is_file() {
                    continue;
                }
                let name = entry.file_name().into_string().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "profile LevelImposter map name is not Unicode",
                    )
                })?;
                if !valid_levelimposter_map_name(&name) {
                    continue;
                }
                if selected.len() >= MAX_MANAGED_LEVELIMPOSTER_MAPS {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "profile has too many LevelImposter maps",
                    ));
                }
                let metadata = fs::symlink_metadata(entry.path())?;
                if !metadata.is_file()
                    || metadata.len() == 0
                    || metadata.len() > MAX_MANAGED_LEVELIMPOSTER_MAP_BYTES
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "profile LevelImposter map has an invalid size",
                    ));
                }
                total_bytes = total_bytes.checked_add(metadata.len()).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "LevelImposter map size overflow",
                    )
                })?;
                if total_bytes > MAX_MANAGED_LEVELIMPOSTER_MAP_TOTAL_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "profile LevelImposter maps exceed the aggregate size limit",
                    ));
                }
                let key = name.to_ascii_lowercase();
                if selected
                    .insert(key, (name, entry.path(), metadata.len()))
                    .is_some()
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "profile LevelImposter maps collide case-insensitively",
                    ));
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let marker = destination.join(MANAGED_LEVELIMPOSTER_MAPS_MARKER);
    let previously_owned = read_managed_levelimposter_maps(&marker)?;
    if !destination.exists() && selected.is_empty() && previously_owned.is_empty() {
        return Ok(());
    }
    if destination.exists() {
        let metadata = fs::symlink_metadata(&destination)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "game LevelImposter map path is not a regular directory",
            ));
        }
        for entry in fs::read_dir(&destination)? {
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "game LevelImposter map name is not Unicode",
                )
            })?;
            if name == MANAGED_LEVELIMPOSTER_MAPS_MARKER {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "game LevelImposter maps contain a symlink",
                ));
            }
            let key = name.to_ascii_lowercase();
            if selected.contains_key(&key) && !previously_owned.contains(&key) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("managed LevelImposter map would overwrite unmanaged file {name}"),
                ));
            }
            if previously_owned.contains(&key) && !file_type.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "managed LevelImposter map target is not a regular file",
                ));
            }
        }
    } else {
        fs::create_dir_all(&destination)?;
    }

    for name in &previously_owned {
        let target = destination.join(name);
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "managed LevelImposter map target changed type",
                ));
            }
            Ok(_) => fs::remove_file(target)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    let mut managed_names = Vec::with_capacity(selected.len());
    for (name, source, expected_len) in selected.values() {
        let target = destination.join(name);
        let temporary = crate::profile::unique_sibling(&target, "map-sync")?;
        let result = copy_bounded_levelimposter_map(source, &temporary, *expected_len)
            .and_then(|()| fs::rename(&temporary, &target));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        managed_names.push(name.clone());
    }
    managed_names.sort_by_key(|name| name.to_ascii_lowercase());
    let marker_json = serde_json::to_vec(&ManagedLevelImposterMaps {
        names: managed_names,
    })
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if marker_json.len() as u64 > MAX_MANAGED_MARKER_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed LevelImposter map marker would be too large",
        ));
    }
    let temporary_marker = crate::profile::unique_sibling(&marker, "map-marker")?;
    let marker_result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_marker)?;
        file.write_all(&marker_json)?;
        file.sync_all()?;
        drop(file);
        match fs::remove_file(&marker) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::rename(&temporary_marker, &marker)
    })();
    if marker_result.is_err() {
        let _ = fs::remove_file(temporary_marker);
    }
    marker_result
}
#[cfg(test)]
mod tests {
    use super::*;

    fn make_pack(pack: &Path) {
        fs::create_dir_all(pack.join("dotnet")).unwrap();
        fs::create_dir_all(pack.join("BepInEx").join("core")).unwrap();
        fs::create_dir_all(pack.join("BepInEx").join("config")).unwrap();
        fs::write(pack.join("winhttp.dll"), b"proxy").unwrap();
        fs::write(pack.join("doorstop_config.ini"), b"[General]").unwrap();
        fs::write(pack.join("dotnet").join("coreclr.dll"), b"clr").unwrap();
        fs::write(
            pack.join("BepInEx").join("core").join(IL2CPP_PRELOADER),
            b"pre",
        )
        .unwrap();
        fs::write(
            pack.join("BepInEx").join("config").join("BepInEx.cfg"),
            b"cfg",
        )
        .unwrap();
    }

    #[test]
    fn loader_cache_accepts_semver_build_metadata_safely() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            loader_cache_dir(temp.path(), "be.753+0d275a4", "x86").unwrap(),
            temp.path()
                .join("bepinex")
                .join("be.753+0d275a4")
                .join("x86")
        );
        for unsafe_key in ["../be.753", "be.753/x86", "CON", "be.753:bad"] {
            assert!(loader_cache_dir(temp.path(), unsafe_key, "x86").is_err());
        }
    }
    fn make_pack_zip(body: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            for path in [
                "Pack/winhttp.dll",
                "Pack/doorstop_config.ini",
                "Pack/dotnet/coreclr.dll",
                "Pack/BepInEx/core/BepInEx.Unity.IL2CPP.dll",
            ] {
                writer.start_file(path, options).unwrap();
                writer.write_all(body).unwrap();
            }
            writer.finish().unwrap();
        }
        bytes
    }

    fn make_tou_package_zip(identity: u8, include_legacy: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            for path in [
                "Tou/.doorstop_version",
                "Tou/winhttp.dll",
                "Tou/doorstop_config.ini",
                "Tou/steam_appid.txt",
                "Tou/changelog.txt",
                "Tou/dotnet/coreclr.dll",
                "Tou/BepInEx/core/BepInEx.Unity.IL2CPP.dll",
                "Tou/BepInEx/unity-libs/System.dll",
                "Tou/BepInEx/patchers/BepInEx.SplashScreen/BepInEx.SplashScreen.GUI.exe",
            ] {
                writer.start_file(path, options).unwrap();
                writer.write_all(&[identity]).unwrap();
            }
            for required in crate::profile::TOU_REQUIRED_FILES {
                writer
                    .start_file(format!("Tou/BepInEx/{required}"), options)
                    .unwrap();
                writer.write_all(&[identity]).unwrap();
            }
            if include_legacy {
                writer
                    .start_file("Tou/BepInEx/plugins/Legacy.dll", options)
                    .unwrap();
                writer.write_all(&[identity]).unwrap();
            }
            writer.finish().unwrap();
        }
        bytes
    }

    #[test]
    fn installs_updates_and_removes_complete_tou_package() {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("Among Us.exe"), b"game").unwrap();
        fs::create_dir_all(game.join("BepInEx/plugins")).unwrap();
        fs::write(game.join("BepInEx/plugins/ExistingMod.dll"), b"existing").unwrap();
        let first_key = "a".repeat(64);
        let second_key = "b".repeat(64);

        install_tou_package(
            &make_tou_package_zip(b'1', true),
            &game,
            &first_key,
            "be.753+0d275a4",
        )
        .unwrap();
        assert!(tou_package_is_current(&game, &first_key).unwrap());
        assert!(game.join("BepInEx/plugins/TownOfUsMira.dll").is_file());
        assert!(game
            .join("BepInEx/config/at.duikbo.regioninstall.cfg")
            .is_file());
        assert!(game.join("BepInEx/unity-libs/System.dll").is_file());
        assert!(game.join("steam_appid.txt").is_file());
        assert!(game.join("BepInEx/plugins/Legacy.dll").is_file());
        let splash_original = game.join(SPLASH_SCREEN_ORIGINAL);
        let splash_runtime = game.join(SPLASH_SCREEN_RUNTIME_NAME);
        fs::rename(&splash_original, &splash_runtime).unwrap();
        assert!(tou_package_is_current(&game, &first_key).unwrap());
        fs::rename(&splash_runtime, &splash_original).unwrap();
        assert_eq!(
            fs::read(game.join("BepInEx/plugins/ExistingMod.dll")).unwrap(),
            b"existing"
        );

        install_tou_package(
            &make_tou_package_zip(b'2', false),
            &game,
            &second_key,
            "be.753+0d275a4",
        )
        .unwrap();
        assert!(!tou_package_is_current(&game, &first_key).unwrap());
        assert!(tou_package_is_current(&game, &second_key).unwrap());
        assert_eq!(
            fs::read(game.join("BepInEx/plugins/TownOfUsMira.dll")).unwrap(),
            b"2"
        );
        assert!(!game.join("BepInEx/plugins/Legacy.dll").exists());

        remove_tou_package(&game).unwrap();
        assert!(!tou_package_is_current(&game, &second_key).unwrap());
        assert!(!game.join("BepInEx/plugins/TownOfUsMira.dll").exists());
        assert!(!game.join("steam_appid.txt").exists());
        assert_eq!(
            fs::read(game.join("BepInEx/plugins/ExistingMod.dll")).unwrap(),
            b"existing"
        );
        assert!(!game
            .join("BepInEx")
            .join(MANAGED_TOU_PACKAGE_MARKER)
            .exists());
    }

    fn pe_proxy(machine: u16, identity: u8) -> Vec<u8> {
        let mut bytes = vec![identity; 72];
        bytes[0..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&(64_u32).to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&machine.to_le_bytes());
        bytes
    }

    fn make_doorstop_zip(config: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            for (path, body) in [
                ("x86/winhttp.dll", pe_proxy(0x014c, b'3')),
                ("x64/winhttp.dll", pe_proxy(0x8664, b'6')),
                ("x86/doorstop_config.ini", config.to_vec()),
                ("x64/doorstop_config.ini", config.to_vec()),
                ("x86/.doorstop_version", b"4.5.1".to_vec()),
                ("x64/.doorstop_version", b"4.5.1".to_vec()),
            ] {
                writer.start_file(path, options).unwrap();
                writer.write_all(&body).unwrap();
            }
            writer.finish().unwrap();
        }
        bytes
    }

    #[test]
    fn configures_patch_without_replacing_bepinex_target() {
        let configured = configured_doorstop_ini(
            b"[General]\r\nenabled=true\r\ntarget_assembly=BepInEx\\core\\BepInEx.Unity.IL2CPP.dll\r\n\r\n[Environment]\r\nOTHER=value\r\n",
        )
        .unwrap();
        let configured = String::from_utf8(configured).unwrap();

        assert!(configured.contains("target_assembly=BepInEx\\core\\BepInEx.Unity.IL2CPP.dll\r\n"));
        assert!(configured.contains("OTHER=value\r\n"));
        assert!(configured.contains("GC_DISABLE_INCREMENTAL=1\r\n"));
        assert_eq!(configured.matches("GC_DISABLE_INCREMENTAL").count(), 1);
    }

    #[test]
    fn rejects_config_that_does_not_target_bepinex() {
        let error = configured_doorstop_ini(
            b"[General]\nenabled=true\ntarget_assembly=Another.Loader.dll\n",
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn installs_matching_patch_proxy_for_each_game_architecture() {
        let archive = make_doorstop_zip(
            b"[General]\nenabled=true\ntarget_assembly=BepInEx\\core\\BepInEx.Unity.IL2CPP.dll\n\n[Environment]\n",
        );
        for (arch, machine, identity) in [("x86", 0x014c_u16, b'3'), ("x64", 0x8664, b'6')] {
            let tmp = tempfile::tempdir().unwrap();
            let pack = tmp.path().join("pack");
            let game = tmp.path().join("game");
            make_pack(&pack);
            fs::create_dir_all(&game).unwrap();
            install_pack(&pack, &game, "be.999").unwrap();
            fs::write(
                game.join("doorstop_config.ini"),
                b"[General]\nenabled=true\ntarget_assembly=BepInEx\\core\\BepInEx.Unity.IL2CPP.dll\n",
            )
            .unwrap();

            install_windows_doorstop_patch(&archive, &game, "4.5.1", arch).unwrap();

            let proxy = fs::read(game.join("winhttp.dll")).unwrap();
            assert_eq!(u16::from_le_bytes([proxy[68], proxy[69]]), machine);
            assert_eq!(proxy[2], identity);
            assert!(has_doorstop_patch(&game, "4.5.1", arch));
            assert!(fs::read_to_string(game.join("doorstop_config.ini"))
                .unwrap()
                .contains("GC_DISABLE_INCREMENTAL=1"));
            assert!(!game.join(DOORSTOP_PATCH_TRANSACTION).exists());
        }
    }

    #[test]
    fn failed_patch_validation_preserves_loader_and_cleans_transaction() {
        let archive = make_doorstop_zip(
            b"[General]\nenabled=true\ntarget_assembly=BepInEx\\core\\BepInEx.Unity.IL2CPP.dll\n",
        );
        let tmp = tempfile::tempdir().unwrap();
        let pack = tmp.path().join("pack");
        let game = tmp.path().join("game");
        make_pack(&pack);
        fs::create_dir_all(&game).unwrap();
        install_pack(&pack, &game, "be.999").unwrap();
        let proxy_before = fs::read(game.join("winhttp.dll")).unwrap();
        let config_before = fs::read(game.join("doorstop_config.ini")).unwrap();

        let error = install_windows_doorstop_patch(&archive, &game, "4.5.1", "x64").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(game.join("winhttp.dll")).unwrap(), proxy_before);
        assert_eq!(
            fs::read(game.join("doorstop_config.ini")).unwrap(),
            config_before
        );
        assert!(!game.join(DOORSTOP_PATCH_TRANSACTION).exists());
        assert!(!game.join("BepInEx").join(DOORSTOP_PATCH_MARKER).exists());
    }

    #[test]
    fn installs_pack_into_game_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let pack = tmp.path().join("pack");
        let game = tmp.path().join("game");
        make_pack(&pack);
        fs::create_dir_all(&game).unwrap();

        install_pack(&pack, &game, "be.999").unwrap();

        assert!(game.join("winhttp.dll").exists());
        assert!(game.join("dotnet").join("coreclr.dll").exists());
        assert!(game
            .join("BepInEx")
            .join("core")
            .join(IL2CPP_PRELOADER)
            .exists());
        assert!(game
            .join("BepInEx")
            .join("config")
            .join("BepInEx.cfg")
            .exists());
        assert!(game.join("BepInEx").join("plugins").is_dir());
        assert!(game.join("steam_appid.txt").exists());
        assert!(is_installed(&game));
        assert!(
            has_loader(&game),
            "marker written so our loader is detected"
        );
        assert_eq!(installed_version(&game).as_deref(), Some("be.999"));
    }

    #[test]
    fn parses_latest_build_from_listing_html() {
        let html = r#"
          <a href="projects/bepinex_be/762/BepInEx-Unity.IL2CPP-win-x86-6.0.0-be.762%2Bbd467c9.zip">x</a>
          <a href="projects/bepinex_be/764/BepInEx-Unity.IL2CPP-win-x86-6.0.0-be.764%2B5f39645.zip">x</a>
          <a href="projects/bepinex_be/763/BepInEx-Unity.IL2CPP-win-x64-6.0.0-be.763%2Bda64b22.zip">x</a>
        "#;
        let (id, url) = parse_latest_build(html, "x86").unwrap();
        assert_eq!(id, "be.764");
        assert_eq!(
            url,
            "https://builds.bepinex.dev/projects/bepinex_be/764/BepInEx-Unity.IL2CPP-win-x86-6.0.0-be.764%2B5f39645.zip"
        );
        // x64 picks the x64 asset
        assert_eq!(parse_latest_build(html, "x64").unwrap().0, "be.763");
    }

    #[test]
    fn console_disabled_in_config() {
        let tmp = tempfile::tempdir().unwrap();
        write_console_off(tmp.path()).unwrap();
        let cfg = fs::read_to_string(
            tmp.path()
                .join("BepInEx")
                .join("config")
                .join("BepInEx.cfg"),
        )
        .unwrap();
        assert!(cfg.contains("[Logging.Console]"));
        assert!(cfg.contains("Enabled = false"));
    }

    #[test]
    fn writes_steam_appid_file() {
        let tmp = tempfile::tempdir().unwrap();
        ensure_steam_appid(tmp.path()).unwrap();
        assert_eq!(
            fs::read_to_string(tmp.path().join("steam_appid.txt")).unwrap(),
            "945360"
        );
    }

    #[test]
    fn install_from_zip_then_sync_plugins() {
        let mut buf = Vec::new();
        {
            use std::io::Write;
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            for (path, body) in [
                ("BepInExPack/winhttp.dll", "proxy"),
                ("BepInExPack/doorstop_config.ini", "[General]"),
                ("BepInExPack/dotnet/coreclr.dll", "clr"),
                ("BepInExPack/BepInEx/core/BepInEx.Unity.IL2CPP.dll", "pre"),
            ] {
                zw.start_file(path, opts).unwrap();
                zw.write_all(body.as_bytes()).unwrap();
            }
            zw.finish().unwrap();
        }
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let cache = tmp.path().join("cache");
        let profiles = tmp.path().join("profiles");
        fs::create_dir_all(&game).unwrap();
        install_pack_from_zip(&buf, &game, &cache, "be.test").unwrap();
        assert!(has_loader(&game));

        // profile has a mod + a disabled mod
        let plugins = profile_plugins_dir(&profiles, "p1");
        fs::create_dir_all(&plugins).unwrap();
        fs::write(plugins.join("TheOtherRoles.dll"), b"mod").unwrap();
        fs::write(plugins.join("Off.dll.disabled"), b"off").unwrap();

        sync_profile_plugins(&profiles, "p1", &game).unwrap();
        let game_plugins = game.join("BepInEx").join("plugins");
        assert!(game_plugins.join("TheOtherRoles.dll").exists());
        assert!(!game_plugins.join("Off.dll.disabled").exists()); // disabled not copied

        // switching to an empty profile clears the old plugin
        let empty = profile_plugins_dir(&profiles, "p2");
        fs::create_dir_all(&empty).unwrap();
        sync_profile_plugins(&profiles, "p2", &game).unwrap();
        assert!(!game_plugins.join("TheOtherRoles.dll").exists());
    }

    #[test]
    fn sync_levelimposter_maps_creates_exact_plugin_subdirectory() {
        let temp = tempfile::tempdir().unwrap();
        let profiles = temp.path().join("profiles");
        let game = temp.path().join("game");
        let map = "0ed1f569-eaf5-4ef6-b91c-f41ad78d4018.lim2";
        let source = profile_plugins_dir(&profiles, "p1").join(LEVELIMPOSTER_MAP_DIRECTORY);
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join(map), b"selected").unwrap();

        sync_levelimposter_maps(&profiles, "p1", &game).unwrap();

        let destination = game.join("BepInEx").join("plugins").join("LevelImposter");
        assert_eq!(fs::read(destination.join(map)).unwrap(), b"selected");
        assert!(destination
            .join(MANAGED_LEVELIMPOSTER_MAPS_MARKER)
            .is_file());
    }

    #[test]
    fn sync_levelimposter_maps_replaces_owned_maps_and_preserves_unmanaged_maps() {
        let temp = tempfile::tempdir().unwrap();
        let profiles = temp.path().join("profiles");
        let game = temp.path().join("game");
        let destination = game
            .join("BepInEx")
            .join("plugins")
            .join(LEVELIMPOSTER_MAP_DIRECTORY);
        let first = "0ed1f569-eaf5-4ef6-b91c-f41ad78d4018.lim2";
        let stale = "ef1d13ec-64ce-4c2c-a45d-816fc4ff46da.lim2";
        let unmanaged = "33eaaab4-b5fb-90f7-fb39-a41291409f93.lim2";
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join(stale), b"stale").unwrap();
        fs::write(destination.join(unmanaged), b"user map").unwrap();
        fs::write(
            destination.join(MANAGED_LEVELIMPOSTER_MAPS_MARKER),
            serde_json::to_vec(&ManagedLevelImposterMaps {
                names: vec![stale.into()],
            })
            .unwrap(),
        )
        .unwrap();

        let source = profile_plugins_dir(&profiles, "p1").join(LEVELIMPOSTER_MAP_DIRECTORY);
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join(first), b"selected").unwrap();

        sync_levelimposter_maps(&profiles, "p1", &game).unwrap();
        assert_eq!(fs::read(destination.join(first)).unwrap(), b"selected");
        assert!(!destination.join(stale).exists());
        assert_eq!(fs::read(destination.join(unmanaged)).unwrap(), b"user map");

        sync_levelimposter_maps(&profiles, "empty", &game).unwrap();
        assert!(!destination.join(first).exists());
        assert_eq!(fs::read(destination.join(unmanaged)).unwrap(), b"user map");
    }

    #[test]
    fn extract_all_rejects_absolute_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&dest).unwrap();
        let escape = tmp.path().join("escaped.txt");
        let abs_name = escape.to_string_lossy().replace('\\', "/");

        let mut buf = Vec::new();
        {
            use std::io::Write;
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            writer.start_file(abs_name.as_str(), options).unwrap();
            writer.write_all(b"EVIL").unwrap();
            writer.start_file("ok.txt", options).unwrap();
            writer.write_all(b"good").unwrap();
            writer.finish().unwrap();
        }

        assert!(extract_all(&buf, &dest).is_err());
        assert!(!dest.join("ok.txt").exists());
        assert!(!escape.exists(), "absolute zip entry escaped dest");
    }

    #[test]
    fn sync_preserves_unmanaged_plugins() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let profiles = tmp.path().join("profiles");
        let game_plugins = game.join("BepInEx").join("plugins");
        fs::create_dir_all(&game_plugins).unwrap();
        fs::write(game_plugins.join("UserMod.dll"), b"user").unwrap();

        let profile_plugins = profile_plugins_dir(&profiles, "p1");
        fs::create_dir_all(&profile_plugins).unwrap();
        fs::write(profile_plugins.join("AppMod.DLL"), b"app").unwrap();

        sync_profile_plugins(&profiles, "p1", &game).unwrap();
        assert_eq!(fs::read(game_plugins.join("AppMod.DLL")).unwrap(), b"app");
        assert_eq!(fs::read(game_plugins.join("UserMod.dll")).unwrap(), b"user");
    }

    #[test]
    fn inventories_nested_unmanaged_plugins_without_flagging_owned_files() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let profiles = tmp.path().join("profiles");
        let game_plugins = game.join("BepInEx/plugins");
        let profile_plugins = profile_plugins_dir(&profiles, "p1");
        fs::create_dir_all(game_plugins.join("Nested")).unwrap();
        fs::create_dir_all(&profile_plugins).unwrap();
        fs::write(game_plugins.join("Managed.dll"), b"managed").unwrap();
        fs::write(game_plugins.join("Same.dll"), b"same").unwrap();
        fs::write(game_plugins.join("User.dll"), b"user").unwrap();
        fs::write(game_plugins.join("Nested/Deep.dll"), b"deep").unwrap();
        fs::write(profile_plugins.join("Same.dll"), b"same").unwrap();
        fs::write(
            game_plugins.join(MANAGED_PLUGINS_MARKER),
            br#"{"names":["Managed.dll"]}"#,
        )
        .unwrap();

        let plugins = unmanaged_plugins(&profiles, "p1", &game).unwrap();

        assert_eq!(
            plugins
                .iter()
                .map(|plugin| (plugin.path.as_str(), plugin.importable))
                .collect::<Vec<_>>(),
            vec![("Nested/Deep.dll", false), ("User.dll", true)]
        );
        let selected =
            selected_unmanaged_plugins(&profiles, "p1", &game, &["User.dll".into()]).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].path, "User.dll");
        assert!(selected[0].importable);
        assert!(
            selected_unmanaged_plugins(&profiles, "p1", &game, &["Missing.dll".into()],).is_err()
        );
    }

    #[test]
    fn quarantines_only_selected_plugins_with_a_recovery_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let profiles = tmp.path().join("profiles");
        let game_plugins = game.join("BepInEx/plugins");
        fs::create_dir_all(game_plugins.join("Nested")).unwrap();
        fs::create_dir_all(profile_plugins_dir(&profiles, "p1")).unwrap();
        fs::write(game_plugins.join("User.dll"), b"user").unwrap();
        fs::write(game_plugins.join("Nested/Deep.dll"), b"deep").unwrap();

        let moved =
            quarantine_unmanaged_plugins(&profiles, "p1", &game, &["Nested/Deep.dll".into()])
                .unwrap();

        assert_eq!(moved.len(), 1);
        assert_eq!(fs::read(game_plugins.join("User.dll")).unwrap(), b"user");
        assert!(!game_plugins.join("Nested/Deep.dll").exists());
        assert_eq!(
            unmanaged_plugins(&profiles, "p1", &game)
                .unwrap()
                .iter()
                .map(|plugin| plugin.path.as_str())
                .collect::<Vec<_>>(),
            vec!["User.dll"]
        );
        let quarantine = game.join("BepInEx").join(UNMANAGED_QUARANTINE_DIR);
        let batches = fs::read_dir(&quarantine)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(
            fs::read(batch.join("plugins/Nested/Deep.dll")).unwrap(),
            b"deep"
        );
        assert!(!batch.join("plugins/User.dll").exists());
        let manifest: PluginQuarantineManifest =
            serde_json::from_slice(&fs::read(batch.join(QUARANTINE_MANIFEST)).unwrap()).unwrap();
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].original_path, "Nested/Deep.dll");
        assert_eq!(manifest.files[0].sha256.len(), 64);
    }

    #[test]
    fn deletes_only_selected_unmanaged_plugins() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let profiles = tmp.path().join("profiles");
        let game_plugins = game.join("BepInEx/plugins");
        fs::create_dir_all(&game_plugins).unwrap();
        fs::create_dir_all(profile_plugins_dir(&profiles, "p1")).unwrap();
        fs::write(game_plugins.join("Delete.dll"), b"delete").unwrap();
        fs::write(game_plugins.join("Keep.dll"), b"keep").unwrap();

        let deleted =
            delete_unmanaged_plugins(&profiles, "p1", &game, &["Delete.dll".into()]).unwrap();

        assert_eq!(
            deleted
                .iter()
                .map(|plugin| plugin.path.as_str())
                .collect::<Vec<_>>(),
            vec!["Delete.dll"]
        );
        assert!(!game_plugins.join("Delete.dll").exists());
        assert_eq!(fs::read(game_plugins.join("Keep.dll")).unwrap(), b"keep");
    }

    #[test]
    fn interrupted_plugin_publication_recovers_before_ownership_classification() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let profiles = tmp.path().join("profiles");
        let destination = game.join("BepInEx").join("plugins");
        let profile_plugins = profile_plugins_dir(&profiles, "p1");
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(&profile_plugins).unwrap();
        fs::write(profile_plugins.join("New.dll"), b"new managed").unwrap();
        fs::write(destination.join("Old.dll"), b"old managed").unwrap();
        fs::write(destination.join("User.dll"), b"unmanaged").unwrap();
        fs::write(
            destination.join(MANAGED_PLUGINS_MARKER),
            br#"{"names":["Old.dll"]}"#,
        )
        .unwrap();

        let transaction = plugin_sync_transaction(&destination).unwrap();
        let stage = transaction.join(PLUGIN_SYNC_STAGE);
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("New.dll"), b"new managed").unwrap();
        fs::write(
            stage.join(MANAGED_PLUGINS_MARKER),
            br#"{"names":["New.dll"]}"#,
        )
        .unwrap();
        let journal = PluginSyncJournal {
            targets: vec![
                PluginSyncTarget {
                    name: "New.dll".into(),
                    old_sha256: None,
                    new_sha256: regular_file_digest(&stage.join("New.dll")).unwrap(),
                },
                PluginSyncTarget {
                    name: "Old.dll".into(),
                    old_sha256: regular_file_digest(&destination.join("Old.dll")).unwrap(),
                    new_sha256: None,
                },
                PluginSyncTarget {
                    name: MANAGED_PLUGINS_MARKER.into(),
                    old_sha256: regular_file_digest(&destination.join(MANAGED_PLUGINS_MARKER))
                        .unwrap(),
                    new_sha256: regular_file_digest(&stage.join(MANAGED_PLUGINS_MARKER)).unwrap(),
                },
            ],
        };
        write_sync_record(
            &transaction,
            PLUGIN_SYNC_JOURNAL_PENDING,
            PLUGIN_SYNC_JOURNAL,
            &serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();

        fs::rename(stage.join("New.dll"), destination.join("New.dll")).unwrap();
        assert_eq!(
            read_managed_plugins(&destination.join(MANAGED_PLUGINS_MARKER)).unwrap(),
            HashSet::from(["old.dll".to_string()])
        );

        sync_profile_plugins(&profiles, "p1", &game).unwrap();

        assert_eq!(
            fs::read(destination.join("New.dll")).unwrap(),
            b"new managed"
        );
        assert!(!destination.join("Old.dll").exists());
        assert_eq!(
            fs::read(destination.join("User.dll")).unwrap(),
            b"unmanaged"
        );
        assert_eq!(
            read_managed_plugins(&destination.join(MANAGED_PLUGINS_MARKER)).unwrap(),
            HashSet::from(["new.dll".to_string()])
        );
        assert!(!transaction.exists());
    }

    #[test]
    fn failed_plugin_commit_keeps_retryable_journal_state() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let profiles = tmp.path().join("profiles");
        let destination = game.join("BepInEx").join("plugins");
        let profile_plugins = profile_plugins_dir(&profiles, "p1");
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(&profile_plugins).unwrap();
        fs::write(profile_plugins.join("Managed.dll"), b"new managed").unwrap();
        fs::write(destination.join("Managed.dll"), b"old managed").unwrap();
        fs::write(destination.join("User.dll"), b"unmanaged").unwrap();
        fs::write(
            destination.join(MANAGED_PLUGINS_MARKER),
            br#"{"names":["Managed.dll"]}"#,
        )
        .unwrap();

        let transaction = plugin_sync_transaction(&destination).unwrap();
        let stage = transaction.join(PLUGIN_SYNC_STAGE);
        let backup = transaction.join(PLUGIN_SYNC_BACKUP);
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("Managed.dll"), b"new managed").unwrap();
        fs::write(
            stage.join(MANAGED_PLUGINS_MARKER),
            br#"{"names":["Managed.dll"]}"#,
        )
        .unwrap();
        let journal = PluginSyncJournal {
            targets: vec![
                PluginSyncTarget {
                    name: "Managed.dll".into(),
                    old_sha256: regular_file_digest(&destination.join("Managed.dll")).unwrap(),
                    new_sha256: regular_file_digest(&stage.join("Managed.dll")).unwrap(),
                },
                PluginSyncTarget {
                    name: MANAGED_PLUGINS_MARKER.into(),
                    old_sha256: regular_file_digest(&destination.join(MANAGED_PLUGINS_MARKER))
                        .unwrap(),
                    new_sha256: regular_file_digest(&stage.join(MANAGED_PLUGINS_MARKER)).unwrap(),
                },
            ],
        };
        write_sync_record(
            &transaction,
            PLUGIN_SYNC_JOURNAL_PENDING,
            PLUGIN_SYNC_JOURNAL,
            &serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        let targets = [
            PathBuf::from("Managed.dll"),
            PathBuf::from(MANAGED_PLUGINS_MARKER),
        ];
        let mut rename_count = 0;

        let error = commit_staged_files_with_sentinel_impl(
            &stage,
            &destination,
            &backup,
            &targets,
            &[PathBuf::new()],
            CommitPolicy {
                sentinel: None,
                cleanup_backup: false,
            },
            |source, target| {
                rename_count += 1;
                if rename_count == 4 {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected marker rename failure",
                    ))
                } else {
                    fs::rename(source, target)
                }
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);

        assert!(transaction.join(PLUGIN_SYNC_JOURNAL).is_file());
        assert!(!backup.exists());
        for target in &journal.targets {
            assert_eq!(
                regular_file_digest(&destination.join(&target.name)).unwrap(),
                target.old_sha256,
                "rollback must restore the journal's old destination state for {}",
                target.name
            );
            assert_eq!(
                regular_file_digest(&stage.join(&target.name)).unwrap(),
                target.new_sha256,
                "rollback must return the journal's new bytes to stage for {}",
                target.name
            );
        }
        assert_eq!(
            fs::read(destination.join("User.dll")).unwrap(),
            b"unmanaged"
        );

        sync_profile_plugins(&profiles, "p1", &game).unwrap();

        assert_eq!(
            fs::read(destination.join("Managed.dll")).unwrap(),
            b"new managed"
        );
        assert_eq!(
            fs::read(destination.join("User.dll")).unwrap(),
            b"unmanaged"
        );
        assert_eq!(
            read_managed_plugins(&destination.join(MANAGED_PLUGINS_MARKER)).unwrap(),
            HashSet::from(["managed.dll".to_string()])
        );
        assert!(!transaction.exists());
    }

    #[test]
    fn interrupted_plugin_recovery_refuses_ambiguous_new_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let destination = tmp.path().join("game").join("BepInEx").join("plugins");
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("User.dll"), b"unmanaged").unwrap();
        fs::write(destination.join(MANAGED_PLUGINS_MARKER), br#"{"names":[]}"#).unwrap();

        let transaction = plugin_sync_transaction(&destination).unwrap();
        let stage = transaction.join(PLUGIN_SYNC_STAGE);
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("New.dll"), b"expected new").unwrap();
        fs::write(
            stage.join(MANAGED_PLUGINS_MARKER),
            br#"{"names":["New.dll"]}"#,
        )
        .unwrap();
        let journal = PluginSyncJournal {
            targets: vec![
                PluginSyncTarget {
                    name: "New.dll".into(),
                    old_sha256: None,
                    new_sha256: regular_file_digest(&stage.join("New.dll")).unwrap(),
                },
                PluginSyncTarget {
                    name: MANAGED_PLUGINS_MARKER.into(),
                    old_sha256: regular_file_digest(&destination.join(MANAGED_PLUGINS_MARKER))
                        .unwrap(),
                    new_sha256: regular_file_digest(&stage.join(MANAGED_PLUGINS_MARKER)).unwrap(),
                },
            ],
        };
        write_sync_record(
            &transaction,
            PLUGIN_SYNC_JOURNAL_PENDING,
            PLUGIN_SYNC_JOURNAL,
            &serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        fs::rename(stage.join("New.dll"), destination.join("New.dll")).unwrap();
        fs::write(destination.join("New.dll"), b"externally changed").unwrap();

        let error = recover_interrupted_plugin_sync(&destination).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read(destination.join("New.dll")).unwrap(),
            b"externally changed"
        );
        assert_eq!(
            fs::read(destination.join("User.dll")).unwrap(),
            b"unmanaged"
        );
        assert!(transaction.exists());
    }

    #[test]
    fn partial_pack_cache_is_not_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("winhttp.dll"), b"partial").unwrap();
        assert!(locate_pack_root(&cache).is_none());
    }
    #[test]
    fn failed_whole_cache_publication_restores_the_old_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        publish_pack_cache(&make_pack_zip(b"old"), &cache).unwrap();
        let mut rename_count = 0;

        let error = publish_pack_cache_with(&make_pack_zip(b"new"), &cache, |source, target| {
            rename_count += 1;
            if rename_count == 2 {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected cache publication failure",
                ))
            } else {
                fs::rename(source, target)
            }
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let root = locate_pack_root(&cache).unwrap();
        for relative in [
            "winhttp.dll",
            "doorstop_config.ini",
            "dotnet/coreclr.dll",
            "BepInEx/core/BepInEx.Unity.IL2CPP.dll",
        ] {
            assert_eq!(fs::read(root.join(relative)).unwrap(), b"old");
        }
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn concurrent_cache_publishers_are_serialized() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::thread;
        use std::time::Duration;

        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        publish_pack_cache(&make_pack_zip(b"old"), &cache).unwrap();
        let active_renames = Arc::new(AtomicUsize::new(0));
        let max_active_renames = Arc::new(AtomicUsize::new(0));
        let mut publishers = Vec::new();
        for generation in [&b"first"[..], &b"second"[..]] {
            let cache = cache.clone();
            let bytes = make_pack_zip(generation);
            let active = Arc::clone(&active_renames);
            let maximum = Arc::clone(&max_active_renames);
            publishers.push(thread::spawn(move || {
                publish_pack_cache_with(&bytes, &cache, |source, target| {
                    let concurrent = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(concurrent, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(5));
                    let result = fs::rename(source, target);
                    active.fetch_sub(1, Ordering::SeqCst);
                    result
                })
                .unwrap();
            }));
        }
        for publisher in publishers {
            publisher.join().unwrap();
        }

        assert_eq!(max_active_renames.load(Ordering::SeqCst), 1);
        let root = locate_pack_root(&cache).unwrap();
        let proxy = fs::read(root.join("winhttp.dll")).unwrap();
        assert!(proxy.as_slice() == b"first" || proxy.as_slice() == b"second");
        for relative in [
            "doorstop_config.ini",
            "dotnet/coreclr.dll",
            "BepInEx/core/BepInEx.Unity.IL2CPP.dll",
        ] {
            assert_eq!(fs::read(root.join(relative)).unwrap(), proxy);
        }
    }

    #[test]
    fn sync_adopts_an_identical_unmanaged_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let game_plugins = tmp.path().join("game/BepInEx/plugins");
        let profile_plugins = tmp.path().join("profiles/p1/BepInEx/plugins");
        fs::create_dir_all(&game_plugins).unwrap();
        fs::create_dir_all(&profile_plugins).unwrap();
        fs::write(game_plugins.join("Reactor.dll"), b"same reactor").unwrap();
        fs::write(profile_plugins.join("Reactor.dll"), b"same reactor").unwrap();

        sync_profile_plugins(&tmp.path().join("profiles"), "p1", &tmp.path().join("game")).unwrap();

        assert_eq!(
            read_managed_plugins(&game_plugins.join(MANAGED_PLUGINS_MARKER)).unwrap(),
            HashSet::from(["reactor.dll".to_string()])
        );
        assert_eq!(
            fs::read(game_plugins.join("Reactor.dll")).unwrap(),
            b"same reactor"
        );
    }

    #[test]
    fn package_shadowed_dependencies_resume_after_package_removal() {
        let tmp = tempfile::tempdir().unwrap();
        let profiles = tmp.path().join("profiles");
        let game = tmp.path().join("game");
        let game_plugins = game.join("BepInEx/plugins");
        let profile_plugins = profiles.join("p1/BepInEx/plugins");
        fs::create_dir_all(&game_plugins).unwrap();
        fs::create_dir_all(&profile_plugins).unwrap();
        fs::write(game_plugins.join("MiraAPI.dll"), b"town of us mira").unwrap();
        fs::write(profile_plugins.join("MiraAPI.dll"), b"standalone mira").unwrap();
        fs::write(profile_plugins.join("Other.dll"), b"other").unwrap();
        fs::write(
            game_plugins.join(MANAGED_PLUGINS_MARKER),
            serde_json::to_vec(&ManagedPlugins {
                names: vec!["MiraAPI.dll".into()],
            })
            .unwrap(),
        )
        .unwrap();

        sync_profile_plugins_shadowing(&profiles, "p1", &game, &["MiraAPI.dll".to_string()])
            .unwrap();

        assert_eq!(
            fs::read(game_plugins.join("MiraAPI.dll")).unwrap(),
            b"town of us mira"
        );
        assert_eq!(fs::read(game_plugins.join("Other.dll")).unwrap(), b"other");
        assert_eq!(
            read_managed_plugins(&game_plugins.join(MANAGED_PLUGINS_MARKER)).unwrap(),
            HashSet::from(["other.dll".to_string()])
        );

        fs::remove_file(game_plugins.join("MiraAPI.dll")).unwrap();
        sync_profile_plugins(&profiles, "p1", &game).unwrap();
        assert_eq!(
            fs::read(game_plugins.join("MiraAPI.dll")).unwrap(),
            b"standalone mira"
        );
    }

    #[test]
    fn failed_sync_collision_preserves_game_plugins() {
        let tmp = tempfile::tempdir().unwrap();
        let game_plugins = tmp.path().join("game/BepInEx/plugins");
        let profile_plugins = tmp.path().join("profiles/p1/BepInEx/plugins");
        fs::create_dir_all(&game_plugins).unwrap();
        fs::create_dir_all(&profile_plugins).unwrap();
        fs::write(game_plugins.join("Same.dll"), b"unmanaged").unwrap();
        fs::write(profile_plugins.join("Same.dll"), b"managed").unwrap();

        assert!(
            sync_profile_plugins(&tmp.path().join("profiles"), "p1", &tmp.path().join("game"),)
                .is_err()
        );
        assert_eq!(
            fs::read(game_plugins.join("Same.dll")).unwrap(),
            b"unmanaged"
        );
    }

    #[test]
    fn loader_commit_hides_and_restores_the_old_completion_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let destination = tmp.path().join("game");
        let stage = tmp.path().join("loader-stage");
        let backup = tmp.path().join("loader-old");
        fs::create_dir_all(destination.join("BepInEx").join("core")).unwrap();
        fs::create_dir_all(stage.join("BepInEx")).unwrap();
        fs::write(destination.join("winhttp.dll"), b"old proxy").unwrap();
        fs::write(
            destination
                .join("BepInEx")
                .join("core")
                .join(IL2CPP_PRELOADER),
            b"old preloader",
        )
        .unwrap();
        fs::write(
            destination.join("BepInEx").join(LOADER_MARKER),
            b"old version",
        )
        .unwrap();
        fs::write(stage.join("winhttp.dll"), b"new proxy").unwrap();
        fs::write(stage.join("BepInEx").join(LOADER_MARKER), b"new version").unwrap();
        assert!(has_loader(&destination));
        let marker_relative = PathBuf::from("BepInEx").join(LOADER_MARKER);
        let targets = [PathBuf::from("winhttp.dll"), marker_relative.clone()];
        let mut rename_count = 0;

        let error = commit_staged_files_with_sentinel(
            &stage,
            &destination,
            &backup,
            &targets,
            &[PathBuf::new(), PathBuf::from("BepInEx")],
            Some(&marker_relative),
            |source, target| {
                rename_count += 1;
                if rename_count > 1 {
                    assert!(
                        !has_loader(&destination),
                        "the completion marker must stay absent during commit"
                    );
                }
                if rename_count == 4 {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected loader commit failure",
                    ))
                } else {
                    fs::rename(source, target)
                }
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        fs::remove_dir_all(&stage).unwrap();

        assert_eq!(
            fs::read(destination.join("winhttp.dll")).unwrap(),
            b"old proxy"
        );
        assert_eq!(
            fs::read(destination.join("BepInEx").join(LOADER_MARKER)).unwrap(),
            b"old version"
        );
        assert!(has_loader(&destination));
        assert!(!backup.exists());
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn incomplete_plugin_rollback_preserves_all_file_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        let destination = tmp.path().join("plugins");
        let stage = tmp.path().join("sync-stage");
        let backup = tmp.path().join("sync-old");
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&backup).unwrap();
        fs::write(destination.join("Managed.dll"), b"installed new").unwrap();
        fs::write(stage.join("Managed.dll"), b"ambiguous staged bytes").unwrap();
        fs::write(backup.join("Managed.dll"), b"old managed").unwrap();
        let targets = [PathBuf::from("Managed.dll")];

        let error =
            rollback_targets(&stage, &destination, &backup, &targets, &targets, &[]).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(destination.join("Managed.dll")).unwrap(),
            b"installed new"
        );
        assert_eq!(
            fs::read(stage.join("Managed.dll")).unwrap(),
            b"ambiguous staged bytes"
        );
        assert_eq!(
            fs::read(backup.join("Managed.dll")).unwrap(),
            b"old managed"
        );
    }

    #[test]
    fn failed_plugin_commit_restores_managed_and_marker_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let destination = tmp.path().join("plugins");
        let stage = tmp.path().join("sync-stage");
        let backup = tmp.path().join("sync-old");
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(&stage).unwrap();
        fs::write(destination.join("Managed.dll"), b"old managed").unwrap();
        fs::write(destination.join("User.dll"), b"unmanaged").unwrap();
        fs::write(
            destination.join(MANAGED_PLUGINS_MARKER),
            br#"{"names":["Managed.dll"]}"#,
        )
        .unwrap();
        fs::write(stage.join("Managed.dll"), b"new managed").unwrap();
        fs::write(
            stage.join(MANAGED_PLUGINS_MARKER),
            br#"{"names":["Managed.dll"]}"#,
        )
        .unwrap();
        let targets = [
            PathBuf::from("Managed.dll"),
            PathBuf::from(MANAGED_PLUGINS_MARKER),
        ];
        let mut rename_count = 0;

        let error = commit_staged_files_with(
            &stage,
            &destination,
            &backup,
            &targets,
            &[PathBuf::new()],
            |source, target| {
                rename_count += 1;
                if rename_count == 4 {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected plugin commit failure",
                    ))
                } else {
                    fs::rename(source, target)
                }
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        fs::remove_dir_all(&stage).unwrap();

        assert_eq!(
            fs::read(destination.join("Managed.dll")).unwrap(),
            b"old managed"
        );
        assert_eq!(
            fs::read(destination.join(MANAGED_PLUGINS_MARKER)).unwrap(),
            br#"{"names":["Managed.dll"]}"#
        );
        assert_eq!(
            fs::read(destination.join("User.dll")).unwrap(),
            b"unmanaged"
        );
        assert!(!backup.exists());
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }
    #[test]
    fn sync_rejects_too_many_plugins_before_creating_a_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let profiles = tmp.path().join("profiles");
        let game = tmp.path().join("game");
        let plugins = profile_plugins_dir(&profiles, "p1");
        fs::create_dir_all(&plugins).unwrap();
        for index in 0..=MAX_MANAGED_PLUGINS {
            fs::write(plugins.join(format!("Plugin{index:04}.dll")), b"x").unwrap();
        }

        let error = sync_profile_plugins(&profiles, "p1", &game).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!game
            .join("BepInEx")
            .join("plugins")
            .join(MANAGED_PLUGINS_MARKER)
            .exists());
    }

    #[test]
    fn sync_rejects_per_file_and_aggregate_plugin_size_limits_before_staging() {
        for aggregate in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let profiles = tmp.path().join("profiles");
            let game = tmp.path().join("game");
            let plugins = profile_plugins_dir(&profiles, "p1");
            fs::create_dir_all(&plugins).unwrap();
            if aggregate {
                for index in 0..3 {
                    let file = File::create(plugins.join(format!("Large{index}.dll"))).unwrap();
                    file.set_len(MAX_MANAGED_PLUGIN_BYTES).unwrap();
                }
            } else {
                let file = File::create(plugins.join("TooLarge.dll")).unwrap();
                file.set_len(MAX_MANAGED_PLUGIN_BYTES + 1).unwrap();
            }

            let error = sync_profile_plugins(&profiles, "p1", &game).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(!game.join("BepInEx").join("plugins").exists());
        }
    }

    #[test]
    fn bounded_plugin_copy_rejects_a_changed_source_length() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("Source.dll");
        let destination = tmp.path().join("Staged.dll");
        fs::write(&source, b"grew").unwrap();

        let error = copy_bounded_profile_dll(&source, &destination, 3).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn invalid_loader_pack_keeps_existing_loader_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let pack = tmp.path().join("bad-pack");
        fs::create_dir_all(&game).unwrap();
        fs::create_dir_all(&pack).unwrap();
        fs::write(game.join("winhttp.dll"), b"old").unwrap();
        fs::write(pack.join("winhttp.dll"), b"new").unwrap();

        assert!(install_pack(&pack, &game, "be.bad").is_err());
        assert_eq!(fs::read(game.join("winhttp.dll")).unwrap(), b"old");
    }

    #[test]
    fn is_outdated_compares_build_ids_and_replaces_incomparable_sources() {
        assert!(is_outdated(Some("be.764"), "be.770"));
        assert!(!is_outdated(Some("be.770"), "be.764"));
        assert!(!is_outdated(Some("be.764"), "be.764"));
        assert!(is_outdated(Some("1.2.3"), "1.3.0"));
        assert!(!is_outdated(Some("1.3.0"), "1.2.3"));
        assert!(!is_outdated(Some("pinned"), "pinned"));
        assert!(is_outdated(Some("pinned-old"), "pinned-new"));
        assert!(is_outdated(Some("pinned"), "au-2026-07"));
        assert!(is_outdated(Some("au-2026-06"), "au-2026-07"));
        assert!(is_outdated(Some("be.770"), "au-2026-07"));
        assert!(is_outdated(Some("1.3.0"), "be.770"));
        assert!(is_outdated(None, "be.764"));
    }
}
