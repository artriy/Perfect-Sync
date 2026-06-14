use crate::catalog::Catalog;
use crate::codec::{decode, CodecError};
use crate::diff::{diff, Action};
use crate::types::ModTag;
use serde::Serialize;

#[derive(Debug, PartialEq, Serialize)]
pub struct PreviewItem {
    pub name: String,
    pub repo: Option<String>,
    pub tags: Vec<ModTag>,
    pub action: Action,
    pub from: Option<String>,
    pub to: String,
    pub detail: String,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Preview {
    pub name: String,
    pub items: Vec<PreviewItem>,
}

pub fn preview(code: &str, cat: &Catalog, installed: &[(String, String)]) -> Result<Preview, CodecError> {
    let manifest = decode(code)?;
    let rows = diff(&manifest, installed);
    let items = rows
        .into_iter()
        .map(|row| {
            let entry = cat.get(&row.id);
            let name = entry.map(|e| e.name.clone()).unwrap_or_else(|| row.id.clone());
            let repo = entry.and_then(|e| e.repo.clone());
            let tags = entry.map(|e| e.tags.clone()).unwrap_or_default();
            let detail = match row.action {
                Action::Install => "not installed yet".to_string(),
                Action::Change => format!("you have {}, lobby needs {}", row.from.clone().unwrap_or_default(), row.to),
                Action::Ok => format!("{}, already cached", row.to),
            };
            PreviewItem { name, repo, tags, action: row.action, from: row.from, to: row.to, detail }
        })
        .collect();
    Ok(Preview {
        name: manifest.name.unwrap_or_else(|| "Imported lobby".to_string()),
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::parse;
    use crate::codec::encode;
    use crate::types::{LobbyManifest, ManifestMod};

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
                id: "AU-Avengers/TOU-Mira".into(),
                v: "1.6.3".into(),
            }],
            loader: None,
        };
        let code = encode(&manifest);
        let p = preview(&code, &cat, &[("AU-Avengers/TOU-Mira".into(), "1.6.2".into())]).unwrap();
        assert_eq!(p.name, "TownOfUs Night");
        assert_eq!(p.items[0].name, "Town of Us - Mira");
        assert_eq!(p.items[0].action, Action::Change);
        assert_eq!(p.items[0].detail, "you have 1.6.2, lobby needs 1.6.3");
    }
}
