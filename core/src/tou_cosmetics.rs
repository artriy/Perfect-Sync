//! Town of Us - Mira ships its custom hats as two files beside the plugin DLL.
//! Perfect Sync installs the release's bare DLL, so launch preparation extracts
//! the matching `touhats.bundle` and `touhats.catalog` from the official full
//! release pack and tracks their exact content in the game plugins directory.

use crate::profile::ProfileRecord;
use crate::resolver::{Release, ResolvedDownload};
use crate::types::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Cursor, Read};
use std::path::Path;

pub const PACKAGE_ID: &str = "AU-Avengers/TOU-Mira";
pub const BUNDLE_NAME: &str = "touhats.bundle";
pub const CATALOG_NAME: &str = "touhats.catalog";
pub const MARKER_NAME: &str = ".perfectsync-tou-cosmetics.json";

const MARKER_SCHEMA: u8 = 1;
const MAX_ARCHIVE_ENTRIES: usize = 8_192;
const MAX_ARCHIVE_BYTES: u64 = 300 * 1024 * 1024;
const MAX_BUNDLE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CATALOG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_MARKER_BYTES: u64 = 4 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CosmeticsError {
    #[error("Town of Us release {version} has no Windows {arch} pack for {store:?}")]
    MissingPack {
        version: String,
        arch: String,
        store: Store,
    },
    #[error("Town of Us release {version} has multiple matching Windows {arch} packs")]
    AmbiguousPack { version: String, arch: String },
    #[error("Town of Us cosmetics pack is invalid: {0}")]
    InvalidPack(String),
    #[error("could not read Town of Us cosmetics pack: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("could not read Town of Us cosmetics: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosmeticsPayload {
    pub bundle: Vec<u8>,
    pub catalog: Vec<u8>,
    pub marker: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CosmeticsMarker {
    schema: u8,
    version: String,
    asset: String,
    bundle_sha256: String,
    catalog_sha256: String,
}

pub fn active_version(profile: &ProfileRecord) -> Option<&str> {
    profile
        .mods
        .iter()
        .find(|installed| {
            installed.enabled && installed.package_id.eq_ignore_ascii_case(PACKAGE_ID)
        })
        .map(|installed| installed.version.as_str())
        .filter(|version| !version.is_empty())
}

fn asset_tokens(name: &str) -> impl Iterator<Item = &str> {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
}

fn supports_store(tokens: &[&str], arch: &str, store: Store) -> bool {
    let has = |expected: &str| {
        tokens
            .iter()
            .any(|token| token.eq_ignore_ascii_case(expected))
    };
    match store {
        Store::Steam => has("steam"),
        Store::Epic => has("epic"),
        Store::Itch => has("itch"),
        Store::Msstore => has("msstore"),
        Store::Manual if arch == "x86" => has("steam") || has("itch"),
        Store::Manual if arch == "x64" => has("epic") || has("msstore"),
        Store::Manual => false,
    }
}

/// Select the official full Windows pack matching the executable architecture
/// and store. The bare DLL is intentionally ignored because it omits the hats.
pub fn select_release_pack(
    release: &Release,
    arch: &str,
    store: Store,
) -> Result<ResolvedDownload, CosmeticsError> {
    let mut candidates = release.assets.iter().filter(|asset| {
        if !asset.name.to_ascii_lowercase().ends_with(".zip") {
            return false;
        }
        let tokens: Vec<&str> = asset_tokens(&asset.name).collect();
        tokens.iter().any(|token| token.eq_ignore_ascii_case(arch))
            && supports_store(&tokens, arch, store)
    });
    let selected = candidates
        .next()
        .ok_or_else(|| CosmeticsError::MissingPack {
            version: release.tag.clone(),
            arch: arch.to_string(),
            store,
        })?;
    if candidates.next().is_some() {
        return Err(CosmeticsError::AmbiguousPack {
            version: release.tag.clone(),
            arch: arch.to_string(),
        });
    }
    if selected.size.bytes() == 0 || selected.size.bytes() > MAX_ARCHIVE_BYTES {
        return Err(CosmeticsError::InvalidPack(
            "release pack has an invalid declared size".into(),
        ));
    }
    Ok(ResolvedDownload {
        url: selected.url.clone(),
        asset_name: selected.name.clone(),
        version: release.tag.clone(),
        size: selected.size,
    })
}

fn archive_basename(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

fn read_entry(
    entry: &mut zip::read::ZipFile<'_>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>, CosmeticsError> {
    if entry.is_dir()
        || entry.size() == 0
        || entry.size() > limit
        || entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 != 0o100000)
    {
        return Err(CosmeticsError::InvalidPack(format!(
            "{name} is empty, oversized, or not a regular file"
        )));
    }
    let expected = entry.size();
    let mut bytes = Vec::with_capacity(expected as usize);
    entry
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(CosmeticsError::Io)?;
    if bytes.len() as u64 != expected {
        return Err(CosmeticsError::InvalidPack(format!(
            "{name} expanded size does not match its ZIP metadata"
        )));
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

/// Extract exactly one bundle/catalog pair from an official Town of Us pack.
pub fn extract_release_pack(
    archive: &[u8],
    version: &str,
    asset_name: &str,
) -> Result<CosmeticsPayload, CosmeticsError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))?;
    if zip.is_empty() || zip.len() > MAX_ARCHIVE_ENTRIES {
        return Err(CosmeticsError::InvalidPack(
            "archive has an invalid entry count".into(),
        ));
    }
    let mut bundle = None;
    let mut catalog = None;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let basename = archive_basename(entry.name());
        let target = if basename.eq_ignore_ascii_case(BUNDLE_NAME) {
            Some((BUNDLE_NAME, MAX_BUNDLE_BYTES, &mut bundle))
        } else if basename.eq_ignore_ascii_case(CATALOG_NAME) {
            Some((CATALOG_NAME, MAX_CATALOG_BYTES, &mut catalog))
        } else {
            None
        };
        let Some((name, limit, destination)) = target else {
            continue;
        };
        if destination.is_some() {
            return Err(CosmeticsError::InvalidPack(format!(
                "archive contains more than one {name}"
            )));
        }
        *destination = Some(read_entry(&mut entry, name, limit)?);
    }
    let bundle = bundle.ok_or_else(|| {
        CosmeticsError::InvalidPack(format!("archive does not contain {BUNDLE_NAME}"))
    })?;
    let catalog = catalog.ok_or_else(|| {
        CosmeticsError::InvalidPack(format!("archive does not contain {CATALOG_NAME}"))
    })?;
    if !bundle.starts_with(b"UnityFS\0") {
        return Err(CosmeticsError::InvalidPack(format!(
            "{BUNDLE_NAME} is not a UnityFS bundle"
        )));
    }
    let catalog_json: serde_json::Value = serde_json::from_slice(&catalog)
        .map_err(|error| CosmeticsError::InvalidPack(format!("invalid {CATALOG_NAME}: {error}")))?;
    let references_bundle = catalog_json
        .get("m_InternalIds")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|ids| {
            ids.iter().filter_map(serde_json::Value::as_str).any(|id| {
                id.replace('\\', "/")
                    .to_ascii_lowercase()
                    .ends_with("/bepinex/plugins/touhats.bundle")
            })
        });
    if !references_bundle {
        return Err(CosmeticsError::InvalidPack(format!(
            "{CATALOG_NAME} does not reference {BUNDLE_NAME}"
        )));
    }
    let marker = serde_json::to_vec(&CosmeticsMarker {
        schema: MARKER_SCHEMA,
        version: version.to_string(),
        asset: asset_name.to_string(),
        bundle_sha256: sha256_hex(&bundle),
        catalog_sha256: sha256_hex(&catalog),
    })
    .map_err(|error| CosmeticsError::InvalidPack(error.to_string()))?;
    Ok(CosmeticsPayload {
        bundle,
        catalog,
        marker,
    })
}

fn regular_file_metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not a regular file", path.display()),
            ))
        }
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn file_matches(path: &Path, limit: u64, expected: &str) -> io::Result<bool> {
    let Some(metadata) = regular_file_metadata(path)? else {
        return Ok(false);
    };
    if metadata.len() == 0 || metadata.len() > limit {
        return Ok(false);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    Ok(bytes.len() as u64 == metadata.len() && sha256_hex(&bytes) == expected)
}

fn read_marker(plugins: &Path) -> io::Result<Option<CosmeticsMarker>> {
    let path = plugins.join(MARKER_NAME);
    let Some(metadata) = regular_file_metadata(&path)? else {
        return Ok(None);
    };
    if metadata.len() == 0 || metadata.len() > MAX_MARKER_BYTES {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)?
        .take(MAX_MARKER_BYTES + 1)
        .read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes).ok())
}

/// Return true only when the tracked version and both exact companion files are
/// already present. Known publish targets are rejected if they are links.
pub fn installation_is_current(plugins: &Path, version: &str) -> io::Result<bool> {
    for name in [BUNDLE_NAME, CATALOG_NAME] {
        regular_file_metadata(&plugins.join(name))?;
    }
    let Some(marker) = read_marker(plugins)? else {
        return Ok(false);
    };
    if marker.schema != MARKER_SCHEMA || marker.version != version {
        return Ok(false);
    }
    Ok(file_matches(
        &plugins.join(BUNDLE_NAME),
        MAX_BUNDLE_BYTES,
        &marker.bundle_sha256,
    )? && file_matches(
        &plugins.join(CATALOG_NAME),
        MAX_CATALOG_BYTES,
        &marker.catalog_sha256,
    )?)
}

/// Remove only files whose content still matches Perfect Sync's ownership
/// marker. Modified or untracked cosmetics are preserved.
pub fn remove_managed_files(plugins: &Path) -> io::Result<()> {
    let marker_path = plugins.join(MARKER_NAME);
    let marker = read_marker(plugins)?;
    if let Some(marker) = marker {
        for (name, limit, digest) in [
            (BUNDLE_NAME, MAX_BUNDLE_BYTES, marker.bundle_sha256),
            (CATALOG_NAME, MAX_CATALOG_BYTES, marker.catalog_sha256),
        ] {
            let path = plugins.join(name);
            if file_matches(&path, limit, &digest)? {
                fs::remove_file(path)?;
            }
        }
    }
    match fs::remove_file(marker_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::InstalledMod;
    use crate::resolver;
    use crate::types::{ModSource, ModTag};
    use std::io::Write;

    const RELEASE: &str = r#"{
        "tag_name":"1.6.3-beta2",
        "assets":[
            {"name":"TouMirav1.6.3b2-x64-epic-msstore.zip","browser_download_url":"https://x/x64.zip","size":200},
            {"name":"TouMirav1.6.3b2-x86-macOS-linux.zip","browser_download_url":"https://x/linux.zip","size":300},
            {"name":"TouMirav1.6.3b2-x86-steam-itch.zip","browser_download_url":"https://x/x86.zip","size":100},
            {"name":"TownOfUsMira.dll","browser_download_url":"https://x/mod.dll","size":50}
        ]
    }"#;

    fn pack(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn valid_catalog() -> &'static [u8] {
        br#"{"m_InternalIds":["{UnityEngine.AddressableAssets.Addressables.RuntimePath}/../../../BepInEx/plugins/touhats.bundle"]}"#
    }

    fn profile(enabled: bool) -> ProfileRecord {
        ProfileRecord {
            id: "crew".into(),
            name: "Crew".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: vec![InstalledMod {
                package_id: PACKAGE_ID.into(),
                name: "Town of Us - Mira".into(),
                repo: Some(PACKAGE_ID.into()),
                version: "1.6.3-beta2".into(),
                versions: vec!["1.6.3-beta2".into()],
                enabled,
                source: ModSource::Github,
                tags: vec![ModTag::Role],
                managed: false,
                update: None,
                file: Some("TownOfUsMira.dll".into()),
                asset: Some("TownOfUsMira.dll".into()),
            }],
            levelimposter_maps: Vec::new(),
        }
    }

    #[test]
    fn selects_exact_windows_pack_for_store_and_architecture() {
        let release = resolver::parse_release(RELEASE).unwrap();
        assert_eq!(
            select_release_pack(&release, "x86", Store::Steam)
                .unwrap()
                .asset_name,
            "TouMirav1.6.3b2-x86-steam-itch.zip"
        );
        assert_eq!(
            select_release_pack(&release, "x86", Store::Itch)
                .unwrap()
                .asset_name,
            "TouMirav1.6.3b2-x86-steam-itch.zip"
        );
        assert_eq!(
            select_release_pack(&release, "x64", Store::Epic)
                .unwrap()
                .asset_name,
            "TouMirav1.6.3b2-x64-epic-msstore.zip"
        );
        assert_eq!(
            select_release_pack(&release, "x64", Store::Msstore)
                .unwrap()
                .asset_name,
            "TouMirav1.6.3b2-x64-epic-msstore.zip"
        );
        assert!(select_release_pack(&release, "x64", Store::Steam).is_err());
    }

    #[test]
    fn extracts_only_the_required_cosmetics_pair() {
        let archive = pack(&[
            ("Pack/BepInEx/plugins/Other.dll", b"dll"),
            ("Pack/BepInEx/plugins/touhats.bundle", b"UnityFS\0bundle"),
            ("Pack/BepInEx/plugins/touhats.catalog", valid_catalog()),
        ]);
        let payload = extract_release_pack(&archive, "1.6.3-beta2", "pack.zip").unwrap();
        assert_eq!(payload.bundle, b"UnityFS\0bundle");
        assert_eq!(payload.catalog, valid_catalog());
        let marker: CosmeticsMarker = serde_json::from_slice(&payload.marker).unwrap();
        assert_eq!(marker.version, "1.6.3-beta2");
        assert_eq!(marker.asset, "pack.zip");
    }

    #[test]
    fn rejects_missing_or_duplicate_companion_files() {
        let missing = pack(&[("Pack/touhats.bundle", b"UnityFS\0bundle")]);
        assert!(extract_release_pack(&missing, "v", "pack.zip").is_err());

        let duplicate = pack(&[
            ("One/touhats.bundle", b"UnityFS\0one"),
            ("Two/touhats.bundle", b"UnityFS\0two"),
            ("Two/touhats.catalog", valid_catalog()),
        ]);
        assert!(extract_release_pack(&duplicate, "v", "pack.zip").is_err());
    }

    #[test]
    fn marker_detects_missing_stale_and_changed_files() {
        let archive = pack(&[
            ("Pack/touhats.bundle", b"UnityFS\0bundle"),
            ("Pack/touhats.catalog", valid_catalog()),
        ]);
        let payload = extract_release_pack(&archive, "v1", "pack.zip").unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let plugins = temporary.path();
        fs::write(plugins.join(BUNDLE_NAME), &payload.bundle).unwrap();
        fs::write(plugins.join(CATALOG_NAME), &payload.catalog).unwrap();
        fs::write(plugins.join(MARKER_NAME), &payload.marker).unwrap();

        assert!(installation_is_current(plugins, "v1").unwrap());
        assert!(!installation_is_current(plugins, "v2").unwrap());
        fs::write(plugins.join(BUNDLE_NAME), b"UnityFS\0changed").unwrap();
        assert!(!installation_is_current(plugins, "v1").unwrap());
    }

    #[test]
    fn cleanup_removes_only_unchanged_managed_files() {
        let archive = pack(&[
            ("Pack/touhats.bundle", b"UnityFS\0bundle"),
            ("Pack/touhats.catalog", valid_catalog()),
        ]);
        let payload = extract_release_pack(&archive, "v1", "pack.zip").unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let plugins = temporary.path();
        fs::write(plugins.join(BUNDLE_NAME), b"UnityFS\0modified").unwrap();
        fs::write(plugins.join(CATALOG_NAME), &payload.catalog).unwrap();
        fs::write(plugins.join(MARKER_NAME), &payload.marker).unwrap();

        remove_managed_files(plugins).unwrap();
        assert!(plugins.join(BUNDLE_NAME).exists());
        assert!(!plugins.join(CATALOG_NAME).exists());
        assert!(!plugins.join(MARKER_NAME).exists());
    }

    #[test]
    fn selected_version_wins_over_version_history() {
        let mut selected = profile(true);
        selected.mods[0].version = "1.6.2".into();
        selected.mods[0].versions = vec!["1.6.3-beta2".into(), "1.6.2".into(), "1.6.1".into()];
        assert_eq!(active_version(&selected), Some("1.6.2"));
        assert_eq!(active_version(&profile(false)), None);
    }
}
