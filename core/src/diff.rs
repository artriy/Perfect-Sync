use crate::types::{canonical_repo_id, LobbyManifest};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Install,
    Change,
    Ok,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct DiffItem {
    pub id: String,
    pub action: Action,
    pub from: Option<String>,
    pub to: String,
    pub asset: Option<String>,
}

/// `installed` is the set of (id, version) the user already has cached/installed.
///
/// Repository identities are matched with GitHub's ASCII case-insensitivity.
/// Version differences are changes because lobby codes reproduce the shared profile.
/// If local state contains duplicate logical identities, the first tuple wins.
pub fn diff(manifest: &LobbyManifest, installed: &[(String, String)]) -> Vec<DiffItem> {
    let mut installed_by_id = HashMap::with_capacity(installed.len());
    for (id, version) in installed {
        installed_by_id
            .entry(canonical_repo_id(id))
            .or_insert(version.as_str());
    }

    manifest
        .mods
        .iter()
        .map(|m| {
            let have = installed_by_id
                .get(&canonical_repo_id(&m.id))
                .copied()
                .map(str::to_owned);
            let action = match &have {
                None => Action::Install,
                Some(version) if *version == m.v => Action::Ok,
                Some(_) => Action::Change,
            };
            DiffItem {
                id: m.id.clone(),
                action,
                from: have,
                to: m.v.clone(),
                asset: m.asset.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ManifestMod;

    fn man(mods: &[(&str, &str)]) -> LobbyManifest {
        LobbyManifest {
            v: 1,
            name: None,
            platform: None,
            game_build: None,
            mods: mods
                .iter()
                .map(|(id, v)| ManifestMod {
                    id: (*id).into(),
                    v: (*v).into(),
                    asset: None,
                })
                .collect(),
            levelimposter_maps: Vec::new(),
            loader: None,
        }
    }

    #[test]
    fn classifies_install_change_and_ok() {
        let m = man(&[("a", "1.0"), ("b", "2.0"), ("c", "3.0")]);
        let installed = vec![
            ("b".to_string(), "1.0".to_string()),
            ("c".to_string(), "3.0".to_string()),
        ];
        let d = diff(&m, &installed);
        assert_eq!(d[0].action, Action::Install);
        assert_eq!(d[1].action, Action::Change);
        assert_eq!(d[2].action, Action::Ok);
        assert_eq!(d[1].from, Some("1.0".to_string()));
        assert_eq!(d[0].asset, None);
    }

    #[test]
    fn matches_repository_id_case_insensitively() {
        let m = man(&[("Owner/Repo", "1.0")]);
        let installed = vec![("owner/repo".to_string(), "1.0".to_string())];
        let d = diff(&m, &installed);
        assert_eq!(d[0].action, Action::Ok);
    }

    #[test]
    fn carries_the_manifest_requested_asset() {
        let mut manifest = man(&[("Owner/Repo", "1.0")]);
        manifest.mods[0].asset = Some("Repo-x86.dll".to_string());
        let items = diff(&manifest, &[]);
        assert_eq!(items[0].asset.as_deref(), Some("Repo-x86.dll"));
    }

    #[test]
    fn duplicate_installed_ids_keep_the_first_tuple() {
        let m = man(&[("Owner/Repo", "1.0")]);
        let installed = vec![
            ("OWNER/REPO".to_string(), "1.0".to_string()),
            ("owner/repo".to_string(), "2.0".to_string()),
        ];
        let d = diff(&m, &installed);
        assert_eq!(d[0].action, Action::Ok);
        assert_eq!(d[0].from.as_deref(), Some("1.0"));
    }
}
