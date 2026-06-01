//! ModSourceResolver: turn a GitHub repo (catalog id or pasted URL) into a
//! concrete downloadable asset for the detected architecture.
//!
//! HTTP is behind the `Http` trait so resolution is unit-testable with a mock
//! and live-testable with the real `ureq` client (see the `#[ignore]` test).

use crate::catalog::{self, AssetRules};
use serde::{Deserialize, Serialize};
use std::io::Read;

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("http error: {0}")]
    Http(String),
    #[error("could not parse response: {0}")]
    Parse(String),
    #[error("no asset matched architecture {0}")]
    NoAsset(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Release {
    #[serde(rename = "tag_name")]
    pub tag: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedDownload {
    pub url: String,
    pub asset_name: String,
    pub version: String,
    pub size: u64,
}

/// Abstracts HTTP so the resolver can be tested without the network.
pub trait Http {
    fn get_text(&self, url: &str) -> Result<String, ResolveError>;
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, ResolveError>;
}

/// Real HTTP client (blocking) used at runtime.
pub struct UreqHttp {
    pub token: Option<String>,
}

impl UreqHttp {
    pub fn new(token: Option<String>) -> Self {
        Self { token }
    }
    fn req(&self, url: &str) -> ureq::Request {
        let mut r = ureq::get(url)
            .set("User-Agent", "perfect-sync")
            .set("Accept", "application/vnd.github+json");
        if let Some(t) = &self.token {
            r = r.set("Authorization", &format!("Bearer {t}"));
        }
        r
    }
}

impl Http for UreqHttp {
    fn get_text(&self, url: &str) -> Result<String, ResolveError> {
        self.req(url)
            .call()
            .map_err(|e| ResolveError::Http(e.to_string()))?
            .into_string()
            .map_err(|e| ResolveError::Http(e.to_string()))
    }
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, ResolveError> {
        let resp = self
            .req(url)
            .call()
            .map_err(|e| ResolveError::Http(e.to_string()))?;
        let mut buf = Vec::new();
        resp.into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| ResolveError::Http(e.to_string()))?;
        Ok(buf)
    }
}

pub fn parse_release(json: &str) -> Result<Release, ResolveError> {
    serde_json::from_str(json).map_err(|e| ResolveError::Parse(e.to_string()))
}

/// Normalize `owner/repo` or any GitHub URL into `owner/repo`.
pub fn parse_repo(input: &str) -> Option<String> {
    let input = input.trim();
    let re = regex::Regex::new(r"github\.com/([^/\s]+)/([^/\s#?]+)").unwrap();
    if let Some(c) = re.captures(input) {
        let repo = c[2].trim_end_matches(".git");
        return Some(format!("{}/{}", &c[1], repo));
    }
    let parts: Vec<&str> = input.split('/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        return Some(input.to_string());
    }
    None
}

/// Choose the right asset for `arch`: catalog regex rules first, then a
/// `dllName` exact match, then the lone `.dll` if there is exactly one.
pub fn pick_asset<'a>(rel: &'a Release, rules: &AssetRules, arch: &str) -> Option<&'a Asset> {
    let names: Vec<String> = rel.assets.iter().map(|a| a.name.clone()).collect();
    if let Some(name) = catalog::select_asset(rules, arch, &names) {
        return rel.assets.iter().find(|a| &a.name == name);
    }
    if let Some(dll) = &rules.dll_name {
        if let Some(a) = rel.assets.iter().find(|a| &a.name == dll) {
            return Some(a);
        }
    }
    let dlls: Vec<&Asset> = rel
        .assets
        .iter()
        .filter(|a| a.name.to_lowercase().ends_with(".dll"))
        .collect();
    if dlls.len() == 1 {
        return Some(dlls[0]);
    }
    None
}

pub fn fetch_latest_release(http: &dyn Http, repo: &str) -> Result<Release, ResolveError> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    parse_release(&http.get_text(&url)?)
}

/// Resolve the latest release of `repo` to a concrete download for `arch`.
pub fn resolve_latest(
    http: &dyn Http,
    repo: &str,
    rules: &AssetRules,
    arch: &str,
) -> Result<ResolvedDownload, ResolveError> {
    let rel = fetch_latest_release(http, repo)?;
    let asset = pick_asset(&rel, rules, arch).ok_or_else(|| ResolveError::NoAsset(arch.into()))?;
    Ok(ResolvedDownload {
        url: asset.url.clone(),
        asset_name: asset.name.clone(),
        version: rel.tag.clone(),
        size: asset.size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::parse;

    const CATALOG: &str = include_str!("../fixtures/catalog.sample.json");

    const RELEASE_JSON: &str = r#"{
        "tag_name": "1.6.3",
        "assets": [
            {"name": "TouMira-v1.6.3-x86-steam-itch.zip", "browser_download_url": "https://x/x86.zip", "size": 100},
            {"name": "TouMira-v1.6.3-x64-epic-msstore.zip", "browser_download_url": "https://x/x64.zip", "size": 200},
            {"name": "TownOfUsMira.dll", "browser_download_url": "https://x/dll", "size": 50}
        ]
    }"#;

    struct MockHttp {
        body: String,
    }
    impl Http for MockHttp {
        fn get_text(&self, _url: &str) -> Result<String, ResolveError> {
            Ok(self.body.clone())
        }
        fn get_bytes(&self, _url: &str) -> Result<Vec<u8>, ResolveError> {
            Ok(self.body.clone().into_bytes())
        }
    }

    #[test]
    fn parses_release() {
        let r = parse_release(RELEASE_JSON).unwrap();
        assert_eq!(r.tag, "1.6.3");
        assert_eq!(r.assets.len(), 3);
    }

    #[test]
    fn parses_repo_from_url_and_slug() {
        assert_eq!(parse_repo("https://github.com/AU-Avengers/TOU-Mira").as_deref(), Some("AU-Avengers/TOU-Mira"));
        assert_eq!(parse_repo("https://github.com/AU-Avengers/TOU-Mira.git").as_deref(), Some("AU-Avengers/TOU-Mira"));
        assert_eq!(parse_repo("NuclearPowered/Reactor").as_deref(), Some("NuclearPowered/Reactor"));
        assert_eq!(parse_repo("not a repo"), None);
    }

    #[test]
    fn picks_asset_per_arch() {
        let cat = parse(CATALOG).unwrap();
        let rules = &cat.get("AU-Avengers/TOU-Mira").unwrap().asset_rules;
        let rel = parse_release(RELEASE_JSON).unwrap();
        assert_eq!(pick_asset(&rel, rules, "x86").unwrap().name, "TouMira-v1.6.3-x86-steam-itch.zip");
        assert_eq!(pick_asset(&rel, rules, "x64").unwrap().name, "TouMira-v1.6.3-x64-epic-msstore.zip");
    }

    #[test]
    fn falls_back_to_lone_dll() {
        // a library with no per-arch rules (MiraAPI) and a single DLL asset
        let cat = parse(CATALOG).unwrap();
        let rules = &cat.get("NuclearPowered/Reactor").unwrap().asset_rules;
        let rel = Release {
            tag: "2.5.0".into(),
            assets: vec![Asset { name: "Reactor.dll".into(), url: "https://x/r".into(), size: 10 }],
        };
        assert_eq!(pick_asset(&rel, rules, "x86").unwrap().name, "Reactor.dll");
    }

    #[test]
    fn resolve_latest_via_mock() {
        let cat = parse(CATALOG).unwrap();
        let rules = &cat.get("AU-Avengers/TOU-Mira").unwrap().asset_rules;
        let http = MockHttp { body: RELEASE_JSON.to_string() };
        let r = resolve_latest(&http, "AU-Avengers/TOU-Mira", rules, "x86").unwrap();
        assert_eq!(r.version, "1.6.3");
        assert_eq!(r.asset_name, "TouMira-v1.6.3-x86-steam-itch.zip");
        assert_eq!(r.url, "https://x/x86.zip");
    }

    // Live network smoke test. Run with: cargo test -p perfect-sync-core -- --ignored
    #[test]
    #[ignore]
    fn live_fetch_reactor_release() {
        let http = UreqHttp::new(None);
        let rel = fetch_latest_release(&http, "NuclearPowered/Reactor").unwrap();
        assert!(!rel.tag.is_empty(), "expected a tag");
        assert!(
            rel.assets.iter().any(|a| a.name.to_lowercase().contains("reactor")),
            "expected a Reactor asset, got: {:?}",
            rel.assets.iter().map(|a| &a.name).collect::<Vec<_>>()
        );
    }
}
