//! ProfileManager: persist profile records and manage a profile's on-disk
//! `BepInEx/plugins` (install a mod DLL, extract a release zip, enable/disable
//! a plugin). All operations take an explicit `profiles_root` so they are
//! tested against temp directories.
//!
//! Records serialize in camelCase to match the frontend's TypeScript types
//! (`packageId`, `crewColor`, `gameBuild`).

use crate::loader;
use crate::types::{LobbyManifest, ManifestMod, ModSource, ModTag};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};

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
    #[serde(default)]
    pub mods: Vec<InstalledMod>,
}

/// A directory of profiles, each `<root>/<id>/profile.json` plus its BepInEx tree.
pub struct ProfileStore {
    pub root: PathBuf,
}

fn validate_id(id: &str) -> io::Result<()> {
    let mut comps = Path::new(id).components();
    let single_normal =
        matches!(comps.next(), Some(std::path::Component::Normal(_))) && comps.next().is_none();
    if id.is_empty() || id.contains('/') || id.contains('\\') || !single_normal {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid profile id"));
    }
    Ok(())
}

impl ProfileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn profile_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    fn manifest_path(&self, id: &str) -> PathBuf {
        self.profile_dir(id).join("profile.json")
    }

    pub fn save(&self, profile: &ProfileRecord) -> io::Result<()> {
        validate_id(&profile.id)?;
        let dir = self.profile_dir(&profile.id);
        fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(profile)?;
        let tmp = dir.join("profile.json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, self.manifest_path(&profile.id))
    }

    pub fn load(&self, id: &str) -> Option<ProfileRecord> {
        let text = fs::read_to_string(self.manifest_path(id)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn list(&self) -> Vec<ProfileRecord> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&self.root) else {
            return out;
        };
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(p) = self.load(name) {
                        out.push(p);
                    }
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    pub fn delete(&self, id: &str) -> io::Result<()> {
        validate_id(id)?;
        let dir = self.profile_dir(id);
        if dir.is_dir() {
            fs::remove_dir_all(dir)?;
        }
        Ok(())
    }
}

/// Encode a profile's enabled mods into a shareable lobby manifest. Versions are
/// preserved exactly so a recipient reproduces a handshake-compatible set.
pub fn to_manifest(profile: &ProfileRecord) -> LobbyManifest {
    LobbyManifest {
        v: 1,
        name: Some(profile.name.clone()),
        platform: None,
        game_build: profile.game_build.clone(),
        mods: profile
            .mods
            .iter()
            .filter(|m| m.enabled)
            .map(|m| ManifestMod {
                id: m.package_id.clone(),
                v: m.version.clone(),
                asset: m.asset.clone(),
            })
            .collect(),
        loader: None,
    }
}

/// Copy a bare mod DLL into a profile's plugins directory. Returns the path.
pub fn install_plugin_dll(profiles_root: &Path, id: &str, dll_src: &Path) -> io::Result<PathBuf> {
    let plugins = loader::profile_plugins_dir(profiles_root, id);
    fs::create_dir_all(&plugins)?;
    let file_name = dll_src
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "source has no file name"))?;
    let dest = plugins.join(file_name);
    fs::copy(dll_src, &dest)?;
    Ok(dest)
}

/// Write downloaded plugin bytes straight into a profile's plugins dir.
pub fn install_plugin_bytes(
    profiles_root: &Path,
    id: &str,
    file_name: &str,
    bytes: &[u8],
) -> io::Result<PathBuf> {
    let plugins = loader::profile_plugins_dir(profiles_root, id);
    fs::create_dir_all(&plugins)?;
    let name = std::path::Path::new(file_name)
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid plugin file name"))?;
    let dest = plugins.join(name);
    fs::write(&dest, bytes)?;
    Ok(dest)
}

/// Remove a plugin file (enabled or `.disabled`) from a profile.
pub fn remove_plugin(profiles_root: &Path, id: &str, file_name: &str) -> io::Result<()> {
    let plugins = loader::profile_plugins_dir(profiles_root, id);
    for candidate in [plugins.join(file_name), plugins.join(format!("{file_name}.disabled"))] {
        if candidate.exists() {
            fs::remove_file(candidate)?;
        }
    }
    Ok(())
}

/// Extract every plugin `.dll` from a release zip into the profile's plugins
/// dir. Handles both bare-dll-in-zip and `.../BepInEx/plugins/*.dll` bundles.
/// Returns the installed plugin file names.
pub fn install_from_zip(profiles_root: &Path, id: &str, bytes: &[u8], only: Option<&str>) -> io::Result<Vec<String>> {
    let plugins = loader::profile_plugins_dir(profiles_root, id);
    fs::create_dir_all(&plugins)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let mut installed = Vec::new();
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().replace('\\', "/");
        let lower = name.to_lowercase();
        let is_plugin = lower.ends_with(".dll") && (lower.contains("/plugins/") || !name.contains('/'));
        if !is_plugin {
            continue;
        }
        let base = Path::new(&name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone());
        if let Some(want) = only {
            if !base.eq_ignore_ascii_case(want) {
                continue;
            }
        }
        let mut out = fs::File::create(plugins.join(&base))?;
        io::copy(&mut file, &mut out)?;
        installed.push(base);
    }
    Ok(installed)
}

/// Enable/disable a plugin by toggling a `.disabled` suffix (BepInEx only loads `.dll`).
pub fn set_plugin_enabled(
    profiles_root: &Path,
    id: &str,
    dll_name: &str,
    enabled: bool,
) -> io::Result<()> {
    let plugins = loader::profile_plugins_dir(profiles_root, id);
    let active = plugins.join(dll_name);
    let disabled = plugins.join(format!("{dll_name}.disabled"));
    if enabled {
        if disabled.exists() {
            fs::rename(disabled, active)?;
        }
    } else if active.exists() {
        fs::rename(active, disabled)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_profile() -> ProfileRecord {
        ProfileRecord {
            id: "tou-night".into(),
            name: "ToU night".into(),
            crew_color: "#9b7bff".into(),
            game_build: Some("17.0.1".into()),
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
        }
    }

    #[test]
    fn store_round_trips_and_uses_camel_case() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(tmp.path());
        let p = sample_profile();
        store.save(&p).unwrap();

        // round-trip
        assert_eq!(store.load("tou-night").unwrap(), p);
        let all = store.list();
        assert_eq!(all.len(), 1);

        // serialized keys must match the TS types
        let raw = fs::read_to_string(tmp.path().join("tou-night").join("profile.json")).unwrap();
        assert!(raw.contains("\"packageId\""));
        assert!(raw.contains("\"crewColor\""));
        assert!(raw.contains("\"gameBuild\""));
        assert!(raw.contains("\"all-client\"")); // ModTag kebab
        assert!(raw.contains("\"github\"")); // ModSource lowercase

        store.delete("tou-night").unwrap();
        assert!(store.load("tou-night").is_none());
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
        // survives a codec round-trip
        let code = crate::codec::encode(&manifest);
        assert_eq!(crate::codec::decode(&code).unwrap(), manifest);
    }

    #[test]
    fn to_manifest_includes_enabled_libraries() {
        // a library/dependency the host has enabled must be in the code verbatim,
        // so the recipient reproduces the EXACT mod set (no re-resolution).
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
        let reactor = manifest.mods.iter().find(|m| m.id == "NuclearPowered/Reactor").unwrap();
        assert_eq!(reactor.v, "2.3.0");
        assert_eq!(reactor.asset.as_deref(), Some("Reactor.dll"));
    }

    #[test]
    fn to_manifest_sets_github_ref_for_custom_mod() {
        // a mod NOT in any catalog, added by pasting a GitHub URL
        let p = ProfileRecord {
            id: "p".into(),
            name: "Custom".into(),
            crew_color: "#fff".into(),
            game_build: None,
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
        };
        let m = to_manifest(&p);
        // id is owner/repo; the recipient derives the GitHub repo from it (no ref needed)
        assert_eq!(m.mods[0].id, "SomeUser/CoolMod");
        assert_eq!(m.mods[0].v, "1.2.3");
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
    fn extracts_plugins_from_zip() {
        // build an in-memory zip with a bundled plugin and a noise file
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            use std::io::Write;
            zw.start_file("BepInEx/plugins/TheOtherRoles.dll", opts).unwrap();
            zw.write_all(b"mod").unwrap();
            zw.start_file("README.md", opts).unwrap();
            zw.write_all(b"readme").unwrap();
            zw.finish().unwrap();
        }
        let tmp = tempfile::tempdir().unwrap();
        let installed = install_from_zip(tmp.path(), "p1", &buf, None).unwrap();
        assert_eq!(installed, vec!["TheOtherRoles.dll".to_string()]);
        assert!(loader::profile_plugins_dir(tmp.path(), "p1")
            .join("TheOtherRoles.dll")
            .exists());
    }

    #[test]
    fn extracts_only_named_dll_when_filtered() {
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            use std::io::Write;
            for n in [
                "BepInEx/plugins/TownOfUsMira.dll",
                "BepInEx/plugins/Reactor.dll",
                "BepInEx/plugins/MiraAPI.dll",
            ] {
                zw.start_file(n, opts).unwrap();
                zw.write_all(b"x").unwrap();
            }
            zw.finish().unwrap();
        }
        let tmp = tempfile::tempdir().unwrap();
        let installed = install_from_zip(tmp.path(), "p1", &buf, Some("TownOfUsMira.dll")).unwrap();
        assert_eq!(installed, vec!["TownOfUsMira.dll".to_string()]);
        let plugins = loader::profile_plugins_dir(tmp.path(), "p1");
        assert!(plugins.join("TownOfUsMira.dll").exists());
        assert!(!plugins.join("Reactor.dll").exists());
        assert!(!plugins.join("MiraAPI.dll").exists());
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

        let dest = install_plugin_bytes(tmp.path(), "p1", "../evil.dll", b"x").unwrap();
        assert_eq!(dest, plugins.join("evil.dll"));
        assert!(!plugins.parent().unwrap().join("evil.dll").exists());
    }
}
