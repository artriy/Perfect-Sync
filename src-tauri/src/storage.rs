//! Verified relocation of large managed game data and package caches.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const STORAGE_MARKER: &str = ".perfectsync-storage.json";
const STORAGE_SCHEMA: u32 = 1;
const MANAGED_DIRECTORY: &str = "managed-games";
const CACHE_DIRECTORY: &str = "cache";
const MAX_STORAGE_FILES: usize = 250_000;
const MAX_STORAGE_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;

static MOVE_SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StorageMarker {
    schema: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeFile {
    relative: PathBuf,
    size: u64,
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
}

#[derive(Debug)]
pub struct PublishedStorage {
    pub root: PathBuf,
    cache_path: PathBuf,
    marker_created: bool,
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

fn read_marker(root: &Path) -> Result<bool, String> {
    let path = root.join(STORAGE_MARKER);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("could not inspect the storage marker: {error}")),
    };
    if is_reparse(&metadata) || !metadata.is_file() || metadata.len() > 4096 {
        return Err("the selected folder has an invalid Perfect Sync storage marker".into());
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("could not read the storage marker: {error}"))?;
    let marker: StorageMarker = serde_json::from_slice(&bytes).map_err(|_| {
        "the selected folder has an invalid Perfect Sync storage marker".to_string()
    })?;
    if marker.schema != STORAGE_SCHEMA {
        return Err("the selected folder uses an unsupported Perfect Sync storage format".into());
    }
    Ok(true)
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
    if !read_marker(&target)? {
        return Err("the configured folder is not owned by Perfect Sync".into());
    }
    let entries = target_entries(&target)?;
    if entries.iter().any(|name| {
        !name.eq_ignore_ascii_case(STORAGE_MARKER)
            && !name.eq_ignore_ascii_case(MANAGED_DIRECTORY)
            && !name.eq_ignore_ascii_case(CACHE_DIRECTORY)
    }) {
        return Err("the configured Perfect Sync storage folder contains unexpected files".into());
    }
    for directory in [target.join(MANAGED_DIRECTORY), target.join(CACHE_DIRECTORY)] {
        if directory.exists() {
            validate_regular_directory(&directory, "A managed storage directory")?;
        }
    }
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

    let marker_existed = read_marker(&target)?;
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
    let reserved_exists = entries.iter().any(|name| {
        name.eq_ignore_ascii_case(MANAGED_DIRECTORY)
            || name.eq_ignore_ascii_case(CACHE_DIRECTORY)
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
            .filter(|name| !name.eq_ignore_ascii_case(STORAGE_MARKER))
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
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| invalid("managed storage size overflow"))?;
                if files.len() >= MAX_STORAGE_FILES || bytes > MAX_STORAGE_BYTES {
                    return Err("managed storage exceeds the safe relocation limit".into());
                }
                files.push(TreeFile {
                    relative,
                    size: metadata.len(),
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

fn sha256_file(path: &Path, expected_size: u64, buffer: &mut [u8]) -> Result<[u8; 32], String> {
    let mut input = File::open(path)
        .map_err(|error| format!("could not verify {}: {error}", path.display()))?;
    let metadata = input
        .metadata()
        .map_err(|error| format!("could not verify {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() != expected_size {
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

fn copy_tree_verified<F>(
    source: &Path,
    destination: &Path,
    manifest: &TreeManifest,
    copied: &mut u64,
    total: u64,
    buffer: &mut [u8],
    progress: &mut F,
) -> Result<(), String>
where
    F: FnMut(u64, u64, &str),
{
    fs::create_dir(destination)
        .map_err(|error| format!("could not create storage staging directory: {error}"))?;
    for relative in &manifest.directories {
        fs::create_dir(destination.join(relative))
            .map_err(|error| format!("could not create storage directory: {error}"))?;
    }
    for file in &manifest.files {
        let source_file = source.join(&file.relative);
        let destination_file = destination.join(&file.relative);
        let message = format!("Copying {}", file.relative.display());
        progress(*copied, total, &message);
        copy_file_verified(&source_file, &destination_file, file.size, buffer)?;
        *copied = copied
            .checked_add(file.size)
            .ok_or_else(|| invalid("storage size overflow"))?;
        progress(*copied, total, &message);
    }
    let after = collect_tree(source)?
        .ok_or_else(|| invalid("managed storage disappeared during relocation"))?;
    if &after != manifest {
        return Err(format!(
            "managed storage changed during relocation: {}",
            source.display()
        ));
    }
    Ok(())
}

fn remove_tree(path: &Path) -> Result<(), String> {
    let manifest = match collect_tree(path)? {
        Some(manifest) => manifest,
        None => return Ok(()),
    };
    for file in manifest.files.iter().rev() {
        fs::remove_file(path.join(&file.relative))
            .map_err(|error| format!("could not remove old storage file: {error}"))?;
    }
    let mut directories = manifest.directories;
    directories.sort_by_key(|relative| std::cmp::Reverse(relative.components().count()));
    for directory in directories {
        fs::remove_dir(path.join(directory))
            .map_err(|error| format!("could not remove old storage directory: {error}"))?;
    }
    fs::remove_dir(path).map_err(|error| format!("could not remove old storage directory: {error}"))
}

fn write_marker(root: &Path) -> Result<(), String> {
    let path = root.join(STORAGE_MARKER);
    let bytes = serde_json::to_vec_pretty(&StorageMarker {
        schema: STORAGE_SCHEMA,
    })
    .map_err(|error| error.to_string())?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("could not create the storage marker: {error}"))?;
    output
        .write_all(&bytes)
        .and_then(|_| output.write_all(b"\n"))
        .and_then(|_| output.flush())
        .and_then(|_| output.sync_all())
        .map_err(|error| format!("could not finish the storage marker: {error}"))
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
    let managed_manifest = collect_tree(&managed_source)?;
    let cache_manifest = collect_tree(current_cache)?;
    let total = managed_manifest
        .as_ref()
        .map_or(0, |manifest| manifest.bytes)
        .checked_add(cache_manifest.as_ref().map_or(0, |manifest| manifest.bytes))
        .ok_or_else(|| invalid("managed storage size overflow"))?;
    progress(0, total, "Preparing verified storage copy");

    let serial = MOVE_SERIAL.fetch_add(1, Ordering::Relaxed);
    let prefix = format!(".perfectsync-storage-move-{}-{serial}", std::process::id());
    let managed_stage = target.root.join(format!("{prefix}-managed"));
    let cache_parent = target
        .cache_path
        .parent()
        .ok_or_else(|| invalid("destination package cache has no parent"))?;
    let cache_stage = cache_parent.join(format!("{prefix}-cache"));
    let managed_final = target.root.join(MANAGED_DIRECTORY);
    let cache_final = target.cache_path.clone();
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut managed_published = false;
    let mut cache_published = false;
    let result = (|| {
        if let Some(manifest) = &managed_manifest {
            copy_tree_verified(
                &managed_source,
                &managed_stage,
                manifest,
                &mut copied,
                total,
                &mut buffer,
                &mut progress,
            )?;
            fs::rename(&managed_stage, &managed_final)
                .map_err(|error| format!("could not publish managed game storage: {error}"))?;
            managed_published = true;
        }
        if let Some(manifest) = &cache_manifest {
            copy_tree_verified(
                current_cache,
                &cache_stage,
                manifest,
                &mut copied,
                total,
                &mut buffer,
                &mut progress,
            )?;
            fs::rename(&cache_stage, &cache_final)
                .map_err(|error| format!("could not publish package cache storage: {error}"))?;
            cache_published = true;
        }
        if !target.marker_existed {
            write_marker(&target.root)?;
        }
        progress(total, total, "Verified the relocated storage copy");
        Ok(PublishedStorage {
            root: target.root.clone(),
            cache_path: cache_final.clone(),
            marker_created: !target.marker_existed,
        })
    })();
    if result.is_err() {
        let _ = remove_tree(&managed_stage);
        let _ = remove_tree(&cache_stage);
        if managed_published {
            let _ = remove_tree(&managed_final);
        }
        if cache_published {
            let _ = remove_tree(&cache_final);
        }
        if !target.marker_existed {
            let _ = fs::remove_file(target.root.join(STORAGE_MARKER));
        }
    }
    result
}

pub fn rollback_published(published: &PublishedStorage) -> Result<(), String> {
    let mut errors = Vec::new();
    for path in [
        published.root.join(MANAGED_DIRECTORY),
        published.cache_path.clone(),
    ] {
        if let Err(error) = remove_tree(&path) {
            errors.push(error);
        }
    }
    if published.marker_created {
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

pub fn cleanup_old_payload(current_root: &Path, current_cache: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    for path in [
        current_root.join(MANAGED_DIRECTORY),
        current_cache.to_path_buf(),
    ] {
        if let Err(error) = remove_tree(&path) {
            errors.push(error);
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_copy_preserves_managed_data_cache_and_empty_directories() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current");
        let cache = temp.path().join("old-cache");
        let target_root = temp.path().join("target");
        fs::create_dir_all(current.join("managed-games/bases/base/game/Empty")).unwrap();
        fs::create_dir_all(current.join("managed-games/workspace/current")).unwrap();
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            current.join("managed-games/bases/base/game/Among Us.exe"),
            b"game",
        )
        .unwrap();
        fs::write(
            current.join("managed-games/workspace/current/profile.bin"),
            b"profile",
        )
        .unwrap();
        fs::write(cache.join("package.zip"), b"archive").unwrap();
        fs::create_dir(&target_root).unwrap();
        let target = StorageTarget {
            root: target_root.clone(),
            configured_path: Some(target_root.to_string_lossy().into_owned()),
            cache_path: target_root.join(CACHE_DIRECTORY),
            marker_existed: false,
        };
        let mut last = (0, 0);
        let published = copy_payload(&current, &cache, &target, |copied, total, _| {
            last = (copied, total)
        })
        .unwrap();

        assert_eq!(
            fs::read(target_root.join("managed-games/bases/base/game/Among Us.exe")).unwrap(),
            b"game"
        );
        assert_eq!(
            fs::read(target_root.join("managed-games/workspace/current/profile.bin")).unwrap(),
            b"profile"
        );
        assert_eq!(
            fs::read(target_root.join("cache/package.zip")).unwrap(),
            b"archive"
        );
        assert!(target_root
            .join("managed-games/bases/base/game/Empty")
            .is_dir());
        assert!(target_root.join(STORAGE_MARKER).is_file());
        assert_eq!(last.0, last.1);

        rollback_published(&published).unwrap();
        assert!(!target_root.join("managed-games").exists());
        assert!(!target_root.join("cache").exists());
        assert!(!target_root.join(STORAGE_MARKER).exists());
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
            validate_configured_root(target.to_str().unwrap(), &default, &app_data, &[],)
                .unwrap_err()
                .contains("not owned")
        );

        write_marker(&target).unwrap();
        fs::create_dir(target.join(MANAGED_DIRECTORY)).unwrap();
        assert!(
            validate_configured_root(target.to_str().unwrap(), &default, &app_data, &[],).is_ok()
        );

        fs::write(target.join("unrelated.txt"), b"user data").unwrap();
        assert!(
            validate_configured_root(target.to_str().unwrap(), &default, &app_data, &[],)
                .unwrap_err()
                .contains("unexpected files")
        );
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
