//! Verified relocation of large managed game data and package caches.

use atomicwrites::{AllowOverwrite, AtomicFile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const STORAGE_MARKER: &str = ".perfectsync-storage.json";
const STORAGE_CLEANUP_MARKER: &str = ".perfectsync-storage-cleanup";
const STORAGE_SCHEMA: u32 = 1;
const MANAGED_DIRECTORY: &str = "managed-games";
const SOURCE_RECORD_DIRECTORY: &str = "sources";
const WORKSPACE_DIRECTORY: &str = "workspace";
const OBSOLETE_BASE_DIRECTORY: &str = "bases";
const CACHE_DIRECTORY: &str = "cache";
const MAX_STORAGE_FILES: usize = 250_000;
const MAX_STORAGE_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_STORAGE_MARKER_BYTES: u64 = 64 * 1024;

static MOVE_SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StorageMarker {
    schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_cleanup: Option<PendingStorageCleanup>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingStorageCleanup {
    token: String,
    trees: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeFile {
    relative: PathBuf,
    size: u64,
    sha256: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeManifest {
    directories: Vec<PathBuf>,
    files: Vec<TreeFile>,
    bytes: u64,
}

#[derive(Debug)]
pub struct StorageTarget {
    pub root: PathBuf,
    pub configured_path: Option<String>,
    cache_path: PathBuf,
    marker_existed: bool,
    protected_sources: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct PublishedStorage {
    pub root: PathBuf,
    marker_created: bool,
    protected_sources: Vec<PathBuf>,
    old_cleanup: Option<PendingStorageCleanup>,
    rollback_cleanup: PendingStorageCleanup,
}

fn invalid(message: impl Into<String>) -> String {
    message.into()
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
#[cfg(windows)]
fn has_single_link(file: &File, _metadata: &fs::Metadata) -> bool {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    struct ByHandleFileInformation {
        file_attributes: u32,
        creation_time: FileTime,
        last_access_time: FileTime,
        last_write_time: FileTime,
        volume_serial_number: u32,
        file_size_high: u32,
        file_size_low: u32,
        number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(
            file: *mut c_void,
            information: *mut ByHandleFileInformation,
        ) -> i32;
    }

    let mut information: ByHandleFileInformation = unsafe { std::mem::zeroed() };
    unsafe {
        GetFileInformationByHandle(file.as_raw_handle(), &mut information) != 0
            && information.number_of_links == 1
    }
}

#[cfg(unix)]
fn has_single_link(_file: &File, metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() == 1
}

#[cfg(not(any(windows, unix)))]
fn has_single_link(_file: &File, _metadata: &fs::Metadata) -> bool {
    true
}

fn path_has_single_link(path: &Path, metadata: &fs::Metadata) -> bool {
    File::open(path).is_ok_and(|file| has_single_link(&file, metadata))
}

fn normalized(path: &Path) -> String {
    let value = path
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    if let Some(unc) = value.strip_prefix("//?/unc/") {
        format!("//{unc}")
    } else {
        value.strip_prefix("//?/").unwrap_or(&value).to_string()
    }
}

fn canonical_or_original(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn same_path(left: &Path, right: &Path) -> bool {
    normalized(left) == normalized(right)
}

fn path_contains(parent: &Path, child: &Path) -> bool {
    let parent = normalized(parent);
    let child = normalized(child);
    child == parent
        || child
            .strip_prefix(&parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    path_contains(left, right) || path_contains(right, left)
}

fn validate_regular_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {label} {}: {error}", path.display()))?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!("{label} must be a regular non-linked directory"));
    }
    Ok(())
}

fn read_marker(root: &Path) -> Result<Option<StorageMarker>, String> {
    let path = root.join(STORAGE_MARKER);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not inspect the storage marker: {error}")),
    };
    if is_reparse(&metadata) || !metadata.is_file() || metadata.len() > MAX_STORAGE_MARKER_BYTES {
        return Err("the selected folder has an invalid Perfect Sync storage marker".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(&path)
        .map_err(|error| format!("could not read the storage marker: {error}"))?
        .take(MAX_STORAGE_MARKER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read the storage marker: {error}"))?;
    if bytes.len() as u64 != metadata.len() {
        return Err("the selected folder has a storage marker that changed while reading".into());
    }
    let marker: StorageMarker = serde_json::from_slice(&bytes).map_err(|_| {
        "the selected folder has an invalid Perfect Sync storage marker".to_string()
    })?;
    if marker.schema != STORAGE_SCHEMA {
        return Err("the selected folder uses an unsupported Perfect Sync storage format".into());
    }
    validate_pending_cleanup(marker.pending_cleanup.as_ref())?;
    Ok(Some(marker))
}

fn validate_pending_cleanup(pending: Option<&PendingStorageCleanup>) -> Result<(), String> {
    let Some(pending) = pending else {
        return Ok(());
    };
    if pending.token.len() != 64
        || !pending
            .token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || pending.trees.is_empty()
        || pending.trees.len() > 16
        || pending
            .trees
            .iter()
            .any(|path| !Path::new(path).is_absolute())
    {
        return Err("the selected folder has invalid pending storage cleanup evidence".into());
    }
    Ok(())
}

fn write_marker_state(root: &Path, marker: &StorageMarker) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(marker).map_err(|error| error.to_string())?;
    AtomicFile::new(root.join(STORAGE_MARKER), AllowOverwrite)
        .write(|output| {
            output.write_all(&bytes)?;
            output.write_all(b"\n")?;
            output.flush()?;
            output.sync_all()
        })
        .map_err(|error| format!("could not save the storage marker: {error}"))
}

fn new_cleanup_token() -> Result<String, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    let serial = MOVE_SERIAL.fetch_add(1, Ordering::Relaxed);
    let seed = format!("{}:{}:{serial}", std::process::id(), elapsed.as_nanos());
    let digest = Sha256::digest(seed.as_bytes());
    let mut token = String::with_capacity(64);
    for byte in digest {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        token.push(HEX[(byte >> 4) as usize] as char);
        token.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(token)
}

fn target_entries(root: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(root)
        .map_err(|error| format!("could not inspect the selected storage folder: {error}"))?
    {
        let entry = entry
            .map_err(|error| format!("could not inspect the selected storage folder: {error}"))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort_by_key(|name| name.to_ascii_lowercase());
    Ok(names)
}
fn reject_storage_name_variants(entries: &[String]) -> Result<(), String> {
    for name in entries {
        for required in [STORAGE_MARKER, MANAGED_DIRECTORY, CACHE_DIRECTORY] {
            if name.eq_ignore_ascii_case(required) && name != required {
                return Err(format!(
                    "storage entry must use the exact lowercase name {required}: {name}"
                ));
            }
        }
        let move_prefix = ".perfectsync-storage-move-";
        if name.to_ascii_lowercase().starts_with(move_prefix) && !name.starts_with(move_prefix) {
            return Err(format!(
                "storage move stage must use its exact lowercase name: {name}"
            ));
        }
    }
    Ok(())
}

pub fn validate_configured_root(
    configured: &str,
    default_root: &Path,
    app_data_root: &Path,
    game_sources: &[PathBuf],
) -> Result<PathBuf, String> {
    let requested = PathBuf::from(configured.trim());
    if !requested.is_absolute() || !requested.is_dir() {
        return Err("the configured storage folder is unavailable".into());
    }
    validate_regular_directory(&requested, "The configured storage folder")?;
    let target = fs::canonicalize(&requested)
        .map_err(|error| format!("could not open the configured storage folder: {error}"))?;
    validate_regular_directory(&target, "The configured storage folder")?;
    let default = canonical_or_original(default_root);
    let app_data = canonical_or_original(app_data_root);
    if paths_overlap(&target, &default) {
        return Err("the custom storage folder overlaps the local default".into());
    }
    if paths_overlap(&target, &app_data) {
        return Err(
            "managed storage must stay separate from Perfect Sync settings and profiles".into(),
        );
    }
    if game_sources
        .iter()
        .map(|source| canonical_or_original(source))
        .any(|source| paths_overlap(&target, &source))
    {
        return Err(
            "managed storage cannot contain an Among Us source or be placed inside one".into(),
        );
    }
    if read_marker(&target)?.is_none() {
        return Err("the configured folder is not owned by Perfect Sync".into());
    }
    retry_pending_storage_cleanup(&target, game_sources)?;
    let entries = target_entries(&target)?;
    reject_storage_name_variants(&entries)?;
    if entries
        .iter()
        .any(|name| name != STORAGE_MARKER && name != MANAGED_DIRECTORY && name != CACHE_DIRECTORY)
    {
        return Err("the configured Perfect Sync storage folder contains unexpected files".into());
    }
    validate_storage_payload(
        &target.join(MANAGED_DIRECTORY),
        &target.join(CACHE_DIRECTORY),
        game_sources,
    )?;
    Ok(target)
}

pub fn resolve_target(
    selected: Option<&str>,
    current_root: &Path,
    default_root: &Path,
    app_data_root: &Path,
    game_sources: &[PathBuf],
) -> Result<Option<StorageTarget>, String> {
    let requested = match selected {
        Some(value) if !value.trim().is_empty() => PathBuf::from(value.trim()),
        Some(_) => return Err("Choose a storage folder or restore the default location".into()),
        None => default_root.to_path_buf(),
    };
    if !requested.is_absolute() {
        return Err("The storage folder must be an absolute path".into());
    }
    if !requested.is_dir() {
        return Err(format!("Storage folder not found: {}", requested.display()));
    }
    validate_regular_directory(&requested, "The storage folder")?;
    let target = fs::canonicalize(&requested)
        .map_err(|error| format!("could not open the selected storage folder: {error}"))?;
    validate_regular_directory(&target, "The storage folder")?;

    let current = canonical_or_original(current_root);
    let default = canonical_or_original(default_root);
    let app_data = canonical_or_original(app_data_root);
    if same_path(&target, &current) {
        return Ok(None);
    }
    if paths_overlap(&target, &current) {
        return Err(
            "The new storage folder cannot contain the current storage folder or be inside it"
                .into(),
        );
    }
    if paths_overlap(&target, &app_data) {
        return Err(
            "Managed storage must stay separate from Perfect Sync settings and profiles".into(),
        );
    }
    if game_sources
        .iter()
        .map(|source| canonical_or_original(source))
        .any(|source| paths_overlap(&target, &source))
    {
        return Err(
            "Managed storage cannot contain an Among Us source or be placed inside one".into(),
        );
    }

    let target_marker = read_marker(&target)?;
    let marker_existed = target_marker.is_some();
    if let Some(pending) = target_marker.and_then(|marker| marker.pending_cleanup) {
        if pending.trees.iter().any(|tree| {
            fs::canonicalize(tree)
                .map(|tree| paths_overlap(&current, &tree))
                .unwrap_or(false)
        }) {
            return Err(
                "The selected storage folder has pending cleanup evidence for the active storage location"
                    .into(),
            );
        }
        retry_storage_cleanup(&target, game_sources, true)?;
    }
    let entries = target_entries(&target)?;
    let is_default = same_path(&target, &default);
    let target_cache = if is_default {
        app_data.join(CACHE_DIRECTORY)
    } else {
        target.join(CACHE_DIRECTORY)
    };
    if target_cache.exists() {
        return Err("The destination package cache already exists; choose another folder".into());
    }
    reject_storage_name_variants(&entries)?;
    let reserved_exists = entries.iter().any(|name| {
        name == MANAGED_DIRECTORY
            || name == CACHE_DIRECTORY
            || name
                .to_ascii_lowercase()
                .starts_with(".perfectsync-storage-move-")
    });
    if reserved_exists {
        return Err(
            "The selected folder already contains managed game data or an interrupted storage move"
                .into(),
        );
    }
    if !is_default {
        let unexpected = entries
            .iter()
            .filter(|name| name.as_str() != STORAGE_MARKER)
            .collect::<Vec<_>>();
        if !unexpected.is_empty() {
            return Err("Choose an empty folder reserved for Perfect Sync storage".into());
        }
        if marker_existed && entries.len() != 1 {
            return Err(
                "The selected Perfect Sync storage folder contains unexpected files".into(),
            );
        }
    }

    Ok(Some(StorageTarget {
        configured_path: (!is_default).then(|| target.to_string_lossy().into_owned()),
        cache_path: target_cache,
        root: target,
        marker_existed,
        protected_sources: game_sources
            .iter()
            .map(|source| canonical_or_original(source))
            .collect(),
    }))
}

fn safe_relative(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Ok(PathBuf::new());
    }
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Ok(path.to_path_buf())
    } else {
        Err("managed storage contains an invalid relative path".into())
    }
}

fn collect_tree(root: &Path) -> Result<Option<TreeManifest>, String> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not inspect {}: {error}", root.display())),
    };
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "managed storage path is not a regular directory: {}",
            root.display()
        ));
    }

    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut pending = VecDeque::from([(root.to_path_buf(), PathBuf::new())]);
    while let Some((directory, relative_root)) = pending.pop_front() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| format!("could not read {}: {error}", directory.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not read {}: {error}", directory.display()))?;
        entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
        for entry in entries {
            let path = entry.path();
            let relative = safe_relative(&relative_root.join(entry.file_name()))?;
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "managed storage contains a link or reparse point: {}",
                    path.display()
                ));
            }
            if metadata.is_dir() {
                directories.push(relative.clone());
                pending.push_back((path, relative));
            } else if metadata.is_file() {
                if !path_has_single_link(&path, &metadata) {
                    return Err(format!(
                        "managed storage contains a multiply-linked file: {}",
                        path.display()
                    ));
                }
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| invalid("managed storage size overflow"))?;
                if files.len() >= MAX_STORAGE_FILES || bytes > MAX_STORAGE_BYTES {
                    return Err("managed storage exceeds the safe relocation limit".into());
                }
                files.push(TreeFile {
                    relative,
                    size: metadata.len(),
                    sha256: sha256_file(&path, metadata.len(), &mut buffer)?,
                });
            } else {
                return Err(format!(
                    "managed storage contains a special file: {}",
                    path.display()
                ));
            }
        }
    }
    directories.sort();
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(Some(TreeManifest {
        directories,
        files,
        bytes,
    }))
}

fn validate_source_separation(
    path: &Path,
    protected_sources: &[PathBuf],
    label: &str,
) -> Result<PathBuf, String> {
    validate_regular_directory(path, label)?;
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("could not canonicalize {label} {}: {error}", path.display()))?;
    validate_regular_directory(&canonical, label)?;
    if protected_sources
        .iter()
        .map(|source| canonical_or_original(source))
        .any(|source| paths_overlap(&canonical, &source))
    {
        return Err(format!(
            "{label} overlaps a recorded Among Us source: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn checked_tree(
    path: &Path,
    protected_sources: &[PathBuf],
    label: &str,
) -> Result<Option<TreeManifest>, String> {
    if !path.exists() {
        return collect_tree(path);
    }
    validate_source_separation(path, protected_sources, label)?;
    collect_tree(path)
}

fn validate_workspace_instances(
    workspace_root: &Path,
    protected_sources: &[PathBuf],
) -> Result<(), String> {
    for profile in fs::read_dir(workspace_root)
        .map_err(|error| format!("could not inspect managed workspace profiles: {error}"))?
    {
        let profile = profile
            .map_err(|error| format!("could not inspect managed workspace profiles: {error}"))?;
        let metadata = fs::symlink_metadata(profile.path()).map_err(|error| error.to_string())?;
        if !metadata.is_dir() {
            continue;
        }
        checked_tree(
            &profile.path(),
            protected_sources,
            "A managed workspace profile directory",
        )?
        .ok_or_else(|| invalid("a managed workspace profile disappeared"))?;
        for instance in fs::read_dir(profile.path())
            .map_err(|error| format!("could not inspect managed workspace instances: {error}"))?
        {
            let instance = instance.map_err(|error| {
                format!("could not inspect managed workspace instances: {error}")
            })?;
            let metadata =
                fs::symlink_metadata(instance.path()).map_err(|error| error.to_string())?;
            if metadata.is_dir() {
                checked_tree(
                    &instance.path(),
                    protected_sources,
                    "A managed workspace instance or stage directory",
                )?
                .ok_or_else(|| invalid("a managed workspace instance disappeared"))?;
            }
        }
    }
    Ok(())
}

fn validate_managed_layout(
    managed_root: &Path,
    protected_sources: &[PathBuf],
) -> Result<(), String> {
    if checked_tree(
        managed_root,
        protected_sources,
        "The managed package directory",
    )?
    .is_none()
    {
        return Ok(());
    }
    for entry in fs::read_dir(managed_root)
        .map_err(|error| format!("could not inspect managed package storage: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("could not inspect managed package storage: {error}"))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| invalid("managed package storage contains a non-portable name"))?;
        if name == STORAGE_CLEANUP_MARKER {
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) || !metadata.is_file() {
                return Err("the managed package cleanup marker is invalid".into());
            }
            continue;
        }
        if ![
            SOURCE_RECORD_DIRECTORY,
            WORKSPACE_DIRECTORY,
            OBSOLETE_BASE_DIRECTORY,
        ]
        .contains(&name)
        {
            return Err(format!(
                "managed package storage contains an unexpected entry: {name}"
            ));
        }
        let label = if name == SOURCE_RECORD_DIRECTORY {
            "The managed source-record directory"
        } else if name == WORKSPACE_DIRECTORY {
            "The managed workspace and instance directory"
        } else {
            "The obsolete managed base directory"
        };
        checked_tree(&entry.path(), protected_sources, label)?
            .ok_or_else(|| invalid(format!("{label} disappeared during validation")))?;
        if name == WORKSPACE_DIRECTORY {
            validate_workspace_instances(&entry.path(), protected_sources)?;
        }
    }
    Ok(())
}

fn validate_storage_payload(
    managed_root: &Path,
    cache_root: &Path,
    protected_sources: &[PathBuf],
) -> Result<(), String> {
    validate_managed_layout(managed_root, protected_sources)?;
    checked_tree(
        cache_root,
        protected_sources,
        "The managed package cache directory",
    )?;
    Ok(())
}

fn create_cleanup_marker(root: &Path, token: &str) -> Result<(), String> {
    validate_regular_directory(root, "The storage cleanup directory")?;
    let marker = root.join(STORAGE_CLEANUP_MARKER);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
        .map_err(|error| format!("could not create a storage cleanup marker: {error}"))?;
    output
        .write_all(token.as_bytes())
        .and_then(|_| output.flush())
        .and_then(|_| output.sync_all())
        .map_err(|error| format!("could not finish a storage cleanup marker: {error}"))
}

fn restore_cleanup_marker(marker: &Path, token: &str) -> Result<(), String> {
    AtomicFile::new(marker, AllowOverwrite)
        .write(|output| {
            output.write_all(token.as_bytes())?;
            output.flush()?;
            output.sync_all()
        })
        .map_err(|error| format!("could not restore the storage cleanup marker: {error}"))
}

fn verified_cleanup_marker(root: &Path, token: &str) -> Result<PathBuf, String> {
    validate_regular_directory(root, "The storage cleanup directory")?;
    let marker = root.join(STORAGE_CLEANUP_MARKER);
    let metadata = fs::symlink_metadata(&marker)
        .map_err(|error| format!("could not inspect the storage cleanup marker: {error}"))?;
    if is_reparse(&metadata) || !metadata.is_file() || metadata.len() > 256 {
        return Err("the storage cleanup ownership marker is invalid".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(&marker)
        .map_err(|error| format!("could not read the storage cleanup marker: {error}"))?
        .take(257)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read the storage cleanup marker: {error}"))?;
    if bytes.len() as u64 != metadata.len() || bytes != token.as_bytes() {
        return Err("the storage cleanup ownership token changed".into());
    }
    Ok(marker)
}

fn disarm_cleanup_tree(root: &Path, token: &str) -> Result<(), String> {
    let marker = verified_cleanup_marker(root, token)?;
    fs::remove_file(marker)
        .map_err(|error| format!("could not disarm the storage cleanup marker: {error}"))
}

fn remove_owned_storage_tree_with<F>(
    root: &Path,
    token: &str,
    protected_sources: &[PathBuf],
    remove_root: F,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> io::Result<()>,
{
    let root = validate_source_separation(
        root,
        protected_sources,
        "The owned storage cleanup directory",
    )?;
    let root = root.as_path();
    let _marker = verified_cleanup_marker(root, token)?;
    let manifest = collect_tree(root)?
        .ok_or_else(|| invalid("the owned storage cleanup directory disappeared"))?;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    for file in manifest.files.iter().rev() {
        if file.relative == Path::new(STORAGE_CLEANUP_MARKER) {
            continue;
        }
        let path = root.join(&file.relative);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("could not revalidate owned storage file: {error}"))?;
        if is_reparse(&metadata)
            || !metadata.is_file()
            || !path_has_single_link(&path, &metadata)
            || metadata.len() != file.size
            || sha256_file(&path, file.size, &mut buffer)? != file.sha256
        {
            return Err(format!(
                "owned storage file changed before deletion: {}",
                path.display()
            ));
        }
        fs::remove_file(&path)
            .map_err(|error| format!("could not remove owned storage file: {error}"))?;
    }
    let mut directories = manifest.directories;
    directories.sort_by_key(|relative| std::cmp::Reverse(relative.components().count()));
    for directory in directories {
        let path = root.join(directory);
        validate_regular_directory(&path, "An owned storage directory")?;
        fs::remove_dir(&path)
            .map_err(|error| format!("could not remove owned storage directory: {error}"))?;
    }
    let marker = verified_cleanup_marker(root, token)?;
    fs::remove_file(&marker)
        .map_err(|error| format!("could not remove the storage cleanup marker: {error}"))?;
    validate_regular_directory(root, "The emptied owned storage directory")?;
    if fs::read_dir(root)
        .map_err(|error| format!("could not revalidate emptied owned storage: {error}"))?
        .next()
        .is_some()
    {
        restore_cleanup_marker(&marker, token)?;
        return Err("owned storage changed before final directory removal".into());
    }
    match remove_root(root) {
        Ok(()) => Ok(()),
        Err(error) => {
            let restore = restore_cleanup_marker(&marker, token);
            Err(match restore {
                Ok(()) => format!("could not remove owned storage directory: {error}"),
                Err(restore) => {
                    format!("could not remove owned storage directory: {error}; {restore}")
                }
            })
        }
    }
}

fn remove_owned_storage_tree(
    root: &Path,
    token: &str,
    protected_sources: &[PathBuf],
) -> Result<(), String> {
    remove_owned_storage_tree_with(root, token, protected_sources, |path| fs::remove_dir(path))
}

fn confirm_missing_cleanup_tree(path: &Path, protected_sources: &[PathBuf]) -> Result<(), String> {
    let mut ancestor = path
        .parent()
        .ok_or_else(|| invalid("pending storage cleanup path has no parent"))?;
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => {
                validate_source_separation(
                    ancestor,
                    protected_sources,
                    "The storage cleanup parent",
                )?;
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| invalid("pending storage cleanup has no available ancestor"))?;
            }
            Err(error) => {
                return Err(format!(
                    "could not inspect the storage cleanup parent: {error}"
                ))
            }
        }
    }
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err("the pending storage cleanup directory reappeared".into()),
        Err(error) => Err(format!(
            "could not confirm pending storage cleanup completion: {error}"
        )),
    }
}

fn sha256_file(path: &Path, expected_size: u64, buffer: &mut [u8]) -> Result<[u8; 32], String> {
    let mut input = File::open(path)
        .map_err(|error| format!("could not verify {}: {error}", path.display()))?;
    let metadata = input
        .metadata()
        .map_err(|error| format!("could not verify {}: {error}", path.display()))?;
    if !metadata.is_file() || !has_single_link(&input, &metadata) || metadata.len() != expected_size
    {
        return Err(format!(
            "file changed during storage relocation: {}",
            path.display()
        ));
    }
    let mut hasher = Sha256::new();
    let mut read = 0_u64;
    loop {
        let count = input
            .read(buffer)
            .map_err(|error| format!("could not verify {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        read = read
            .checked_add(count as u64)
            .ok_or_else(|| invalid("storage file size overflow"))?;
        hasher.update(&buffer[..count]);
    }
    if read != expected_size {
        return Err(format!(
            "file changed during storage relocation: {}",
            path.display()
        ));
    }
    Ok(hasher.finalize().into())
}

fn copy_file_verified(
    source: &Path,
    destination: &Path,
    expected_size: u64,
    buffer: &mut [u8],
) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("could not inspect {}: {error}", source.display()))?;
    if is_reparse(&source_metadata)
        || !source_metadata.is_file()
        || !path_has_single_link(source, &source_metadata)
        || source_metadata.len() != expected_size
    {
        return Err(format!(
            "file changed during storage relocation: {}",
            source.display()
        ));
    }
    let expected_digest = sha256_file(source, expected_size, buffer)?;

    let mut input = File::open(source)
        .map_err(|error| format!("could not open {}: {error}", source.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("could not create {}: {error}", destination.display()))?;
    let mut copied = 0_u64;
    let mut copied_hasher = Sha256::new();
    loop {
        let count = input
            .read(buffer)
            .map_err(|error| format!("could not read {}: {error}", source.display()))?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(count as u64)
            .ok_or_else(|| invalid("storage file size overflow"))?;
        if copied > expected_size {
            return Err(format!(
                "file changed during storage relocation: {}",
                source.display()
            ));
        }
        copied_hasher.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(|error| format!("could not write {}: {error}", destination.display()))?;
    }
    output
        .flush()
        .and_then(|_| output.sync_all())
        .map_err(|error| format!("could not finish {}: {error}", destination.display()))?;
    if copied != expected_size || <[u8; 32]>::from(copied_hasher.finalize()) != expected_digest {
        return Err(format!(
            "file changed during storage relocation: {}",
            source.display()
        ));
    }
    let destination_digest = sha256_file(destination, expected_size, buffer)?;
    if destination_digest != expected_digest {
        return Err(format!(
            "copied storage file failed verification: {}",
            destination.display()
        ));
    }
    Ok(())
}

struct VerifiedStorageCopy<'a, F> {
    token: &'a str,
    protected_sources: &'a [PathBuf],
    copied: &'a mut u64,
    total: u64,
    buffer: &'a mut [u8],
    progress: &'a mut F,
}

fn copy_tree_verified<F>(
    source: &Path,
    destination: &Path,
    manifest: &TreeManifest,
    copy: &mut VerifiedStorageCopy<'_, F>,
) -> Result<(), String>
where
    F: FnMut(u64, u64, &str),
{
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("storage staging directory has no parent"))?;
    validate_source_separation(parent, copy.protected_sources, "The storage staging parent")?;
    fs::create_dir(destination)
        .map_err(|error| format!("could not create storage staging directory: {error}"))?;
    validate_source_separation(
        destination,
        copy.protected_sources,
        "The storage staging directory",
    )?;
    create_cleanup_marker(destination, copy.token)?;
    for relative in &manifest.directories {
        fs::create_dir(destination.join(relative))
            .map_err(|error| format!("could not create storage directory: {error}"))?;
    }
    for file in &manifest.files {
        let source_file = source.join(&file.relative);
        let destination_file = destination.join(&file.relative);
        let message = format!("Copying {}", file.relative.display());
        (copy.progress)(*copy.copied, copy.total, &message);
        copy_file_verified(&source_file, &destination_file, file.size, copy.buffer)?;
        *copy.copied = copy
            .copied
            .checked_add(file.size)
            .ok_or_else(|| invalid("storage size overflow"))?;
        (copy.progress)(*copy.copied, copy.total, &message);
    }
    let after = collect_tree(source)?
        .ok_or_else(|| invalid("managed storage disappeared during relocation"))?;
    if &after != manifest {
        return Err(format!(
            "managed storage changed during relocation: {}",
            source.display()
        ));
    }
    let mut copied_manifest = collect_tree(destination)?
        .ok_or_else(|| invalid("copied storage disappeared during verification"))?;
    let marker_position = copied_manifest
        .files
        .iter()
        .position(|file| file.relative == Path::new(STORAGE_CLEANUP_MARKER))
        .ok_or_else(|| invalid("copied storage lost its cleanup ownership marker"))?;
    let marker_file = copied_manifest.files.remove(marker_position);
    copied_manifest.bytes = copied_manifest
        .bytes
        .checked_sub(marker_file.size)
        .ok_or_else(|| invalid("copied storage size underflow"))?;
    if &copied_manifest != manifest {
        return Err(format!(
            "copied storage tree failed verification: {}",
            destination.display()
        ));
    }
    Ok(())
}

fn marker_with_cleanup(pending_cleanup: Option<PendingStorageCleanup>) -> StorageMarker {
    StorageMarker {
        schema: STORAGE_SCHEMA,
        pending_cleanup,
    }
}

fn cleanup_tree_paths(
    managed_root: &Path,
    cache_root: &Path,
    protected_sources: &[PathBuf],
) -> Result<Vec<PathBuf>, String> {
    validate_storage_payload(managed_root, cache_root, protected_sources)?;
    let mut trees = Vec::new();
    if managed_root.exists() {
        for name in [
            SOURCE_RECORD_DIRECTORY,
            WORKSPACE_DIRECTORY,
            OBSOLETE_BASE_DIRECTORY,
        ] {
            let path = managed_root.join(name);
            if path.exists() {
                trees.push(validate_source_separation(
                    &path,
                    protected_sources,
                    "A managed storage subtree",
                )?);
            }
        }
        trees.push(validate_source_separation(
            managed_root,
            protected_sources,
            "The managed package directory",
        )?);
    }
    if cache_root.exists() {
        trees.push(validate_source_separation(
            cache_root,
            protected_sources,
            "The managed package cache directory",
        )?);
    }
    Ok(trees)
}

fn pending_cleanup(token: &str, trees: &[PathBuf]) -> PendingStorageCleanup {
    PendingStorageCleanup {
        token: token.to_string(),
        trees: trees
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    }
}

fn arm_cleanup_trees(trees: &[PathBuf], token: &str) -> Result<(), String> {
    let mut armed: Vec<&Path> = Vec::new();
    for tree in trees {
        armed.push(tree.as_path());
        if let Err(error) = create_cleanup_marker(tree, token) {
            let mut errors = vec![error];
            for armed_tree in armed.iter().rev() {
                match fs::symlink_metadata(armed_tree.join(STORAGE_CLEANUP_MARKER)) {
                    Ok(_) => {
                        if let Err(disarm) = disarm_cleanup_tree(armed_tree, token) {
                            errors.push(format!(
                                "partial cleanup marker could not be disarmed at {}: {disarm}",
                                armed_tree.display()
                            ));
                        }
                    }
                    Err(disarm) if disarm.kind() == io::ErrorKind::NotFound => {}
                    Err(disarm) => errors.push(format!(
                        "partial cleanup marker could not be inspected at {}: {disarm}",
                        armed_tree.display()
                    )),
                }
            }
            return Err(errors.join("; "));
        }
    }
    Ok(())
}

fn clear_pending_cleanup(storage_root: &Path) -> Result<(), String> {
    let mut marker = read_marker(storage_root)?
        .ok_or_else(|| invalid("the storage cleanup evidence marker disappeared"))?;
    marker.pending_cleanup = None;
    write_marker_state(storage_root, &marker)
}
fn finalize_empty_owned_tree(path: &Path, protected_sources: &[PathBuf]) -> Result<(), String> {
    let path = validate_source_separation(
        path,
        protected_sources,
        "The stranded empty storage cleanup directory",
    )?;
    if fs::read_dir(&path)
        .map_err(|error| format!("could not inspect stranded storage cleanup: {error}"))?
        .next()
        .is_some()
    {
        return Err(
            "pending storage cleanup has no ownership marker and is not an empty directory".into(),
        );
    }
    validate_regular_directory(&path, "The stranded empty storage cleanup directory")?;
    fs::remove_dir(&path)
        .map_err(|error| format!("could not finalize stranded storage cleanup: {error}"))
}

fn remove_pending_trees(
    pending: &PendingStorageCleanup,
    protected_sources: &[PathBuf],
) -> Result<(), String> {
    for tree in &pending.trees {
        let path = Path::new(tree);
        match fs::symlink_metadata(path) {
            Ok(_) => match fs::symlink_metadata(path.join(STORAGE_CLEANUP_MARKER)) {
                Ok(_) => {
                    remove_owned_storage_tree(path, &pending.token, protected_sources)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    finalize_empty_owned_tree(path, protected_sources)?;
                }
                Err(error) => {
                    return Err(format!(
                        "could not inspect owned storage cleanup marker: {error}"
                    ))
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                confirm_missing_cleanup_tree(path, protected_sources)?;
            }
            Err(error) => {
                return Err(format!(
                    "could not inspect owned storage cleanup path: {error}"
                ))
            }
        }
    }
    Ok(())
}

fn retry_storage_cleanup(
    storage_root: &Path,
    protected_sources: &[PathBuf],
    allow_storage_overlap: bool,
) -> Result<bool, String> {
    let Some(marker) = read_marker(storage_root)? else {
        return Ok(false);
    };
    let Some(pending) = marker.pending_cleanup else {
        return Ok(false);
    };
    if !allow_storage_overlap {
        let active = fs::canonicalize(storage_root)
            .map_err(|error| format!("could not canonicalize active storage: {error}"))?;
        for tree in &pending.trees {
            if let Ok(canonical) = fs::canonicalize(tree) {
                if paths_overlap(&active, &canonical) {
                    return Err(
                        "pending storage cleanup overlaps the active storage location".into(),
                    );
                }
            }
        }
    }
    remove_pending_trees(&pending, protected_sources)?;
    clear_pending_cleanup(storage_root)?;
    Ok(true)
}

pub fn retry_pending_storage_cleanup(
    storage_root: &Path,
    protected_sources: &[PathBuf],
) -> Result<bool, String> {
    retry_storage_cleanup(storage_root, protected_sources, false)
}

pub fn copy_payload<F>(
    current_root: &Path,
    current_cache: &Path,
    target: &StorageTarget,
    mut progress: F,
) -> Result<PublishedStorage, String>
where
    F: FnMut(u64, u64, &str),
{
    let managed_source = current_root.join(MANAGED_DIRECTORY);
    validate_storage_payload(&managed_source, current_cache, &target.protected_sources)?;
    let source_records = checked_tree(
        &managed_source.join(SOURCE_RECORD_DIRECTORY),
        &target.protected_sources,
        "The managed source-record directory",
    )?;
    let workspaces = checked_tree(
        &managed_source.join(WORKSPACE_DIRECTORY),
        &target.protected_sources,
        "The managed workspace and instance directory",
    )?;
    checked_tree(
        &managed_source.join(OBSOLETE_BASE_DIRECTORY),
        &target.protected_sources,
        "The obsolete managed base directory",
    )?;
    let cache_manifest = checked_tree(
        current_cache,
        &target.protected_sources,
        "The managed package cache directory",
    )?;
    let total = source_records
        .as_ref()
        .map_or(0, |manifest| manifest.bytes)
        .checked_add(workspaces.as_ref().map_or(0, |manifest| manifest.bytes))
        .and_then(|bytes| {
            bytes.checked_add(cache_manifest.as_ref().map_or(0, |manifest| manifest.bytes))
        })
        .ok_or_else(|| invalid("managed storage size overflow"))?;
    let total_files = source_records
        .as_ref()
        .map_or(0, |manifest| manifest.files.len())
        .checked_add(
            workspaces
                .as_ref()
                .map_or(0, |manifest| manifest.files.len()),
        )
        .and_then(|files| {
            files.checked_add(
                cache_manifest
                    .as_ref()
                    .map_or(0, |manifest| manifest.files.len()),
            )
        })
        .ok_or_else(|| invalid("managed storage file count overflow"))?;
    if total > MAX_STORAGE_BYTES || total_files > MAX_STORAGE_FILES {
        return Err("managed storage exceeds the safe relocation limit".into());
    }
    progress(0, total, "Preparing verified storage copy");

    let token = new_cleanup_token()?;
    let serial = MOVE_SERIAL.fetch_add(1, Ordering::Relaxed);
    let prefix = format!(".perfectsync-storage-move-{}-{serial}", std::process::id());
    let managed_stage = target.root.join(format!("{prefix}-managed"));
    let cache_parent = target
        .cache_path
        .parent()
        .ok_or_else(|| invalid("destination package cache has no parent"))?;
    validate_source_separation(
        cache_parent,
        &target.protected_sources,
        "The package cache staging parent",
    )?;
    let cache_stage = cache_parent.join(format!("{prefix}-cache"));
    let managed_final = target.root.join(MANAGED_DIRECTORY);
    let cache_final = target.cache_path.clone();
    for path in [&managed_stage, &cache_stage, &managed_final, &cache_final] {
        if path.exists() {
            return Err(format!(
                "storage relocation destination already exists: {}",
                path.display()
            ));
        }
    }

    let mut rollback_trees = Vec::new();
    if source_records.is_some() || workspaces.is_some() {
        if source_records.is_some() {
            rollback_trees.push(managed_stage.join(SOURCE_RECORD_DIRECTORY));
        }
        if workspaces.is_some() {
            rollback_trees.push(managed_stage.join(WORKSPACE_DIRECTORY));
        }
        rollback_trees.push(managed_stage.clone());
    }
    if cache_manifest.is_some() {
        rollback_trees.push(cache_stage.clone());
    }
    if source_records.is_some() || workspaces.is_some() {
        if source_records.is_some() {
            rollback_trees.push(managed_final.join(SOURCE_RECORD_DIRECTORY));
        }
        if workspaces.is_some() {
            rollback_trees.push(managed_final.join(WORKSPACE_DIRECTORY));
        }
        rollback_trees.push(managed_final.clone());
    }
    if cache_manifest.is_some() {
        rollback_trees.push(cache_final.clone());
    }
    let rollback_cleanup = pending_cleanup(&token, &rollback_trees);
    write_marker_state(
        &target.root,
        &marker_with_cleanup(Some(rollback_cleanup.clone())),
    )?;

    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let result = (|| {
        let mut copy = VerifiedStorageCopy {
            token: &token,
            protected_sources: &target.protected_sources,
            copied: &mut copied,
            total,
            buffer: &mut buffer,
            progress: &mut progress,
        };
        if source_records.is_some() || workspaces.is_some() {
            fs::create_dir(&managed_stage)
                .map_err(|error| format!("could not create managed storage stage: {error}"))?;
            validate_source_separation(
                &managed_stage,
                &target.protected_sources,
                "The managed storage stage",
            )?;
            create_cleanup_marker(&managed_stage, &token)?;
            if let Some(manifest) = &source_records {
                copy_tree_verified(
                    &managed_source.join(SOURCE_RECORD_DIRECTORY),
                    &managed_stage.join(SOURCE_RECORD_DIRECTORY),
                    manifest,
                    &mut copy,
                )?;
            }
            if let Some(manifest) = &workspaces {
                copy_tree_verified(
                    &managed_source.join(WORKSPACE_DIRECTORY),
                    &managed_stage.join(WORKSPACE_DIRECTORY),
                    manifest,
                    &mut copy,
                )?;
            }
            validate_managed_layout(&managed_stage, &target.protected_sources)?;
            validate_source_separation(
                &target.root,
                &target.protected_sources,
                "The managed storage publish parent",
            )?;
            fs::rename(&managed_stage, &managed_final)
                .map_err(|error| format!("could not publish managed game storage: {error}"))?;
            validate_managed_layout(&managed_final, &target.protected_sources)?;
        }
        if let Some(manifest) = &cache_manifest {
            copy_tree_verified(current_cache, &cache_stage, manifest, &mut copy)?;
            validate_source_separation(
                cache_parent,
                &target.protected_sources,
                "The package cache publish parent",
            )?;
            fs::rename(&cache_stage, &cache_final)
                .map_err(|error| format!("could not publish package cache storage: {error}"))?;
            checked_tree(
                &cache_final,
                &target.protected_sources,
                "The published package cache directory",
            )?;
        }
        (copy.progress)(
            copy.total,
            copy.total,
            "Verified the relocated storage copy",
        );
        Ok(PublishedStorage {
            root: target.root.clone(),
            marker_created: !target.marker_existed,
            protected_sources: target.protected_sources.clone(),
            rollback_cleanup: rollback_cleanup.clone(),
            old_cleanup: None,
        })
    })();
    match result {
        Ok(published) => Ok(published),
        Err(error) => {
            let cleanup = remove_pending_trees(&rollback_cleanup, &target.protected_sources)
                .and_then(|_| clear_pending_cleanup(&target.root))
                .err();
            Err(match cleanup {
                Some(cleanup) => {
                    format!("{error}; automatic cleanup remains pending: {cleanup}")
                }
                None => error,
            })
        }
    }
}

fn disarm_pending_markers(
    pending: &PendingStorageCleanup,
    protected_sources: &[PathBuf],
) -> Vec<String> {
    let mut errors = Vec::new();
    for tree in &pending.trees {
        let path = Path::new(tree);
        match fs::symlink_metadata(path.join(STORAGE_CLEANUP_MARKER)) {
            Ok(_) => {
                if let Err(error) = validate_source_separation(
                    path,
                    protected_sources,
                    "A disarmed storage cleanup directory",
                )
                .and_then(|path| disarm_cleanup_tree(&path, &pending.token))
                {
                    errors.push(error);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => errors.push(error.to_string()),
        }
    }
    errors
}
fn ensure_pending_markers(
    pending: &PendingStorageCleanup,
    protected_sources: &[PathBuf],
) -> Result<(), String> {
    for tree in &pending.trees {
        let path = Path::new(tree);
        if !path.exists() {
            continue;
        }
        let path = validate_source_separation(
            path,
            protected_sources,
            "A rollback storage cleanup directory",
        )?;
        let marker = path.join(STORAGE_CLEANUP_MARKER);
        match fs::symlink_metadata(&marker) {
            Ok(_) => {
                verified_cleanup_marker(&path, &pending.token)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_cleanup_marker(&path, &pending.token)?;
            }
            Err(error) => {
                return Err(format!(
                    "could not inspect a storage cleanup marker: {error}"
                ))
            }
        }
    }
    Ok(())
}

fn prepare_published_cleanup_with<F>(
    published: &mut PublishedStorage,
    current_root: &Path,
    current_cache: &Path,
    mut write_state: F,
) -> Result<(), String>
where
    F: FnMut(&Path, &StorageMarker) -> Result<(), String>,
{
    let old_trees = cleanup_tree_paths(
        &current_root.join(MANAGED_DIRECTORY),
        current_cache,
        &published.protected_sources,
    )?;
    if old_trees.is_empty() {
        write_state(&published.root, &marker_with_cleanup(None))?;
        let errors =
            disarm_pending_markers(&published.rollback_cleanup, &published.protected_sources);
        if errors.is_empty() {
            return Ok(());
        }
        return Err(errors.join("; "));
    }
    arm_cleanup_trees(&old_trees, &published.rollback_cleanup.token)?;
    let pending = pending_cleanup(&published.rollback_cleanup.token, &old_trees);
    if let Err(error) = write_state(&published.root, &marker_with_cleanup(Some(pending.clone()))) {
        let mut errors = vec![error];
        errors.extend(disarm_pending_markers(
            &pending,
            &published.protected_sources,
        ));
        return Err(errors.join("; "));
    }
    published.old_cleanup = Some(pending);
    let errors = disarm_pending_markers(&published.rollback_cleanup, &published.protected_sources);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn prepare_published_cleanup(
    published: &mut PublishedStorage,
    current_root: &Path,
    current_cache: &Path,
) -> Result<(), String> {
    prepare_published_cleanup_with(published, current_root, current_cache, write_marker_state)
}

pub fn rollback_published(published: &PublishedStorage) -> Result<(), String> {
    let mut errors = published
        .old_cleanup
        .as_ref()
        .map(|pending| disarm_pending_markers(pending, &published.protected_sources))
        .unwrap_or_default();
    if let Err(error) =
        ensure_pending_markers(&published.rollback_cleanup, &published.protected_sources)
    {
        let disarm = disarm_published_cleanup(published).err();
        return Err(match disarm {
            Some(disarm) => {
                format!("{error}; automatic cleanup could not be disabled: {disarm}")
            }
            None => error,
        });
    }
    if let Err(error) = write_marker_state(
        &published.root,
        &marker_with_cleanup(Some(published.rollback_cleanup.clone())),
    ) {
        let disarm = disarm_published_cleanup(published).err();
        return Err(match disarm {
            Some(disarm) => format!("{error}; automatic cleanup could not be disabled: {disarm}"),
            None => error,
        });
    }
    if let Err(error) =
        remove_pending_trees(&published.rollback_cleanup, &published.protected_sources)
    {
        errors.push(error);
    } else if let Err(error) = clear_pending_cleanup(&published.root) {
        errors.push(error);
    } else if published.marker_created {
        match fs::remove_file(published.root.join(STORAGE_MARKER)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => errors.push(format!("could not remove the storage marker: {error}")),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn disarm_published_cleanup_with<F>(
    published: &PublishedStorage,
    clear_evidence: F,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let mut errors = published
        .old_cleanup
        .as_ref()
        .map(|pending| disarm_pending_markers(pending, &published.protected_sources))
        .unwrap_or_default();
    errors.extend(disarm_pending_markers(
        &published.rollback_cleanup,
        &published.protected_sources,
    ));
    if let Err(error) = clear_evidence(&published.root) {
        errors.push(format!("cleanup evidence could not be cleared: {error}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn disarm_published_cleanup(published: &PublishedStorage) -> Result<(), String> {
    disarm_published_cleanup_with(published, clear_pending_cleanup)
}

pub fn commit_published_and_cleanup(
    published: &mut PublishedStorage,
    current_root: &Path,
    current_cache: &Path,
) -> Vec<String> {
    if published.old_cleanup.is_none() {
        if let Err(error) = prepare_published_cleanup(published, current_root, current_cache) {
            return vec![error];
        }
    }
    let mut errors = Vec::new();
    if published.old_cleanup.is_some() {
        if let Err(error) =
            retry_pending_storage_cleanup(&published.root, &published.protected_sources)
        {
            errors.push(error);
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(root: &Path) -> StorageTarget {
        StorageTarget {
            root: root.to_path_buf(),
            configured_path: Some(root.to_string_lossy().into_owned()),
            cache_path: root.join(CACHE_DIRECTORY),
            marker_existed: false,
            protected_sources: Vec::new(),
        }
    }

    fn create_direct_source_payload(current: &Path, cache: &Path) {
        fs::create_dir_all(current.join("managed-games/sources")).unwrap();
        fs::create_dir_all(current.join("managed-games/workspace/profile/current/Empty")).unwrap();
        fs::create_dir_all(current.join("managed-games/bases/obsolete/game")).unwrap();
        fs::create_dir_all(cache).unwrap();
        fs::write(
            current.join("managed-games/sources/instance.json"),
            b"source record",
        )
        .unwrap();
        fs::write(
            current.join("managed-games/workspace/profile/current/profile.bin"),
            b"profile",
        )
        .unwrap();
        fs::write(
            current.join("managed-games/bases/obsolete/game/Among Us.exe"),
            b"obsolete",
        )
        .unwrap();
        fs::write(cache.join("package.zip"), b"archive").unwrap();
    }
    #[test]
    fn same_size_source_mutation_is_detected_after_the_full_copy() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current");
        let cache = temp.path().join("cache");
        let target_root = temp.path().join("target");
        let source_record = current.join("managed-games/sources/instance.json");
        fs::create_dir_all(source_record.parent().unwrap()).unwrap();
        fs::create_dir(&cache).unwrap();
        fs::create_dir(&target_root).unwrap();
        fs::write(&source_record, b"aaaa").unwrap();
        let target = target(&target_root);
        let mut mutated = false;

        let error = copy_payload(&current, &cache, &target, |_, _, _| {
            if !mutated {
                fs::write(&source_record, b"bbbb").unwrap();
                mutated = true;
            }
        })
        .unwrap_err();

        assert!(error.contains("changed during relocation"), "{error}");
    }

    #[test]
    fn multiply_linked_managed_files_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let sources = temp.path().join("managed-games/sources");
        fs::create_dir_all(&sources).unwrap();
        fs::write(sources.join("first.json"), b"record").unwrap();
        fs::hard_link(sources.join("first.json"), sources.join("second.json")).unwrap();

        assert!(collect_tree(&sources)
            .unwrap_err()
            .contains("multiply-linked"));
    }

    #[test]
    fn partial_cleanup_arming_surfaces_every_disarm_failure() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(second.join(STORAGE_CLEANUP_MARKER), "b".repeat(64)).unwrap();

        let error =
            arm_cleanup_trees(&[first.clone(), second.clone()], &"a".repeat(64)).unwrap_err();

        assert!(error.contains("partial cleanup marker could not be disarmed"));
        assert!(!first.join(STORAGE_CLEANUP_MARKER).exists());
        assert_eq!(
            fs::read(second.join(STORAGE_CLEANUP_MARKER)).unwrap(),
            "b".repeat(64).as_bytes()
        );
    }

    #[test]
    fn prepare_cleanup_surfaces_partial_disarm_failures() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current");
        let cache = temp.path().join("old-cache");
        let target_root = temp.path().join("target");
        create_direct_source_payload(&current, &cache);
        fs::create_dir(&target_root).unwrap();
        let target = target(&target_root);
        let mut published = copy_payload(&current, &cache, &target, |_, _, _| {}).unwrap();
        let replaced_marker = current
            .join("managed-games/sources")
            .join(STORAGE_CLEANUP_MARKER);

        let error = prepare_published_cleanup_with(&mut published, &current, &cache, |_, _| {
            fs::write(&replaced_marker, "b".repeat(64)).unwrap();
            Err("storage evidence write failed".into())
        })
        .unwrap_err();

        assert!(error.contains("storage evidence write failed"));
        assert!(error.contains("ownership token changed"));
        assert_eq!(
            fs::read(&replaced_marker).unwrap(),
            "b".repeat(64).as_bytes()
        );
        for path in [
            current.join("managed-games/workspace"),
            current.join("managed-games/bases"),
            current.join(MANAGED_DIRECTORY),
            cache.clone(),
        ] {
            assert!(!path.join(STORAGE_CLEANUP_MARKER).exists());
        }
    }

    #[test]
    fn top_level_storage_names_require_exact_lowercase_spelling() {
        let temp = tempfile::tempdir().unwrap();
        let managed = temp.path().join("managed-games");
        let cache = temp.path().join("cache");
        fs::create_dir_all(managed.join("Sources")).unwrap();
        fs::create_dir(&cache).unwrap();
        assert!(validate_storage_payload(&managed, &cache, &[])
            .unwrap_err()
            .contains("unexpected entry"));

        let current = temp.path().join("current");
        let default = temp.path().join("default");
        let app_data = temp.path().join("app-data");
        let target_root = temp.path().join("target");
        for path in [&current, &default, &app_data, &target_root] {
            fs::create_dir_all(path).unwrap();
        }
        fs::create_dir(target_root.join("Managed-Games")).unwrap();
        assert!(resolve_target(
            Some(target_root.to_str().unwrap()),
            &current,
            &default,
            &app_data,
            &[],
        )
        .unwrap_err()
        .contains("exact lowercase"));
    }

    #[test]
    fn pending_evidence_finalizes_only_an_empty_stranded_directory() {
        let temp = tempfile::tempdir().unwrap();
        let target_root = temp.path().join("target");
        let stranded = temp.path().join("stranded");
        fs::create_dir(&target_root).unwrap();
        fs::create_dir(&stranded).unwrap();
        write_marker_state(
            &target_root,
            &marker_with_cleanup(Some(PendingStorageCleanup {
                token: "a".repeat(64),
                trees: vec![stranded.to_string_lossy().into_owned()],
            })),
        )
        .unwrap();

        assert!(retry_pending_storage_cleanup(&target_root, &[]).unwrap());
        assert!(!stranded.exists());

        fs::create_dir(&stranded).unwrap();
        fs::write(stranded.join("payload.bin"), b"payload").unwrap();
        write_marker_state(
            &target_root,
            &marker_with_cleanup(Some(PendingStorageCleanup {
                token: "a".repeat(64),
                trees: vec![stranded.to_string_lossy().into_owned()],
            })),
        )
        .unwrap();
        assert!(retry_pending_storage_cleanup(&target_root, &[]).is_err());
        assert_eq!(fs::read(stranded.join("payload.bin")).unwrap(), b"payload");
    }

    #[test]
    fn verified_copy_moves_direct_source_payload_and_omits_obsolete_bases() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current");
        let cache = temp.path().join("old-cache");
        let target_root = temp.path().join("target");
        create_direct_source_payload(&current, &cache);
        fs::create_dir(&target_root).unwrap();
        let target = target(&target_root);
        let mut last = (0, 0);

        let published = copy_payload(&current, &cache, &target, |copied, total, _| {
            last = (copied, total)
        })
        .unwrap();

        assert_eq!(
            fs::read(target_root.join("managed-games/sources/instance.json")).unwrap(),
            b"source record"
        );
        assert_eq!(
            fs::read(target_root.join("managed-games/workspace/profile/current/profile.bin"))
                .unwrap(),
            b"profile"
        );
        assert_eq!(
            fs::read(target_root.join("cache/package.zip")).unwrap(),
            b"archive"
        );
        assert!(!target_root.join("managed-games/bases").exists());
        assert!(target_root
            .join("managed-games/workspace/profile/current/Empty")
            .is_dir());
        assert_eq!(last.0, last.1);

        rollback_published(&published).unwrap();
        assert!(!target_root.join(MANAGED_DIRECTORY).exists());
        assert!(!target_root.join(CACHE_DIRECTORY).exists());
        assert!(!target_root.join(STORAGE_MARKER).exists());
    }

    #[test]
    fn committed_move_cleans_every_old_owned_subtree_and_disarms_the_copy() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current");
        let cache = temp.path().join("old-cache");
        let target_root = temp.path().join("target");
        create_direct_source_payload(&current, &cache);
        fs::create_dir(&target_root).unwrap();
        let target = target(&target_root);
        let mut published = copy_payload(&current, &cache, &target, |_, _, _| {}).unwrap();
        prepare_published_cleanup(&mut published, &current, &cache).unwrap();
        assert!(read_marker(&target_root)
            .unwrap()
            .unwrap()
            .pending_cleanup
            .is_some());
        for path in [
            current.join("managed-games/sources"),
            current.join("managed-games/workspace"),
            current.join("managed-games/bases"),
            current.join(MANAGED_DIRECTORY),
            cache.clone(),
        ] {
            assert!(path.join(STORAGE_CLEANUP_MARKER).is_file());
        }
        for path in [
            target_root.join("managed-games/sources"),
            target_root.join("managed-games/workspace"),
            target_root.join(MANAGED_DIRECTORY),
            target_root.join(CACHE_DIRECTORY),
        ] {
            assert!(!path.join(STORAGE_CLEANUP_MARKER).exists());
        }

        assert!(commit_published_and_cleanup(&mut published, &current, &cache).is_empty());

        assert!(!current.join(MANAGED_DIRECTORY).exists());
        assert!(!cache.exists());
        for path in [
            target_root.join(MANAGED_DIRECTORY),
            target_root.join("managed-games/sources"),
            target_root.join("managed-games/workspace"),
            target_root.join(CACHE_DIRECTORY),
        ] {
            assert!(!path.join(STORAGE_CLEANUP_MARKER).exists());
        }
        assert!(read_marker(&target_root)
            .unwrap()
            .unwrap()
            .pending_cleanup
            .is_none());
    }

    #[test]
    fn cleanup_requires_the_exact_persisted_ownership_token() {
        let temp = tempfile::tempdir().unwrap();
        let target_root = temp.path().join("target");
        let old = temp.path().join("old");
        fs::create_dir(&target_root).unwrap();
        fs::create_dir(&old).unwrap();
        fs::write(old.join("keep.bin"), b"keep").unwrap();
        create_cleanup_marker(&old, &"a".repeat(64)).unwrap();
        write_marker_state(
            &target_root,
            &marker_with_cleanup(Some(PendingStorageCleanup {
                token: "a".repeat(64),
                trees: vec![old.to_string_lossy().into_owned()],
            })),
        )
        .unwrap();
        fs::write(old.join(STORAGE_CLEANUP_MARKER), "b".repeat(64)).unwrap();

        assert!(retry_pending_storage_cleanup(&target_root, &[]).is_err());
        assert_eq!(fs::read(old.join("keep.bin")).unwrap(), b"keep");
        assert!(read_marker(&target_root)
            .unwrap()
            .unwrap()
            .pending_cleanup
            .is_some());
    }

    #[test]
    fn cleanup_evidence_retries_after_state_reload() {
        let temp = tempfile::tempdir().unwrap();
        let target_root = temp.path().join("target");
        let old = temp.path().join("old");
        let token = "a".repeat(64);
        fs::create_dir(&target_root).unwrap();
        fs::create_dir(&old).unwrap();
        fs::write(old.join("old.bin"), b"old").unwrap();
        create_cleanup_marker(&old, &token).unwrap();
        write_marker_state(
            &target_root,
            &marker_with_cleanup(Some(PendingStorageCleanup {
                token,
                trees: vec![old.to_string_lossy().into_owned()],
            })),
        )
        .unwrap();

        let reloaded = read_marker(&target_root).unwrap().unwrap();
        assert!(reloaded.pending_cleanup.is_some());
        assert!(retry_pending_storage_cleanup(&target_root, &[]).unwrap());
        assert!(!old.exists());
        assert!(read_marker(&target_root)
            .unwrap()
            .unwrap()
            .pending_cleanup
            .is_none());
    }

    #[test]
    fn final_directory_failure_restores_cleanup_marker() {
        let temp = tempfile::tempdir().unwrap();
        let tree = temp.path().join("tree");
        let token = "a".repeat(64);
        fs::create_dir(&tree).unwrap();
        fs::write(tree.join("payload.bin"), b"payload").unwrap();
        create_cleanup_marker(&tree, &token).unwrap();

        let error = remove_owned_storage_tree_with(&tree, &token, &[], |_| {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "held"))
        })
        .unwrap_err();

        assert!(error.contains("held"));
        assert_eq!(
            fs::read(tree.join(STORAGE_CLEANUP_MARKER)).unwrap(),
            token.as_bytes()
        );
    }

    #[test]
    fn ambiguous_rollback_disarms_cleanup_and_retains_the_copy() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current");
        let cache = temp.path().join("old-cache");
        let target_root = temp.path().join("target");
        create_direct_source_payload(&current, &cache);
        fs::create_dir(&target_root).unwrap();
        let target = target(&target_root);
        let mut published = copy_payload(&current, &cache, &target, |_, _, _| {}).unwrap();
        prepare_published_cleanup(&mut published, &current, &cache).unwrap();

        disarm_published_cleanup(&published).unwrap();

        assert!(read_marker(&target_root)
            .unwrap()
            .unwrap()
            .pending_cleanup
            .is_none());
        assert_eq!(
            fs::read(target_root.join("managed-games/sources/instance.json")).unwrap(),
            b"source record"
        );
        assert_eq!(
            fs::read(current.join("managed-games/sources/instance.json")).unwrap(),
            b"source record"
        );
        for path in [
            current.join("managed-games/sources"),
            current.join("managed-games/workspace"),
            current.join("managed-games/bases"),
            current.join(MANAGED_DIRECTORY),
            cache.clone(),
            target_root.join("managed-games/sources"),
            target_root.join("managed-games/workspace"),
            target_root.join(MANAGED_DIRECTORY),
            target_root.join(CACHE_DIRECTORY),
        ] {
            assert!(!path.join(STORAGE_CLEANUP_MARKER).exists());
        }
    }

    #[test]
    fn disarm_reports_cleanup_evidence_that_could_not_be_cleared() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current");
        let cache = temp.path().join("old-cache");
        let target_root = temp.path().join("target");
        create_direct_source_payload(&current, &cache);
        fs::create_dir(&target_root).unwrap();
        let target = target(&target_root);
        let mut published = copy_payload(&current, &cache, &target, |_, _, _| {}).unwrap();
        prepare_published_cleanup(&mut published, &current, &cache).unwrap();

        let error =
            disarm_published_cleanup_with(&published, |_| Err("storage marker is locked".into()))
                .unwrap_err();

        assert!(error.contains("cleanup evidence could not be cleared"));
        assert!(read_marker(&target_root)
            .unwrap()
            .unwrap()
            .pending_cleanup
            .is_some());
        for path in [
            current.join("managed-games/sources"),
            current.join("managed-games/workspace"),
            current.join("managed-games/bases"),
            current.join(MANAGED_DIRECTORY),
            cache.clone(),
            target_root.join("managed-games/sources"),
            target_root.join("managed-games/workspace"),
            target_root.join(MANAGED_DIRECTORY),
            target_root.join(CACHE_DIRECTORY),
        ] {
            assert!(!path.join(STORAGE_CLEANUP_MARKER).exists());
        }
    }

    #[test]
    fn abandoned_precommit_evidence_cannot_clean_the_authoritative_root() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current");
        let cache = temp.path().join("old-cache");
        let target_root = temp.path().join("target");
        let default = temp.path().join("default");
        let app_data = temp.path().join("app-data");
        create_direct_source_payload(&current, &cache);
        for path in [&target_root, &default, &app_data] {
            fs::create_dir(path).unwrap();
        }
        let target = target(&target_root);
        let mut published = copy_payload(&current, &cache, &target, |_, _, _| {}).unwrap();
        prepare_published_cleanup(&mut published, &current, &cache).unwrap();

        assert!(resolve_target(
            Some(target_root.to_str().unwrap()),
            &current,
            &default,
            &app_data,
            &[],
        )
        .unwrap_err()
        .contains("active storage"));
        assert_eq!(
            fs::read(current.join("managed-games/sources/instance.json")).unwrap(),
            b"source record"
        );
    }

    #[test]
    fn target_must_be_empty_and_separate_from_sources() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current");
        let default = temp.path().join("default");
        let app_data = temp.path().join("app-data");
        let target = temp.path().join("target");
        let game = temp.path().join("game");
        for path in [&current, &default, &app_data, &target, &game] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(target.join("notes.txt"), b"owned by user").unwrap();
        assert!(resolve_target(
            Some(target.to_str().unwrap()),
            &current,
            &default,
            &app_data,
            std::slice::from_ref(&game),
        )
        .unwrap_err()
        .contains("empty folder"));
        assert!(resolve_target(
            Some(game.to_str().unwrap()),
            &current,
            &default,
            &app_data,
            std::slice::from_ref(&game),
        )
        .unwrap_err()
        .contains("Among Us source"));
    }

    #[test]
    fn configured_custom_root_requires_ownership_marker_and_known_contents() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let default = temp.path().join("default");
        let app_data = temp.path().join("app-data");
        for path in [&target, &default, &app_data] {
            fs::create_dir_all(path).unwrap();
        }
        assert!(
            validate_configured_root(target.to_str().unwrap(), &default, &app_data, &[])
                .unwrap_err()
                .contains("not owned")
        );

        write_marker_state(&target, &marker_with_cleanup(None)).unwrap();
        fs::create_dir(target.join(MANAGED_DIRECTORY)).unwrap();
        assert!(
            validate_configured_root(target.to_str().unwrap(), &default, &app_data, &[]).is_ok()
        );

        fs::write(target.join("unrelated.txt"), b"user data").unwrap();
        assert!(
            validate_configured_root(target.to_str().unwrap(), &default, &app_data, &[])
                .unwrap_err()
                .contains("unexpected files")
        );
    }

    #[cfg(unix)]
    #[test]
    fn every_managed_subtree_rejects_links_independently() {
        use std::os::unix::fs::symlink;

        for relative in [
            "managed-games/sources/link",
            "managed-games/bases/link",
            "managed-games/workspace/profile/.stage.1/link",
            "cache/link",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("root");
            let outside = temp.path().join("outside");
            fs::create_dir_all(root.join("managed-games/sources")).unwrap();
            fs::create_dir_all(root.join("managed-games/bases")).unwrap();
            fs::create_dir_all(root.join("managed-games/workspace/profile/.stage.1")).unwrap();
            fs::create_dir_all(root.join("cache")).unwrap();
            fs::create_dir(&outside).unwrap();
            symlink(&outside, root.join(relative)).unwrap();

            assert!(validate_storage_payload(
                &root.join(MANAGED_DIRECTORY),
                &root.join(CACHE_DIRECTORY),
                &[],
            )
            .unwrap_err()
            .contains("reparse"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn linked_storage_target_is_rejected() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current");
        let default = temp.path().join("default");
        let app_data = temp.path().join("app-data");
        let real_target = temp.path().join("real-target");
        let linked_target = temp.path().join("linked-target");
        for path in [&current, &default, &app_data, &real_target] {
            fs::create_dir_all(path).unwrap();
        }
        symlink(&real_target, &linked_target).unwrap();
        assert!(resolve_target(
            Some(linked_target.to_str().unwrap()),
            &current,
            &default,
            &app_data,
            &[],
        )
        .unwrap_err()
        .contains("non-linked"));
    }
}
