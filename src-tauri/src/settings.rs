//! App settings and persistent data under the host's standard application-data directory.

use atomicwrites::{AllowOverwrite, AtomicFile};
use perfect_sync_core::types::{Arch, Runtime, Store};
use perfect_sync_core::{compat, game, process};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, RwLock};

const KEYRING_SERVICE: &str = "com.artriy.perfectsync";
const KEYRING_USER: &str = "github-token";
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
const V016_PROFILE_RESET_MARKER: &str = ".perfectsync-v0.1.6-profile-reset";

static APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static DEFAULT_MANAGED_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static MANAGED_DATA_DIR: OnceLock<RwLock<PathBuf>> = OnceLock::new();
static SETTINGS_IO: Mutex<()> = Mutex::new(());

/// A mod the user always wants merged into any lobby code they apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalMod {
    pub repo: String,
    pub tag: String,
    pub asset: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalLocalMod {
    pub path: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInstance {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub executable_identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_file_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_byte_count: Option<u64>,
    pub arch: Arch,
    pub store: Store,
    pub runtime: Runtime,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub build: Option<String>,
    #[serde(default = "default_true")]
    pub writable: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub game_instances: Vec<GameInstance>,
    #[serde(default)]
    pub personal_mods: Vec<PersonalMod>,
    #[serde(default)]
    pub personal_local_mods: Vec<PersonalLocalMod>,
    #[serde(default)]
    pub setup_complete: bool,
    /// True after the user has selected and recorded an exact original source.
    #[serde(default)]
    pub fresh_source_setup_complete: bool,
    #[serde(default)]
    pub skip_launch_warning: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub active_profile: Option<String>,
    /// Custom root for large managed game data and downloaded package caches.
    /// `None` keeps the platform-local default.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub storage_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TokenAction {
    Unchanged,
    Set { token: String },
    Clear,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    #[serde(flatten)]
    pub settings: Settings,
    pub has_github_token: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_warning: Option<String>,
    pub active_storage_path: String,
    pub default_storage_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_warning: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacySecretSettings {
    github_token: Option<String>,
}

#[derive(Debug)]
pub enum SettingsError {
    NotInitialized,
    InvalidDataDirectory,
    AlreadyInitialized,
    LockPoisoned,
    Io {
        operation: &'static str,
        source: io::Error,
    },
    Json(serde_json::Error),
    Keyring(keyring::Error),
    ManualRecoveryRequired,
    SettingsTooLarge,
    Transaction {
        operation: Box<SettingsError>,
        rollback: Option<String>,
    },
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized => write!(f, "application data directory is not initialized"),
            Self::InvalidDataDirectory => {
                write!(f, "application data directory must be an absolute path")
            }
            Self::AlreadyInitialized => {
                write!(f, "application data directory was already initialized")
            }
            Self::LockPoisoned => write!(f, "settings storage lock is unavailable"),
            Self::Io { operation, source } => {
                write!(f, "failed to {operation} settings ({:?})", source.kind())
            }
            Self::Json(source) => write!(
                f,
                "failed to serialize settings at line {} column {}",
                source.line(),
                source.column()
            ),
            Self::Keyring(_) => write!(f, "OS credential storage operation failed"),
            Self::ManualRecoveryRequired => write!(
                f,
                "saved settings cannot be recovered safely; move settings.json to a secure location and remove it manually before retrying"
            ),
            Self::SettingsTooLarge => write!(
                f,
                "serialized settings exceed the limit of {MAX_SETTINGS_BYTES} bytes"
            ),
            Self::Transaction {
                operation,
                rollback,
            } => {
                write!(f, "settings transaction failed: {operation}")?;
                if let Some(rollback) = rollback {
                    write!(f, "; additionally rollback failed: {rollback}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json(source) => Some(source),
            Self::Keyring(source) => Some(source),
            _ => None,
        }
    }
}

fn io_error(operation: &'static str, source: io::Error) -> SettingsError {
    SettingsError::Io { operation, source }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyGameSettings {
    game_path: Option<String>,
    arch: Option<Arch>,
    store: Option<Store>,
    runtime: Option<Runtime>,
}

fn migrate_legacy_game(text: &str) -> Option<GameInstance> {
    let legacy: LegacyGameSettings = serde_json::from_str(text).ok()?;
    let path = legacy.game_path?;
    let store = legacy.store.unwrap_or(Store::Manual);
    let arch = game::exe_arch(&Path::new(&path).join(process::GAME_EXE))
        .or(legacy.arch)
        .unwrap_or_else(|| game::arch_for_store(store));
    let runtime = legacy
        .runtime
        .unwrap_or_else(|| compat::resolve(Path::new(&path)).runtime);
    let name = match store {
        Store::Steam => "Steam",
        Store::Epic => "Epic Games",
        Store::Itch => "itch.io",
        Store::Msstore => "Microsoft Store",
        Store::Manual => "Among Us",
    };
    Some(GameInstance {
        id: "game-1".to_string(),
        name: name.to_string(),
        path,
        executable_identity: None,
        source_fingerprint: None,
        source_file_count: None,
        source_byte_count: None,
        arch,
        store,
        runtime,
        build: None,
        writable: true,
    })
}

pub fn initialize_app_data_dir(path: PathBuf) -> Result<(), SettingsError> {
    if !path.is_absolute() {
        return Err(SettingsError::InvalidDataDirectory);
    }
    if let Some(existing) = APP_DATA_DIR.get() {
        return if existing == &path {
            Ok(())
        } else {
            Err(SettingsError::AlreadyInitialized)
        };
    }
    fs::create_dir_all(&path).map_err(|e| io_error("create the application data directory", e))?;
    APP_DATA_DIR
        .set(path)
        .map_err(|_| SettingsError::AlreadyInitialized)
}
pub fn initialize_managed_data_dir(path: PathBuf) -> Result<(), SettingsError> {
    if !path.is_absolute() {
        return Err(SettingsError::InvalidDataDirectory);
    }
    if let Some(existing) = DEFAULT_MANAGED_DATA_DIR.get() {
        return if existing == &path {
            Ok(())
        } else {
            Err(SettingsError::AlreadyInitialized)
        };
    }
    fs::create_dir_all(&path).map_err(|e| io_error("create the managed data directory", e))?;
    DEFAULT_MANAGED_DATA_DIR
        .set(path.clone())
        .map_err(|_| SettingsError::AlreadyInitialized)?;
    MANAGED_DATA_DIR
        .set(RwLock::new(path))
        .map_err(|_| SettingsError::AlreadyInitialized)
}

pub fn set_managed_data_dir(path: PathBuf) -> Result<(), SettingsError> {
    if !path.is_absolute() {
        return Err(SettingsError::InvalidDataDirectory);
    }
    fs::create_dir_all(&path).map_err(|e| io_error("create the managed data directory", e))?;
    let active = MANAGED_DATA_DIR
        .get()
        .ok_or(SettingsError::NotInitialized)?;
    *active.write().map_err(|_| SettingsError::LockPoisoned)? = path;
    Ok(())
}

pub fn managed_data_dir() -> PathBuf {
    MANAGED_DATA_DIR
        .get()
        .and_then(|path| path.read().ok().map(|path| path.clone()))
        .unwrap_or_else(app_data_dir)
}

pub fn default_managed_data_dir() -> PathBuf {
    DEFAULT_MANAGED_DATA_DIR
        .get()
        .cloned()
        .unwrap_or_else(app_data_dir)
}

pub fn cache_dir() -> PathBuf {
    let active = managed_data_dir();
    if active == default_managed_data_dir() {
        app_data_dir().join("cache")
    } else {
        active.join("cache")
    }
}

pub fn cache_dir_if_initialized() -> Option<PathBuf> {
    APP_DATA_DIR.get().map(|_| cache_dir())
}

pub fn catalog_cache_path() -> PathBuf {
    app_data_dir().join("catalog.json")
}

pub fn user_catalog_path() -> PathBuf {
    app_data_dir().join("user_catalog.json")
}

pub fn app_data_dir() -> PathBuf {
    APP_DATA_DIR
        .get()
        .cloned()
        .expect("application data directory must be initialized during Tauri setup")
}

pub fn profiles_root() -> PathBuf {
    app_data_dir().join("profiles")
}

fn settings_path() -> Result<PathBuf, SettingsError> {
    APP_DATA_DIR
        .get()
        .map(|path| path.join("settings.json"))
        .ok_or(SettingsError::NotInitialized)
}

fn lock_settings() -> Result<MutexGuard<'static, ()>, SettingsError> {
    SETTINGS_IO.lock().map_err(|_| SettingsError::LockPoisoned)
}

struct ParsedSettings {
    settings: Settings,
    legacy_token: Option<String>,
    needs_scrub: bool,
}

enum ReadSettings {
    Missing,
    Valid(ParsedSettings),
    Recovered(String),
}

enum DecodedSettings {
    Valid(ParsedSettings),
    Invalid {
        value: serde_json::Value,
        legacy_token: Option<String>,
    },
}

fn open_bounded(path: &Path) -> Result<Option<(File, Vec<u8>, bool)>, SettingsError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("open", error)),
    };
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error("read", e))?;
    let oversized = bytes.len() as u64 > MAX_SETTINGS_BYTES;
    Ok(Some((file, bytes, oversized)))
}

fn sync_parent(_path: &Path) -> Result<(), SettingsError> {
    #[cfg(unix)]
    {
        let parent = _path.parent().ok_or_else(|| {
            io_error(
                "locate the settings directory",
                io::ErrorKind::NotFound.into(),
            )
        })?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| io_error("sync the settings directory", e))?;
    }
    Ok(())
}

fn quarantine_with_sync<F>(
    path: &Path,
    sanitized: &[u8],
    sync_directory: F,
) -> Result<String, SettingsError>
where
    F: Fn(&Path) -> Result<(), SettingsError>,
{
    for sequence in 0..10_000_u32 {
        let name = format!("settings.corrupt-{}-{sequence}.json", std::process::id());
        let candidate = path.with_file_name(&name);
        let mut output = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error("create a settings recovery file", error)),
        };
        let write_result = (|| -> io::Result<()> {
            output.write_all(sanitized)?;
            output.flush()?;
            output.sync_all()
        })();
        if let Err(error) = write_result {
            drop(output);
            let _ = fs::remove_file(&candidate);
            return Err(io_error("preserve malformed settings", error));
        }
        drop(output);

        // Make the recovery name durable before removing the only other name.
        sync_directory(&candidate)?;
        fs::remove_file(path).map_err(|e| io_error("quarantine malformed settings", e))?;
        sync_directory(path)?;
        return Ok(name);
    }
    Err(io_error(
        "create a unique settings recovery file",
        io::ErrorKind::AlreadyExists.into(),
    ))
}

fn quarantine(path: &Path, sanitized: &[u8]) -> Result<String, SettingsError> {
    quarantine_with_sync(path, sanitized, sync_parent)
}

fn recovery_warning(name: &str) -> String {
    format!(
        "Settings were reset because the saved file was invalid. A sanitized copy with legacy credentials removed was preserved as {name}."
    )
}

fn redact_legacy_secrets(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            object.remove("githubToken");
            for nested in object.values_mut() {
                redact_legacy_secrets(nested);
            }
        }
        serde_json::Value::Array(array) => {
            for nested in array {
                redact_legacy_secrets(nested);
            }
        }
        _ => {}
    }
}

fn decode_settings(bytes: &[u8]) -> Result<DecodedSettings, serde_json::Error> {
    let value = serde_json::from_slice::<serde_json::Value>(bytes)?;
    let parsed = Settings::deserialize(&value);
    let legacy = LegacySecretSettings::deserialize(&value);
    let needs_scrub = value
        .as_object()
        .is_some_and(|object| object.contains_key("githubToken"));
    let legacy_token = legacy.ok().and_then(|legacy| legacy.github_token);

    match parsed {
        Ok(mut settings) => {
            if settings.game_instances.is_empty() {
                if let Ok(text) = std::str::from_utf8(bytes) {
                    if let Some(instance) = migrate_legacy_game(text) {
                        settings.game_instances.push(instance);
                    }
                }
            }
            Ok(DecodedSettings::Valid(ParsedSettings {
                settings,
                legacy_token,
                needs_scrub,
            }))
        }
        Err(_) => Ok(DecodedSettings::Invalid {
            value,
            legacy_token,
        }),
    }
}

fn read_settings_at(path: &Path) -> Result<ReadSettings, SettingsError> {
    let Some((file, bytes, oversized)) = open_bounded(path)? else {
        return Ok(ReadSettings::Missing);
    };
    if oversized {
        return Err(SettingsError::ManualRecoveryRequired);
    }

    let decoded = decode_settings(&bytes).map_err(|_| SettingsError::ManualRecoveryRequired)?;
    match decoded {
        DecodedSettings::Valid(parsed) => Ok(ReadSettings::Valid(parsed)),
        DecodedSettings::Invalid {
            mut value,
            legacy_token,
        } => {
            migrate_token_unlocked(legacy_token)?;
            redact_legacy_secrets(&mut value);
            let mut sanitized = serde_json::to_vec_pretty(&value).map_err(SettingsError::Json)?;
            sanitized.push(b'\n');
            drop(file);
            let name = quarantine(path, &sanitized)?;
            log::warn!("malformed settings were quarantined in {name}");
            Ok(ReadSettings::Recovered(recovery_warning(&name)))
        }
    }
}

fn read_settings_unlocked() -> Result<ReadSettings, SettingsError> {
    read_settings_at(&settings_path()?)
}

fn keyring_entry() -> Result<keyring::Entry, SettingsError> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(SettingsError::Keyring)
}

fn github_token_unlocked() -> Result<Option<SecretString>, SettingsError> {
    match keyring_entry()?.get_password() {
        Ok(token) => Ok(Some(SecretString::new(token))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(SettingsError::Keyring(error)),
    }
}

fn set_github_token_unlocked(token: SecretString) -> Result<(), SettingsError> {
    keyring_entry()?
        .set_password(token.expose_secret())
        .map_err(SettingsError::Keyring)?;
    log::info!("GitHub credential was stored in the OS keyring");
    Ok(())
}

fn migrate_token_with<Lookup, StoreToken>(
    token: Option<String>,
    credential_exists: Lookup,
    store_token: StoreToken,
) -> Result<bool, SettingsError>
where
    Lookup: FnOnce() -> Result<bool, SettingsError>,
    StoreToken: FnOnce(String) -> Result<(), SettingsError>,
{
    let Some(token) = token else {
        return Ok(false);
    };
    if !credential_exists()? {
        store_token(token)?;
    }
    Ok(true)
}

fn migrate_token_unlocked(token: Option<String>) -> Result<bool, SettingsError> {
    migrate_token_with(
        token,
        || github_token_unlocked().map(|token| token.is_some()),
        |token| set_github_token_unlocked(SecretString::new(token)),
    )
}

fn write_settings_at<F>(
    path: &Path,
    settings: &Settings,
    sync_directory: F,
) -> Result<(), SettingsError>
where
    F: Fn(&Path) -> Result<(), SettingsError>,
{
    let mut bytes = serde_json::to_vec_pretty(settings).map_err(SettingsError::Json)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(SettingsError::SettingsTooLarge);
    }
    let parent = path.parent().ok_or_else(|| {
        io_error(
            "locate the settings directory",
            io::ErrorKind::NotFound.into(),
        )
    })?;
    fs::create_dir_all(parent).map_err(|e| io_error("create the settings directory", e))?;
    AtomicFile::new(path, AllowOverwrite)
        .write(|file| {
            file.write_all(&bytes)?;
            file.flush()?;
            file.sync_all()
        })
        .map_err(|error| io_error("atomically replace", error.into()))?;
    sync_directory(path)
}

fn write_settings_unlocked(settings: &Settings) -> Result<(), SettingsError> {
    let path = settings_path()?;
    write_settings_at(&path, settings, sync_parent)
}

fn remove_profile_reset_tree(path: &Path) -> Result<(), SettingsError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error("inspect profile data for the v0.1.6 reset", error)),
    };
    if metadata.file_type().is_symlink() {
        return fs::remove_file(path)
            .map_err(|error| io_error("remove linked profile data for the v0.1.6 reset", error));
    }
    if !metadata.is_dir() {
        return Err(io_error(
            "validate profile data for the v0.1.6 reset",
            io::Error::new(
                io::ErrorKind::InvalidData,
                "profile data path is not a directory",
            ),
        ));
    }
    fs::remove_dir_all(path)
        .map_err(|error| io_error("remove profile data for the v0.1.6 reset", error))
}

fn write_profile_reset_marker(path: &Path) -> Result<(), SettingsError> {
    AtomicFile::new(path, AllowOverwrite)
        .write(|file| {
            file.write_all(b"v0.1.6\n")?;
            file.flush()?;
            file.sync_all()
        })
        .map_err(|error| io_error("record the v0.1.6 profile reset", error.into()))?;
    sync_parent(path)
}

fn reset_v016_profiles_at(
    app_data: &Path,
    managed_roots: &[PathBuf],
    saved: &Settings,
) -> Result<Settings, SettingsError> {
    let marker = app_data.join(V016_PROFILE_RESET_MARKER);
    match fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.is_file() => return Ok(saved.clone()),
        Ok(_) => {
            return Err(io_error(
                "validate the v0.1.6 profile reset marker",
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "profile reset marker is not a regular file",
                ),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(io_error("inspect the v0.1.6 profile reset marker", error));
        }
    }

    for root in managed_roots {
        let metadata = fs::symlink_metadata(root)
            .map_err(|error| io_error("access managed data for the v0.1.6 reset", error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io_error(
                "validate managed data for the v0.1.6 reset",
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "managed data root is not a regular directory",
                ),
            ));
        }
    }

    remove_profile_reset_tree(&app_data.join("profiles"))?;
    for root in managed_roots {
        remove_profile_reset_tree(&root.join("managed-games"))?;
    }

    let reset = Settings {
        storage_path: saved.storage_path.clone(),
        ..Settings::default()
    };
    write_settings_at(&app_data.join("settings.json"), &reset, sync_parent)?;
    write_profile_reset_marker(&marker)?;
    Ok(reset)
}

/// Delete every pre-v0.1.6 profile and managed game artifact exactly once.
/// The selected large-data root remains configured, while setup, sources,
/// personal mods, and profile selection return to first-run defaults.
pub fn reset_v016_profiles_once(saved: &Settings) -> Result<Settings, SettingsError> {
    let _guard = lock_settings()?;
    let app_data = app_data_dir();
    let mut managed_roots = vec![
        managed_data_dir(),
        default_managed_data_dir(),
        app_data.clone(),
    ];
    if let Some(configured) = saved.storage_path.as_deref() {
        managed_roots.push(PathBuf::from(configured));
    }
    let reset = reset_v016_profiles_at(&app_data, &managed_roots, saved)?;
    if reset.setup_complete != saved.setup_complete
        || reset.active_profile != saved.active_profile
        || reset.game_instances.len() != saved.game_instances.len()
    {
        log::info!("completed the one-time v0.1.6 profile reset");
    }
    Ok(reset)
}

fn replace_github_token_unlocked(token: Option<&SecretString>) -> Result<(), SettingsError> {
    match token {
        Some(token) => set_github_token_unlocked(token.clone()),
        None => match keyring_entry()?.delete_password() {
            Ok(()) | Err(keyring::Error::NoEntry) => {
                log::info!("GitHub credential was cleared from the OS keyring");
                Ok(())
            }
            Err(error) => Err(SettingsError::Keyring(error)),
        },
    }
}

fn rollback_failure(
    settings: Result<(), SettingsError>,
    token: Result<(), SettingsError>,
) -> Option<String> {
    match (settings, token) {
        (Ok(()), Ok(())) => None,
        (Err(settings), Ok(())) => Some(format!("settings restore failed: {settings}")),
        (Ok(()), Err(token)) => Some(format!("credential restore failed: {token}")),
        (Err(settings), Err(token)) => Some(format!(
            "settings restore failed: {settings}; credential restore failed: {token}"
        )),
    }
}

fn apply_transaction_with<Save, ReplaceToken>(
    old_settings: &Settings,
    old_token: Option<&SecretString>,
    new_settings: &Settings,
    token_action: &TokenAction,
    mut save: Save,
    mut replace_token: ReplaceToken,
) -> Result<(), SettingsError>
where
    Save: FnMut(&Settings) -> Result<(), SettingsError>,
    ReplaceToken: FnMut(Option<&SecretString>) -> Result<(), SettingsError>,
{
    if let Err(operation) = save(new_settings) {
        let rollback = rollback_failure(save(old_settings), replace_token(old_token));
        return Err(SettingsError::Transaction {
            operation: Box::new(operation),
            rollback,
        });
    }

    let replacement = match token_action {
        TokenAction::Unchanged => return Ok(()),
        TokenAction::Set { token } => Some(SecretString::new(token.clone())),
        TokenAction::Clear => None,
    };
    if let Err(operation) = replace_token(replacement.as_ref()) {
        let rollback = rollback_failure(save(old_settings), replace_token(old_token));
        return Err(SettingsError::Transaction {
            operation: Box::new(operation),
            rollback,
        });
    }
    Ok(())
}

fn make_view(
    settings: Settings,
    has_github_token: bool,
    recovery_warning: Option<String>,
) -> SettingsView {
    let active = managed_data_dir();
    let default = default_managed_data_dir();
    let configured = settings.storage_path.as_deref().map(Path::new);
    let storage_warning = configured
        .filter(|configured| *configured != active)
        .map(|configured| {
            format!(
                "The configured storage folder {} is unavailable. Perfect Sync is using {} for this session.",
                configured.display(),
                active.display()
            )
        });
    SettingsView {
        settings,
        has_github_token,
        recovery_warning,
        active_storage_path: active.to_string_lossy().into_owned(),
        default_storage_path: default.to_string_lossy().into_owned(),
        storage_warning,
    }
}

pub fn apply_transaction(
    settings: &Settings,
    token_action: &TokenAction,
) -> Result<SettingsView, SettingsError> {
    let _guard = lock_settings()?;
    // Loading first migrates and scrubs a legacy plaintext token before a
    // clear can remove the keyring copy, so the plaintext cannot be re-imported.
    let (old_settings, recovery_warning) = load_unlocked()?;
    let old_token = github_token_unlocked()?;
    apply_transaction_with(
        &old_settings,
        old_token.as_ref(),
        settings,
        token_action,
        write_settings_unlocked,
        replace_github_token_unlocked,
    )?;
    let has_github_token = match token_action {
        TokenAction::Unchanged => old_token.is_some(),
        TokenAction::Set { .. } => true,
        TokenAction::Clear => false,
    };
    Ok(make_view(
        settings.clone(),
        has_github_token,
        recovery_warning,
    ))
}

fn finish_parsed_settings_with<Migrate, Write>(
    parsed: ParsedSettings,
    migrate: Migrate,
    write: Write,
) -> Result<Settings, SettingsError>
where
    Migrate: FnOnce(Option<String>) -> Result<bool, SettingsError>,
    Write: FnOnce(&Settings) -> Result<(), SettingsError>,
{
    if parsed.needs_scrub {
        migrate(parsed.legacy_token)?;
        write(&parsed.settings)?;
        log::info!("legacy GitHub credential was removed from settings");
    }
    Ok(parsed.settings)
}

fn load_unlocked() -> Result<(Settings, Option<String>), SettingsError> {
    match read_settings_unlocked()? {
        ReadSettings::Missing => Ok((Settings::default(), None)),
        ReadSettings::Recovered(warning) => Ok((Settings::default(), Some(warning))),
        ReadSettings::Valid(parsed) => {
            let settings = finish_parsed_settings_with(
                parsed,
                migrate_token_unlocked,
                write_settings_unlocked,
            )?;
            Ok((settings, None))
        }
    }
}

pub fn load() -> Result<Settings, SettingsError> {
    let _guard = lock_settings()?;
    load_unlocked().map(|(settings, _)| settings)
}

pub fn save(settings: &Settings) -> Result<(), SettingsError> {
    let _guard = lock_settings()?;
    match read_settings_unlocked()? {
        ReadSettings::Valid(parsed) => {
            if parsed.needs_scrub {
                migrate_token_unlocked(parsed.legacy_token)?;
                log::info!("legacy GitHub credential was removed from settings");
            }
        }
        ReadSettings::Recovered(_) => {
            log::warn!("recovered malformed settings before saving");
        }
        ReadSettings::Missing => {}
    }
    write_settings_unlocked(settings)
}
pub fn update_game_instance_source(
    expected: &GameInstance,
    refreshed: &GameInstance,
) -> Result<bool, SettingsError> {
    let _guard = lock_settings()?;
    let (mut settings, _) = load_unlocked()?;
    let Some(instance) = settings
        .game_instances
        .iter_mut()
        .find(|instance| instance.id == expected.id)
    else {
        return Ok(false);
    };
    if refreshed.id != expected.id
        || instance.path != expected.path
        || instance.arch != expected.arch
        || instance.store != expected.store
        || instance.runtime != expected.runtime
        || instance.build != expected.build
        || instance.source_fingerprint != expected.source_fingerprint
        || instance.source_file_count != expected.source_file_count
        || instance.source_byte_count != expected.source_byte_count
    {
        return Ok(false);
    }
    instance.executable_identity = refreshed.executable_identity.clone();
    instance.source_fingerprint = refreshed.source_fingerprint.clone();
    instance.source_file_count = refreshed.source_file_count;
    instance.source_byte_count = refreshed.source_byte_count;
    instance.runtime = refreshed.runtime;
    instance.build = refreshed.build.clone();
    instance.writable = refreshed.writable;
    write_settings_unlocked(&settings)?;
    Ok(true)
}

pub fn set_active_profile(profile_id: &str) -> Result<(), SettingsError> {
    let _guard = lock_settings()?;
    let (mut settings, _) = load_unlocked()?;
    if settings.active_profile.as_deref() == Some(profile_id) {
        return Ok(());
    }
    settings.active_profile = Some(profile_id.to_string());
    write_settings_unlocked(&settings)
}

pub fn view() -> Result<SettingsView, SettingsError> {
    let _guard = lock_settings()?;
    let (settings, recovery_warning) = load_unlocked()?;
    let has_github_token = github_token_unlocked()?.is_some();
    Ok(make_view(settings, has_github_token, recovery_warning))
}

pub fn github_token() -> Result<Option<SecretString>, SettingsError> {
    let _guard = lock_settings()?;
    github_token_unlocked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v016_profile_reset_is_complete_and_idempotent() {
        let temporary = tempfile::tempdir().unwrap();
        let app_data = temporary.path().join("app-data");
        let local_data = temporary.path().join("local-data");
        let custom_data = temporary.path().join("custom-data");
        let managed_roots = [custom_data.clone(), local_data.clone(), app_data.clone()];
        for root in &managed_roots {
            fs::create_dir_all(root.join("managed-games/workspace/old-profile")).unwrap();
            fs::create_dir_all(root.join("cache")).unwrap();
            fs::write(root.join("cache/keep.bin"), b"cache").unwrap();
        }
        fs::create_dir_all(app_data.join("profiles/old-profile/BepInEx/plugins")).unwrap();
        fs::write(
            app_data.join("profiles/old-profile/profile.json"),
            b"old profile",
        )
        .unwrap();
        fs::write(app_data.join("catalog.json"), b"catalog").unwrap();

        let saved = Settings {
            game_instances: vec![GameInstance {
                id: "steam".into(),
                name: "Steam".into(),
                path: "C:/Among Us".into(),
                executable_identity: Some("identity".into()),
                source_fingerprint: None,
                source_file_count: None,
                source_byte_count: None,
                arch: Arch::X86,
                store: Store::Steam,
                runtime: Runtime::Native,
                build: Some("2026.3.31".into()),
                writable: true,
            }],
            personal_mods: vec![PersonalMod {
                repo: "owner/mod".into(),
                tag: "v1".into(),
                asset: "mod.dll".into(),
                name: Some("Mod".into()),
                enabled: true,
            }],
            personal_local_mods: vec![PersonalLocalMod {
                path: "C:/mods/local.dll".into(),
                name: "Local".into(),
                enabled: true,
            }],
            setup_complete: true,
            fresh_source_setup_complete: true,
            skip_launch_warning: true,
            active_profile: Some("old-profile".into()),
            storage_path: Some(custom_data.to_string_lossy().into_owned()),
        };
        write_settings_at(&app_data.join("settings.json"), &saved, |_| Ok(())).unwrap();

        let reset = reset_v016_profiles_at(&app_data, &managed_roots, &saved).unwrap();
        assert!(reset.game_instances.is_empty());
        assert!(reset.personal_mods.is_empty());
        assert!(reset.personal_local_mods.is_empty());
        assert!(!reset.setup_complete);
        assert!(!reset.fresh_source_setup_complete);
        assert!(!reset.skip_launch_warning);
        assert!(reset.active_profile.is_none());
        assert_eq!(reset.storage_path, saved.storage_path);
        assert!(!app_data.join("profiles").exists());
        for root in &managed_roots {
            assert!(!root.join("managed-games").exists());
            assert_eq!(fs::read(root.join("cache/keep.bin")).unwrap(), b"cache");
        }
        assert_eq!(fs::read(app_data.join("catalog.json")).unwrap(), b"catalog");
        assert_eq!(
            fs::read(app_data.join(V016_PROFILE_RESET_MARKER)).unwrap(),
            b"v0.1.6\n"
        );
        let persisted: Settings =
            serde_json::from_slice(&fs::read(app_data.join("settings.json")).unwrap()).unwrap();
        assert!(!persisted.setup_complete);
        assert!(persisted.game_instances.is_empty());
        assert_eq!(persisted.storage_path, saved.storage_path);

        fs::create_dir_all(app_data.join("profiles/new-profile")).unwrap();
        fs::create_dir_all(custom_data.join("managed-games/workspace/new-profile")).unwrap();
        let mut completed = reset;
        completed.setup_complete = true;
        completed.active_profile = Some("new-profile".into());
        write_settings_at(&app_data.join("settings.json"), &completed, |_| Ok(())).unwrap();
        let unchanged = reset_v016_profiles_at(&app_data, &managed_roots, &completed).unwrap();
        assert!(unchanged.setup_complete);
        assert_eq!(unchanged.active_profile.as_deref(), Some("new-profile"));
        assert!(app_data.join("profiles/new-profile").is_dir());
        assert!(custom_data
            .join("managed-games/workspace/new-profile")
            .is_dir());
        let persisted: Settings =
            serde_json::from_slice(&fs::read(app_data.join("settings.json")).unwrap()).unwrap();
        assert!(persisted.setup_complete);
        assert_eq!(persisted.active_profile.as_deref(), Some("new-profile"));
    }

    #[test]
    fn personal_mod_without_enabled_defaults_on() {
        let pm: PersonalMod =
            serde_json::from_str(r#"{"repo":"a/b","tag":"v1","asset":"x.dll"}"#).unwrap();
        assert!(pm.enabled);
    }

    #[test]
    fn game_instances_round_trip() {
        let settings: Settings = serde_json::from_str(
            r#"{"gameInstances":[{"id":"steam","name":"Steam","path":"C:/Among Us","arch":"x86","store":"steam","runtime":"native"}]}"#,
        )
        .unwrap();
        assert_eq!(settings.game_instances[0].id, "steam");
        assert!(serde_json::to_string(&settings)
            .unwrap()
            .contains("\"gameInstances\""));
    }

    #[test]
    fn source_record_fields_round_trip_with_game_instance() {
        let settings: Settings = serde_json::from_str(
            r#"{"gameInstances":[{"id":"steam","name":"Steam","path":"C:/Among Us","sourceFingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","sourceFileCount":42,"sourceByteCount":4096,"arch":"x86","store":"steam","runtime":"native","build":"2026.8.4"}]}"#,
        )
        .unwrap();
        let instance = &settings.game_instances[0];
        assert_eq!(instance.source_file_count, Some(42));
        assert_eq!(instance.source_byte_count, Some(4096));
        let serialized = serde_json::to_string(&settings).unwrap();
        assert!(serialized.contains("\"sourceFingerprint\""));
        assert!(serialized.contains("\"sourceFileCount\":42"));
        assert!(serialized.contains("\"sourceByteCount\":4096"));
    }

    #[test]
    fn legacy_settings_require_fresh_source_setup_once() {
        let legacy: Settings = serde_json::from_str(r#"{"setupComplete":true}"#).unwrap();
        assert!(legacy.setup_complete);
        assert!(!legacy.fresh_source_setup_complete);

        let migrated: Settings =
            serde_json::from_str(r#"{"setupComplete":true,"freshSourceSetupComplete":true}"#)
                .unwrap();
        assert!(migrated.fresh_source_setup_complete);
    }

    #[test]
    fn migrates_the_legacy_single_game() {
        let text = r#"{"gamePath":"C:/Among Us","arch":"x64","store":"epic","runtime":"native"}"#;
        let game = migrate_legacy_game(text).unwrap();
        assert_eq!(game.path, "C:/Among Us");
        assert_eq!(game.arch, Arch::X64);
        assert_eq!(game.store, Store::Epic);
    }

    #[test]
    fn legacy_secret_and_catalog_fields_are_never_serialized() {
        let settings: Settings = serde_json::from_str(
            r#"{"githubToken":"secret-value","catalogUrl":"https://example.invalid","setupComplete":true}"#,
        )
        .unwrap();
        let serialized = serde_json::to_string(&settings).unwrap();
        assert!(!serialized.contains("githubToken"));
        assert!(!serialized.contains("catalogUrl"));
        assert!(!serialized.contains("secret-value"));
    }

    #[test]
    fn settings_view_exposes_only_credential_presence() {
        let view = SettingsView {
            settings: Settings::default(),
            has_github_token: true,
            recovery_warning: None,
            active_storage_path: "C:\\Perfect-Sync".into(),
            default_storage_path: "C:\\Perfect-Sync".into(),
            storage_warning: None,
        };
        let serialized = serde_json::to_string(&view).unwrap();
        assert!(serialized.contains("\"hasGithubToken\":true"));
        assert!(!serialized.contains("githubToken"));
    }

    #[test]
    fn relative_application_data_directory_is_rejected() {
        assert!(matches!(
            initialize_app_data_dir(PathBuf::from("Perfect-Sync")),
            Err(SettingsError::InvalidDataDirectory)
        ));
    }

    #[test]
    fn stale_legacy_token_does_not_replace_an_existing_credential() {
        use std::cell::Cell;

        let decoded =
            decode_settings(br#"{"gameInstances":"invalid","githubToken":"stale-token"}"#).unwrap();
        let DecodedSettings::Invalid {
            mut value,
            legacy_token,
        } = decoded
        else {
            panic!("settings payload should be malformed");
        };
        let stored = Cell::new(false);
        let migrated = migrate_token_with(
            legacy_token,
            || Ok(true),
            |_| {
                stored.set(true);
                Ok(())
            },
        )
        .unwrap();
        redact_legacy_secrets(&mut value);
        let sanitized = serde_json::to_string(&value).unwrap();

        assert!(migrated);
        assert!(!stored.get());
        assert!(!sanitized.contains("githubToken"));
        assert!(!sanitized.contains("stale-token"));
    }

    #[test]
    fn credential_lookup_error_aborts_before_storage() {
        use std::cell::Cell;

        let stored = Cell::new(false);
        let result = migrate_token_with(
            Some("legacy-token".to_string()),
            || {
                Err(io_error(
                    "query test credential",
                    io::ErrorKind::PermissionDenied.into(),
                ))
            },
            |_| {
                stored.set(true);
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(!stored.get());
    }

    #[test]
    fn valid_settings_survive_a_malformed_legacy_secret_field() {
        let decoded =
            decode_settings(br#"{"setupComplete":true,"githubToken":{"invalid":"secret"}}"#)
                .unwrap();
        let DecodedSettings::Valid(parsed) = decoded else {
            panic!("settings payload should remain valid");
        };

        assert!(parsed.settings.setup_complete);
        assert!(parsed.needs_scrub);
        assert!(parsed.legacy_token.is_none());
    }

    #[test]
    fn malformed_parseable_settings_are_sanitized_before_quarantine() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(
            &path,
            br#"{"gameInstances":"invalid","githubToken":{"value":"secret-value"}}"#,
        )
        .unwrap();

        let recovered = read_settings_at(&path).unwrap();
        let ReadSettings::Recovered(warning) = recovered else {
            panic!("malformed settings should be recovered");
        };
        let recovery_name = warning
            .split_whitespace()
            .last()
            .unwrap()
            .trim_end_matches('.');
        let recovery = fs::read(directory.path().join(recovery_name)).unwrap();

        assert!(!path.exists());
        assert!(!recovery
            .windows(b"githubToken".len())
            .any(|window| window == b"githubToken"));
        assert!(!recovery
            .windows(b"secret-value".len())
            .any(|window| window == b"secret-value"));
        assert!(warning.contains("sanitized copy"));
        assert!(!warning.contains("original bytes"));
    }

    #[test]
    fn unparseable_settings_are_neither_duplicated_nor_removed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let bytes = b"{\"githubToken\":\"possible-secret\"";
        fs::write(&path, bytes).unwrap();
        let error = match read_settings_at(&path) {
            Err(error @ SettingsError::ManualRecoveryRequired) => error,
            _ => panic!("unparseable settings should require manual recovery"),
        };
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(!error.to_string().contains("possible-secret"));
    }

    #[test]
    fn oversized_settings_are_neither_duplicated_nor_removed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let bytes = vec![b'x'; MAX_SETTINGS_BYTES as usize + 1];
        fs::write(&path, &bytes).unwrap();

        let result = read_settings_at(&path);
        assert!(matches!(result, Err(SettingsError::ManualRecoveryRequired)));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn quarantine_syncs_each_durable_name_transition() {
        use std::cell::RefCell;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, b"invalid").unwrap();
        let states = RefCell::new(Vec::new());

        quarantine_with_sync(&path, b"sanitized", |_| {
            let names = fs::read_dir(directory.path()).unwrap().count();
            states.borrow_mut().push((path.exists(), names));
            Ok(())
        })
        .unwrap();

        assert_eq!(*states.borrow(), [(true, 2), (false, 1)]);
    }

    #[test]
    fn atomic_settings_publication_has_a_parent_directory_barrier() {
        use std::cell::Cell;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let barriers = Cell::new(0);
        write_settings_at(&path, &Settings::default(), |_| {
            assert!(path.exists());
            barriers.set(barriers.get() + 1);
            Ok(())
        })
        .unwrap();

        assert_eq!(barriers.get(), 1);
    }
    #[test]
    fn oversized_save_preserves_the_published_settings() {
        use std::cell::Cell;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let old = Settings {
            setup_complete: true,
            ..Settings::default()
        };
        write_settings_at(&path, &old, |_| Ok(())).unwrap();
        let original = fs::read(&path).unwrap();
        let oversized = Settings {
            personal_mods: vec![PersonalMod {
                repo: "Owner/Repo".into(),
                tag: "v1".into(),
                asset: "mod.dll".into(),
                name: Some("x".repeat(MAX_SETTINGS_BYTES as usize)),
                enabled: true,
            }],
            ..Settings::default()
        };
        let barriers = Cell::new(0);

        let error = write_settings_at(&path, &oversized, |_| {
            barriers.set(barriers.get() + 1);
            Ok(())
        })
        .unwrap_err();

        assert!(matches!(error, SettingsError::SettingsTooLarge));
        assert_eq!(barriers.get(), 0);
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn legacy_token_is_migrated_and_scrubbed_before_clear() {
        use std::cell::RefCell;

        let DecodedSettings::Valid(parsed) =
            decode_settings(br#"{"setupComplete":true,"githubToken":"legacy"}"#).unwrap()
        else {
            panic!("settings should decode");
        };
        let events = RefCell::new(Vec::new());
        let normalized = finish_parsed_settings_with(
            parsed,
            |token| {
                assert_eq!(token.as_deref(), Some("legacy"));
                events.borrow_mut().push("migrate");
                Ok(true)
            },
            |settings| {
                assert!(!serde_json::to_string(settings)
                    .unwrap()
                    .contains("githubToken"));
                events.borrow_mut().push("scrub");
                Ok(())
            },
        )
        .unwrap();
        events.borrow_mut().push("clear");

        assert!(normalized.setup_complete);
        assert_eq!(*events.borrow(), ["migrate", "scrub", "clear"]);
    }

    #[test]
    fn combined_mutation_rolls_back_settings_and_token_on_failure() {
        use std::cell::{Cell, RefCell};

        let old_settings = Settings {
            setup_complete: false,
            ..Settings::default()
        };
        let new_settings = Settings {
            setup_complete: true,
            ..Settings::default()
        };
        let old_token = SecretString::new("old-token".into());
        let published_settings = RefCell::new(old_settings.clone());
        let published_token = RefCell::new(Some("old-token".to_string()));
        let reject_new_token = Cell::new(true);

        let result = apply_transaction_with(
            &old_settings,
            Some(&old_token),
            &new_settings,
            &TokenAction::Set {
                token: "new-token".into(),
            },
            |settings| {
                *published_settings.borrow_mut() = settings.clone();
                Ok(())
            },
            |token| {
                let value = token.map(|token| token.expose_secret().to_string());
                if value.as_deref() == Some("new-token") && reject_new_token.replace(false) {
                    return Err(io_error(
                        "inject credential failure",
                        io::ErrorKind::PermissionDenied.into(),
                    ));
                }
                *published_token.borrow_mut() = value;
                Ok(())
            },
        );

        assert!(matches!(result, Err(SettingsError::Transaction { .. })));
        assert!(!published_settings.borrow().setup_complete);
        assert_eq!(published_token.borrow().as_deref(), Some("old-token"));
    }
}
