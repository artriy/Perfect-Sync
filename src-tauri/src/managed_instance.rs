use crate::settings::{self, GameInstance};
use atomicwrites::{AllowOverwrite, AtomicFile};
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
use std::time::Instant;

pub const INSTANCE_MARKER: &str = ".perfectsync-instance.json";
const LEGACY_BASE_RECORD: &str = "base.json";
const BASE_VALIDATION_RECORD: &str = ".perfectsync-base-validation.json";
const BASE_VALIDATION_SCHEMA: u32 = 3;
const SOURCE_RECORD_SCHEMA: u32 = 6;
const PROFILE_REVISION_RECORD: &str = ".perfectsync-profile-revision.json";
const PROFILE_REVISION_SCHEMA: u32 = 1;
const WORKSPACE_VALIDATION_RECORD: &str = ".perfectsync-workspace-validation.json";
const WORKSPACE_VALIDATION_SCHEMA: u32 = 1;
const SCHEMA: u32 = 6;
const MAX_FILES: usize = 200_000;
const MAX_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_RECORD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SOURCE_RECORDS: usize = 4_096;
const MAX_CONFIG_FILES: usize = 4_096;
const MAX_CONFIG_BYTES: u64 = 128 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
static SERIAL: AtomicU64 = AtomicU64::new(0);
static VALIDATED_BASES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
static SOURCE_MUTATION_LOCK: Mutex<()> = Mutex::new(());

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

const SPLASH_SCREEN_ORIGINAL: &str =
    "BepInEx/patchers/BepInEx.SplashScreen/BepInEx.SplashScreen.GUI.exe";
const SPLASH_SCREEN_RUNTIME_NAME: &str =
    "BepInEx/patchers/BepInEx.SplashScreen/Among Us.SplashScreen.GUI.exe";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyBaseRecord {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRecord {
    pub schema: u32,
    pub fingerprint: String,
    pub game_instance_id: String,
    pub path: String,
    pub arch: Arch,
    pub store: Store,
    pub runtime: Runtime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_build: Option<String>,
    pub file_count: u64,
    pub byte_count: u64,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BaseValidationRecord {
    schema: u32,
    base_id: String,
    manifest_sha256: String,
    metadata_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileRevisionRecord {
    schema: u32,
    metadata_fingerprint: String,
    revision: String,
    material_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceValidationRecord {
    schema: u32,
    marker_sha256: String,
    metadata_fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct ManagedSource {
    pub record: SourceRecord,
    pub source_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct LegacyBase {
    record: LegacyBaseRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceMarker {
    pub schema: u32,
    pub source_record_id: String,
    pub source_fingerprint: String,
    pub game_instance_id: String,
    pub profile_id: String,
    pub profile_revision: String,
    pub material_revision: String,
    pub managed_files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyWorkspaceMarker {
    schema: u32,
    base_id: String,
    base_manifest_sha256: String,
    game_instance_id: String,
    profile_id: String,
    profile_revision: String,
    #[serde(default)]
    material_revision: Option<String>,
    managed_files: Vec<ManifestFile>,
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
fn file_identity(metadata: &fs::Metadata) -> (u64, u64) {
    use std::os::windows::fs::MetadataExt;
    (metadata.creation_time(), 0)
}

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(any(windows, unix)))]
fn file_identity(_metadata: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

fn managed_root() -> PathBuf {
    settings::managed_data_dir().join("managed-games")
}

fn bases_root() -> PathBuf {
    managed_root().join("bases")
}

fn sources_root() -> PathBuf {
    managed_root().join("sources")
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
    let Some(marker) = read_json::<serde_json::Value>(&legacy.join(INSTANCE_MARKER))? else {
        return Ok(());
    };
    let marker_profile = marker
        .get("profileId")
        .and_then(serde_json::Value::as_str)
        .ok_or("legacy workspace marker has no profile identity")?;
    if marker_profile != workspace_id && marker_profile != "_vanilla" {
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
        "Cannot create an isolated workspace because the selected source contains mod-loader artifacts: {}{suffix}. Perfect Sync did not change the source. Select a separate vanilla Among Us folder; this one can remain modded.",
        shown.join(", ")
    ))
}

fn copy_file_hashed(source: &Path, destination: &Path) -> Result<ManifestFile, String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "{} is not a regular non-link file",
            source.display()
        ));
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
            let mut copied = copy_file_hashed(&source_path, &target)?;
            copied.path = safe_relative(&relative)?;
            files.push(copied);
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn base_validation_key(root: &Path, record: &LegacyBaseRecord) -> String {
    format!(
        "{}\0{}",
        normalized_path(root),
        record.manifest_sha256.to_ascii_lowercase()
    )
}

fn base_metadata_fingerprint(root: &Path, record: &LegacyBaseRecord) -> Result<String, String> {
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

fn base_is_validated(
    root: &Path,
    record: &LegacyBaseRecord,
    fingerprint: &str,
) -> Result<bool, String> {
    let key = base_validation_key(root, record);
    let validated = VALIDATED_BASES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "managed base validation cache is poisoned".to_string())?
        .get(&key)
        .is_some_and(|cached| cached == fingerprint);
    if validated {
        return Ok(true);
    }

    let persisted = read_json::<BaseValidationRecord>(&root.join(BASE_VALIDATION_RECORD)).ok();
    let validated = persisted.flatten().is_some_and(|cached| {
        cached.schema == BASE_VALIDATION_SCHEMA
            && cached.base_id == record.id
            && cached
                .manifest_sha256
                .eq_ignore_ascii_case(&record.manifest_sha256)
            && cached.metadata_fingerprint == fingerprint
    });
    if validated {
        VALIDATED_BASES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|_| "managed base validation cache is poisoned".to_string())?
            .insert(key, fingerprint.to_string());
    }
    Ok(validated)
}

fn mark_base_validated(root: &Path, record: &LegacyBaseRecord) -> Result<(), String> {
    let fingerprint = base_metadata_fingerprint(root, record)?;
    write_json(
        &root.join(BASE_VALIDATION_RECORD),
        &BaseValidationRecord {
            schema: BASE_VALIDATION_SCHEMA,
            base_id: record.id.clone(),
            manifest_sha256: record.manifest_sha256.clone(),
            metadata_fingerprint: fingerprint.clone(),
        },
    )?;
    VALIDATED_BASES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "managed base validation cache is poisoned".to_string())?
        .insert(base_validation_key(root, record), fingerprint);
    Ok(())
}

fn validate_base_files(root: &Path, record: &LegacyBaseRecord) -> Result<(), String> {
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
    mark_base_validated(root, record)
}

fn manifest_digest(files: &[ManifestFile]) -> Result<String, String> {
    let bytes = serde_json::to_vec(files).map_err(|error| error.to_string())?;
    Ok(sha256_bytes(&bytes))
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
    AtomicFile::new(path, AllowOverwrite)
        .write(|output| {
            output.write_all(&bytes)?;
            output.flush()?;
            output.sync_all()
        })
        .map_err(|error| error.to_string())
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
fn reject_source_storage_overlap(source: &Path, storage: &Path) -> Result<(), String> {
    let storage = fs::canonicalize(storage)
        .map_err(|error| format!("could not resolve managed storage: {error}"))?;
    let metadata = fs::symlink_metadata(&storage)
        .map_err(|error| format!("could not inspect managed storage: {error}"))?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("managed storage must be a regular non-link directory".into());
    }
    if source.starts_with(&storage) || storage.starts_with(source) {
        return Err(
            "Managed storage cannot contain an Among Us source or be placed inside one".into(),
        );
    }
    Ok(())
}

fn instance_slot(id: &str) -> String {
    let digest = sha256_bytes(id.as_bytes());
    digest[..32].to_string()
}

fn source_record_id_parts(fingerprint: &str, path: &str) -> String {
    sha256_bytes(format!("{fingerprint}\0{path}").as_bytes())
}

fn source_record_id(record: &SourceRecord) -> String {
    source_record_id_parts(&record.fingerprint, &record.path)
}

fn legacy_source_record_path(instance_id: &str) -> PathBuf {
    sources_root().join(format!("{}.json", instance_slot(instance_id)))
}

fn source_records_root() -> PathBuf {
    sources_root().join("records")
}

fn instance_source_records_root(instance_id: &str) -> PathBuf {
    source_records_root().join(instance_slot(instance_id))
}

fn source_record_path(instance_id: &str, record_id: &str) -> Result<PathBuf, String> {
    if record_id.len() != 64 || !record_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("managed source record identity is invalid".into());
    }
    Ok(instance_source_records_root(instance_id).join(format!("{record_id}.json")))
}

fn validate_source_record(record: &SourceRecord) -> Result<(), String> {
    if record.schema != SOURCE_RECORD_SCHEMA
        || record.fingerprint.len() != 64
        || !record
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || record.game_instance_id.trim().is_empty()
        || record.path.trim().is_empty()
        || !Path::new(&record.path).is_absolute()
        || record.files.is_empty()
        || record.files.len() > MAX_FILES
        || record.file_count != record.files.len() as u64
        || record.byte_count
            != record
                .files
                .iter()
                .try_fold(0_u64, |total, file| {
                    total
                        .checked_add(file.size)
                        .filter(|bytes| *bytes <= MAX_BYTES)
                })
                .ok_or("source record exceeds the managed storage limit")?
        || manifest_digest(&record.files)? != record.fingerprint
    {
        return Err("Among Us source record is invalid".into());
    }
    Ok(())
}

fn persist_source_record(record: &SourceRecord) -> Result<String, String> {
    validate_source_record(record)?;
    let record_id = source_record_id(record);
    let path = source_record_path(&record.game_instance_id, &record_id)?;
    if let Some(existing) = read_json::<SourceRecord>(&path)? {
        validate_source_record(&existing)?;
        if serde_json::to_vec(&existing).map_err(|error| error.to_string())?
            != serde_json::to_vec(record).map_err(|error| error.to_string())?
        {
            return Err("immutable managed source record conflicts with existing metadata".into());
        }
    } else {
        write_json(&path, record)?;
    }
    Ok(record_id)
}

fn migrate_legacy_source_record(instance_id: &str) -> Result<(), String> {
    let path = legacy_source_record_path(instance_id);
    let Some(record) = read_json::<SourceRecord>(&path)? else {
        return Ok(());
    };
    if record.game_instance_id != instance_id {
        return Err("legacy managed source record has the wrong instance identity".into());
    }
    persist_source_record(&record)?;
    fs::remove_file(path).map_err(|error| error.to_string())
}

fn load_instance_source_records(instance_id: &str) -> Result<Vec<SourceRecord>, String> {
    migrate_legacy_source_record(instance_id)?;
    let root = instance_source_records_root(instance_id);
    let entries = match sorted_entries(&root) {
        Ok(entries) => entries,
        Err(_) if !root.exists() => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    if entries.len() > MAX_SOURCE_RECORDS {
        return Err("managed source history contains too many records".into());
    }
    let mut records = Vec::with_capacity(entries.len());
    for entry in entries {
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) || !metadata.is_file() {
            return Err("managed source history contains an unsafe entry".into());
        }
        let Some(record) = read_json::<SourceRecord>(&entry.path())? else {
            continue;
        };
        validate_source_record(&record)?;
        if record.game_instance_id != instance_id
            || entry.file_name().to_string_lossy() != format!("{}.json", source_record_id(&record))
        {
            return Err("managed source history record identity is invalid".into());
        }
        records.push(record);
    }
    Ok(records)
}

fn source_matches_instance(record: &SourceRecord, instance: &GameInstance) -> bool {
    record.game_instance_id == instance.id
        && Path::new(&record.path) == Path::new(&instance.path)
        && record.arch == instance.arch
        && record.store == instance.store
        && record.runtime == instance.runtime
        && (instance.build.is_none() || record.observed_build == instance.build)
        && instance
            .source_fingerprint
            .as_deref()
            .is_none_or(|fingerprint| fingerprint.eq_ignore_ascii_case(&record.fingerprint))
        && instance
            .source_file_count
            .is_none_or(|count| count == record.file_count)
        && instance
            .source_byte_count
            .is_none_or(|bytes| bytes == record.byte_count)
}

fn managed_source(record: SourceRecord) -> ManagedSource {
    ManagedSource {
        source_dir: PathBuf::from(&record.path),
        record,
    }
}

fn load_source_record(instance: &GameInstance) -> Result<Option<ManagedSource>, String> {
    let records = load_instance_source_records(&instance.id)?;
    let selected = if let Some(fingerprint) = instance.source_fingerprint.as_deref() {
        let record_id = source_record_id_parts(fingerprint, &instance.path);
        records
            .into_iter()
            .find(|record| source_record_id(record) == record_id)
    } else {
        let mut matching = records
            .into_iter()
            .filter(|record| source_matches_instance(record, instance));
        let first = matching.next();
        if matching.next().is_some() {
            None
        } else {
            first
        }
    };
    let Some(record) = selected else {
        return Ok(None);
    };
    if !source_matches_instance(&record, instance) {
        return Err(
            "The selected Among Us source no longer matches its saved source record. Save the source again."
                .into(),
        );
    }
    Ok(Some(managed_source(record)))
}

fn load_marker_source(marker: &WorkspaceMarker) -> Result<ManagedSource, String> {
    migrate_legacy_source_record(&marker.game_instance_id)?;
    let path = source_record_path(&marker.game_instance_id, &marker.source_record_id)?;
    let record =
        read_json::<SourceRecord>(&path)?.ok_or("active workspace source record is missing")?;
    validate_source_record(&record)?;
    if source_record_id(&record) != marker.source_record_id
        || record.fingerprint != marker.source_fingerprint
        || record.game_instance_id != marker.game_instance_id
    {
        return Err("active workspace source record does not match its marker".into());
    }
    Ok(managed_source(record))
}

pub fn saved_source(instance: &GameInstance) -> Result<Option<ManagedSource>, String> {
    load_source_record(instance)
}

pub fn record_source(instance: &GameInstance) -> Result<ManagedSource, String> {
    let _guard = SOURCE_MUTATION_LOCK
        .lock()
        .map_err(|_| "managed source lock is poisoned".to_string())?;
    if instance.id.trim().is_empty() {
        return Err("game instance has no identity".into());
    }
    let source = fs::canonicalize(Path::new(&instance.path))
        .map_err(|error| format!("The selected Among Us source is unavailable: {error}"))?;
    if source != Path::new(&instance.path) {
        return Err("The saved Among Us source path is not canonical".into());
    }
    let metadata = fs::symlink_metadata(&source).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("Among Us source must be a regular non-link directory".into());
    }
    fs::create_dir_all(managed_root()).map_err(|error| error.to_string())?;
    reject_source_storage_overlap(&source, &settings::managed_data_dir())?;
    require_exact_source(&source)?;
    let files = collect_tree(&source, false)?;
    if !files
        .iter()
        .any(|file| file.path.eq_ignore_ascii_case("Among Us.exe"))
    {
        return Err("Among Us source did not produce an executable".into());
    }
    let fingerprint = manifest_digest(&files)?;
    let byte_count = files
        .iter()
        .try_fold(0_u64, |total, file| {
            total
                .checked_add(file.size)
                .filter(|bytes| *bytes <= MAX_BYTES)
        })
        .ok_or("Among Us source exceeds the managed storage limit")?;
    let record = SourceRecord {
        schema: SOURCE_RECORD_SCHEMA,
        fingerprint,
        game_instance_id: instance.id.clone(),
        path: source.to_string_lossy().into_owned(),
        arch: instance.arch,
        store: instance.store,
        runtime: instance.runtime,
        observed_build: instance.build.clone(),
        file_count: files.len() as u64,
        byte_count,
        files,
    };
    require_exact_source(&source)?;
    verify_source_fingerprint(&source, &record)?;
    persist_source_record(&record)?;
    Ok(managed_source(record))
}

pub fn rebind_source_record(
    instance: &GameInstance,
    new_path: &Path,
) -> Result<Option<ManagedSource>, String> {
    let _guard = SOURCE_MUTATION_LOCK
        .lock()
        .map_err(|_| "managed source lock is poisoned".to_string())?;
    let Some(existing) = load_source_record(instance)? else {
        return Ok(None);
    };
    let canonical = fs::canonicalize(new_path)
        .map_err(|error| format!("The moved Among Us source is unavailable: {error}"))?;
    if canonical != new_path {
        return Err("The moved Among Us source path is not canonical".into());
    }
    let metadata = fs::symlink_metadata(&canonical).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("Moved Among Us source must be a regular non-link directory".into());
    }
    reject_source_storage_overlap(&canonical, &settings::managed_data_dir())?;
    require_exact_source(&canonical)?;
    verify_source_fingerprint(&canonical, &existing.record)?;
    let mut record = existing.record;
    record.path = canonical.to_string_lossy().into_owned();
    persist_source_record(&record)?;
    Ok(Some(managed_source(record)))
}

pub fn source_for_rebuild(
    instance: &GameInstance,
    preferred_build: Option<&str>,
) -> Result<ManagedSource, String> {
    let source = load_source_record(instance)?.ok_or(
        "The selected Among Us source has no complete fingerprint record. Save the source again.",
    )?;
    if preferred_build.is_some_and(|build| source.record.observed_build.as_deref() != Some(build)) {
        return Err(format!(
            "Profile requires Among Us build {}, but the selected source record is build {}",
            preferred_build.unwrap_or_default(),
            source.record.observed_build.as_deref().unwrap_or("unknown")
        ));
    }
    Ok(source)
}

fn observed_build_matches(record: &SourceRecord, observed: Option<&str>) -> bool {
    record.observed_build.as_deref() == observed
}

pub fn ensure_source_build_allows_launch(source: &ManagedSource) -> Result<(), String> {
    let Ok(canonical) = fs::canonicalize(&source.record.path) else {
        return Ok(());
    };
    if canonical != Path::new(&source.record.path) {
        return Err("The selected Among Us source path now resolves to a different folder".into());
    }
    if !observed_build_matches(
        &source.record,
        perfect_sync_core::game::detect_build(&canonical).as_deref(),
    ) {
        return Err(
            "The selected Among Us source build has changed. Save the source again before launching."
                .into(),
        );
    }
    Ok(())
}

pub fn cached_active_source(
    instance: &GameInstance,
    preferred_build: Option<&str>,
    profile_id: &str,
    revision: &str,
    material_revision: &str,
    workspace_id: &str,
) -> Result<Option<ManagedSource>, String> {
    let started = Instant::now();
    if !valid_profile_revision(revision) || !valid_profile_revision(material_revision) {
        return Err("profile revision is invalid".into());
    }
    let Some(marker) = active_marker(workspace_id)? else {
        return Ok(None);
    };
    if marker.game_instance_id != instance.id
        || marker.profile_id != profile_id
        || marker.profile_revision != revision
        || marker.material_revision != material_revision
    {
        return Ok(None);
    }
    let source = load_marker_source(&marker)?;
    if preferred_build.is_some_and(|build| source.record.observed_build.as_deref() != Some(build))
        || !active_matches(
            &source,
            profile_id,
            revision,
            material_revision,
            workspace_id,
        )?
    {
        return Ok(None);
    }
    ensure_source_build_allows_launch(&source)?;
    log::info!(
        target: "perfect_sync::performance",
        "cached_active_source completed in {} ms ({} files)",
        started.elapsed().as_millis(),
        source.record.file_count
    );
    Ok(Some(source))
}

fn exact_source_path(source: &ManagedSource) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(&source.record.path).map_err(|error| {
        format!(
            "The selected Among Us source is unavailable. Restore access to it or save the source again: {error}"
        )
    })?;
    if canonical != Path::new(&source.record.path) {
        return Err("The selected Among Us source path now resolves to a different folder".into());
    }
    let metadata = fs::symlink_metadata(&canonical).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("Among Us source must be a regular non-link directory".into());
    }
    reject_source_storage_overlap(&canonical, &settings::managed_data_dir())?;
    require_exact_source(&canonical)?;
    Ok(canonical)
}

fn verify_source_fingerprint(source: &Path, record: &SourceRecord) -> Result<(), String> {
    let files = collect_tree(source, false)?;
    let bytes = files
        .iter()
        .try_fold(0_u64, |total, file| {
            total
                .checked_add(file.size)
                .filter(|bytes| *bytes <= MAX_BYTES)
        })
        .ok_or("Among Us source exceeds the managed storage limit")?;
    if files.len() as u64 != record.file_count
        || bytes != record.byte_count
        || manifest_digest(&files)? != record.fingerprint
        || files != record.files
    {
        return Err(
            "The selected Among Us source fingerprint changed. Save the source again before rebuilding."
                .into(),
        );
    }
    Ok(())
}

pub fn ensure_exact_source_available(source: &ManagedSource) -> Result<(), String> {
    let source_path = exact_source_path(source)?;
    verify_source_fingerprint(&source_path, &source.record)
}

pub fn begin_workspace(source: &ManagedSource, workspace_id: &str) -> Result<PathBuf, String> {
    let _guard = SOURCE_MUTATION_LOCK
        .lock()
        .map_err(|_| "managed source lock is poisoned".to_string())?;
    let source_dir = exact_source_path(source)?;
    let workspace = workspace_root(workspace_id)?;
    fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    let active = workspace_game_dir(workspace_id)?;
    recover_destination(&active)?;
    remove_prefixed_children(&workspace, ".stage.")?;
    let stage = unique_child(&workspace, "stage");
    let result = (|| {
        let copied_files = copy_source_tree(&source_dir, &stage)?;
        let copied_bytes = copied_files
            .iter()
            .try_fold(0_u64, |total, file| {
                total
                    .checked_add(file.size)
                    .filter(|bytes| *bytes <= MAX_BYTES)
            })
            .ok_or("copied workspace exceeds the managed storage limit")?;
        if copied_files != source.record.files
            || copied_files.len() as u64 != source.record.file_count
            || copied_bytes != source.record.byte_count
            || manifest_digest(&copied_files)? != source.record.fingerprint
        {
            return Err("The copied Among Us files do not match the selected source record".into());
        }
        verify_source_fingerprint(&stage, &source.record)
            .map_err(|_| "The staged Among Us copy failed fingerprint verification".to_string())?;
        verify_source_fingerprint(&source_dir, &source.record)?;
        Ok(())
    })();
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

fn managed_delta(
    source_record: &SourceRecord,
    stage: &Path,
    verify_source_contents: bool,
) -> Result<Vec<ManifestFile>, String> {
    let metadata = fs::symlink_metadata(stage).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("managed workspace stage is not a regular directory".into());
    }
    let source_files: HashMap<String, &ManifestFile> = source_record
        .files
        .iter()
        .map(|file| (file.path.to_ascii_lowercase(), file))
        .collect();
    let mut seen_source = HashSet::with_capacity(source_files.len());
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
            if let Some(expected) = source_files.get(&key) {
                seen_source.insert(key);
                if metadata.len() != expected.size
                    || verify_source_contents
                        && sha256_file(&source, Some(expected.size))? != expected.sha256
                {
                    return Err(format!(
                        "source file was replaced while building the workspace: {}",
                        expected.path
                    ));
                }
                continue;
            }
            managed.push(ManifestFile {
                path: portable,
                size: metadata.len(),
                sha256: sha256_file(&source, Some(metadata.len()))?,
            });
        }
    }
    if seen_source.len() != source_files.len() {
        return Err("managed workspace is missing source files".into());
    }
    managed.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(managed)
}

pub fn publish_workspace(
    stage: &Path,
    source: &ManagedSource,
    profile_id: &str,
    profile_revision: &str,
    material_revision: &str,
    workspace_id: &str,
) -> Result<PathBuf, String> {
    profile::validate_profile_id(profile_id).map_err(|error| error.to_string())?;
    profile::validate_profile_id(workspace_id).map_err(|error| error.to_string())?;
    if !valid_profile_revision(profile_revision) || !valid_profile_revision(material_revision) {
        return Err("profile revision is invalid".into());
    }
    let expected_parent = workspace_root(workspace_id)?;
    if stage.parent() != Some(expected_parent.as_path()) {
        return Err("workspace stage does not belong to the requested profile".into());
    }
    let managed_files = managed_delta(&source.record, stage, true)?;
    let marker = WorkspaceMarker {
        schema: SCHEMA,
        source_record_id: source_record_id(&source.record),
        source_fingerprint: source.record.fingerprint.clone(),
        game_instance_id: source.record.game_instance_id.clone(),
        profile_id: profile_id.to_string(),
        profile_revision: profile_revision.to_string(),
        material_revision: material_revision.to_string(),
        managed_files,
    };
    write_json(&stage.join(INSTANCE_MARKER), &marker)?;
    let active = workspace_game_dir(workspace_id)?;
    publish_directory(stage, &active)?;
    if let Err(error) = persist_workspace_validation(source, &active, &marker, workspace_id) {
        log::warn!("could not cache published workspace validation: {error}");
    }
    Ok(active)
}

pub fn refresh_workspace_marker(
    source: &ManagedSource,
    profile_id: &str,
    profile_revision: &str,
    material_revision: &str,
    workspace_id: &str,
) -> Result<PathBuf, String> {
    profile::validate_profile_id(profile_id).map_err(|error| error.to_string())?;
    profile::validate_profile_id(workspace_id).map_err(|error| error.to_string())?;
    if !valid_profile_revision(profile_revision) || !valid_profile_revision(material_revision) {
        return Err("profile revision is invalid".into());
    }
    let active = workspace_game_dir(workspace_id)?;
    let managed_files = managed_delta(&source.record, &active, false)?;
    let marker = WorkspaceMarker {
        schema: SCHEMA,
        source_record_id: source_record_id(&source.record),
        source_fingerprint: source.record.fingerprint.clone(),
        game_instance_id: source.record.game_instance_id.clone(),
        profile_id: profile_id.to_string(),
        profile_revision: profile_revision.to_string(),
        material_revision: material_revision.to_string(),
        managed_files,
    };
    write_json(&active.join(INSTANCE_MARKER), &marker)?;
    if let Err(error) = persist_workspace_validation(source, &active, &marker, workspace_id) {
        log::warn!("could not cache updated workspace validation: {error}");
    }
    Ok(active)
}
fn load_legacy_base_at(root: &Path) -> Result<Option<LegacyBase>, String> {
    let Some(record) = read_json::<LegacyBaseRecord>(&root.join(LEGACY_BASE_RECORD))? else {
        return Ok(None);
    };
    if !(1..=5).contains(&record.schema)
        || !record.exact_source_snapshot
        || record.id.len() != 64
        || !record.id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || record.files.is_empty()
        || record.files.len() > MAX_FILES
        || manifest_digest(&record.files)? != record.manifest_sha256
    {
        return Err("legacy immutable base record is invalid".into());
    }
    validate_base_files(root, &record)?;
    Ok(Some(LegacyBase { record }))
}

fn legacy_base_for_marker(marker: &LegacyWorkspaceMarker) -> Result<LegacyBase, String> {
    let container = bases_root().join(instance_slot(&marker.game_instance_id));
    let generation = container.join("versions").join(&marker.base_id);
    let legacy = load_legacy_base_at(&generation)?
        .or(load_legacy_base_at(&container)?)
        .ok_or("legacy workspace has no verified base migration evidence")?;
    if legacy.record.id != marker.base_id
        || legacy.record.manifest_sha256 != marker.base_manifest_sha256
        || legacy.record.game_instance_id != marker.game_instance_id
    {
        return Err("legacy workspace base evidence does not match its marker".into());
    }
    Ok(legacy)
}

fn source_record_from_legacy(base: &LegacyBase) -> Result<SourceRecord, String> {
    if !base.record.exact_source_snapshot
        || manifest_digest(&base.record.files)? != base.record.manifest_sha256
    {
        return Err("legacy base is not complete source migration evidence".into());
    }
    let byte_count = base
        .record
        .files
        .iter()
        .try_fold(0_u64, |total, file| {
            total
                .checked_add(file.size)
                .filter(|bytes| *bytes <= MAX_BYTES)
        })
        .ok_or("legacy source record exceeds the managed storage limit")?;
    let record = SourceRecord {
        schema: SOURCE_RECORD_SCHEMA,
        fingerprint: base.record.manifest_sha256.clone(),
        game_instance_id: base.record.game_instance_id.clone(),
        path: base.record.source_path.clone(),
        arch: base.record.arch,
        store: base.record.store,
        runtime: base.record.runtime,
        observed_build: base.record.build.clone(),
        file_count: base.record.files.len() as u64,
        byte_count,
        files: base.record.files.clone(),
    };
    validate_source_record(&record)?;
    Ok(record)
}

fn validate_legacy_workspace(
    active: &Path,
    marker: &LegacyWorkspaceMarker,
    base: &LegacyBase,
) -> Result<(), String> {
    for expected in &base.record.files {
        let path = active.join(relative_path(&expected.path)?);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| "legacy workspace is missing a verified base file".to_string())?;
        if is_reparse(&metadata)
            || !metadata.is_file()
            || metadata.len() != expected.size
            || sha256_file(&path, Some(expected.size))? != expected.sha256
        {
            return Err(format!(
                "legacy workspace source file does not match its verified base: {}",
                expected.path
            ));
        }
    }
    let mut managed_names = HashSet::with_capacity(marker.managed_files.len());
    for file in &marker.managed_files {
        if verified_managed_path(active, file)?.is_none() {
            return Err("legacy workspace managed files do not match its marker".into());
        }
        managed_names.insert(file.path.to_ascii_lowercase());
    }
    for path in protected_files(active)? {
        if !managed_names.contains(&path.to_ascii_lowercase()) {
            return Err("legacy workspace contains an unrecorded protected file".into());
        }
    }
    Ok(())
}

fn migrate_legacy_marker(
    active: &Path,
    marker: LegacyWorkspaceMarker,
) -> Result<WorkspaceMarker, String> {
    if !(1..=5).contains(&marker.schema)
        || !valid_profile_revision(&marker.profile_revision)
        || marker.managed_files.len() > MAX_FILES
    {
        return Err("legacy active workspace marker is invalid".into());
    }
    profile::validate_profile_id(&marker.profile_id).map_err(|error| error.to_string())?;
    let base = legacy_base_for_marker(&marker)?;
    validate_legacy_workspace(active, &marker, &base)?;
    let record = source_record_from_legacy(&base)?;
    let record_id = persist_source_record(&record)?;
    let material_revision = marker.material_revision.unwrap_or_else(|| {
        if marker.profile_id == "_vanilla" {
            marker.profile_revision.clone()
        } else {
            profile_revisions(&settings::profiles_root().join(&marker.profile_id))
                .map(|(_, material)| material)
                .unwrap_or_else(|_| marker.profile_revision.clone())
        }
    });
    let migrated = WorkspaceMarker {
        schema: SCHEMA,
        source_record_id: record_id,
        source_fingerprint: record.fingerprint,
        game_instance_id: marker.game_instance_id,
        profile_id: marker.profile_id,
        profile_revision: marker.profile_revision,
        material_revision,
        managed_files: marker.managed_files,
    };
    write_json(&active.join(INSTANCE_MARKER), &migrated)?;
    Ok(migrated)
}

fn read_active_marker(active: &Path) -> Result<Option<WorkspaceMarker>, String> {
    let _guard = SOURCE_MUTATION_LOCK
        .lock()
        .map_err(|_| "managed source lock is poisoned".to_string())?;
    let marker_path = active.join(INSTANCE_MARKER);
    let Some(value) = read_json::<serde_json::Value>(&marker_path)? else {
        return Ok(None);
    };
    if value.get("schema").and_then(serde_json::Value::as_u64) == Some(SCHEMA as u64) {
        return serde_json::from_value(value)
            .map(Some)
            .map_err(|error| format!("could not read {}: {error}", marker_path.display()));
    }
    let legacy = serde_json::from_value::<LegacyWorkspaceMarker>(value)
        .map_err(|error| format!("could not read {}: {error}", marker_path.display()))?;
    migrate_legacy_marker(active, legacy).map(Some)
}

fn legacy_bases() -> Result<Vec<LegacyBase>, String> {
    let root = bases_root();
    let containers = match sorted_entries(&root) {
        Ok(entries) => entries,
        Err(_) if !root.exists() => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut bases = Vec::new();
    for container in containers {
        let metadata = fs::symlink_metadata(container.path()).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) || !metadata.is_dir() {
            return Err("obsolete base storage contains an unsafe entry".into());
        }
        if let Some(base) = load_legacy_base_at(&container.path())? {
            bases.push(base);
        }
        let versions = container.path().join("versions");
        let generations = match sorted_entries(&versions) {
            Ok(entries) => entries,
            Err(_) if !versions.exists() => continue,
            Err(error) => return Err(error),
        };
        for generation in generations {
            let metadata =
                fs::symlink_metadata(generation.path()).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) || !metadata.is_dir() {
                return Err("obsolete base generations contain an unsafe entry".into());
            }
            if let Some(base) = load_legacy_base_at(&generation.path())? {
                bases.push(base);
            }
        }
    }
    Ok(bases)
}

fn cleanup_obsolete_bases(sources: &[SourceRecord]) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(bases_root()) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("obsolete base storage is not a regular non-link directory".into());
    }
    let managed_metadata = fs::symlink_metadata(managed_root())
        .map_err(|error| format!("could not inspect managed storage: {error}"))?;
    if is_reparse(&managed_metadata) || !managed_metadata.is_dir() {
        return Err("managed storage is not a regular non-link directory".into());
    }
    let managed_lexical = managed_root();
    let bases_lexical = bases_root();
    let managed = fs::canonicalize(&managed_lexical)
        .map_err(|error| format!("could not resolve managed storage: {error}"))?;
    let bases = fs::canonicalize(&bases_lexical)
        .map_err(|error| format!("could not resolve obsolete base storage: {error}"))?;
    if !bases.starts_with(&managed) || bases == managed {
        return Err("obsolete base storage failed canonical containment checks".into());
    }

    let mut verified_paths = Vec::new();
    let mut verified_records = HashSet::new();
    for source in sources {
        validate_source_record(source)?;
        if !verified_records.insert(source_record_id(source)) {
            continue;
        }
        let lexical = Path::new(&source.path);
        if lexical.starts_with(&managed_lexical)
            || managed_lexical.starts_with(lexical)
            || lexical.starts_with(&bases_lexical)
            || bases_lexical.starts_with(lexical)
        {
            return Err("obsolete base storage failed lexical source containment checks".into());
        }
        let canonical = match fs::canonicalize(lexical) {
            Ok(path) => path,
            Err(_) => return Ok(false),
        };
        let source_metadata =
            fs::symlink_metadata(&canonical).map_err(|error| error.to_string())?;
        if is_reparse(&source_metadata) || !source_metadata.is_dir() {
            return Ok(false);
        }
        if canonical != lexical
            || canonical.starts_with(&managed)
            || managed.starts_with(&canonical)
            || canonical.starts_with(&bases)
            || bases.starts_with(&canonical)
        {
            return Err("obsolete base storage failed canonical source containment checks".into());
        }
        if verify_source_fingerprint(&canonical, source).is_err() {
            return Ok(false);
        }
        verified_paths.push(canonical);
    }
    for (index, left) in verified_paths.iter().enumerate() {
        for right in &verified_paths[index + 1..] {
            if left != right && (left.starts_with(right) || right.starts_with(left)) {
                return Err("recorded Among Us source paths overlap".into());
            }
        }
    }
    remove_tree(&bases)?;
    Ok(true)
}

fn marker_schema(active: &Path) -> Result<Option<u32>, String> {
    Ok(
        read_json::<serde_json::Value>(&active.join(INSTANCE_MARKER))?
            .and_then(|value| value.get("schema").and_then(serde_json::Value::as_u64))
            .and_then(|schema| u32::try_from(schema).ok()),
    )
}

fn migrate_global_profile_workspace() -> Result<bool, String> {
    let global = workspaces_root().join("current");
    recover_destination(&global)?;
    if !global.exists() {
        return Ok(false);
    }
    let value = read_json::<serde_json::Value>(&global.join(INSTANCE_MARKER))?
        .ok_or("legacy global workspace has no marker")?;
    let profile_id = value
        .get("profileId")
        .and_then(serde_json::Value::as_str)
        .ok_or("legacy global workspace marker has no profile identity")?;
    if profile_id == "_vanilla" {
        return Ok(true);
    }
    profile::validate_profile_id(profile_id).map_err(|error| error.to_string())?;
    let destination_root = workspace_root(profile_id)?;
    let destination = destination_root.join("current");
    if destination.exists() {
        return Ok(true);
    }
    fs::create_dir_all(&destination_root).map_err(|error| error.to_string())?;
    fs::rename(&global, &destination)
        .map_err(|error| format!("could not migrate the global managed workspace: {error}"))?;
    Ok(false)
}

pub fn migrate_direct_source_storage() -> Result<(), String> {
    let mut legacy_marker_remains = migrate_global_profile_workspace()?;
    let root = workspaces_root();
    let entries = match sorted_entries(&root) {
        Ok(entries) => entries,
        Err(_) if !root.exists() => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut migrated_sources = Vec::new();
    for entry in entries {
        if entry.file_name().to_string_lossy() == "current" {
            continue;
        }
        let workspace = entry.path();
        let metadata = fs::symlink_metadata(&workspace).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) || !metadata.is_dir() {
            return Err("managed workspace root contains an unsafe entry".into());
        }
        let active = workspace.join("current");
        recover_destination(&active)?;
        if !active.exists() {
            continue;
        }
        if let Some(marker) = read_active_marker(&active)? {
            let source = load_marker_source(&marker)?;
            migrated_sources.push(source.record);
        }
        if marker_schema(&active)?.is_some_and(|schema| schema != SCHEMA) {
            legacy_marker_remains = true;
        }
    }
    if workspaces_root().join("current").exists() {
        legacy_marker_remains = true;
    }
    if legacy_marker_remains {
        return Ok(());
    }

    let legacy = legacy_bases()?;
    for base in legacy {
        let source = source_record_from_legacy(&base)?;
        if !migrated_sources
            .iter()
            .any(|record| source_record_id(record) == source_record_id(&source))
        {
            return Ok(());
        }
    }
    let _ = cleanup_obsolete_bases(&migrated_sources)?;
    Ok(())
}

pub fn active_marker(workspace_id: &str) -> Result<Option<WorkspaceMarker>, String> {
    let active = workspace_game_dir(workspace_id)?;
    recover_destination(&active)?;
    let Some(marker) = read_active_marker(&active)? else {
        return Ok(None);
    };
    if marker.schema != SCHEMA
        || marker.source_fingerprint.len() != 64
        || marker.source_record_id.len() != 64
        || !marker
            .source_record_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || !valid_profile_revision(&marker.profile_revision)
        || !valid_profile_revision(&marker.material_revision)
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
fn valid_profile_revision(revision: &str) -> bool {
    revision.len() == 64 && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn refresh_active_profile_revision(
    workspace_id: &str,
    profile_id: &str,
    expected_revision: &str,
    revision: &str,
) -> Result<bool, String> {
    profile::validate_profile_id(profile_id).map_err(|error| error.to_string())?;
    if !valid_profile_revision(expected_revision) || !valid_profile_revision(revision) {
        return Err("profile revision is invalid".into());
    }
    let Some(mut marker) = active_marker(workspace_id)? else {
        return Ok(false);
    };
    if marker.profile_id != profile_id || marker.profile_revision != expected_revision {
        return Ok(false);
    }
    let old_marker_sha256 =
        sha256_bytes(&serde_json::to_vec(&marker).map_err(|error| error.to_string())?);
    let validation_path = workspace_root(workspace_id)?.join(WORKSPACE_VALIDATION_RECORD);
    let validation = read_json::<WorkspaceValidationRecord>(&validation_path)
        .ok()
        .flatten();
    marker.profile_revision = revision.to_string();
    write_json(
        &workspace_game_dir(workspace_id)?.join(INSTANCE_MARKER),
        &marker,
    )?;
    if let Some(mut validation) = validation.filter(|record| {
        record.schema == WORKSPACE_VALIDATION_SCHEMA && record.marker_sha256 == old_marker_sha256
    }) {
        validation.marker_sha256 =
            sha256_bytes(&serde_json::to_vec(&marker).map_err(|error| error.to_string())?);
        write_json(&validation_path, &validation)?;
    }
    Ok(true)
}

fn existing_managed_path(
    active: &Path,
    expected: &ManifestFile,
) -> Result<Option<PathBuf>, String> {
    let original = active.join(relative_path(&expected.path)?);
    let runtime_alias = expected
        .path
        .eq_ignore_ascii_case(SPLASH_SCREEN_ORIGINAL)
        .then(|| active.join(SPLASH_SCREEN_RUNTIME_NAME));
    let original_exists = match fs::symlink_metadata(&original) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.to_string()),
    };
    let alias_exists = match runtime_alias.as_deref() {
        Some(alias) => match fs::symlink_metadata(alias) {
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.to_string()),
        },
        None => false,
    };
    if original_exists == alias_exists {
        return Ok(None);
    }
    Ok(Some(if original_exists {
        original
    } else {
        runtime_alias.expect("runtime alias exists when the original does not")
    }))
}

fn verified_managed_path(
    active: &Path,
    expected: &ManifestFile,
) -> Result<Option<PathBuf>, String> {
    let Some(path) = existing_managed_path(active, expected)? else {
        return Ok(None);
    };
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if is_reparse(&metadata)
        || !metadata.is_file()
        || metadata.len() != expected.size
        || sha256_file(&path, Some(expected.size))? != expected.sha256
    {
        return Ok(None);
    }
    Ok(Some(path))
}

fn workspace_metadata_fingerprint(
    active: &Path,
    marker: &WorkspaceMarker,
    source_record: &SourceRecord,
) -> Result<Option<(String, HashSet<String>)>, String> {
    let mut hasher = Sha256::new();
    let mut known_names =
        HashSet::with_capacity(source_record.files.len() + marker.managed_files.len());
    for expected in &source_record.files {
        if !known_names.insert(expected.path.to_ascii_lowercase()) {
            return Err("source record has case-colliding paths".into());
        }
        let path = active.join(relative_path(&expected.path)?);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if is_reparse(&metadata) || !metadata.is_file() || metadata.len() != expected.size {
            return Ok(None);
        }
        let modified = metadata
            .modified()
            .map_err(|error| error.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| format!("source file timestamp is invalid: {}", expected.path))?
            .as_nanos();
        let identity = file_identity(&metadata);
        hasher.update(b"source");
        hasher.update((expected.path.len() as u64).to_le_bytes());
        hasher.update(expected.path.as_bytes());
        hasher.update(metadata.len().to_le_bytes());
        hasher.update(modified.to_le_bytes());
        hasher.update(identity.0.to_le_bytes());
        hasher.update(identity.1.to_le_bytes());
    }
    for expected in &marker.managed_files {
        if !known_names.insert(expected.path.to_ascii_lowercase()) {
            return Err("active workspace marker has case-colliding paths".into());
        }
        let Some(path) = existing_managed_path(active, expected)? else {
            return Ok(None);
        };
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) || !metadata.is_file() || metadata.len() != expected.size {
            return Ok(None);
        }
        let portable = safe_relative(
            path.strip_prefix(active)
                .map_err(|_| "managed file escaped its workspace")?,
        )?;
        if !portable.eq_ignore_ascii_case(&expected.path) {
            known_names.insert(portable.to_ascii_lowercase());
        }
        let modified = metadata
            .modified()
            .map_err(|error| error.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| format!("managed file timestamp is invalid: {portable}"))?
            .as_nanos();
        let identity = file_identity(&metadata);
        hasher.update(b"managed");
        hasher.update((portable.len() as u64).to_le_bytes());
        hasher.update(portable.as_bytes());
        hasher.update(metadata.len().to_le_bytes());
        hasher.update(modified.to_le_bytes());
        hasher.update(identity.0.to_le_bytes());
        hasher.update(identity.1.to_le_bytes());
    }
    for path in protected_files(active)? {
        if !known_names.contains(&path.to_ascii_lowercase()) {
            return Ok(None);
        }
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
    }
    Ok(Some((hex_digest(hasher.finalize()), known_names)))
}

fn persist_workspace_validation(
    source: &ManagedSource,
    active: &Path,
    marker: &WorkspaceMarker,
    workspace_id: &str,
) -> Result<(), String> {
    let (metadata_fingerprint, _) = workspace_metadata_fingerprint(active, marker, &source.record)?
        .ok_or("published workspace metadata does not match its marker")?;
    let marker_sha256 =
        sha256_bytes(&serde_json::to_vec(marker).map_err(|error| error.to_string())?);
    write_json(
        &workspace_root(workspace_id)?.join(WORKSPACE_VALIDATION_RECORD),
        &WorkspaceValidationRecord {
            schema: WORKSPACE_VALIDATION_SCHEMA,
            marker_sha256,
            metadata_fingerprint,
        },
    )
}

pub fn active_matches(
    source: &ManagedSource,
    profile_id: &str,
    revision: &str,
    material_revision: &str,
    workspace_id: &str,
) -> Result<bool, String> {
    let started = Instant::now();
    let Some(marker) = active_marker(workspace_id)? else {
        return Ok(false);
    };
    if marker.source_record_id != source_record_id(&source.record)
        || marker.source_fingerprint != source.record.fingerprint
        || marker.game_instance_id != source.record.game_instance_id
        || marker.profile_id != profile_id
        || marker.profile_revision != revision
        || marker.material_revision != material_revision
    {
        return Ok(false);
    }
    let active = workspace_game_dir(workspace_id)?;
    let Some((metadata_fingerprint, known_names)) =
        workspace_metadata_fingerprint(&active, &marker, &source.record)?
    else {
        return Ok(false);
    };
    let marker_sha256 =
        sha256_bytes(&serde_json::to_vec(&marker).map_err(|error| error.to_string())?);
    let validation_path = workspace_root(workspace_id)?.join(WORKSPACE_VALIDATION_RECORD);
    if read_json::<WorkspaceValidationRecord>(&validation_path)
        .ok()
        .flatten()
        .is_some_and(|record| {
            record.schema == WORKSPACE_VALIDATION_SCHEMA
                && record.marker_sha256 == marker_sha256
                && record.metadata_fingerprint == metadata_fingerprint
        })
    {
        log::info!(
            target: "perfect_sync::performance",
            "workspace_validation_cache completed in {} ms ({} files)",
            started.elapsed().as_millis(),
            source.record.files.len() + marker.managed_files.len()
        );
        return Ok(true);
    }

    let mut bytes = 0_u64;
    for expected in &source.record.files {
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
        bytes = bytes
            .checked_add(expected.size)
            .ok_or("workspace validation byte count overflow")?;
    }
    for expected in &marker.managed_files {
        if verified_managed_path(&active, expected)?.is_none() {
            return Ok(false);
        }
        bytes = bytes
            .checked_add(expected.size)
            .ok_or("workspace validation byte count overflow")?;
    }
    for path in protected_files(&active)? {
        if !known_names.contains(&path.to_ascii_lowercase()) {
            return Ok(false);
        }
    }
    let Some((verified_fingerprint, _)) =
        workspace_metadata_fingerprint(&active, &marker, &source.record)?
    else {
        return Ok(false);
    };
    if verified_fingerprint != metadata_fingerprint {
        return Ok(false);
    }
    write_json(
        &validation_path,
        &WorkspaceValidationRecord {
            schema: WORKSPACE_VALIDATION_SCHEMA,
            marker_sha256,
            metadata_fingerprint,
        },
    )?;
    log::info!(
        target: "perfect_sync::performance",
        "workspace_validation_rehash completed in {} ms ({} files, {bytes} bytes)",
        started.elapsed().as_millis(),
        source.record.files.len() + marker.managed_files.len()
    );
    Ok(true)
}

fn profile_metadata_fingerprint(profile_root: &Path) -> Result<String, String> {
    let mut pending = vec![(profile_root.to_path_buf(), PathBuf::new())];
    let mut files = Vec::new();
    let mut bytes = 0_u64;
    while let Some((directory, relative_root)) = pending.pop() {
        for entry in sorted_entries(&directory)? {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err(format!("profile contains a link: {}", path.display()));
            }
            let relative = relative_root.join(entry.file_name());
            let portable = safe_relative(&relative)?;
            if portable.eq_ignore_ascii_case("profile.json")
                || portable.eq_ignore_ascii_case(PROFILE_REVISION_RECORD)
            {
                continue;
            }
            if metadata.is_dir() {
                pending.push((path, relative));
                continue;
            }
            if !metadata.is_file() {
                return Err(format!(
                    "profile contains unsupported entry {}",
                    path.display()
                ));
            }
            if files.len() >= MAX_FILES {
                return Err("profile contains too many files".into());
            }
            bytes = bytes
                .checked_add(metadata.len())
                .filter(|total| *total <= MAX_BYTES)
                .ok_or("profile exceeds its byte limit")?;
            let modified = metadata
                .modified()
                .map_err(|error| error.to_string())?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| format!("profile timestamp is invalid: {portable}"))?
                .as_nanos();
            files.push((portable, metadata.len(), modified, file_identity(&metadata)));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (path, size, modified, identity) in files {
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(size.to_le_bytes());
        hasher.update(modified.to_le_bytes());
        hasher.update(identity.0.to_le_bytes());
        hasher.update(identity.1.to_le_bytes());
    }
    Ok(hex_digest(hasher.finalize()))
}

pub fn profile_revisions(profile_root: &Path) -> Result<(String, String), String> {
    if !profile_root.is_dir() {
        return Ok((
            hex_digest(Sha256::digest([])),
            hex_digest(Sha256::digest([])),
        ));
    }
    let started = Instant::now();
    let metadata_fingerprint = profile_metadata_fingerprint(profile_root)?;
    let record_path = profile_root.join(PROFILE_REVISION_RECORD);
    if let Ok(Some(record)) = read_json::<ProfileRevisionRecord>(&record_path) {
        if record.schema == PROFILE_REVISION_SCHEMA
            && record.metadata_fingerprint == metadata_fingerprint
            && valid_profile_revision(&record.revision)
            && valid_profile_revision(&record.material_revision)
        {
            log::info!(
                target: "perfect_sync::performance",
                "profile_revision_cache completed in {} ms",
                started.elapsed().as_millis()
            );
            return Ok((record.revision, record.material_revision));
        }
    }

    let mut revision = Sha256::new();
    let mut material_revision = Sha256::new();
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    for file in collect_tree(profile_root, false)? {
        if file.path.eq_ignore_ascii_case("profile.json")
            || file.path.eq_ignore_ascii_case(PROFILE_REVISION_RECORD)
        {
            continue;
        }
        let update = |hasher: &mut Sha256| {
            hasher.update((file.path.len() as u64).to_le_bytes());
            hasher.update(file.path.as_bytes());
            hasher.update(file.size.to_le_bytes());
            hasher.update(file.sha256.as_bytes());
        };
        update(&mut revision);
        if !is_mutable_path(&file.path) {
            update(&mut material_revision);
        }
        files += 1;
        bytes += file.size;
    }
    let revision = hex_digest(revision.finalize());
    let material_revision = hex_digest(material_revision.finalize());
    write_json(
        &record_path,
        &ProfileRevisionRecord {
            schema: PROFILE_REVISION_SCHEMA,
            metadata_fingerprint,
            revision: revision.clone(),
            material_revision: material_revision.clone(),
        },
    )?;
    log::info!(
        target: "perfect_sync::performance",
        "profile_revision_rehash completed in {} ms ({files} files, {bytes} bytes)",
        started.elapsed().as_millis()
    );
    Ok((revision, material_revision))
}

pub fn profile_revision(profile_root: &Path) -> Result<String, String> {
    profile_revisions(profile_root).map(|(revision, _)| revision)
}

fn config_manifest(root: &Path) -> Result<Option<Vec<ManifestFile>>, String> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("profile configuration is not a regular directory".into());
    }

    let mut pending = vec![(root.to_path_buf(), PathBuf::new())];
    let mut manifest = Vec::new();
    let mut bytes = 0_u64;
    while let Some((directory, relative_root)) = pending.pop() {
        for entry in sorted_entries(&directory)? {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "profile configuration contains a link: {}",
                    path.display()
                ));
            }
            let relative = relative_root.join(entry.file_name());
            if metadata.is_dir() {
                pending.push((path, relative));
                continue;
            }
            if !metadata.is_file() {
                return Err("profile configuration contains an unsupported entry".into());
            }
            if manifest.len() >= MAX_CONFIG_FILES {
                return Err("profile configuration contains too many files".into());
            }
            bytes = bytes
                .checked_add(metadata.len())
                .filter(|total| *total <= MAX_CONFIG_BYTES)
                .ok_or("profile configuration exceeds its byte limit")?;
            manifest.push(ManifestFile {
                path: safe_relative(&relative)?,
                size: metadata.len(),
                sha256: sha256_file(&path, Some(metadata.len()))?,
            });
        }
    }
    manifest.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(Some(manifest))
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

fn publish_config_if_changed(source: &Path, destination: &Path) -> Result<bool, String> {
    if config_manifest(source)? == config_manifest(destination)? {
        return Ok(false);
    }
    let parent = destination
        .parent()
        .ok_or("profile configuration has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let stage = unique_child(parent, "config-stage");
    let result = (|| {
        copy_config_tree(source, &stage, false)?;
        publish_directory(&stage, destination)?;
        Ok(true)
    })();
    if result.is_err() {
        let _ = remove_tree(&stage);
    }
    result
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
    publish_config_if_changed(&source, &profile_root.join("BepInEx").join("config")).map(|_| ())
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

    fn source_record(root: &Path, build: Option<&str>) -> SourceRecord {
        let files = collect_tree(root, false).unwrap();
        SourceRecord {
            schema: SOURCE_RECORD_SCHEMA,
            fingerprint: manifest_digest(&files).unwrap(),
            game_instance_id: "source-1".into(),
            path: fs::canonicalize(root)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            arch: Arch::X86,
            store: Store::Steam,
            runtime: Runtime::Native,
            observed_build: build.map(str::to_string),
            file_count: files.len() as u64,
            byte_count: files.iter().map(|file| file.size).sum(),
            files,
        }
    }

    fn game_tree() -> tempfile::TempDir {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir_all(temporary.path().join("Among Us_Data")).unwrap();
        fs::write(temporary.path().join("Among Us.exe"), b"executable").unwrap();
        fs::write(
            temporary.path().join("Among Us_Data").join("data.unity3d"),
            b"game data",
        )
        .unwrap();
        temporary
    }

    fn game_instance(root: &Path, id: &str, build: Option<&str>) -> GameInstance {
        GameInstance {
            id: id.into(),
            name: "Source".into(),
            path: fs::canonicalize(root)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            executable_identity: None,
            source_fingerprint: None,
            source_file_count: None,
            source_byte_count: None,
            arch: Arch::X86,
            store: Store::Steam,
            runtime: Runtime::Native,
            build: build.map(str::to_string),
            writable: true,
        }
    }

    fn legacy_workspace(record: &SourceRecord, profile_id: &str, global: bool) -> PathBuf {
        let generation = bases_root()
            .join(instance_slot(&record.game_instance_id))
            .join("versions")
            .join(&record.fingerprint);
        fs::create_dir_all(&generation).unwrap();
        copy_source_tree(Path::new(&record.path), &generation.join("game")).unwrap();
        let executable = record
            .files
            .iter()
            .find(|file| file.path.eq_ignore_ascii_case("Among Us.exe"))
            .unwrap();
        write_json(
            &generation.join(LEGACY_BASE_RECORD),
            &LegacyBaseRecord {
                schema: 5,
                id: record.fingerprint.clone(),
                game_instance_id: record.game_instance_id.clone(),
                source_path: record.path.clone(),
                source_executable_sha256: executable.sha256.clone(),
                arch: record.arch,
                store: record.store,
                runtime: record.runtime,
                build: record.observed_build.clone(),
                manifest_sha256: record.fingerprint.clone(),
                exact_source_snapshot: true,
                files: record.files.clone(),
            },
        )
        .unwrap();
        let active = if global {
            workspaces_root().join("current")
        } else {
            workspace_root(profile_id).unwrap().join("current")
        };
        fs::create_dir_all(active.parent().unwrap()).unwrap();
        copy_source_tree(Path::new(&record.path), &active).unwrap();
        let revision = sha256_bytes(profile_id.as_bytes());
        write_json(
            &active.join(INSTANCE_MARKER),
            &LegacyWorkspaceMarker {
                schema: 5,
                base_id: record.fingerprint.clone(),
                base_manifest_sha256: record.fingerprint.clone(),
                game_instance_id: record.game_instance_id.clone(),
                profile_id: profile_id.into(),
                profile_revision: revision.clone(),
                material_revision: Some(revision),
                managed_files: Vec::new(),
            },
        )
        .unwrap();
        active
    }

    #[test]
    fn schema_six_source_record_binds_manifest_counts_bytes_and_path() {
        let source = game_tree();
        let record = source_record(source.path(), Some("2026.8.4"));

        validate_source_record(&record).unwrap();
        assert_eq!(record.schema, 6);
        assert_eq!(record.file_count, 2);
        assert_eq!(record.byte_count, 19);
        assert_eq!(
            normalized_path(Path::new(&record.path)),
            normalized_path(&fs::canonicalize(source.path()).unwrap())
        );
    }

    #[test]
    fn source_record_rejects_incomplete_counts_and_bytes() {
        let source = game_tree();
        let mut record = source_record(source.path(), None);
        record.file_count += 1;
        assert!(validate_source_record(&record).is_err());
        record.file_count -= 1;
        record.byte_count += 1;
        assert!(validate_source_record(&record).is_err());
    }

    #[test]
    fn direct_copy_and_both_fingerprint_checks_match_the_record() {
        let source = game_tree();
        let record = source_record(source.path(), None);
        let destination_parent = tempfile::tempdir().unwrap();
        let destination = destination_parent.path().join("stage");

        let copied = copy_source_tree(source.path(), &destination).unwrap();

        assert_eq!(copied, record.files);
        verify_source_fingerprint(&destination, &record).unwrap();
        verify_source_fingerprint(source.path(), &record).unwrap();
    }

    #[test]
    fn staged_copy_verification_detects_mutation() {
        let source = game_tree();
        let record = source_record(source.path(), None);
        let destination_parent = tempfile::tempdir().unwrap();
        let destination = destination_parent.path().join("stage");
        copy_source_tree(source.path(), &destination).unwrap();
        fs::write(destination.join("Among Us.exe"), b"changed").unwrap();

        let error = verify_source_fingerprint(&destination, &record).unwrap_err();
        assert!(error.contains("fingerprint changed"));
    }

    #[test]
    fn source_race_verification_detects_mutation_after_copy() {
        let source = game_tree();
        let record = source_record(source.path(), None);
        let destination_parent = tempfile::tempdir().unwrap();
        copy_source_tree(source.path(), &destination_parent.path().join("stage")).unwrap();
        fs::write(source.path().join("Among Us_Data/data.unity3d"), b"changed").unwrap();

        let error = verify_source_fingerprint(source.path(), &record).unwrap_err();
        assert!(error.contains("fingerprint changed"));
    }

    #[test]
    fn launch_build_guard_allows_unavailable_and_same_build_sources() {
        let source = game_tree();
        let record = source_record(source.path(), Some("2026.8.4"));
        assert!(observed_build_matches(&record, Some("2026.8.4")));
        assert!(!observed_build_matches(&record, Some("2026.8.5")));
        let unavailable = ManagedSource {
            source_dir: PathBuf::from("missing-source"),
            record: SourceRecord {
                path: source.path().join("missing").to_string_lossy().into_owned(),
                ..record
            },
        };
        ensure_source_build_allows_launch(&unavailable).unwrap();
    }

    #[test]
    fn same_build_content_change_does_not_change_launch_build_decision() {
        let source = game_tree();
        let record = source_record(source.path(), Some("2026.8.4"));
        fs::write(source.path().join("Among Us_Data/data.unity3d"), b"changed").unwrap();

        assert!(observed_build_matches(&record, Some("2026.8.4")));
        assert!(verify_source_fingerprint(source.path(), &record).is_err());
    }

    #[test]
    fn marker_binds_source_and_both_profile_revisions() {
        let marker = WorkspaceMarker {
            schema: SCHEMA,
            source_record_id: "d".repeat(64),
            source_fingerprint: "a".repeat(64),
            game_instance_id: "source-1".into(),
            profile_id: "profile-1".into(),
            profile_revision: "b".repeat(64),
            material_revision: "c".repeat(64),
            managed_files: Vec::new(),
        };
        let value = serde_json::to_value(&marker).unwrap();

        assert_eq!(value["sourceRecordId"], "d".repeat(64));
        assert_eq!(value["schema"], 6);
        assert_eq!(value["sourceFingerprint"], "a".repeat(64));
        assert_eq!(value["materialRevision"], "c".repeat(64));
        assert!(value.get("baseId").is_none());
    }

    #[test]
    fn managed_delta_records_only_profile_overlays() {
        let source = game_tree();
        let record = source_record(source.path(), None);
        let destination_parent = tempfile::tempdir().unwrap();
        let stage = destination_parent.path().join("stage");
        copy_source_tree(source.path(), &stage).unwrap();
        fs::create_dir_all(stage.join("BepInEx/plugins")).unwrap();
        fs::write(stage.join("BepInEx/plugins/mod.dll"), b"mod").unwrap();

        let managed = managed_delta(&record, &stage, true).unwrap();

        assert_eq!(managed.len(), 1);
        assert_eq!(managed[0].path, "BepInEx/plugins/mod.dll");
    }

    #[test]
    fn workspace_validation_detects_deleted_non_executable_source_file() {
        let source = game_tree();
        let record = source_record(source.path(), None);
        let temporary = tempfile::tempdir().unwrap();
        let active = temporary.path().join("current");
        copy_source_tree(source.path(), &active).unwrap();
        let marker = WorkspaceMarker {
            schema: SCHEMA,
            source_record_id: source_record_id(&record),
            source_fingerprint: record.fingerprint.clone(),
            game_instance_id: record.game_instance_id.clone(),
            profile_id: "profile-1".into(),
            profile_revision: "a".repeat(64),
            material_revision: "b".repeat(64),
            managed_files: Vec::new(),
        };
        assert!(workspace_metadata_fingerprint(&active, &marker, &record)
            .unwrap()
            .is_some());

        fs::remove_file(active.join("Among Us_Data/data.unity3d")).unwrap();

        assert!(workspace_metadata_fingerprint(&active, &marker, &record)
            .unwrap()
            .is_none());
    }

    #[test]
    fn incomplete_legacy_base_cannot_become_source_evidence() {
        let source = game_tree();
        let record = source_record(source.path(), None);
        let legacy = LegacyBase {
            record: LegacyBaseRecord {
                schema: 5,
                id: record.fingerprint.clone(),
                game_instance_id: record.game_instance_id.clone(),
                source_path: record.path.clone(),
                source_executable_sha256: record.files[0].sha256.clone(),
                arch: record.arch,
                store: record.store,
                runtime: record.runtime,
                build: record.observed_build.clone(),
                manifest_sha256: record.fingerprint,
                exact_source_snapshot: false,
                files: record.files,
            },
        };

        assert!(source_record_from_legacy(&legacy).is_err());
    }

    #[test]
    fn publication_failure_keeps_the_previous_workspace() {
        let temporary = tempfile::tempdir().unwrap();
        let destination = temporary.path().join("current");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("state"), b"working").unwrap();

        assert!(publish_directory(&temporary.path().join("missing-stage"), &destination).is_err());
        assert_eq!(fs::read(destination.join("state")).unwrap(), b"working");
    }

    #[test]
    fn interrupted_publication_recovers_the_newest_retired_workspace() {
        let temporary = tempfile::tempdir().unwrap();
        let destination = temporary.path().join("current");
        let older = temporary.path().join(".current-old.1");
        let newer = temporary.path().join(".current-old.2");
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

    #[test]
    fn immutable_source_history_keeps_crash_safe_settings_selection_and_rebinding() {
        const CHILD: &str = "PERFECT_SYNC_SOURCE_HISTORY_CHILD";
        const ROOT: &str = "PERFECT_SYNC_SOURCE_HISTORY_ROOT";
        const TEST: &str =
            "managed_instance::tests::immutable_source_history_keeps_crash_safe_settings_selection_and_rebinding";

        if std::env::var_os(CHILD).is_some() {
            let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
            settings::initialize_managed_data_dir(root.join("managed")).unwrap();
            let source = game_tree();
            let instance = game_instance(source.path(), "source-1", Some("2026.8.4"));
            let first = record_source(&instance).unwrap();
            let mut old_settings = instance.clone();
            old_settings.source_fingerprint = Some(first.record.fingerprint.clone());
            old_settings.source_file_count = Some(first.record.file_count);
            old_settings.source_byte_count = Some(first.record.byte_count);

            fs::write(
                source.path().join("Among Us_Data/data.unity3d"),
                b"new game data",
            )
            .unwrap();
            let second = record_source(&instance).unwrap();
            assert_ne!(first.record.fingerprint, second.record.fingerprint);
            assert_eq!(
                saved_source(&old_settings)
                    .unwrap()
                    .unwrap()
                    .record
                    .fingerprint,
                first.record.fingerprint
            );
            let mut unknown_build = old_settings.clone();
            unknown_build.build = None;
            assert_eq!(
                saved_source(&unknown_build)
                    .unwrap()
                    .unwrap()
                    .record
                    .observed_build
                    .as_deref(),
                Some("2026.8.4")
            );

            let mut new_settings = instance;
            new_settings.source_fingerprint = Some(second.record.fingerprint.clone());
            new_settings.source_file_count = Some(second.record.file_count);
            new_settings.source_byte_count = Some(second.record.byte_count);
            assert_eq!(
                saved_source(&new_settings)
                    .unwrap()
                    .unwrap()
                    .record
                    .fingerprint,
                second.record.fingerprint
            );
            assert_eq!(load_instance_source_records("source-1").unwrap().len(), 2);

            let moved = root.join("moved-source");
            copy_source_tree(source.path(), &moved).unwrap();
            fs::write(moved.join("Among Us_Data/data.unity3d"), b"wrong data").unwrap();
            let moved = fs::canonicalize(moved).unwrap();
            assert!(rebind_source_record(&new_settings, &moved)
                .unwrap_err()
                .contains("fingerprint changed"));
            remove_tree(&moved).unwrap();
            copy_source_tree(source.path(), &moved).unwrap();
            let rebound = rebind_source_record(&new_settings, &moved)
                .unwrap()
                .unwrap();
            assert_eq!(Path::new(&rebound.record.path), moved);
            assert_eq!(rebound.record.fingerprint, second.record.fingerprint);
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .env(ROOT, root.path())
            .args(["--exact", TEST, "--nocapture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn valid_workspace_is_reused_when_source_changes_or_becomes_unavailable() {
        const CHILD: &str = "PERFECT_SYNC_DIRECT_SOURCE_LIFECYCLE_CHILD";
        const ROOT: &str = "PERFECT_SYNC_DIRECT_SOURCE_LIFECYCLE_ROOT";
        const TEST: &str =
            "managed_instance::tests::valid_workspace_is_reused_when_source_changes_or_becomes_unavailable";

        if std::env::var_os(CHILD).is_some() {
            settings::initialize_managed_data_dir(PathBuf::from(std::env::var_os(ROOT).unwrap()))
                .unwrap();
            let source = game_tree();
            fs::write(
                source.path().join("Among Us_Data/globalgamemanagers"),
                b"2026.8.4 original",
            )
            .unwrap();
            let instance = game_instance(source.path(), "source-1", Some("2026.8.4"));
            let managed = record_source(&instance).unwrap();
            let revision = "a".repeat(64);
            let material_revision = "b".repeat(64);
            let stage = begin_workspace(&managed, "profile-1").unwrap();
            fs::create_dir_all(stage.join("BepInEx/plugins")).unwrap();
            fs::write(stage.join("BepInEx/plugins/mod.dll"), b"profile mod").unwrap();
            let active = publish_workspace(
                &stage,
                &managed,
                "profile-1",
                &revision,
                &material_revision,
                "profile-1",
            )
            .unwrap();

            assert!(active_matches(
                &managed,
                "profile-1",
                &revision,
                &material_revision,
                "profile-1",
            )
            .unwrap());
            fs::write(
                source.path().join("Among Us_Data/globalgamemanagers"),
                b"2026.8.4 changed content",
            )
            .unwrap();
            ensure_source_build_allows_launch(&managed).unwrap();
            assert!(active_matches(
                &managed,
                "profile-1",
                &revision,
                &material_revision,
                "profile-1",
            )
            .unwrap());
            assert_eq!(
                fs::read(active.join("Among Us_Data/globalgamemanagers")).unwrap(),
                b"2026.8.4 original"
            );
            fs::remove_dir_all(source.path()).unwrap();
            ensure_source_build_allows_launch(&managed).unwrap();
            assert!(active_matches(
                &managed,
                "profile-1",
                &revision,
                &material_revision,
                "profile-1",
            )
            .unwrap());
            assert!(ensure_exact_source_available(&managed)
                .unwrap_err()
                .contains("source is unavailable"));
            assert!(begin_workspace(&managed, "profile-1")
                .unwrap_err()
                .contains("source is unavailable"));
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .env(ROOT, root.path())
            .args(["--exact", TEST, "--nocapture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn reachable_source_with_changed_build_blocks_launch() {
        let source = game_tree();
        fs::write(
            source.path().join("Among Us_Data/globalgamemanagers"),
            b"2026.8.4",
        )
        .unwrap();
        let managed = managed_source(source_record(source.path(), Some("2026.8.4")));
        ensure_source_build_allows_launch(&managed).unwrap();

        fs::write(
            source.path().join("Among Us_Data/globalgamemanagers"),
            b"2026.8.5",
        )
        .unwrap();

        assert!(ensure_source_build_allows_launch(&managed)
            .unwrap_err()
            .contains("build has changed"));
    }

    #[test]
    fn schema_five_workspace_migrates_in_place_and_removes_safe_obsolete_bases() {
        const CHILD: &str = "PERFECT_SYNC_DIRECT_SOURCE_MIGRATION_CHILD";
        const ROOT: &str = "PERFECT_SYNC_DIRECT_SOURCE_MIGRATION_ROOT";
        const TEST: &str =
            "managed_instance::tests::schema_five_workspace_migrates_in_place_and_removes_safe_obsolete_bases";

        if std::env::var_os(CHILD).is_some() {
            settings::initialize_managed_data_dir(PathBuf::from(std::env::var_os(ROOT).unwrap()))
                .unwrap();
            let source = game_tree();
            let source_record = source_record(source.path(), Some("2026.8.4"));
            let generation = bases_root()
                .join(instance_slot(&source_record.game_instance_id))
                .join("versions")
                .join(&source_record.fingerprint);
            fs::create_dir_all(&generation).unwrap();
            copy_source_tree(source.path(), &generation.join("game")).unwrap();
            let executable = source_record
                .files
                .iter()
                .find(|file| file.path.eq_ignore_ascii_case("Among Us.exe"))
                .unwrap();
            write_json(
                &generation.join(LEGACY_BASE_RECORD),
                &LegacyBaseRecord {
                    schema: 5,
                    id: source_record.fingerprint.clone(),
                    game_instance_id: source_record.game_instance_id.clone(),
                    source_path: source_record.path.clone(),
                    source_executable_sha256: executable.sha256.clone(),
                    arch: source_record.arch,
                    store: source_record.store,
                    runtime: source_record.runtime,
                    build: source_record.observed_build.clone(),
                    manifest_sha256: source_record.fingerprint.clone(),
                    exact_source_snapshot: true,
                    files: source_record.files.clone(),
                },
            )
            .unwrap();

            let active = workspaces_root().join("current");
            fs::create_dir_all(active.parent().unwrap()).unwrap();
            copy_source_tree(source.path(), &active).unwrap();
            fs::create_dir_all(active.join("BepInEx/plugins")).unwrap();
            let plugin = active.join("BepInEx/plugins/mod.dll");
            fs::write(&plugin, b"profile mod").unwrap();
            let managed_file = ManifestFile {
                path: "BepInEx/plugins/mod.dll".into(),
                size: fs::metadata(&plugin).unwrap().len(),
                sha256: sha256_file(&plugin, None).unwrap(),
            };
            let revision = "c".repeat(64);
            write_json(
                &active.join(INSTANCE_MARKER),
                &LegacyWorkspaceMarker {
                    schema: 5,
                    base_id: source_record.fingerprint.clone(),
                    base_manifest_sha256: source_record.fingerprint.clone(),
                    game_instance_id: source_record.game_instance_id.clone(),
                    profile_id: "profile-1".into(),
                    profile_revision: revision.clone(),
                    material_revision: Some(revision),
                    managed_files: vec![managed_file],
                },
            )
            .unwrap();
            let original_workspace_bytes =
                fs::read(active.join("Among Us_Data/data.unity3d")).unwrap();

            migrate_direct_source_storage().unwrap();

            let migrated_active = workspace_root("profile-1").unwrap().join("current");
            assert!(!active.exists());
            assert_eq!(
                fs::read(migrated_active.join("Among Us_Data/data.unity3d")).unwrap(),
                original_workspace_bytes
            );
            assert_eq!(
                fs::read(migrated_active.join("BepInEx/plugins/mod.dll")).unwrap(),
                b"profile mod"
            );
            let marker = read_json::<WorkspaceMarker>(&migrated_active.join(INSTANCE_MARKER))
                .unwrap()
                .unwrap();
            assert_eq!(marker.schema, 6);
            assert_eq!(marker.source_fingerprint, source_record.fingerprint);
            assert!(!bases_root().exists());
            let migrated = read_json::<SourceRecord>(
                &source_record_path(
                    &source_record.game_instance_id,
                    &source_record_id(&source_record),
                )
                .unwrap(),
            )
            .unwrap()
            .unwrap();
            assert_eq!(migrated.file_count, source_record.file_count);
            assert_eq!(migrated.byte_count, source_record.byte_count);
            assert_eq!(migrated.files, source_record.files);
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .env(ROOT, root.path())
            .args(["--exact", TEST, "--nocapture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn two_legacy_profile_histories_migrate_to_distinct_immutable_source_records() {
        const CHILD: &str = "PERFECT_SYNC_MULTI_HISTORY_MIGRATION_CHILD";
        const ROOT: &str = "PERFECT_SYNC_MULTI_HISTORY_MIGRATION_ROOT";
        const TEST: &str =
            "managed_instance::tests::two_legacy_profile_histories_migrate_to_distinct_immutable_source_records";

        if std::env::var_os(CHILD).is_some() {
            settings::initialize_managed_data_dir(PathBuf::from(std::env::var_os(ROOT).unwrap()))
                .unwrap();
            let first_source = game_tree();
            let first = source_record(first_source.path(), Some("2026.7.1"));
            let second_source = game_tree();
            fs::write(
                second_source.path().join("Among Us_Data/data.unity3d"),
                b"newer game data",
            )
            .unwrap();
            let second = source_record(second_source.path(), Some("2026.8.4"));
            assert_ne!(first.fingerprint, second.fingerprint);
            let first_active = legacy_workspace(&first, "profile-old", false);
            let second_active = legacy_workspace(&second, "profile-new", false);

            migrate_direct_source_storage().unwrap();

            let first_marker = read_json::<WorkspaceMarker>(&first_active.join(INSTANCE_MARKER))
                .unwrap()
                .unwrap();
            let second_marker = read_json::<WorkspaceMarker>(&second_active.join(INSTANCE_MARKER))
                .unwrap()
                .unwrap();
            assert_eq!(first_marker.source_fingerprint, first.fingerprint);
            assert_eq!(second_marker.source_fingerprint, second.fingerprint);
            assert_ne!(
                first_marker.source_record_id,
                second_marker.source_record_id
            );
            assert_eq!(load_instance_source_records("source-1").unwrap().len(), 2);
            assert_eq!(
                load_marker_source(&first_marker).unwrap().record.path,
                first.path
            );
            assert_eq!(
                load_marker_source(&second_marker).unwrap().record.path,
                second.path
            );
            assert!(!bases_root().exists());
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .env(ROOT, root.path())
            .args(["--exact", TEST, "--nocapture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn global_legacy_vanilla_workspace_is_deferred_without_base_cleanup() {
        const CHILD: &str = "PERFECT_SYNC_GLOBAL_VANILLA_CHILD";
        const ROOT: &str = "PERFECT_SYNC_GLOBAL_VANILLA_ROOT";
        const TEST: &str =
            "managed_instance::tests::global_legacy_vanilla_workspace_is_deferred_without_base_cleanup";

        if std::env::var_os(CHILD).is_some() {
            settings::initialize_managed_data_dir(PathBuf::from(std::env::var_os(ROOT).unwrap()))
                .unwrap();
            let source = game_tree();
            let record = source_record(source.path(), Some("2026.8.4"));
            let global = legacy_workspace(&record, "_vanilla", true);

            migrate_direct_source_storage().unwrap();

            assert!(global.exists());
            assert_eq!(marker_schema(&global).unwrap(), Some(5));
            assert!(bases_root().exists());
            assert!(load_instance_source_records("source-1").unwrap().is_empty());
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .env(ROOT, root.path())
            .args(["--exact", TEST, "--nocapture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
