//! ProfileManager: persist profile records and manage a profile's on-disk
//! `BepInEx/plugins` (install a mod DLL, extract a release zip, enable/disable
//! a plugin). All operations take an explicit `profiles_root` so they are
//! tested against temp directories.
//!
//! Records serialize in camelCase to match the frontend's TypeScript types
//! (`packageId`, `crewColor`, `gameBuild`).

use crate::types::{
    valid_levelimposter_map_id, LobbyManifest, ManifestMod, ModSource, ModTag, MAX_MANIFEST_MAPS,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const MAX_PROFILE_ID_BYTES: usize = 64;
const MAX_DLL_NAME_BYTES: usize = 180;
const MAX_PROFILE_JSON_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PLUGIN_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PLUGIN_ARCHIVE_ENTRIES: usize = 4_096;
const MAX_PLUGIN_ARCHIVE_PATH_BYTES: usize = 1_024;
static SAVE_LOCK: Mutex<()> = Mutex::new(());
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledMod {
    pub package_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo: Option<String>,
    pub version: String,
    #[serde(default)]
    pub versions: Vec<String>,
    pub enabled: bool,
    pub source: ModSource,
    pub tags: Vec<ModTag>,
    #[serde(default)]
    pub managed: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub update: Option<String>,
    /// installed plugin file name, used to enable/disable/remove the mod
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub asset: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRecord {
    pub id: String,
    pub name: String,
    pub crew_color: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub game_build: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub game_instance_id: Option<String>,
    #[serde(default)]
    pub mods: Vec<InstalledMod>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levelimposter_maps: Vec<String>,
}

/// A directory of profiles, each `<root>/<id>/profile.json` plus its BepInEx tree.
pub struct ProfileStore {
    pub root: PathBuf,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn is_windows_device_name(name: &str) -> bool {
    let stem = name
        .trim_end_matches(['.', ' '])
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

/// Validate the one portable component used for every profile directory.
pub fn validate_profile_id(id: &str) -> io::Result<()> {
    if id.is_empty()
        || id.len() > MAX_PROFILE_ID_BYTES
        || !id.is_ascii()
        || id.ends_with('.')
        || id.ends_with(' ')
        || is_windows_device_name(id)
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(invalid("invalid profile id"));
    }
    let mut components = Path::new(id).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(invalid("invalid profile id"));
    }
    Ok(())
}

/// Validate a single ASCII-only portable DLL filename. Paths and disabled-name suffixes are not accepted.
pub fn validate_dll_name(name: &str) -> io::Result<()> {
    if !name.is_ascii() {
        return Err(invalid(
            "DLL file name must be an ASCII-only portable basename",
        ));
    }
    if name.is_empty()
        || name.len() > MAX_DLL_NAME_BYTES
        || name.ends_with('.')
        || name.ends_with(' ')
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
        || is_windows_device_name(name)
        || !name
            .rsplit_once('.')
            .is_some_and(|(stem, ext)| !stem.is_empty() && ext.eq_ignore_ascii_case("dll"))
    {
        return Err(invalid("invalid DLL file name"));
    }
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(invalid("invalid DLL file name"));
    }
    Ok(())
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

pub(crate) fn reject_reparse(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_reparse(&metadata) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing reparse-point path {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

pub(crate) fn unique_sibling(path: &Path, label: &str) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("destination has no parent directory"))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("destination has no portable file name"))?;
    for _ in 0..128 {
        let number = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{name}.{label}.{}.{number}", std::process::id()));
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique temporary path",
    ))
}

#[cfg(windows)]
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
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
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn checked_profile_dir(root: &Path, id: &str, create: bool) -> io::Result<PathBuf> {
    validate_profile_id(id)?;
    reject_reparse(root)?;
    let dir = root.join(id);
    reject_reparse(&dir)?;
    if create {
        fs::create_dir_all(&dir)?;
        reject_reparse(&dir)?;
    }
    Ok(dir)
}

fn validate_record(profile: &ProfileRecord) -> io::Result<()> {
    validate_profile_id(&profile.id)?;
    let mut files = HashSet::new();
    for file in profile.mods.iter().filter_map(|m| m.file.as_deref()) {
        validate_dll_name(file)?;
        if !files.insert(file.to_ascii_lowercase()) {
            return Err(invalid(
                "profile contains duplicate or case-colliding DLL filenames",
            ));
        }
    }
    if profile.levelimposter_maps.len() > MAX_MANIFEST_MAPS {
        return Err(invalid("profile contains too many LevelImposter maps"));
    }
    let mut maps = HashSet::with_capacity(profile.levelimposter_maps.len());
    for id in &profile.levelimposter_maps {
        if !valid_levelimposter_map_id(id) || !maps.insert(id.to_ascii_lowercase()) {
            return Err(invalid(
                "profile contains invalid or duplicate LevelImposter map ids",
            ));
        }
    }
    Ok(())
}

impl ProfileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn profile_dir(&self, id: &str) -> io::Result<PathBuf> {
        checked_profile_dir(&self.root, id, false)
    }

    fn reject_case_collision(&self, id: &str) -> io::Result<()> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = entry?;
            if let Some(other) = entry.file_name().to_str() {
                if other != id && other.eq_ignore_ascii_case(id) {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "profile id collides case-insensitively with an existing profile",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn save(&self, profile: &ProfileRecord) -> io::Result<()> {
        self.save_with_publisher(profile, atomic_replace)
    }

    fn save_with_publisher<F>(&self, profile: &ProfileRecord, publish: F) -> io::Result<()>
    where
        F: FnOnce(&Path, &Path) -> io::Result<()>,
    {
        validate_record(profile)?;
        let json = serde_json::to_vec_pretty(profile)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if json.len() as u64 > MAX_PROFILE_JSON_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile manifest exceeds the size limit",
            ));
        }

        let _guard = SAVE_LOCK
            .lock()
            .map_err(|_| io::Error::other("profile save lock is poisoned"))?;
        reject_reparse(&self.root)?;
        fs::create_dir_all(&self.root)?;
        reject_reparse(&self.root)?;
        self.reject_case_collision(&profile.id)?;

        let dir = checked_profile_dir(&self.root, &profile.id, false)?;
        let created_dir = match fs::create_dir(&dir) {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(error),
        };
        let manifest = dir.join("profile.json");
        let mut tmp = None;
        let result = (|| {
            reject_reparse(&dir)?;
            reject_reparse(&manifest)?;
            if manifest.exists() {
                self.load(&profile.id)?;
            }
            let tmp_path = unique_sibling(&manifest, "tmp")?;
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&tmp_path)?;
            tmp = Some(tmp_path.clone());
            file.write_all(&json)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            sync_parent(&dir)?;
            reject_reparse(&manifest)?;
            publish(&tmp_path, &manifest)
        })();
        if result.is_err() {
            if let Some(tmp) = tmp {
                match fs::remove_file(tmp) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(_) => {}
                }
            }
            if created_dir && reject_reparse(&dir).is_ok() {
                match fs::remove_dir_all(&dir) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(_) => {}
                }
            }
        }
        result
    }

    /// `Ok(None)` means absent. Invalid, unreadable, or corrupt data is an error.
    pub fn load(&self, id: &str) -> io::Result<Option<ProfileRecord>> {
        let dir = checked_profile_dir(&self.root, id, false)?;
        let manifest = dir.join("profile.json");
        reject_reparse(&manifest)?;
        let metadata = match fs::metadata(&manifest) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if metadata.len() > MAX_PROFILE_JSON_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile manifest exceeds the size limit",
            ));
        }
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "profile manifest is not a regular file",
            ));
        }
        let text = fs::read_to_string(&manifest)?;
        let profile: ProfileRecord = serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        validate_record(&profile)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        if profile.id != id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "persisted profile id does not match its directory",
            ));
        }
        Ok(Some(profile))
    }

    pub fn list(&self) -> io::Result<Vec<ProfileRecord>> {
        reject_reparse(&self.root)?;
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut ids = HashSet::new();
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "profile root contains a symlink or reparse point",
                ));
            }
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().into_string().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "non-Unicode profile id")
            })?;
            if !ids.insert(name.to_ascii_lowercase()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "profile ids collide case-insensitively",
                ));
            }
            validate_profile_id(&name)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            let profile = self.load(&name)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "profile directory has no manifest",
                )
            })?;
            out.push(profile);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    pub fn delete(&self, id: &str) -> io::Result<()> {
        let dir = checked_profile_dir(&self.root, id, false)?;
        match fs::symlink_metadata(&dir) {
            Ok(metadata) if is_reparse(&metadata) => {
                return Err(invalid("refusing to delete a profile reparse point"));
            }
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(dir)?,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "profile is not a directory",
                ))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(())
    }
}

/// Encode a profile's enabled, shareable mods into a lobby manifest.
/// Mod versions, release assets, and LevelImposter maps are preserved exactly.
pub fn to_manifest(profile: &ProfileRecord) -> LobbyManifest {
    let levelimposter_enabled = profile.mods.iter().any(|installed| {
        installed.enabled
            && (installed
                .package_id
                .eq_ignore_ascii_case("DigiWorm0/LevelImposter")
                || installed
                    .repo
                    .as_deref()
                    .is_some_and(|repo| repo.eq_ignore_ascii_case("DigiWorm0/LevelImposter")))
    });
    LobbyManifest {
        v: 1,
        name: Some(profile.name.clone()),
        platform: None,
        game_build: None,
        mods: profile
            .mods
            .iter()
            .filter(|m| m.enabled && m.source != ModSource::File)
            .map(|m| ManifestMod {
                id: m.package_id.clone(),
                v: m.version.clone(),
                asset: m.asset.clone(),
            })
            .collect(),
        levelimposter_maps: if levelimposter_enabled {
            profile.levelimposter_maps.clone()
        } else {
            Vec::new()
        },
        loader: None,
    }
}

fn checked_plugins_dir(profiles_root: &Path, id: &str, create: bool) -> io::Result<PathBuf> {
    let profile = checked_profile_dir(profiles_root, id, create)?;
    let bep = profile.join("BepInEx");
    let plugins = bep.join("plugins");
    for path in [&bep, &plugins] {
        reject_reparse(path)?;
    }
    if create {
        fs::create_dir_all(&plugins)?;
        for path in [&bep, &plugins] {
            reject_reparse(path)?;
        }
    }
    Ok(plugins)
}

fn reject_case_collision(dir: &Path, name: &str) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let other = entry.file_name().into_string().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "non-Unicode plugin filename")
        })?;
        if other != name && other.eq_ignore_ascii_case(name) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "plugin filename collides case-insensitively",
            ));
        }
    }
    Ok(())
}

fn publish_plugin<R: Read>(
    profiles_root: &Path,
    id: &str,
    name: &str,
    mut reader: R,
) -> io::Result<PathBuf> {
    validate_dll_name(name)?;
    let plugins = checked_plugins_dir(profiles_root, id, true)?;
    reject_case_collision(&plugins, name)?;
    let destination = plugins.join(name);
    reject_reparse(&destination)?;
    let tmp = unique_sibling(&destination, "install")?;
    let result = (|| {
        let mut output = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        let written = io::copy(&mut reader.by_ref().take(MAX_PLUGIN_BYTES + 1), &mut output)?;
        if written == 0 || written > MAX_PLUGIN_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin DLL is empty or exceeds the expanded-size limit",
            ));
        }
        output.sync_all()?;
        sync_parent(&plugins)?;
        reject_reparse(&destination)?;
        atomic_replace(&tmp, &destination)?;
        Ok(destination.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Copy a bare mod DLL into a profile's plugins directory. Returns the path.
pub fn install_plugin_dll(profiles_root: &Path, id: &str, dll_src: &Path) -> io::Result<PathBuf> {
    reject_reparse(dll_src)?;
    let metadata = fs::metadata(dll_src)?;
    if !metadata.is_file() {
        return Err(invalid("plugin source is not a regular file"));
    }
    let name = dll_src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| invalid("source has no portable file name"))?;
    publish_plugin(profiles_root, id, name, File::open(dll_src)?)
}

/// Write downloaded plugin bytes into a profile's plugins directory atomically.
pub fn install_plugin_bytes(
    profiles_root: &Path,
    id: &str,
    file_name: &str,
    bytes: &[u8],
) -> io::Result<PathBuf> {
    publish_plugin(profiles_root, id, file_name, Cursor::new(bytes))
}

/// Extract one catalog-declared DLL from a release ZIP without materializing
/// any archive-controlled path. The expected DLL must occur exactly once.
pub fn install_plugin_zip_bytes(
    profiles_root: &Path,
    id: &str,
    dll_name: &str,
    bytes: &[u8],
) -> io::Result<PathBuf> {
    validate_profile_id(id)?;
    validate_dll_name(dll_name)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if archive.is_empty() || archive.len() > MAX_PLUGIN_ARCHIVE_ENTRIES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "plugin ZIP is empty or has too many entries",
        ));
    }

    let mut selected = None;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let entry_name = entry.name();
        if entry_name.len() > MAX_PLUGIN_ARCHIVE_PATH_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin ZIP contains an overlong path",
            ));
        }
        let basename = entry_name.rsplit('/').next().unwrap_or_default();
        if !basename.eq_ignore_ascii_case(dll_name) {
            continue;
        }
        if entry.is_dir() || entry.enclosed_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin ZIP uses an unsafe path for the declared DLL",
            ));
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin ZIP declares the DLL as a symbolic link",
            ));
        }
        if entry.size() == 0 || entry.size() > MAX_PLUGIN_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin DLL is empty or exceeds the expanded-size limit",
            ));
        }
        if selected.replace(index).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin ZIP contains the declared DLL more than once",
            ));
        }
    }

    let index = selected.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("plugin ZIP does not contain {dll_name}"),
        )
    })?;
    let entry = archive
        .by_index(index)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    publish_plugin(profiles_root, id, dll_name, entry)
}

/// Remove a plugin file (enabled or `.disabled`) from a profile.
pub fn remove_plugin(profiles_root: &Path, id: &str, file_name: &str) -> io::Result<()> {
    validate_dll_name(file_name)?;
    let plugins = checked_plugins_dir(profiles_root, id, false)?;
    for candidate in [
        plugins.join(file_name),
        plugins.join(format!("{file_name}.disabled")),
    ] {
        reject_reparse(&candidate)?;
        match fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Enable/disable a plugin by toggling a `.disabled` suffix (BepInEx only loads `.dll`).
pub fn set_plugin_enabled(
    profiles_root: &Path,
    id: &str,
    dll_name: &str,
    enabled: bool,
) -> io::Result<()> {
    validate_dll_name(dll_name)?;
    let plugins = checked_plugins_dir(profiles_root, id, false)?;
    let active = plugins.join(dll_name);
    let disabled = plugins.join(format!("{dll_name}.disabled"));
    reject_reparse(&active)?;
    reject_reparse(&disabled)?;
    let active_exists = match fs::metadata(&active) {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "plugin is not a regular file",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let disabled_exists = match fs::metadata(&disabled) {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "disabled plugin is not a regular file",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if active_exists && disabled_exists {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "both enabled and disabled plugin files exist",
        ));
    }
    sync_parent(&plugins)?;
    if enabled {
        if disabled_exists {
            atomic_replace(&disabled, &active)?;
        } else if !active_exists {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "plugin file not found",
            ));
        }
    } else if active_exists {
        atomic_replace(&active, &disabled)?;
    } else if !disabled_exists {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "plugin file not found",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader;

    fn sample_profile() -> ProfileRecord {
        ProfileRecord {
            id: "tou-night".into(),
            name: "ToU night".into(),
            crew_color: "#9b7bff".into(),
            game_build: Some("17.0.1".into()),
            game_instance_id: Some("steam".into()),
            mods: vec![InstalledMod {
                package_id: "AU-Avengers/TOU-Mira".into(),
                name: "Town of Us - Mira".into(),
                repo: Some("AU-Avengers/TOU-Mira".into()),
                version: "1.6.3".into(),
                versions: vec!["1.6.3".into()],
                enabled: true,
                source: ModSource::Github,
                tags: vec![ModTag::Role, ModTag::AllClient],
                managed: false,
                update: None,
                file: Some("TownOfUsMira.dll".into()),
                asset: Some("TownOfUsMira.zip".into()),
            }],
            levelimposter_maps: Vec::new(),
        }
    }

    #[test]
    fn store_round_trips_and_uses_camel_case() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(tmp.path());
        let p = sample_profile();
        store.save(&p).unwrap();

        // round-trip
        assert_eq!(store.load("tou-night").unwrap().unwrap(), p);
        let all = store.list().unwrap();
        assert_eq!(all.len(), 1);

        // serialized keys must match the TS types
        let raw = fs::read_to_string(tmp.path().join("tou-night").join("profile.json")).unwrap();
        assert!(raw.contains("\"packageId\""));
        assert!(raw.contains("\"crewColor\""));
        assert!(raw.contains("\"gameBuild\""));
        assert!(raw.contains("\"gameInstanceId\": \"steam\""));
        assert!(raw.contains("\"all-client\"")); // ModTag kebab
        assert!(raw.contains("\"github\"")); // ModSource lowercase

        store.delete("tou-night").unwrap();
        assert!(store.load("tou-night").unwrap().is_none());
    }

    #[test]
    fn failed_first_save_removes_the_profile_directory_it_created() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(tmp.path());
        let profile = sample_profile();
        let profile_dir = tmp.path().join(&profile.id);

        let error = store
            .save_with_publisher(&profile, |_, _| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected publication failure",
                ))
            })
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(!profile_dir.exists());
    }

    #[test]
    fn failed_update_preserves_the_existing_profile_and_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(tmp.path());
        let old_profile = sample_profile();
        store.save(&old_profile).unwrap();
        let profile_dir = tmp.path().join(&old_profile.id);
        let manifest = profile_dir.join("profile.json");
        let old_manifest = fs::read(&manifest).unwrap();
        let marker = profile_dir.join("keep");
        fs::write(&marker, b"existing profile data").unwrap();

        let mut update = old_profile.clone();
        update.name = "replacement".into();
        let error = store
            .save_with_publisher(&update, |_, _| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected publication failure",
                ))
            })
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(profile_dir.is_dir());
        assert_eq!(fs::read(manifest).unwrap(), old_manifest);
        assert_eq!(fs::read(marker).unwrap(), b"existing profile data");
        assert_eq!(store.load(&old_profile.id).unwrap(), Some(old_profile));
    }

    #[test]
    fn rejects_non_ascii_dll_names_as_non_portable() {
        for name in ["Mód.dll", "\u{212a}ey.dll"] {
            let error = validate_dll_name(name).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("ASCII-only portable basename"));
        }

        let tmp = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(tmp.path());
        let mut profile = sample_profile();
        profile.mods[0].file = Some("\u{212a}ey.dll".into());
        let error = store.save(&profile).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!tmp.path().join(&profile.id).exists());

        let error = install_plugin_bytes(tmp.path(), "safe", "\u{212a}ey.dll", b"x").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!tmp.path().join("safe").exists());
    }

    #[test]
    fn to_manifest_keeps_enabled_mods_and_round_trips() {
        let mut p = sample_profile();
        p.mods.push(InstalledMod {
            package_id: "Disabled/Mod".into(),
            name: "Disabled".into(),
            repo: None,
            version: "0.1".into(),
            versions: vec!["0.1".into()],
            enabled: false,
            source: ModSource::Github,
            tags: vec![],
            managed: false,
            update: None,
            file: None,
            asset: None,
        });
        let manifest = to_manifest(&p);
        // disabled mod is excluded
        assert_eq!(manifest.mods.len(), 1);
        assert_eq!(manifest.mods[0].id, "AU-Avengers/TOU-Mira");
        assert_eq!(manifest.mods[0].v, "1.6.3");
        assert_eq!(manifest.mods[0].asset.as_deref(), Some("TownOfUsMira.zip"));
        assert!(manifest.game_build.is_none());
        // survives a codec round-trip
        let code = crate::codec::encode(&manifest).unwrap();
        assert_eq!(crate::codec::decode(&code).unwrap(), manifest);
    }

    #[test]
    fn to_manifest_includes_enabled_libraries() {
        // Enabled libraries remain part of the exact requested mod set.
        let mut p = sample_profile();
        p.mods.push(InstalledMod {
            package_id: "NuclearPowered/Reactor".into(),
            name: "Reactor".into(),
            repo: Some("NuclearPowered/Reactor".into()),
            version: "2.3.0".into(),
            versions: vec!["2.3.0".into()],
            enabled: true,
            source: ModSource::Github,
            tags: vec![ModTag::Library],
            managed: true,
            update: None,
            file: Some("Reactor.dll".into()),
            asset: Some("Reactor.dll".into()),
        });
        let manifest = to_manifest(&p);
        assert_eq!(manifest.mods.len(), 2);
        let reactor = manifest
            .mods
            .iter()
            .find(|m| m.id == "NuclearPowered/Reactor")
            .unwrap();
        assert_eq!(reactor.v, "2.3.0");
        assert_eq!(reactor.asset.as_deref(), Some("Reactor.dll"));
    }

    #[test]
    fn to_manifest_includes_maps_only_with_enabled_levelimposter() {
        let map_id = "0ed1f569-eaf5-4ef6-b91c-f41ad78d4018";
        let mut profile = sample_profile();
        profile.levelimposter_maps.push(map_id.into());
        profile.mods.push(InstalledMod {
            package_id: "DigiWorm0/LevelImposter".into(),
            name: "LevelImposter".into(),
            repo: Some("DigiWorm0/LevelImposter".into()),
            version: "v0.21.2-beta".into(),
            versions: vec!["v0.21.2-beta".into()],
            enabled: true,
            source: ModSource::Github,
            tags: vec![ModTag::Map],
            managed: false,
            update: None,
            file: Some("LevelImposter.dll".into()),
            asset: Some("LevelImposter.dll".into()),
        });

        assert_eq!(to_manifest(&profile).levelimposter_maps, [map_id]);
        profile
            .mods
            .iter_mut()
            .find(|installed| installed.package_id == "DigiWorm0/LevelImposter")
            .unwrap()
            .enabled = false;
        assert!(to_manifest(&profile).levelimposter_maps.is_empty());
    }

    fn plugin_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut archive = zip::ZipWriter::new(&mut bytes);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, contents) in entries {
                archive.start_file(*name, options).unwrap();
                archive.write_all(contents).unwrap();
            }
            archive.finish().unwrap();
        }
        bytes.into_inner()
    }

    #[test]
    fn installs_exact_declared_dll_from_nested_release_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = plugin_zip(&[
            ("README.md", b"docs"),
            ("BepInEx/plugins/TownOfUs.dll", b"plugin"),
        ]);
        let destination =
            install_plugin_zip_bytes(tmp.path(), "p1", "TownOfUs.dll", &bytes).unwrap();
        assert_eq!(
            destination,
            loader::profile_plugins_dir(tmp.path(), "p1").join("TownOfUs.dll")
        );
        assert_eq!(fs::read(destination).unwrap(), b"plugin");
        assert!(!tmp.path().join("p1").join("README.md").exists());
    }

    #[test]
    fn rejects_ambiguous_or_missing_dll_in_release_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let duplicate = plugin_zip(&[("one/Mod.dll", b"one"), ("two/Mod.dll", b"two")]);
        assert_eq!(
            install_plugin_zip_bytes(tmp.path(), "p1", "Mod.dll", &duplicate)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let missing = plugin_zip(&[("Other.dll", b"other")]);
        assert_eq!(
            install_plugin_zip_bytes(tmp.path(), "p1", "Mod.dll", &missing)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn to_manifest_keeps_custom_github_release_and_asset() {
        // A mod not in the catalog still retains the exact installed GitHub release.
        let p = ProfileRecord {
            id: "p".into(),
            name: "Custom".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: vec![InstalledMod {
                package_id: "SomeUser/CoolMod".into(),
                name: "CoolMod".into(),
                repo: Some("SomeUser/CoolMod".into()),
                version: "1.2.3".into(),
                versions: vec!["1.2.3".into()],
                enabled: true,
                source: ModSource::Github,
                tags: vec![],
                managed: false,
                update: None,
                file: Some("CoolMod.dll".into()),
                asset: Some("CoolMod.dll".into()),
            }],
            levelimposter_maps: Vec::new(),
        };
        let m = to_manifest(&p);
        // The recipient derives the repository from the id and installs this exact asset.
        assert_eq!(m.mods[0].id, "SomeUser/CoolMod");
        assert_eq!(m.mods[0].v, "1.2.3");
        assert_eq!(m.mods[0].asset.as_deref(), Some("CoolMod.dll"));
        assert_eq!(
            crate::resolver::parse_repo(&m.mods[0].id).as_deref(),
            Some("SomeUser/CoolMod")
        );
    }

    #[test]
    fn installs_bare_dll() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("Reactor.dll");
        fs::write(&src, b"dll-bytes").unwrap();
        let dest = install_plugin_dll(tmp.path(), "p1", &src).unwrap();
        assert!(dest.ends_with("Reactor.dll"));
        assert_eq!(fs::read(dest).unwrap(), b"dll-bytes");
    }

    #[test]
    fn toggles_plugin_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("Mod.dll");
        fs::write(&src, b"x").unwrap();
        install_plugin_dll(tmp.path(), "p1", &src).unwrap();
        let plugins = loader::profile_plugins_dir(tmp.path(), "p1");

        set_plugin_enabled(tmp.path(), "p1", "Mod.dll", false).unwrap();
        assert!(!plugins.join("Mod.dll").exists());
        assert!(plugins.join("Mod.dll.disabled").exists());

        set_plugin_enabled(tmp.path(), "p1", "Mod.dll", true).unwrap();
        assert!(plugins.join("Mod.dll").exists());
        assert!(!plugins.join("Mod.dll.disabled").exists());
    }

    #[test]
    fn delete_rejects_unsafe_ids_and_keeps_root() {
        let tmp = tempfile::tempdir().unwrap();
        let sentinel = tmp.path().join("keep.txt");
        fs::write(&sentinel, b"keep").unwrap();
        let store = ProfileStore::new(tmp.path());
        assert!(store.delete("").is_err());
        assert!(store.delete(".").is_err());
        assert!(store.delete("..").is_err());
        assert!(tmp.path().is_dir());
        assert!(sentinel.exists());
    }

    #[test]
    fn install_plugin_bytes_basenames_and_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = loader::profile_plugins_dir(tmp.path(), "p1");

        let dest = install_plugin_bytes(tmp.path(), "p1", "Cool.dll", b"x").unwrap();
        assert_eq!(dest, plugins.join("Cool.dll"));
        assert!(dest.exists());

        assert!(install_plugin_bytes(tmp.path(), "p1", "../evil.dll", b"x").is_err());
        assert!(!plugins.parent().unwrap().join("evil.dll").exists());
    }
    #[test]
    fn corrupt_or_mismatched_profiles_are_reported_and_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(tmp.path());
        let profile_dir = tmp.path().join("safe");
        fs::create_dir_all(&profile_dir).unwrap();
        let manifest = profile_dir.join("profile.json");
        fs::write(&manifest, b"{broken").unwrap();
        assert_eq!(
            store.load("safe").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let mut replacement = sample_profile();
        replacement.id = "safe".into();
        assert!(store.save(&replacement).is_err());
        assert_eq!(fs::read(&manifest).unwrap(), b"{broken");

        fs::write(&manifest, serde_json::to_vec(&sample_profile()).unwrap()).unwrap();
        assert_eq!(
            store.load("safe").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(store.list().is_err());
    }

    #[test]
    fn rejects_reserved_ids_and_dll_names_without_touching_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        let sentinel = tmp.path().join("sentinel");
        fs::write(&sentinel, b"keep").unwrap();
        for id in ["CON", "aux.txt", "bad.", "bad ", "C:\\escape", "/absolute"] {
            assert!(install_plugin_bytes(tmp.path(), id, "Safe.dll", b"x").is_err());
        }
        for name in [
            "CON.dll",
            "bad.dll.",
            "bad.dll ",
            "dir/mod.dll",
            "x.dll:stream",
        ] {
            assert!(install_plugin_bytes(tmp.path(), "safe", name, b"x").is_err());
        }
        assert_eq!(fs::read(sentinel).unwrap(), b"keep");
    }
}
