use std::collections::HashSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    X86,
    X64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Store {
    Steam,
    Epic,
    Itch,
    Msstore,
    Manual,
}

/// How an Among Us install is executed. Among Us is a Windows-only build, so on
/// non-Windows hosts it runs through Steam Proton or a Wine-based bottle manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Native,
    Proton,
    Wine,
    Crossover,
    Whisky,
    Bottles,
}

/// How vetted a mod is. Drives the trust badge and warns on unknown mods pulled
/// in by a shared lobby code. Anything not in the catalog is `Flagged`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Trust {
    Trusted,
    Community,
    #[default]
    Flagged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModTag {
    Role,
    AllClient,
    HostOnly,
    Map,
    Cosmetic,
    Library,
    Loader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModSource {
    Catalog,
    Github,
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Platform {
    pub store: Store,
    pub arch: Arch,
}

/// One mod in a share code. Kept minimal to keep codes short: `id` is
/// `owner/repo` (source is always GitHub, derivable from it), `v` is the tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestMod {
    pub id: String,
    pub v: String,
    /// exact asset file the host installed, so a custom/multi-asset repo
    /// resolves to the same file (omitted when there's nothing special to pin).
    #[serde(rename = "a", skip_serializing_if = "Option::is_none", default)]
    pub asset: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoaderPins {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bepinex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reactor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LobbyManifest {
    pub v: u8,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub platform: Option<Platform>,
    #[serde(rename = "gameBuild", skip_serializing_if = "Option::is_none", default)]
    pub game_build: Option<String>,
    pub mods: Vec<ManifestMod>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub loader: Option<LoaderPins>,
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "maps")]
    pub levelimposter_maps: Vec<String>,
}

pub const LOBBY_SCHEMA_VERSION: u8 = 1;
pub const MAX_MANIFEST_NAME_LEN: usize = 128;
pub const MAX_MANIFEST_MODS: usize = 64;
pub const MAX_REPO_ID_LEN: usize = 140;
pub const MAX_VERSION_LEN: usize = 128;
pub const MAX_RELEASE_TAG_LEN: usize = 255;
pub const MAX_ASSET_NAME_LEN: usize = 255;
pub const MAX_MANIFEST_MAPS: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestValidationError {
    #[error("malformed lobby manifest: {0}")]
    Malformed(&'static str),
    #[error("unsupported lobby schema version {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported lobby feature: {0}")]
    UnsupportedFeature(&'static str),
    #[error("{field} exceeds the limit of {limit} bytes/items")]
    ExcessiveInput { field: &'static str, limit: usize },
}

impl LobbyManifest {
    pub fn validate(&self) -> Result<(), ManifestValidationError> {
        if self.v != LOBBY_SCHEMA_VERSION {
            return Err(ManifestValidationError::UnsupportedVersion(self.v));
        }

        if let Some(name) = &self.name {
            validate_len(name, "manifest name", MAX_MANIFEST_NAME_LEN)?;
            if name.trim().is_empty() || name.chars().any(char::is_control) {
                return Err(ManifestValidationError::Malformed("invalid manifest name"));
            }
        }
        if let Some(build) = &self.game_build {
            validate_version(build, "game build")?;
        }
        if self.mods.len() > MAX_MANIFEST_MODS {
            return Err(ManifestValidationError::ExcessiveInput {
                field: "manifest mod count",
                limit: MAX_MANIFEST_MODS,
            });
        }
        if self.levelimposter_maps.len() > MAX_MANIFEST_MAPS {
            return Err(ManifestValidationError::ExcessiveInput {
                field: "LevelImposter map count",
                limit: MAX_MANIFEST_MAPS,
            });
        }

        if let Some(loader) = &self.loader {
            if let Some(version) = &loader.bepinex {
                validate_version(version, "BepInEx version")?;
            }
            if let Some(version) = &loader.reactor {
                validate_version(version, "Reactor version")?;
            }
        }

        if self.platform.is_some() {
            return Err(ManifestValidationError::UnsupportedFeature("platform pin"));
        }
        if self.loader.is_some() {
            return Err(ManifestValidationError::UnsupportedFeature("loader pins"));
        }

        let mut ids = HashSet::with_capacity(self.mods.len());
        for manifest_mod in &self.mods {
            validate_repo_id(&manifest_mod.id)?;
            validate_release_tag(&manifest_mod.v)?;
            if let Some(asset) = &manifest_mod.asset {
                validate_asset_name(asset)?;
            }
            if !ids.insert(canonical_repo_id(&manifest_mod.id)) {
                return Err(ManifestValidationError::Malformed(
                    "duplicate mod repository identity",
                ));
            }
        }

        if !self.levelimposter_maps.is_empty() && !ids.contains("digiworm0/levelimposter") {
            return Err(ManifestValidationError::Malformed(
                "LevelImposter maps require the LevelImposter mod",
            ));
        }

        let mut maps = HashSet::with_capacity(self.levelimposter_maps.len());
        for map in &self.levelimposter_maps {
            if !valid_levelimposter_map_id(map) || !maps.insert(map.to_ascii_lowercase()) {
                return Err(ManifestValidationError::Malformed(
                    "invalid or duplicate LevelImposter map id",
                ));
            }
        }
        Ok(())
    }
}

pub fn valid_levelimposter_map_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

pub(crate) fn canonical_repo_id(id: &str) -> String {
    id.to_ascii_lowercase()
}

fn validate_len(
    value: &str,
    field: &'static str,
    limit: usize,
) -> Result<(), ManifestValidationError> {
    if value.len() > limit {
        return Err(ManifestValidationError::ExcessiveInput { field, limit });
    }
    Ok(())
}

fn validate_version(version: &str, field: &'static str) -> Result<(), ManifestValidationError> {
    validate_len(version, field, MAX_VERSION_LEN)?;
    if version.is_empty()
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
    {
        return Err(ManifestValidationError::Malformed("invalid version"));
    }
    Ok(())
}

fn validate_release_tag(tag: &str) -> Result<(), ManifestValidationError> {
    validate_len(tag, "mod release tag", MAX_RELEASE_TAG_LEN)?;
    if tag.is_empty() || tag.chars().any(char::is_control) {
        return Err(ManifestValidationError::Malformed(
            "invalid mod release tag",
        ));
    }
    Ok(())
}

fn validate_repo_id(id: &str) -> Result<(), ManifestValidationError> {
    validate_len(id, "mod repository identity", MAX_REPO_ID_LEN)?;
    let Some((owner, repo)) = id.split_once('/') else {
        return Err(ManifestValidationError::Malformed(
            "repository identity must be owner/repo",
        ));
    };
    if repo.contains('/') {
        return Err(ManifestValidationError::Malformed(
            "repository identity must be owner/repo",
        ));
    }
    if owner.is_empty()
        || owner.len() > 39
        || !owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || owner.as_bytes().windows(2).any(|pair| pair == b"--")
        || !owner
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !owner
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(ManifestValidationError::Malformed(
            "invalid GitHub repository owner",
        ));
    }
    if repo.is_empty()
        || repo.len() > 100
        || matches!(repo, "." | "..")
        || repo
            .get(repo.len().saturating_sub(4)..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".git"))
        || !repo
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(ManifestValidationError::Malformed(
            "invalid GitHub repository name",
        ));
    }
    Ok(())
}

fn validate_asset_name(asset: &str) -> Result<(), ManifestValidationError> {
    validate_len(asset, "asset name", MAX_ASSET_NAME_LEN)?;
    if asset.is_empty()
        || matches!(asset, "." | "..")
        || asset.ends_with('.')
        || asset.ends_with(' ')
        || asset.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*')
        })
        || is_windows_reserved_name(asset)
    {
        return Err(ManifestValidationError::Malformed(
            "asset must be a portable basename",
        ));
    }
    Ok(())
}

fn is_windows_reserved_name(asset: &str) -> bool {
    let stem = asset.split('.').next().unwrap_or(asset);
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "COM¹" | "COM²" | "COM³" | "LPT¹" | "LPT²" | "LPT³"
    ) || (upper.len() == 4
        && (upper.starts_with("COM") || upper.starts_with("LPT"))
        && matches!(upper.as_bytes()[3], b'1'..=b'9'))
}
