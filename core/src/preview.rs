use crate::catalog::Catalog;
use crate::codec::{decode, CodecError};
use crate::diff::{diff, Action};
use crate::types::{canonical_repo_id, ModTag, Trust};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, PartialEq, Serialize)]
pub struct PreviewItem {
    pub name: String,
    pub repo: Option<String>,
    pub tags: Vec<ModTag>,
    pub action: Action,
    pub from: Option<String>,
    pub to: String,
    pub asset: Option<String>,
    pub detail: String,
    pub trust: Trust,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub name: String,
    pub game_build: Option<String>,
    pub items: Vec<PreviewItem>,
    pub levelimposter_maps: Vec<String>,
}

pub fn preview(
    code: &str,
    cat: &Catalog,
    installed: &[(String, String)],
) -> Result<Preview, CodecError> {
    let manifest = decode(code)?;
    let mut catalog_by_id = HashMap::with_capacity(cat.mods.len());
    for entry in &cat.mods {
        catalog_by_id
            .entry(canonical_repo_id(&entry.id))
            .or_insert(entry);
    }
    let rows = diff(&manifest, installed);
    let items = rows
        .into_iter()
        .map(|row| {
            let entry = catalog_by_id.get(&canonical_repo_id(&row.id)).copied();
            let name = entry
                .map(|e| e.name.clone())
                .unwrap_or_else(|| row.id.clone());
            let repo = entry.and_then(|e| e.repo.clone());
            let tags = entry.map(|e| e.tags.clone()).unwrap_or_default();
            let detail = match row.action {
                Action::Install => "not installed yet".to_string(),
                Action::Change => format!(
                    "you have {}, lobby needs {}",
                    row.from.clone().unwrap_or_default(),
                    row.to
                ),
                Action::Ok => format!("{}, already installed", row.to),
            };
            PreviewItem {
                name,
                repo,
                tags,
                action: row.action,
                from: row.from,
                to: row.to,
                asset: row.asset,
                detail,
                trust: entry.map(|e| e.trust).unwrap_or(Trust::Flagged),
            }
        })
        .collect();
    let game_build = None;
    Ok(Preview {
        name: manifest
            .name
            .unwrap_or_else(|| "Imported lobby".to_string()),
        game_build,
        levelimposter_maps: manifest.levelimposter_maps,
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::parse;
    use crate::codec::encode;
    use crate::types::{
        LobbyManifest, ManifestMod, MAX_ASSET_NAME_LEN, MAX_MANIFEST_MODS, MAX_MANIFEST_NAME_LEN,
        MAX_VERSION_LEN,
    };

    const SAMPLE: &str = include_str!("../fixtures/catalog.sample.json");

    #[test]
    fn builds_named_preview_rows() {
        let cat = parse(SAMPLE).unwrap();
        let manifest = LobbyManifest {
            v: 1,
            name: Some("TownOfUs Night".into()),
            platform: None,
            game_build: Some("17.0.1".into()),
            mods: vec![ManifestMod {
                id: "au-avengers/tou-mira".into(),
                v: "1.6.3".into(),
                asset: Some("TouMira-v1.6.3-x86-steam-itch.zip".into()),
            }],
            levelimposter_maps: Vec::new(),
            loader: None,
        };
        let code = encode(&manifest).unwrap();
        let p = preview(
            &code,
            &cat,
            &[("AU-AVENGERS/TOU-MIRA".into(), "1.6.2".into())],
        )
        .unwrap();
        assert_eq!(p.name, "TownOfUs Night");
        assert_eq!(p.items[0].name, "Town of Us - Mira");
        assert_eq!(p.items[0].action, Action::Change);
        assert_eq!(p.items[0].detail, "you have 1.6.2, lobby needs 1.6.3");
        assert_eq!(p.items[0].to, "1.6.3");
        assert_eq!(
            p.items[0].asset.as_deref(),
            Some("TouMira-v1.6.3-x86-steam-itch.zip")
        );
        assert!(p.game_build.is_none());
    }

    #[test]
    fn rejects_an_invalid_manifest_before_building_preview() {
        let manifest = LobbyManifest {
            v: 1,
            name: None,
            platform: None,
            game_build: None,
            mods: vec![
                ManifestMod {
                    id: "Owner/Repo".into(),
                    v: "1".into(),
                    asset: None,
                },
                ManifestMod {
                    id: "owner/repo".into(),
                    v: "2".into(),
                    asset: None,
                },
            ],
            levelimposter_maps: Vec::new(),
            loader: None,
        };
        assert_eq!(
            encode(&manifest),
            Err(CodecError::MalformedManifest(
                "duplicate mod repository identity"
            ))
        );
    }

    #[test]
    fn previews_a_maximum_valid_manifest() {
        let cat = parse(SAMPLE).unwrap();
        let version = "1".repeat(MAX_VERSION_LEN);
        let asset = format!("{}.dll", "a".repeat(MAX_ASSET_NAME_LEN - 4));
        let manifest = LobbyManifest {
            v: 1,
            name: Some("n".repeat(MAX_MANIFEST_NAME_LEN)),
            platform: None,
            game_build: Some(version.clone()),
            mods: (0..MAX_MANIFEST_MODS)
                .map(|i| ManifestMod {
                    id: format!("Owner/Repo{i}"),
                    v: version.clone(),
                    asset: Some(asset.clone()),
                })
                .collect(),
            levelimposter_maps: Vec::new(),
            loader: None,
        };
        let result = preview(&encode(&manifest).unwrap(), &cat, &[]).unwrap();
        assert_eq!(result.items.len(), MAX_MANIFEST_MODS);
    }
}
