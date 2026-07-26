use crate::types::{ModTag, Trust};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssetArchRule {
    #[serde(rename = "match")]
    pub pat: String,
    pub prefer: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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
    #[serde(rename = "dependencyVersions", default)]
    pub dependency_versions: HashMap<String, String>,
    #[serde(rename = "assetRules")]
    pub asset_rules: AssetRules,
    #[serde(default)]
    pub trust: Trust,
}

/// Where the BepInEx engine comes from. GitHub's BepInEx releases lag behind
/// current Among Us (Cpp2IL metadata mismatch), so the loader uses BepInEx's
/// own maintained Among Us pack: `packUrl` direct, else the latest via API.
/// (This is only the BepInEx engine, never a mod.)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoaderInfo {
    /// builds.bepinex.dev listing page; the app scrapes the newest build (preferred).
    #[serde(rename = "buildsUrl", default)]
    pub builds_url: Option<String>,
    /// fallback: BepInEx Among Us pack experimental API (latest download_url).
    #[serde(rename = "thunderstoreApi", default)]
    pub thunderstore_api: Option<String>,
    /// last-resort fixed pack url.
    #[serde(rename = "packUrl", default)]
    pub pack_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Catalog {
    pub schema: u32,
    pub mods: Vec<CatalogEntry>,
    #[serde(default)]
    pub loader: Option<LoaderInfo>,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("invalid catalog JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported catalog schema {0}; expected schema 1")]
    Schema(u32),
    #[error("invalid catalog identity {0:?}")]
    Identity(String),
    #[error("duplicate catalog id (case-insensitive): {0}")]
    DuplicateId(String),
    #[error("duplicate catalog repository (case-insensitive): {0}")]
    DuplicateRepo(String),
    #[error("catalog repository {repo:?} aliases another entry's id {id:?} (case-insensitive)")]
    RepositoryAliasesId { id: String, repo: String },
    #[error("invalid asset rule for {id}: {reason}")]
    AssetRule { id: String, reason: String },
    #[error("catalog dependency {dependency:?} required by {id:?} is missing")]
    MissingDependency { id: String, dependency: String },
    #[error("invalid dependency version requirement for {id} -> {dependency}: {reason}")]
    DependencyVersion {
        id: String,
        dependency: String,
        reason: String,
    },
    #[error("catalog dependency cycle includes {0:?}")]
    DependencyCycle(String),
    #[error("loader URL must be an absolute HTTPS URL: {0}")]
    LoaderUrl(String),
}

impl Catalog {
    /// Catalog identities are GitHub slugs and therefore ASCII case-insensitive.
    pub fn get(&self, id: &str) -> Option<&CatalogEntry> {
        self.mods.iter().find(|m| m.id.eq_ignore_ascii_case(id))
    }
}

pub(crate) fn valid_repo_slug(value: &str) -> bool {
    let Some((owner, repo)) = value.split_once('/') else {
        return false;
    };
    if owner.is_empty()
        || owner.len() > 39
        || repo.is_empty()
        || repo.len() > 100
        || repo.contains('/')
        || repo == "."
        || repo == ".."
        || (repo.len() >= 4 && repo.as_bytes()[repo.len() - 4..].eq_ignore_ascii_case(b".git"))
    {
        return false;
    }
    let owner_bytes = owner.as_bytes();
    if !owner_bytes[0].is_ascii_alphanumeric()
        || !owner_bytes[owner_bytes.len() - 1].is_ascii_alphanumeric()
        || owner.contains("--")
        || !owner_bytes
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'-')
    {
        return false;
    }
    repo.bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

fn validate_loader_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn validate(mut catalog: Catalog) -> Result<Catalog, CatalogError> {
    if catalog.schema != 1 {
        return Err(CatalogError::Schema(catalog.schema));
    }

    let mut ids = HashMap::with_capacity(catalog.mods.len());
    let mut repos = HashSet::with_capacity(catalog.mods.len());
    for (index, entry) in catalog.mods.iter().enumerate() {
        if !valid_repo_slug(&entry.id) {
            return Err(CatalogError::Identity(entry.id.clone()));
        }
        let folded_id = entry.id.to_ascii_lowercase();
        if ids.insert(folded_id, index).is_some() {
            return Err(CatalogError::DuplicateId(entry.id.clone()));
        }
        let repo = entry.repo.as_deref().unwrap_or(&entry.id);
        if !valid_repo_slug(repo) {
            return Err(CatalogError::Identity(repo.to_string()));
        }
        if !repos.insert(repo.to_ascii_lowercase()) {
            return Err(CatalogError::DuplicateRepo(repo.to_string()));
        }
        for (arch, rule) in &entry.asset_rules.per_arch {
            if arch != "x86" && arch != "x64" {
                return Err(CatalogError::AssetRule {
                    id: entry.id.clone(),
                    reason: format!("unsupported architecture key {arch:?}"),
                });
            }
            Regex::new(&rule.pat).map_err(|error| CatalogError::AssetRule {
                id: entry.id.clone(),
                reason: format!("invalid regex for {arch}: {error}"),
            })?;
        }
    }
    for (index, entry) in catalog.mods.iter().enumerate() {
        let repo = entry.repo.as_deref().unwrap_or(&entry.id);
        if let Some(&owner) = ids.get(&repo.to_ascii_lowercase()) {
            if owner != index {
                return Err(CatalogError::RepositoryAliasesId {
                    id: catalog.mods[owner].id.clone(),
                    repo: repo.to_string(),
                });
            }
        }
    }

    // Resolve dependency spelling to the catalog's canonical spelling while
    // validating every edge. This keeps serialized/install identifiers stable.
    for entry_index in 0..catalog.mods.len() {
        let owner = catalog.mods[entry_index].id.clone();
        for dependency_index in 0..catalog.mods[entry_index].dependencies.len() {
            let requested = catalog.mods[entry_index].dependencies[dependency_index].clone();
            if !valid_repo_slug(&requested) {
                return Err(CatalogError::Identity(requested));
            }
            let Some(&target) = ids.get(&requested.to_ascii_lowercase()) else {
                return Err(CatalogError::MissingDependency {
                    id: owner,
                    dependency: requested,
                });
            };
            let canonical = catalog.mods[target].id.clone();
            catalog.mods[entry_index].dependencies[dependency_index] = canonical;
        }
    }

    for entry_index in 0..catalog.mods.len() {
        let owner = catalog.mods[entry_index].id.clone();
        let mut requirements = HashMap::new();
        for (requested, requirement) in catalog.mods[entry_index].dependency_versions.clone() {
            if !valid_repo_slug(&requested) {
                return Err(CatalogError::Identity(requested));
            }
            let Some(&target) = ids.get(&requested.to_ascii_lowercase()) else {
                return Err(CatalogError::MissingDependency {
                    id: owner.clone(),
                    dependency: requested,
                });
            };
            let canonical = catalog.mods[target].id.clone();
            if !catalog.mods[entry_index]
                .dependencies
                .iter()
                .any(|dependency| dependency.eq_ignore_ascii_case(&canonical))
            {
                return Err(CatalogError::DependencyVersion {
                    id: owner.clone(),
                    dependency: canonical,
                    reason: "requirement target is not a direct dependency".into(),
                });
            }
            semver::VersionReq::parse(requirement.trim()).map_err(|error| {
                CatalogError::DependencyVersion {
                    id: owner.clone(),
                    dependency: canonical.clone(),
                    reason: error.to_string(),
                }
            })?;
            if requirements
                .insert(canonical.clone(), requirement)
                .is_some()
            {
                return Err(CatalogError::DependencyVersion {
                    id: owner.clone(),
                    dependency: canonical,
                    reason: "duplicate requirement target".into(),
                });
            }
        }
        catalog.mods[entry_index].dependency_versions = requirements;
    }

    // Iterative DFS avoids recursion on an untrusted catalog.
    let mut state = vec![0u8; catalog.mods.len()];
    for root in 0..catalog.mods.len() {
        if state[root] != 0 {
            continue;
        }
        state[root] = 1;
        let mut stack = vec![(root, 0usize)];
        while let Some((node, next)) = stack.last_mut() {
            if *next == catalog.mods[*node].dependencies.len() {
                state[*node] = 2;
                stack.pop();
                continue;
            }
            let dependency = &catalog.mods[*node].dependencies[*next];
            *next += 1;
            let target = ids[&dependency.to_ascii_lowercase()];
            match state[target] {
                0 => {
                    state[target] = 1;
                    stack.push((target, 0));
                }
                1 => {
                    return Err(CatalogError::DependencyCycle(
                        catalog.mods[target].id.clone(),
                    ))
                }
                _ => {}
            }
        }
    }

    if let Some(loader) = &catalog.loader {
        for value in [
            loader.builds_url.as_deref(),
            loader.thunderstore_api.as_deref(),
            loader.pack_url.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !validate_loader_url(value) {
                return Err(CatalogError::LoaderUrl(value.to_string()));
            }
        }
    }
    Ok(catalog)
}

pub fn parse(json: &str) -> Result<Catalog, CatalogError> {
    validate(serde_json::from_str(json)?)
}

/// Pick an unambiguous asset name that matches the architecture rule. If a
/// preferred extension is present, only preferred matches participate.
pub fn select_asset<'a>(rules: &AssetRules, arch: &str, names: &'a [String]) -> Option<&'a String> {
    if arch != "x86" && arch != "x64" {
        return None;
    }
    let rule = rules.per_arch.get(arch)?;
    let re = Regex::new(&rule.pat).ok()?;
    let matches: Vec<&String> = names.iter().filter(|name| re.is_match(name)).collect();
    if let Some(preferred) = &rule.prefer {
        let suffix = format!(".{}", preferred.to_ascii_lowercase());
        let preferred: Vec<&String> = matches
            .iter()
            .copied()
            .filter(|name| name.to_ascii_lowercase().ends_with(&suffix))
            .collect();
        if !preferred.is_empty() {
            return (preferred.len() == 1).then_some(preferred[0]);
        }
    }
    (matches.len() == 1).then(|| matches[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../fixtures/catalog.sample.json");

    #[test]
    fn parses_fixture_and_looks_up_case_insensitively() {
        let cat = parse(SAMPLE).unwrap();
        assert_eq!(cat.schema, 1);
        assert!(cat.get("au-avengers/tou-mira").is_some());
    }

    #[test]
    fn direct_dll_rule_does_not_select_archives() {
        let cat = parse(SAMPLE).unwrap();
        let rules = &cat.get("AU-Avengers/TOU-Mira").unwrap().asset_rules;
        let names = vec![
            "TouMira-v1.6.3-x64-epic-msstore.zip".to_string(),
            "TouMira-v1.6.3-x86-steam-itch.zip".to_string(),
            "TownOfUsMira.dll".to_string(),
        ];
        assert!(select_asset(rules, "x86", &names).is_none());
        assert!(select_asset(rules, "x64", &names).is_none());
        assert_eq!(rules.dll_name.as_deref(), Some("TownOfUsMira.dll"));
    }

    fn entry(id: &str, dependencies: &[&str]) -> String {
        format!(
            r#"{{"id":"{id}","name":"n","summary":"s","repo":"{id}","tags":[],"dependencies":{},"assetRules":{{}}}}"#,
            serde_json::to_string(dependencies).unwrap()
        )
    }

    #[test]
    fn asset_rules_reject_misspelled_keys_but_default_omitted_fields() {
        let misspelled = r#"{"schema":1,"mods":[{"id":"A/One","name":"n","summary":"s","repo":null,"tags":[],"dependencies":[],"assetRules":{"bundlesLoder":true}}]}"#;
        assert!(matches!(parse(misspelled), Err(CatalogError::Json(_))));

        let omitted = r#"{"schema":1,"mods":[{"id":"A/One","name":"n","summary":"s","repo":null,"tags":[],"dependencies":[],"assetRules":{}}]}"#;
        let catalog = parse(omitted).unwrap();
        let rules = &catalog.mods[0].asset_rules;
        assert!(rules.per_arch.is_empty());
        assert!(rules.dll_name.is_none());
        assert!(!rules.bundles_loader);
    }

    #[test]
    fn rejects_uppercase_git_repository_suffix() {
        let catalog = r#"{"schema":1,"mods":[{"id":"A/One","name":"n","summary":"s","repo":"A/Repo.GIT","tags":[],"dependencies":[],"assetRules":{}}]}"#;
        assert!(matches!(
            parse(catalog),
            Err(CatalogError::Identity(repo)) if repo == "A/Repo.GIT"
        ));
    }

    #[test]
    fn rejects_duplicate_implicit_and_explicit_repositories() {
        let catalog = r#"{"schema":1,"mods":[
            {"id":"Shared/Repo","name":"n","summary":"s","repo":null,"tags":[],"dependencies":[],"assetRules":{}},
            {"id":"Other/Mod","name":"n","summary":"s","repo":"shared/repo","tags":[],"dependencies":[],"assetRules":{}}
        ]}"#;
        assert!(matches!(
            parse(catalog),
            Err(CatalogError::DuplicateRepo(repo)) if repo == "shared/repo"
        ));
    }

    #[test]
    fn rejects_cross_entry_repository_id_alias_but_allows_own_pair() {
        let collision = r#"{"schema":1,"mods":[
            {"id":"A/One","name":"n","summary":"s","repo":"b/two","tags":[],"dependencies":[],"assetRules":{}},
            {"id":"B/Two","name":"n","summary":"s","repo":"B/Repository","tags":[],"dependencies":[],"assetRules":{}}
        ]}"#;
        assert!(matches!(
            parse(collision),
            Err(CatalogError::RepositoryAliasesId { id, repo })
                if id == "B/Two" && repo == "b/two"
        ));

        let self_pair = r#"{"schema":1,"mods":[
            {"id":"A/One","name":"n","summary":"s","repo":"a/one","tags":[],"dependencies":[],"assetRules":{}},
            {"id":"B/Two","name":"n","summary":"s","repo":null,"tags":[],"dependencies":[],"assetRules":{}}
        ]}"#;
        let catalog = parse(self_pair).unwrap();
        assert_eq!(catalog.mods.len(), 2);
    }

    #[test]
    fn rejects_bad_schema_duplicates_rules_edges_and_cycles() {
        assert!(matches!(
            parse(r#"{"schema":2,"mods":[]}"#),
            Err(CatalogError::Schema(2))
        ));
        let duplicate = format!(
            r#"{{"schema":1,"mods":[{},{}]}}"#,
            entry("A-One/Repo", &[]),
            entry("a-one/repo", &[])
        );
        assert!(matches!(
            parse(&duplicate),
            Err(CatalogError::DuplicateId(_))
        ));
        let duplicate_repo = r#"{"schema":1,"mods":[
            {"id":"A/One","name":"n","summary":"s","repo":"Shared/Repo","tags":[],"dependencies":[],"assetRules":{}},
            {"id":"B/Two","name":"n","summary":"s","repo":"shared/repo","tags":[],"dependencies":[],"assetRules":{}}
        ]}"#;
        assert!(matches!(
            parse(duplicate_repo),
            Err(CatalogError::DuplicateRepo(_))
        ));
        let bad_rule = r#"{"schema":1,"mods":[{"id":"A/One","name":"n","summary":"s","repo":null,"tags":[],"dependencies":[],"assetRules":{"perArch":{"arm64":{"match":"["}}}}]}"#;
        assert!(matches!(
            parse(bad_rule),
            Err(CatalogError::AssetRule { .. })
        ));
        let bad_regex = r#"{"schema":1,"mods":[{"id":"A/One","name":"n","summary":"s","repo":null,"tags":[],"dependencies":[],"assetRules":{"perArch":{"x86":{"match":"["}}}}]}"#;
        assert!(matches!(
            parse(bad_regex),
            Err(CatalogError::AssetRule { .. })
        ));
        let dangling = format!(r#"{{"schema":1,"mods":[{}]}}"#, entry("A/One", &["B/Two"]));
        assert!(matches!(
            parse(&dangling),
            Err(CatalogError::MissingDependency { .. })
        ));
        let cycle = format!(
            r#"{{"schema":1,"mods":[{},{}]}}"#,
            entry("A/One", &["B/Two"]),
            entry("B/Two", &["A/One"])
        );
        assert!(matches!(
            parse(&cycle),
            Err(CatalogError::DependencyCycle(_))
        ));
    }

    #[test]
    fn rejects_insecure_loader_urls_and_ambiguous_rule_matches() {
        assert!(matches!(
            parse(r#"{"schema":1,"mods":[],"loader":{"packUrl":"http://example.com/a.zip"}}"#),
            Err(CatalogError::LoaderUrl(_))
        ));
        let rules = AssetRules {
            per_arch: HashMap::from([(
                "x86".into(),
                AssetArchRule {
                    pat: r"(?i)\.zip$".into(),
                    prefer: None,
                },
            )]),
            dll_name: None,
            bundles_loader: false,
        };
        assert!(select_asset(&rules, "x86", &["a.zip".into(), "b.zip".into()]).is_none());
    }
    #[test]
    fn validates_and_canonicalizes_versioned_dependencies() {
        let document = r#"{"schema":1,"mods":[
            {"id":"A/Root","name":"Root","summary":"s","repo":null,"tags":[],"dependencies":["b/library"],"dependencyVersions":{"B/LIBRARY":">=2.0.0, <3.0.0"},"assetRules":{}},
            {"id":"B/Library","name":"Library","summary":"s","repo":null,"tags":[],"dependencies":[],"assetRules":{}}
        ]}"#;
        let catalog = parse(document).unwrap();
        let root = catalog.get("A/Root").unwrap();
        assert_eq!(
            root.dependency_versions
                .get("B/Library")
                .map(String::as_str),
            Some(">=2.0.0, <3.0.0")
        );

        let invalid = document.replace(">=2.0.0, <3.0.0", "not-a-requirement");
        assert!(matches!(
            parse(&invalid),
            Err(CatalogError::DependencyVersion { .. })
        ));
    }
}
