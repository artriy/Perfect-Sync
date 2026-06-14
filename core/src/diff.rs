use crate::types::LobbyManifest;
use serde::Serialize;

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
}

/// `installed` is the set of (id, version) the user already has cached/installed.
pub fn diff(manifest: &LobbyManifest, installed: &[(String, String)]) -> Vec<DiffItem> {
    manifest
        .mods
        .iter()
        .map(|m| {
            let have = installed
                .iter()
                .find(|(id, _)| id == &m.id)
                .map(|(_, v)| v.clone());
            let action = match &have {
                None => Action::Install,
                Some(v) if *v == m.v => Action::Ok,
                Some(_) => Action::Change,
            };
            DiffItem {
                id: m.id.clone(),
                action,
                from: have,
                to: m.v.clone(),
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
                })
                .collect(),
            loader: None,
        }
    }

    #[test]
    fn classifies_install_change_ok() {
        let m = man(&[("a", "1.0"), ("b", "2.0"), ("c", "3.0")]);
        let installed = vec![("b".to_string(), "1.0".to_string()), ("c".to_string(), "3.0".to_string())];
        let d = diff(&m, &installed);
        assert_eq!(d[0].action, Action::Install); // a not installed
        assert_eq!(d[1].action, Action::Change); // b 1.0 -> 2.0
        assert_eq!(d[2].action, Action::Ok); // c already 3.0
        assert_eq!(d[1].from, Some("1.0".to_string()));
    }
}
