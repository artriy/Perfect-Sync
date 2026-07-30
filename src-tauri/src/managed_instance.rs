use crate::settings::{self, GameInstance};
use perfect_sync_core::profile;
use perfect_sync_core::types::{Arch, Runtime, Store};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex, OnceLock,
};

pub const INSTANCE_MARKER: &str = ".perfectsync-instance.json";
const BASE_RECORD: &str = "base.json";
const SCHEMA: u32 = 1;
const MAX_FILES: usize = 200_000;
const MAX_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_RECORD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_BASE_GENERATIONS: usize = 128;
const MAX_CONFIG_FILES: usize = 4_096;
const MAX_CONFIG_BYTES: u64 = 128 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
static SERIAL: AtomicU64 = AtomicU64::new(0);
static VALIDATED_BASES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

const MOD_SOURCE_ROOTS: &[&str] = &[
    "bepinex",
    "corsaccosmetics",
    "dotnet",
    "winhttp.dll",
    "winhttp.dll.perfectsync-disabled",
    ".perfectsync-winhttp.disabled",
    "doorstop_config.ini",
    ".doorstop_version",
    "changelog.txt",
    "steam_appid.txt",
    "epicgamesstarter.exe",
    "egsauth.json",
    "logoutput.log",
    "errorlog.log",
];

const PROTECTED_DIRECTORIES: &[&str] = &[
    "BepInEx/core",
    "BepInEx/patchers",
    "BepInEx/plugins",
    "dotnet",
];

const MUTABLE_DIRECTORIES: &[&str] = &[
    "BepInEx/config",
    "BepInEx/cache",
    "BepInEx/interop",
    "BepInEx/unity-libs",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BaseRecord {
    pub schema: u32,
    pub id: String,
    pub game_instance_id: String,
    pub source_path: String,
    pub source_executable_sha256: String,
    pub arch: Arch,
    pub store: Store,
    pub runtime: Runtime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    pub manifest_sha256: String,
    #[serde(default)]
    pub exact_source_snapshot: bool,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone)]
pub struct GameBase {
    pub record: BaseRecord,
    pub game_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceMarker {
    pub schema: u32,
    pub base_id: String,
    pub base_manifest_sha256: String,
    pub game_instance_id: String,
    pub profile_id: String,
    pub profile_revision: String,
    pub managed_files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoaderPreference {
    pub schema: u32,
    #[serde(default)]
    pub apply_doorstop_fix: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader_version: Option<String>,
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
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    left.file_size() == right.file_size()
        && left.creation_time() == right.creation_time()
        && left.last_write_time() == right.last_write_time()
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(any(windows, unix)))]
fn same_file_identity(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    false
}

fn managed_root() -> PathBuf {
    settings::managed_data_dir().join("managed-games")
}

fn bases_root() -> PathBuf {
    managed_root().join("bases")
}

fn workspaces_root() -> PathBuf {
    managed_root().join("workspace")
}

fn workspace_root(workspace_id: &str) -> Result<PathBuf, String> {
    profile::validate_profile_id(workspace_id).map_err(|error| error.to_string())?;
    Ok(workspaces_root().join(workspace_id))
}

fn migrate_legacy_workspace(workspace_id: &str, workspace: &Path) -> Result<(), String> {
    let legacy = workspaces_root().join("current");
    recover_destination(&legacy)?;
    if workspace.join("current").exists() || !legacy.exists() {
        return Ok(());
    }
    let Some(marker) = read_json::<WorkspaceMarker>(&legacy.join(INSTANCE_MARKER))? else {
        return Ok(());
    };
    if marker.profile_id != workspace_id && marker.profile_id != "_vanilla" {
        return Ok(());
    }
    fs::create_dir_all(workspace).map_err(|error| error.to_string())?;
    fs::rename(&legacy, workspace.join("current"))
        .map_err(|error| format!("could not migrate the legacy managed workspace: {error}"))
}

pub fn workspace_game_dir(workspace_id: &str) -> Result<PathBuf, String> {
    let workspace = workspace_root(workspace_id)?;
    migrate_legacy_workspace(workspace_id, &workspace)?;
    Ok(workspace.join("current"))
}

pub fn workspace_ids() -> Result<Vec<String>, String> {
    let root = workspaces_root();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let Some(id) = name.to_str() else {
            continue;
        };
        if id == "current" || id.starts_with('.') || profile::validate_profile_id(id).is_err() {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if !is_reparse(&metadata) && metadata.is_dir() {
            ids.push(id.to_string());
        }
    }
    ids.sort();
    Ok(ids)
}

fn unique_child(parent: &Path, label: &str) -> PathBuf {
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{label}.{}.{}", std::process::id(), serial))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = digest.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes))
}

fn sha256_file(path: &Path, expected_size: Option<u64>) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err(format!("{} is not a regular non-link file", path.display()));
    }
    if let Some(expected) = expected_size {
        if metadata.len() != expected {
            return Err(format!("{} changed size", path.display()));
        }
    }
    if metadata.len() > MAX_BYTES {
        return Err(format!("{} exceeds the managed file limit", path.display()));
    }
    let mut input = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut read = 0_u64;
    loop {
        let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        read = read
            .checked_add(count as u64)
            .ok_or("managed file size overflow")?;
        hasher.update(&buffer[..count]);
    }
    if read != metadata.len() {
        return Err(format!("{} changed while it was hashed", path.display()));
    }
    Ok(hex_digest(hasher.finalize()))
}

fn safe_relative(path: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                let name = name
                    .to_str()
                    .ok_or("managed game paths must be valid Unicode")?;
                if name.is_empty()
                    || name == "."
                    || name == ".."
                    || name.chars().any(char::is_control)
                {
                    return Err("managed game contains an invalid path component".into());
                }
                parts.push(name);
            }
            _ => return Err("managed game path must be relative".into()),
        }
    }
    if parts.is_empty() {
        return Err("managed game path is empty".into());
    }
    Ok(parts.join("/"))
}

fn relative_path(value: &str) -> Result<PathBuf, String> {
    if value.is_empty() || value.len() > 4_096 || value.contains('\\') {
        return Err("managed manifest contains an invalid path".into());
    }
    let path = Path::new(value);
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => output.push(name),
            _ => return Err("managed manifest path is not relative".into()),
        }
    }
    if output.as_os_str().is_empty() {
        return Err("managed manifest path is empty".into());
    }
    Ok(output)
}

fn sorted_entries(path: &Path) -> Result<Vec<fs::DirEntry>, String> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
    Ok(entries)
}

pub fn source_mod_artifacts(source: &Path) -> Result<Vec<String>, String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("Among Us source must be a regular non-link directory".into());
    }
    let mut found = Vec::new();
    for entry in sorted_entries(source)? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if MOD_SOURCE_ROOTS
            .iter()
            .any(|artifact| name.eq_ignore_ascii_case(artifact))
            || name.to_ascii_lowercase().starts_with(".perfectsync-")
        {
            found.push(name);
        }
    }
    Ok(found)
}

fn require_exact_source(source: &Path) -> Result<(), String> {
    let artifacts = source_mod_artifacts(source)?;
    if artifacts.is_empty() {
        return Ok(());
    }
    let shown = artifacts.iter().take(8).cloned().collect::<Vec<_>>();
    let remaining = artifacts.len().saturating_sub(shown.len());
    let suffix = if remaining == 0 {
        String::new()
    } else {
        format!(" and {remaining} more")
    };
    Err(format!(
        "Cannot create an exact clean base because the selected source contains mod-loader artifacts: {}{suffix}. Perfect Sync did not change the source. Select a separate vanilla Among Us folder; this one can remain modded.",
        shown.join(", ")
    ))
}

fn copy_file_hashed(
    source: &Path,
    destination: &Path,
    expected: Option<&ManifestFile>,
) -> Result<ManifestFile, String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "{} is not a regular non-link file",
            source.display()
        ));
    }
    if let Some(expected) = expected {
        if metadata.len() != expected.size {
            return Err(format!("immutable base file changed: {}", expected.path));
        }
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut input = File::open(source).map_err(|error| error.to_string())?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut copied = 0_u64;
    loop {
        let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(count as u64)
            .filter(|total| *total <= MAX_BYTES)
            .ok_or("managed file exceeds its safety limit")?;
        hasher.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(|error| error.to_string())?;
    }
    output.sync_all().map_err(|error| error.to_string())?;
    let final_metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&final_metadata)
        || !final_metadata.is_file()
        || final_metadata.len() != metadata.len()
        || final_metadata.modified().ok() != metadata.modified().ok()
    {
        return Err(format!("{} changed while it was copied", source.display()));
    }
    if copied != metadata.len() {
        return Err(format!("{} changed while it was copied", source.display()));
    }
    let digest = hex_digest(hasher.finalize());
    if let Some(expected) = expected {
        if digest != expected.sha256 {
            return Err(format!("immutable base file changed: {}", expected.path));
        }
    }
    Ok(ManifestFile {
        path: String::new(),
        size: copied,
        sha256: digest,
    })
}

fn copy_source_tree(source: &Path, destination: &Path) -> Result<Vec<ManifestFile>, String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("Among Us source must be a regular non-link directory".into());
    }
    fs::create_dir(destination).map_err(|error| error.to_string())?;
    let mut pending = vec![(
        source.to_path_buf(),
        destination.to_path_buf(),
        PathBuf::new(),
    )];
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    while let Some((from, to, relative_root)) = pending.pop() {
        for entry in sorted_entries(&from)? {
            let name = entry.file_name();
            let source_path = entry.path();
            let metadata = fs::symlink_metadata(&source_path).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "Among Us source contains a link or reparse point: {}",
                    source_path.display()
                ));
            }
            let relative = relative_root.join(&name);
            let target = to.join(&name);
            if metadata.is_dir() {
                fs::create_dir(&target).map_err(|error| error.to_string())?;
                pending.push((source_path, target, relative));
                continue;
            }
            if !metadata.is_file() {
                return Err(format!("Unsupported game entry: {}", source_path.display()));
            }
            if files.len() >= MAX_FILES {
                return Err("Among Us source contains too many files".into());
            }
            total_bytes = total_bytes
                .checked_add(metadata.len())
                .filter(|total| *total <= MAX_BYTES)
                .ok_or("Among Us source exceeds the managed storage limit")?;
            let mut copied = copy_file_hashed(&source_path, &target, None)?;
            copied.path = safe_relative(&relative)?;
            files.push(copied);
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn validate_exact_source_layout(source: &Path, files: &[ManifestFile]) -> Result<(), String> {
    let expected = files
        .iter()
        .map(|file| (file.path.to_ascii_lowercase(), file))
        .collect::<HashMap<_, _>>();
    if expected.len() != files.len() {
        return Err("Among Us source contains case-colliding paths".into());
    }
    let mut seen = HashSet::with_capacity(files.len());
    let mut pending = vec![(source.to_path_buf(), PathBuf::new())];
    while let Some((directory, relative_root)) = pending.pop() {
        for entry in sorted_entries(&directory)? {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "Among Us source contains a link or reparse point: {}",
                    path.display()
                ));
            }
            let relative = relative_root.join(entry.file_name());
            if metadata.is_dir() {
                pending.push((path, relative));
                continue;
            }
            if !metadata.is_file() {
                return Err(format!("Unsupported game entry: {}", path.display()));
            }
            let portable = safe_relative(&relative)?;
            let key = portable.to_ascii_lowercase();
            let Some(file) = expected.get(&key) else {
                return Err(format!(
                    "Among Us source changed while its clean base was being created: {portable}"
                ));
            };
            if metadata.len() != file.size || !seen.insert(key) {
                return Err(format!(
                    "Among Us source changed while its clean base was being created: {portable}"
                ));
            }
        }
    }
    if seen.len() != files.len() {
        return Err("Among Us source changed while its clean base was being created".into());
    }
    Ok(())
}

fn base_validation_key(root: &Path, record: &BaseRecord) -> String {
    format!(
        "{}\0{}",
        normalized_path(root),
        record.manifest_sha256.to_ascii_lowercase()
    )
}

fn base_metadata_fingerprint(root: &Path, record: &BaseRecord) -> Result<String, String> {
    let game_dir = root.join("game");
    let mut hasher = Sha256::new();
    for expected in &record.files {
        let path = game_dir.join(relative_path(&expected.path)?);
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) || !metadata.is_file() || metadata.len() != expected.size {
            return Err(format!("immutable base file changed: {}", expected.path));
        }
        let modified = metadata
            .modified()
            .map_err(|error| error.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| format!("immutable base timestamp is invalid: {}", expected.path))?
            .as_nanos();
        hasher.update((expected.path.len() as u64).to_le_bytes());
        hasher.update(expected.path.as_bytes());
        hasher.update(expected.size.to_le_bytes());
        hasher.update(modified.to_le_bytes());
    }
    Ok(hex_digest(hasher.finalize()))
}

fn base_is_validated(root: &Path, record: &BaseRecord, fingerprint: &str) -> Result<bool, String> {
    VALIDATED_BASES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "managed base validation cache is poisoned".to_string())
        .map(|validated| {
            validated
                .get(&base_validation_key(root, record))
                .is_some_and(|cached| cached == fingerprint)
        })
}

fn mark_base_validated(root: &Path, record: &BaseRecord) -> Result<(), String> {
    let fingerprint = base_metadata_fingerprint(root, record)?;
    VALIDATED_BASES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "managed base validation cache is poisoned".to_string())?
        .insert(base_validation_key(root, record), fingerprint);
    Ok(())
}

fn validate_base_files(root: &Path, record: &BaseRecord) -> Result<(), String> {
    let fingerprint = base_metadata_fingerprint(root, record)?;
    if base_is_validated(root, record, &fingerprint)? {
        return Ok(());
    }
    let game_dir = root.join("game");
    let mut seen = HashSet::with_capacity(record.files.len());
    for expected in &record.files {
        if !seen.insert(expected.path.to_ascii_lowercase()) {
            return Err("immutable base manifest has case-colliding paths".into());
        }
        let path = game_dir.join(relative_path(&expected.path)?);
        if sha256_file(&path, Some(expected.size))? != expected.sha256 {
            return Err(format!("immutable base file changed: {}", expected.path));
        }
    }
    VALIDATED_BASES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "managed base validation cache is poisoned".to_string())?
        .insert(base_validation_key(root, record), fingerprint);
    Ok(())
}

fn copy_base_file(
    source: &Path,
    destination: &Path,
    expected: &ManifestFile,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_file() || metadata.len() != expected.size {
        return Err(format!("immutable base file changed: {}", expected.path));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    copy_file_hashed(source, destination, Some(expected))?;
    Ok(())
}

fn copy_base_tree(base: &GameBase, destination: &Path) -> Result<(), String> {
    let root = base
        .game_dir
        .parent()
        .ok_or("immutable base game directory has no parent")?;
    validate_base_files(root, &base.record)?;
    fs::create_dir(destination).map_err(|error| error.to_string())?;
    let mut seen = HashSet::with_capacity(base.record.files.len());
    let mut total = 0_u64;
    for expected in &base.record.files {
        let key = expected.path.to_ascii_lowercase();
        if !seen.insert(key) {
            return Err("immutable base manifest has case-colliding paths".into());
        }
        total = total
            .checked_add(expected.size)
            .filter(|bytes| *bytes <= MAX_BYTES)
            .ok_or("immutable base exceeds the managed storage limit")?;
        let relative = relative_path(&expected.path)?;
        let source = base.game_dir.join(&relative);
        let target = destination.join(&relative);
        copy_base_file(&source, &target, expected)?;
    }
    Ok(())
}

fn manifest_digest(files: &[ManifestFile]) -> Result<String, String> {
    let bytes = serde_json::to_vec(files).map_err(|error| error.to_string())?;
    Ok(sha256_bytes(&bytes))
}

fn base_generation_id(
    manifest_sha256: &str,
    executable_sha256: &str,
    build: Option<&str>,
) -> Result<String, String> {
    let identity = serde_json::to_vec(&(manifest_sha256, executable_sha256, build))
        .map_err(|error| error.to_string())?;
    Ok(sha256_bytes(&identity))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES {
        return Err(format!("{} is not a valid managed record", path.display()));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .map_err(|error| error.to_string())?
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("could not read {}: {error}", path.display()))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err("managed record exceeds its size limit".into());
    }
    let parent = path.parent().ok_or("managed record has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = unique_child(parent, "record");
    let result = (|| {
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        output
            .write_all(&bytes)
            .map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
        drop(output);
        if path.exists() {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
        fs::rename(&temporary, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn remove_tree(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_reparse(&metadata) => Err(format!(
            "refusing to remove link or reparse point {}",
            path.display()
        )),
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path).map_err(|error| error.to_string())
        }
        Ok(metadata) if metadata.is_file() => {
            fs::remove_file(path).map_err(|error| error.to_string())
        }
        Ok(_) => Err(format!("unsupported managed entry {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}
fn retired_prefix(destination: &Path) -> Result<String, String> {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("managed destination has no portable name")?;
    Ok(format!(".{name}-old."))
}

fn recover_destination(destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or("managed destination has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let prefix = retired_prefix(destination)?;
    let mut retired = sorted_entries(parent)?
        .into_iter()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .collect::<Vec<_>>();
    if retired.len() > 128 {
        return Err("too many interrupted managed publications require recovery".into());
    }
    retired.sort_by_key(|entry| {
        entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    if destination.exists() {
        for entry in retired {
            remove_tree(&entry.path())?;
        }
        return Ok(());
    }
    let Some(recovery) = retired.pop() else {
        return Ok(());
    };
    fs::rename(recovery.path(), destination)
        .map_err(|error| format!("could not recover the previous managed directory: {error}"))?;
    for entry in retired {
        remove_tree(&entry.path())?;
    }
    Ok(())
}

fn remove_prefixed_children(parent: &Path, prefix: &str) -> Result<(), String> {
    let entries = sorted_entries(parent)?
        .into_iter()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        .collect::<Vec<_>>();
    if entries.len() > 128 {
        return Err("too many interrupted managed staging directories require recovery".into());
    }
    for entry in entries {
        remove_tree(&entry.path())?;
    }
    Ok(())
}

fn publish_directory(stage: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or("managed destination has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    recover_destination(destination)?;
    let destination_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("managed destination has no portable name")?;
    let old = unique_child(parent, &format!("{destination_name}-old"));
    let had_destination = destination.exists();
    if had_destination {
        fs::rename(destination, &old)
            .map_err(|error| format!("could not retire the previous managed directory: {error}"))?;
    }
    if let Err(error) = fs::rename(stage, destination) {
        if had_destination {
            let _ = fs::rename(&old, destination);
        }
        return Err(format!("could not publish the managed directory: {error}"));
    }
    if had_destination {
        remove_tree(&old)?;
    }
    Ok(())
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn instance_slot(id: &str) -> String {
    let digest = sha256_bytes(id.as_bytes());
    digest[..32].to_string()
}

fn load_base_at(root: &Path) -> Result<Option<GameBase>, String> {
    let Some(record) = read_json::<BaseRecord>(&root.join(BASE_RECORD))? else {
        return Ok(None);
    };
    if record.schema != SCHEMA
        || record.id.len() != 64
        || !record.id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || record.files.is_empty()
        || record.files.len() > MAX_FILES
        || manifest_digest(&record.files)? != record.manifest_sha256
    {
        return Err("immutable base record is invalid".into());
    }
    validate_base_files(root, &record)?;
    let game_dir = root.join("game");
    Ok(Some(GameBase { record, game_dir }))
}

fn base_matches_instance(record: &BaseRecord, instance: &GameInstance) -> bool {
    record.game_instance_id == instance.id
        && record.arch == instance.arch
        && record.store == instance.store
}

fn select_existing_base<'a>(
    bases: &'a [&GameBase],
    instance: &GameInstance,
    preferred_build: Option<&str>,
    current_executable_sha256: &str,
    allow_legacy_current: bool,
) -> Option<&'a GameBase> {
    if let Some(build) = preferred_build {
        if let Some(base) = bases.iter().copied().find(|base| {
            base_matches_instance(&base.record, instance)
                && base.record.build.as_deref() == Some(build)
                && (base.record.exact_source_snapshot
                    || instance.build.as_deref() != Some(build)
                    || allow_legacy_current)
        }) {
            return Some(base);
        }
        if instance.build.as_deref() != Some(build) {
            return None;
        }
    }

    bases.iter().copied().find(|base| {
        base_matches_instance(&base.record, instance)
            && base.record.source_executable_sha256 == current_executable_sha256
            && base.record.build == instance.build
            && (base.record.exact_source_snapshot || allow_legacy_current)
    })
}

fn load_base_generations(container: &Path) -> Result<Vec<GameBase>, String> {
    let versions = container.join("versions");
    if !versions.exists() {
        return Ok(Vec::new());
    }
    let metadata = fs::symlink_metadata(&versions).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("immutable base versions path is not a regular directory".into());
    }
    let mut bases = Vec::new();
    for entry in sorted_entries(&versions)? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".base-stage.") {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if is_reparse(&metadata)
            || !metadata.is_dir()
            || name.len() != 64
            || !name.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!("invalid immutable base generation: {name}"));
        }
        let base = load_base_at(&entry.path())?
            .ok_or_else(|| format!("immutable base generation {name} has no record"))?;
        if base.record.id != name {
            return Err(format!(
                "immutable base generation {name} has the wrong identity"
            ));
        }
        bases.push(base);
        if bases.len() > MAX_BASE_GENERATIONS {
            return Err("too many immutable base generations".into());
        }
    }
    Ok(bases)
}

fn garbage_collect_base_generations(
    instance: &GameInstance,
    current_executable_sha256: &str,
    keep_id: &str,
    generations: &[GameBase],
) -> Result<(), String> {
    let referenced_builds = profile::ProfileStore::new(settings::profiles_root())
        .list()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|record| record.game_instance_id.as_deref() == Some(instance.id.as_str()))
        .filter_map(|record| record.game_build)
        .collect::<HashSet<_>>();

    for base in generations {
        let keep = base.record.id == keep_id
            || base.record.source_executable_sha256 == current_executable_sha256
            || base
                .record
                .build
                .as_ref()
                .is_some_and(|build| referenced_builds.contains(build));
        if keep {
            continue;
        }
        let root = base
            .game_dir
            .parent()
            .ok_or("immutable base generation has no parent")?;
        remove_tree(root)?;
    }
    Ok(())
}

pub fn ensure_base(
    instance: &GameInstance,
    preferred_build: Option<&str>,
) -> Result<GameBase, String> {
    if instance.id.trim().is_empty() {
        return Err("game instance has no identity".into());
    }
    let source = fs::canonicalize(Path::new(&instance.path))
        .map_err(|error| format!("could not open the Among Us source: {error}"))?;
    let source_metadata = fs::symlink_metadata(&source).map_err(|error| error.to_string())?;
    if is_reparse(&source_metadata) || !source_metadata.is_dir() {
        return Err("Among Us source must be a regular non-link directory".into());
    }
    let executable_sha256 = sha256_file(&source.join("Among Us.exe"), None)?;
    let bases = bases_root();
    fs::create_dir_all(&bases).map_err(|error| error.to_string())?;
    let container = bases.join(instance_slot(&instance.id));
    fs::create_dir_all(&container).map_err(|error| error.to_string())?;
    let versions = container.join("versions");
    fs::create_dir_all(&versions).map_err(|error| error.to_string())?;
    remove_prefixed_children(&versions, ".base-stage.")?;

    let legacy = load_base_at(&container)?;
    let generations = load_base_generations(&container)?;
    let all_bases = generations.iter().chain(legacy.iter()).collect::<Vec<_>>();

    let mut selected = select_existing_base(
        &all_bases,
        instance,
        preferred_build,
        &executable_sha256,
        false,
    );
    if selected.is_none() && legacy.is_some() && !source_mod_artifacts(&source)?.is_empty() {
        selected = select_existing_base(
            &all_bases,
            instance,
            preferred_build,
            &executable_sha256,
            true,
        );
    }
    if let Some(base) = selected {
        let selected = base.clone();
        garbage_collect_base_generations(
            instance,
            &executable_sha256,
            &selected.record.id,
            &generations,
        )?;
        return Ok(selected);
    }

    if let Some(build) = preferred_build {
        if instance.build.as_deref() != Some(build) {
            return Err(format!(
                "Profile requires Among Us build {build}, but its immutable clean base is unavailable and the selected source is build {}",
                instance.build.as_deref().unwrap_or("unknown")
            ));
        }
    }
    if generations.len() >= MAX_BASE_GENERATIONS {
        return Err("too many referenced immutable base generations".into());
    }
    require_exact_source(&source)?;

    let stage = unique_child(&versions, "base-stage");
    fs::create_dir(&stage).map_err(|error| error.to_string())?;
    let result = (|| {
        let files = copy_source_tree(&source, &stage.join("game"))?;
        validate_exact_source_layout(&source, &files)?;
        if !files
            .iter()
            .any(|file| file.path.eq_ignore_ascii_case("Among Us.exe"))
        {
            return Err(invalid("Among Us source did not produce an executable"));
        }
        let manifest_sha256 = manifest_digest(&files)?;
        let generation_id = base_generation_id(
            &manifest_sha256,
            &executable_sha256,
            instance.build.as_deref(),
        )?;
        let record = BaseRecord {
            schema: SCHEMA,
            id: generation_id,
            game_instance_id: instance.id.clone(),
            source_path: source.to_string_lossy().into_owned(),
            source_executable_sha256: executable_sha256.clone(),
            arch: instance.arch,
            store: instance.store,
            runtime: instance.runtime,
            build: instance.build.clone(),
            manifest_sha256: manifest_sha256.clone(),
            exact_source_snapshot: true,
            files,
        };
        write_json(&stage.join(BASE_RECORD), &record)?;
        let root = versions.join(&record.id);
        if root.exists() {
            remove_tree(&stage)?;
        } else {
            fs::rename(&stage, &root)
                .map_err(|error| format!("could not publish immutable base: {error}"))?;
        }
        mark_base_validated(&root, &record)?;
        let base = load_base_at(&root)?
            .ok_or_else(|| "published immutable base is missing".to_string())?;
        let updated_generations = load_base_generations(&container)?;
        garbage_collect_base_generations(
            instance,
            &executable_sha256,
            &base.record.id,
            &updated_generations,
        )?;
        Ok(base)
    })();
    if result.is_err() && stage.exists() {
        let _ = remove_tree(&stage);
    }
    result
}

pub fn begin_workspace(base: &GameBase, workspace_id: &str) -> Result<PathBuf, String> {
    let workspace = workspace_root(workspace_id)?;
    fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    let active = workspace_game_dir(workspace_id)?;
    recover_destination(&active)?;
    remove_prefixed_children(&workspace, ".stage.")?;
    let stage = unique_child(&workspace, "stage");
    let result = copy_base_tree(base, &stage);
    if let Err(error) = result {
        let _ = remove_tree(&stage);
        return Err(error);
    }
    Ok(stage)
}

pub fn discard_workspace(stage: &Path, workspace_id: &str) -> Result<(), String> {
    let workspace = workspace_root(workspace_id)?;
    fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    let canonical_parent = fs::canonicalize(&workspace).map_err(|error| error.to_string())?;
    let stage_parent = stage
        .parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
        .ok_or("workspace stage has no valid parent")?;
    if canonical_parent != stage_parent
        || !stage
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(".stage."))
    {
        return Err("refusing to remove a path outside managed workspace staging".into());
    }
    remove_tree(stage)
}

fn collect_tree(root: &Path, excluded_mutable: bool) -> Result<Vec<ManifestFile>, String> {
    let metadata = fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!("{} is not a regular directory", root.display()));
    }
    let mut pending = vec![(root.to_path_buf(), PathBuf::new())];
    let mut files = Vec::new();
    let mut bytes = 0_u64;
    while let Some((directory, relative_root)) = pending.pop() {
        for entry in sorted_entries(&directory)? {
            let source = entry.path();
            let metadata = fs::symlink_metadata(&source).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "managed tree contains a link: {}",
                    source.display()
                ));
            }
            let relative = relative_root.join(entry.file_name());
            let portable = safe_relative(&relative)?;
            if portable == INSTANCE_MARKER {
                continue;
            }
            if excluded_mutable
                && MUTABLE_DIRECTORIES.iter().any(|prefix| {
                    portable.eq_ignore_ascii_case(prefix)
                        || portable
                            .to_ascii_lowercase()
                            .starts_with(&format!("{}/", prefix.to_ascii_lowercase()))
                })
            {
                continue;
            }
            if metadata.is_dir() {
                pending.push((source, relative));
            } else if metadata.is_file() {
                if files.len() >= MAX_FILES {
                    return Err("managed tree contains too many files".into());
                }
                bytes = bytes
                    .checked_add(metadata.len())
                    .filter(|total| *total <= MAX_BYTES)
                    .ok_or("managed tree exceeds its byte limit")?;
                files.push(ManifestFile {
                    path: portable,
                    size: metadata.len(),
                    sha256: sha256_file(&source, Some(metadata.len()))?,
                });
            } else {
                return Err(format!(
                    "managed tree contains unsupported entry {}",
                    source.display()
                ));
            }
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn managed_delta(base: &GameBase, stage: &Path) -> Result<Vec<ManifestFile>, String> {
    let metadata = fs::symlink_metadata(stage).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("managed workspace stage is not a regular directory".into());
    }
    let base_files: HashMap<String, &ManifestFile> = base
        .record
        .files
        .iter()
        .map(|file| (file.path.to_ascii_lowercase(), file))
        .collect();
    let mut seen_base = HashSet::with_capacity(base_files.len());
    let mut pending = vec![(stage.to_path_buf(), PathBuf::new())];
    let mut managed = Vec::new();
    let mut file_count = 0_usize;
    let mut bytes = 0_u64;
    while let Some((directory, relative_root)) = pending.pop() {
        for entry in sorted_entries(&directory)? {
            let source = entry.path();
            let metadata = fs::symlink_metadata(&source).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "managed tree contains a link: {}",
                    source.display()
                ));
            }
            let relative = relative_root.join(entry.file_name());
            let portable = safe_relative(&relative)?;
            if portable == INSTANCE_MARKER {
                continue;
            }
            if MUTABLE_DIRECTORIES.iter().any(|prefix| {
                portable.eq_ignore_ascii_case(prefix)
                    || portable
                        .to_ascii_lowercase()
                        .starts_with(&format!("{}/", prefix.to_ascii_lowercase()))
            }) {
                continue;
            }
            if metadata.is_dir() {
                pending.push((source, relative));
                continue;
            }
            if !metadata.is_file() {
                return Err(format!(
                    "managed tree contains unsupported entry {}",
                    source.display()
                ));
            }
            file_count = file_count
                .checked_add(1)
                .filter(|count| *count <= MAX_FILES)
                .ok_or("managed tree contains too many files")?;
            bytes = bytes
                .checked_add(metadata.len())
                .filter(|total| *total <= MAX_BYTES)
                .ok_or("managed tree exceeds its byte limit")?;
            let key = portable.to_ascii_lowercase();
            if let Some(expected) = base_files.get(&key) {
                seen_base.insert(key);
                if metadata.len() != expected.size {
                    return Err(format!(
                        "immutable base file was replaced while building: {}",
                        expected.path
                    ));
                }
                let base_metadata =
                    fs::symlink_metadata(base.game_dir.join(relative_path(&expected.path)?))
                        .map_err(|error| error.to_string())?;
                if same_file_identity(&metadata, &base_metadata) {
                    continue;
                }
                if sha256_file(&source, Some(expected.size))? == expected.sha256 {
                    continue;
                }
                return Err(format!(
                    "immutable base file was replaced while building: {}",
                    expected.path
                ));
            }
            managed.push(ManifestFile {
                path: portable,
                size: metadata.len(),
                sha256: sha256_file(&source, Some(metadata.len()))?,
            });
        }
    }
    if seen_base.len() != base_files.len() {
        return Err("managed workspace is missing immutable base files".into());
    }
    managed.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(managed)
}

pub fn publish_workspace(
    stage: &Path,
    base: &GameBase,
    profile_id: &str,
    profile_revision: &str,
    workspace_id: &str,
) -> Result<PathBuf, String> {
    profile::validate_profile_id(profile_id).map_err(|error| error.to_string())?;
    profile::validate_profile_id(workspace_id).map_err(|error| error.to_string())?;
    if profile_revision.len() != 64
        || !profile_revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("profile revision is invalid".into());
    }
    let expected_parent = workspace_root(workspace_id)?;
    if stage.parent() != Some(expected_parent.as_path()) {
        return Err("workspace stage does not belong to the requested profile".into());
    }
    let managed_files = managed_delta(base, stage)?;
    let marker = WorkspaceMarker {
        schema: SCHEMA,
        base_id: base.record.id.clone(),
        base_manifest_sha256: base.record.manifest_sha256.clone(),
        game_instance_id: base.record.game_instance_id.clone(),
        profile_id: profile_id.to_string(),
        profile_revision: profile_revision.to_string(),
        managed_files,
    };
    write_json(&stage.join(INSTANCE_MARKER), &marker)?;
    let active = workspace_game_dir(workspace_id)?;
    publish_directory(stage, &active)?;
    Ok(active)
}

pub fn active_marker(workspace_id: &str) -> Result<Option<WorkspaceMarker>, String> {
    let active = workspace_game_dir(workspace_id)?;
    recover_destination(&active)?;
    let Some(marker) = read_json::<WorkspaceMarker>(&active.join(INSTANCE_MARKER))? else {
        return Ok(None);
    };
    if marker.schema != SCHEMA
        || marker.base_id.len() != 64
        || marker.profile_revision.len() != 64
        || marker.managed_files.len() > MAX_FILES
    {
        return Err("active workspace marker is invalid".into());
    }
    profile::validate_profile_id(&marker.profile_id).map_err(|error| error.to_string())?;
    Ok(Some(marker))
}

fn is_mutable_path(path: &str) -> bool {
    MUTABLE_DIRECTORIES.iter().any(|prefix| {
        path.eq_ignore_ascii_case(prefix)
            || path
                .to_ascii_lowercase()
                .starts_with(&format!("{}/", prefix.to_ascii_lowercase()))
    })
}

fn protected_files(root: &Path) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    for prefix in PROTECTED_DIRECTORIES {
        let directory = root.join(relative_path(prefix)?);
        let metadata = match fs::symlink_metadata(&directory) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if is_reparse(&metadata) || !metadata.is_dir() {
            return Err(format!("protected workspace path is invalid: {prefix}"));
        }
        let mut pending = vec![(directory, PathBuf::from(prefix))];
        while let Some((current, relative_root)) = pending.pop() {
            for entry in sorted_entries(&current)? {
                let metadata =
                    fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
                if is_reparse(&metadata) {
                    return Err(format!(
                        "protected workspace contains a link: {}",
                        entry.path().display()
                    ));
                }
                let relative = relative_root.join(entry.file_name());
                let portable = safe_relative(&relative)?;
                if is_mutable_path(&portable) {
                    continue;
                }
                if metadata.is_dir() {
                    pending.push((entry.path(), relative));
                } else if metadata.is_file() {
                    paths.push(portable);
                } else {
                    return Err(format!(
                        "protected workspace contains unsupported entry {}",
                        entry.path().display()
                    ));
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub fn active_matches(
    base: &GameBase,
    profile_id: &str,
    revision: &str,
    workspace_id: &str,
) -> Result<bool, String> {
    let Some(marker) = active_marker(workspace_id)? else {
        return Ok(false);
    };
    if marker.base_id != base.record.id
        || marker.base_manifest_sha256 != base.record.manifest_sha256
        || marker.game_instance_id != base.record.game_instance_id
        || marker.profile_id != profile_id
        || marker.profile_revision != revision
    {
        return Ok(false);
    }
    let active = workspace_game_dir(workspace_id)?;
    let mut managed_names = HashSet::with_capacity(marker.managed_files.len());
    for expected in &marker.managed_files {
        if !managed_names.insert(expected.path.to_ascii_lowercase()) {
            return Err("active workspace marker has case-colliding paths".into());
        }
        let path = active.join(relative_path(&expected.path)?);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
        };
        if is_reparse(&metadata)
            || !metadata.is_file()
            || metadata.len() != expected.size
            || sha256_file(&path, Some(expected.size))? != expected.sha256
        {
            return Ok(false);
        }
    }
    let Some(base_executable) = base
        .record
        .files
        .iter()
        .find(|file| file.path.eq_ignore_ascii_case("Among Us.exe"))
    else {
        return Err("immutable base manifest is missing Among Us.exe".into());
    };
    let executable = active.join("Among Us.exe");
    if sha256_file(&executable, Some(base_executable.size))? != base_executable.sha256 {
        return Ok(false);
    }
    for path in protected_files(&active)? {
        if !managed_names.contains(&path.to_ascii_lowercase()) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn profile_revision(profile_root: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    if profile_root.is_dir() {
        for file in collect_tree(profile_root, false)? {
            if file.path.eq_ignore_ascii_case("profile.json") {
                continue;
            }
            hasher.update((file.path.len() as u64).to_le_bytes());
            hasher.update(file.path.as_bytes());
            hasher.update(file.size.to_le_bytes());
            hasher.update(file.sha256.as_bytes());
        }
    }
    Ok(hex_digest(hasher.finalize()))
}

fn copy_config_tree(source: &Path, destination: &Path, replace: bool) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("profile configuration is not a regular directory".into());
    }
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let mut pending = vec![(source.to_path_buf(), destination.to_path_buf())];
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    while let Some((from, to)) = pending.pop() {
        for entry in sorted_entries(&from)? {
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "profile configuration contains a link: {}",
                    entry.path().display()
                ));
            }
            let target = to.join(entry.file_name());
            if metadata.is_dir() {
                fs::create_dir_all(&target).map_err(|error| error.to_string())?;
                pending.push((entry.path(), target));
            } else if metadata.is_file() {
                files += 1;
                bytes = bytes
                    .checked_add(metadata.len())
                    .filter(|total| *total <= MAX_CONFIG_BYTES)
                    .ok_or("profile configuration exceeds its byte limit")?;
                if files > MAX_CONFIG_FILES {
                    return Err("profile configuration contains too many files".into());
                }
                let mut input = File::open(entry.path()).map_err(|error| error.to_string())?;
                let mut options = OpenOptions::new();
                options.write(true);
                if replace {
                    options.create(true).truncate(true);
                } else {
                    options.create_new(true);
                }
                let mut output = options.open(&target).map_err(|error| error.to_string())?;
                let copied =
                    io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
                if copied != metadata.len() {
                    return Err("profile configuration changed while it was copied".into());
                }
                output.sync_all().map_err(|error| error.to_string())?;
            } else {
                return Err("profile configuration contains an unsupported entry".into());
            }
        }
    }
    Ok(())
}

pub fn capture_workspace_config(profiles_root: &Path, workspace_id: &str) -> Result<(), String> {
    let Some(marker) = active_marker(workspace_id)? else {
        return Ok(());
    };
    if marker.profile_id == "_vanilla" {
        return Ok(());
    }
    let source = workspace_game_dir(workspace_id)?
        .join("BepInEx")
        .join("config");
    let metadata = match fs::symlink_metadata(&source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("active workspace configuration is not a regular directory".into());
    }
    let profile_root = profiles_root.join(&marker.profile_id);
    let profile_metadata =
        fs::symlink_metadata(&profile_root).map_err(|error| error.to_string())?;
    if is_reparse(&profile_metadata) || !profile_metadata.is_dir() {
        return Err("active workspace refers to an unavailable profile".into());
    }
    let destination = profile_root.join("BepInEx").join("config");
    let parent = destination
        .parent()
        .ok_or("profile configuration has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let stage = unique_child(parent, "config-stage");
    let result = (|| {
        copy_config_tree(&source, &stage, false)?;
        publish_directory(&stage, &destination)
    })();
    if result.is_err() {
        let _ = remove_tree(&stage);
    }
    result
}

pub fn overlay_profile_config(profile_root: &Path, game_dir: &Path) -> Result<(), String> {
    let source = profile_root.join("BepInEx").join("config");
    match fs::symlink_metadata(&source) {
        Ok(metadata) if !is_reparse(&metadata) && metadata.is_dir() => {
            copy_config_tree(&source, &game_dir.join("BepInEx").join("config"), true)
        }
        Ok(_) => Err("profile configuration is not a regular directory".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn preference_path(profile_root: &Path) -> PathBuf {
    profile_root.join(".perfectsync-loader-preference.json")
}

pub fn loader_preference(profile_root: &Path) -> Result<LoaderPreference, String> {
    let Some(preference) = read_json::<LoaderPreference>(&preference_path(profile_root))? else {
        return Ok(LoaderPreference {
            schema: SCHEMA,
            ..LoaderPreference::default()
        });
    };
    if preference.schema != SCHEMA {
        return Err("profile loader preference has an unsupported schema".into());
    }
    Ok(preference)
}

pub fn save_loader_preference(
    profile_root: &Path,
    apply_doorstop_fix: bool,
    loader_version: Option<String>,
) -> Result<(), String> {
    let preference = LoaderPreference {
        schema: SCHEMA,
        apply_doorstop_fix,
        loader_version,
    };
    write_json(&preference_path(profile_root), &preference)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection_instance(build: &str) -> GameInstance {
        GameInstance {
            id: "source-1".into(),
            name: "Source".into(),
            path: "C:/Among Us".into(),
            executable_identity: None,
            arch: Arch::X86,
            store: Store::Steam,
            runtime: Runtime::Native,
            build: Some(build.into()),
            writable: true,
        }
    }

    fn selection_base(id: &str, build: &str, executable: &str, exact: bool) -> GameBase {
        GameBase {
            record: BaseRecord {
                schema: SCHEMA,
                id: id.into(),
                game_instance_id: "source-1".into(),
                source_path: "C:/Among Us".into(),
                source_executable_sha256: executable.into(),
                arch: Arch::X86,
                store: Store::Steam,
                runtime: Runtime::Native,
                build: Some(build.into()),
                manifest_sha256: id.into(),
                exact_source_snapshot: exact,
                files: Vec::new(),
            },
            game_dir: PathBuf::from(id),
        }
    }

    #[test]
    fn base_selection_honors_a_profiles_historical_build() {
        let instance = selection_instance("2026.7");
        let current = selection_base("current", "2026.7", "current-exe", true);
        let historical = selection_base("historical", "2026.6", "old-exe", true);
        let bases = [&current, &historical];

        let selected =
            select_existing_base(&bases, &instance, Some("2026.6"), "current-exe", false).unwrap();
        assert_eq!(selected.record.id, "historical");
        assert!(
            select_existing_base(&bases, &instance, Some("2026.5"), "current-exe", false).is_none()
        );
    }

    #[test]
    fn clean_current_source_rebuilds_a_legacy_sanitized_base() {
        let instance = selection_instance("2026.7");
        let legacy = selection_base("legacy", "2026.7", "current-exe", false);
        let bases = [&legacy];

        assert!(
            select_existing_base(&bases, &instance, Some("2026.7"), "current-exe", false).is_none()
        );
        assert_eq!(
            select_existing_base(&bases, &instance, Some("2026.7"), "current-exe", true)
                .unwrap()
                .record
                .id,
            "legacy"
        );
    }

    #[test]
    fn legacy_base_records_deserialize_as_non_exact_snapshots() {
        let legacy = selection_base(&"a".repeat(64), "2026.7", &"b".repeat(64), true);
        let mut value = serde_json::to_value(&legacy.record).unwrap();
        value.as_object_mut().unwrap().remove("exactSourceSnapshot");

        let migrated: BaseRecord = serde_json::from_value(value).unwrap();

        assert!(!migrated.exact_source_snapshot);
    }

    #[test]
    fn modded_source_is_rejected_without_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(source.join("bEpInEx/plugins")).unwrap();
        fs::write(source.join("Among Us.exe"), b"game executable").unwrap();
        fs::write(source.join("bEpInEx/plugins/Stale.dll"), b"stale mod").unwrap();
        fs::write(source.join("WINHTTP.DLL"), b"stale loader").unwrap();

        let error = require_exact_source(&source).unwrap_err();

        assert!(error.contains("bEpInEx"));
        assert!(error.contains("WINHTTP.DLL"));
        assert_eq!(
            fs::read(source.join("bEpInEx/plugins/Stale.dll")).unwrap(),
            b"stale mod"
        );
        assert_eq!(
            fs::read(source.join("WINHTTP.DLL")).unwrap(),
            b"stale loader"
        );
    }

    #[test]
    fn exact_base_and_managed_delta_enforce_file_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(source.join("Among Us_Data")).unwrap();
        fs::write(source.join("Among Us.exe"), b"game executable").unwrap();
        fs::write(source.join("GameAssembly.dll"), b"game assembly").unwrap();
        fs::write(source.join("Among Us_Data/data.unity3d"), b"game data").unwrap();
        require_exact_source(&source).unwrap();

        let root = temp.path().join("base");
        fs::create_dir(&root).unwrap();
        let game_dir = root.join("game");
        let files = copy_source_tree(&source, &game_dir).unwrap();
        validate_exact_source_layout(&source, &files).unwrap();
        let manifest_sha256 = manifest_digest(&files).unwrap();
        let base = GameBase {
            record: BaseRecord {
                schema: SCHEMA,
                id: manifest_sha256.clone(),
                game_instance_id: "source-1".into(),
                source_path: source.to_string_lossy().into(),
                source_executable_sha256: sha256_file(&source.join("Among Us.exe"), None).unwrap(),
                arch: Arch::X86,
                store: Store::Steam,
                runtime: Runtime::Native,
                build: None,
                manifest_sha256,
                exact_source_snapshot: true,
                files,
            },
            game_dir,
        };

        assert_eq!(
            fs::read(base.game_dir.join("Among Us_Data/data.unity3d")).unwrap(),
            b"game data"
        );

        let stage = temp.path().join("stage");
        copy_base_tree(&base, &stage).unwrap();
        fs::create_dir_all(stage.join("BepInEx/plugins")).unwrap();
        fs::create_dir_all(stage.join("BepInEx/config")).unwrap();
        fs::write(stage.join("BepInEx/plugins/Profile.dll"), b"profile mod").unwrap();
        fs::write(stage.join("BepInEx/config/Profile.cfg"), b"user setting").unwrap();
        let delta = managed_delta(&base, &stage).unwrap();
        assert!(delta.iter().any(|file| file
            .path
            .eq_ignore_ascii_case("BepInEx/plugins/Profile.dll")));
        assert!(!delta
            .iter()
            .any(|file| file.path.eq_ignore_ascii_case("BepInEx/config/Profile.cfg")));

        fs::write(base.game_dir.join("GameAssembly.dll"), b"changed").unwrap();
        let error = copy_base_tree(&base, &temp.path().join("invalid-stage")).unwrap_err();
        assert!(error.contains("immutable base file changed"));
    }

    #[test]
    fn base_staging_copies_files_without_linking_mutable_workspace_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("base.bin");
        let destination = temp.path().join("stage/base.bin");
        fs::write(&source, b"immutable game bytes").unwrap();
        let expected = ManifestFile {
            path: "base.bin".into(),
            size: fs::metadata(&source).unwrap().len(),
            sha256: sha256_file(&source, None).unwrap(),
        };

        copy_base_file(&source, &destination, &expected).unwrap();
        fs::write(&destination, b"workspace changed").unwrap();

        assert_eq!(fs::read(source).unwrap(), b"immutable game bytes");
        assert_eq!(fs::read(destination).unwrap(), b"workspace changed");
    }

    #[test]
    fn profile_config_overrides_packaged_defaults_and_changes_revision() {
        let temp = tempfile::tempdir().unwrap();
        let profile_root = temp.path().join("profile");
        let profile_config = profile_root.join("BepInEx/config");
        fs::create_dir_all(&profile_config).unwrap();
        fs::write(profile_config.join("mod.cfg"), b"profile value").unwrap();
        let game = temp.path().join("game");
        fs::create_dir_all(game.join("BepInEx/config")).unwrap();
        fs::write(game.join("BepInEx/config/mod.cfg"), b"package default").unwrap();

        let before = profile_revision(&profile_root).unwrap();
        overlay_profile_config(&profile_root, &game).unwrap();
        assert_eq!(
            fs::read(game.join("BepInEx/config/mod.cfg")).unwrap(),
            b"profile value"
        );
        fs::write(profile_config.join("mod.cfg"), b"changed value").unwrap();
        let after = profile_revision(&profile_root).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn profile_metadata_does_not_invalidate_materialized_files() {
        let temp = tempfile::tempdir().unwrap();
        let profile_root = temp.path().join("profile");
        fs::create_dir(&profile_root).unwrap();
        fs::write(profile_root.join("profile.json"), br#"{"name":"Before"}"#).unwrap();
        fs::write(profile_root.join("mod.dll"), b"plugin").unwrap();
        let before = profile_revision(&profile_root).unwrap();

        fs::write(profile_root.join("profile.json"), br#"{"name":"After"}"#).unwrap();

        assert_eq!(profile_revision(&profile_root).unwrap(), before);
    }

    #[test]
    fn failed_publication_restores_the_previous_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("current");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("state"), b"working").unwrap();

        let error =
            publish_directory(&temp.path().join("missing-stage"), &destination).unwrap_err();

        assert!(error.contains("could not publish"));
        assert_eq!(fs::read(destination.join("state")).unwrap(), b"working");
        assert!(sorted_entries(temp.path())
            .unwrap()
            .iter()
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".current-old.")));
    }

    #[test]
    fn interrupted_publication_restores_the_newest_retired_directory() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("current");
        let older = temp.path().join(".current-old.1");
        let newer = temp.path().join(".current-old.2");
        fs::create_dir(&older).unwrap();
        fs::write(older.join("state"), b"older").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        fs::create_dir(&newer).unwrap();
        fs::write(newer.join("state"), b"newer").unwrap();

        recover_destination(&destination).unwrap();

        assert_eq!(fs::read(destination.join("state")).unwrap(), b"newer");
        assert!(!older.exists());
        assert!(!newer.exists());
    }
}
