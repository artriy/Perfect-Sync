use crate::types::ModTag;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssetArchRule {
    #[serde(rename = "match")]
    pub pat: String,
    pub prefer: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssetRules {
    #[serde(rename = "perArch", default)]
    pub per_arch: HashMap<String, AssetArchRule>,
    #[serde(rename = "dllName", default)]
    pub dll_name: Option<String>,
    #[serde(rename = "bundlesLoader", default)]
    pub bundles_loader: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub repo: Option<String>,
    pub tags: Vec<ModTag>,
    pub dependencies: Vec<String>,
    #[serde(rename = "assetRules")]
    pub asset_rules: AssetRules,
}

/// Where the BepInEx loader pack comes from: a GitHub repo + release, resolved
/// per-arch exactly like a mod (pure GitHub, no third-party registry).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoaderInfo {
    pub repo: String,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(rename = "assetRules")]
    pub asset_rules: AssetRules,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Catalog {
    pub schema: u32,
    pub mods: Vec<CatalogEntry>,
    #[serde(default)]
    pub loader: Option<LoaderInfo>,
}

impl Catalog {
    pub fn get(&self, id: &str) -> Option<&CatalogEntry> {
        self.mods.iter().find(|m| m.id == id)
    }
}

pub fn parse(json: &str) -> Result<Catalog, serde_json::Error> {
    serde_json::from_str(json)
}

/// Pick the asset name that matches the given arch's regex; honor `prefer` extension.
pub fn select_asset<'a>(rules: &AssetRules, arch: &str, names: &'a [String]) -> Option<&'a String> {
    let rule = rules.per_arch.get(arch)?;
    let re = Regex::new(&rule.pat).ok()?;
    let mut matches: Vec<&String> = names.iter().filter(|n| re.is_match(n)).collect();
    if matches.is_empty() {
        return None;
    }
    if let Some(pref) = &rule.prefer {
        let suffix = format!(".{}", pref.to_lowercase());
        matches.sort_by_key(|n| if n.to_lowercase().ends_with(&suffix) { 0 } else { 1 });
    }
    Some(matches[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../fixtures/catalog.sample.json");

    #[test]
    fn parses_fixture() {
        let cat = parse(SAMPLE).unwrap();
        assert_eq!(cat.schema, 1);
        assert!(cat.get("AU-Avengers/TOU-Mira").is_some());
    }

    #[test]
    fn selects_x86_steam_asset() {
        let cat = parse(SAMPLE).unwrap();
        let rules = &cat.get("AU-Avengers/TOU-Mira").unwrap().asset_rules;
        let names = vec![
            "TouMira-v1.6.3-x64-epic-msstore.zip".to_string(),
            "TouMira-v1.6.3-x86-steam-itch.zip".to_string(),
            "TownOfUsMira.dll".to_string(),
        ];
        assert_eq!(select_asset(rules, "x86", &names).unwrap(), "TouMira-v1.6.3-x86-steam-itch.zip");
        assert_eq!(select_asset(rules, "x64", &names).unwrap(), "TouMira-v1.6.3-x64-epic-msstore.zip");
    }
}
