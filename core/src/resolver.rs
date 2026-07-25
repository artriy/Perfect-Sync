//! ModSourceResolver: turn a GitHub repo (catalog id or pasted URL) into a
//! concrete downloadable asset for the detected architecture.
//!
//! HTTP is behind the `Http` trait so resolution is unit-testable with a mock.

use crate::catalog::{self, AssetRules};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::Read;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("http error: {0}")]
    Http(String),
    #[error("HTTP status {0}")]
    HttpStatus(u16),
    #[error("could not parse response: {0}")]
    Parse(String),
    #[error("invalid repository or GitHub URL: {0:?}")]
    InvalidRepo(String),
    #[error("only absolute HTTPS URLs are allowed: {0:?}")]
    InsecureUrl(String),
    #[error("{0}")]
    NoAsset(String),
    #[error("{0}")]
    NoRelease(String),
    #[error("download size mismatch: expected {expected} bytes, received {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("download SHA-256 digest does not match release metadata")]
    DigestMismatch,
}

/// The declared byte length and optional GitHub SHA-256 digest travel together
/// from an Asset into a ResolvedDownload. Serialization remains the historical
/// numeric byte length, keeping the frontend contract stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetSize {
    bytes: u64,
    sha256: Option<[u8; 32]>,
}

impl AssetSize {
    pub const fn new(bytes: u64) -> Self {
        Self {
            bytes,
            sha256: None,
        }
    }

    pub const fn bytes(self) -> u64 {
        self.bytes
    }
}

impl fmt::Display for AssetSize {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.bytes)
    }
}
impl PartialEq<u64> for AssetSize {
    fn eq(&self, other: &u64) -> bool {
        self.bytes == *other
    }
}

impl PartialEq<AssetSize> for u64 {
    fn eq(&self, other: &AssetSize) -> bool {
        *self == other.bytes
    }
}

impl Serialize for AssetSize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(self.bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: AssetSize,
}

#[derive(Deserialize)]
struct AssetWire {
    name: String,
    #[serde(rename = "browser_download_url")]
    url: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

fn parse_sha256(value: &str) -> Option<[u8; 32]> {
    let hex = value.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut result = [0u8; 32];
    for (index, output) in result.iter_mut().enumerate() {
        *output = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(result)
}

fn format_sha256(value: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(71);
    result.push_str("sha256:");
    for byte in value {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

impl<'de> Deserialize<'de> for Asset {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = AssetWire::deserialize(deserializer)?;
        let sha256 = wire
            .digest
            .as_deref()
            .map(|value| {
                parse_sha256(value).ok_or_else(|| {
                    <D::Error as serde::de::Error>::custom("invalid GitHub sha256 digest")
                })
            })
            .transpose()?;
        Ok(Self {
            name: wire.name,
            url: wire.url,
            size: AssetSize {
                bytes: wire.size,
                sha256,
            },
        })
    }
}

impl Serialize for Asset {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state =
            serializer.serialize_struct("Asset", if self.size.sha256.is_some() { 4 } else { 3 })?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("browser_download_url", &self.url)?;
        state.serialize_field("size", &self.size.bytes)?;
        if let Some(digest) = &self.size.sha256 {
            state.serialize_field("digest", &format_sha256(digest))?;
        }
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Release {
    #[serde(rename = "tag_name")]
    pub tag: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedDownload {
    pub url: String,
    pub asset_name: String,
    pub version: String,
    pub size: AssetSize,
}

/// Abstracts raw HTTP so callers and resolver tests can use controlled sources.
pub trait Http {
    fn get_text(&self, url: &str) -> Result<String, ResolveError>;
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, ResolveError>;

    fn get_bytes_with_progress(
        &self,
        url: &str,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<Vec<u8>, ResolveError> {
        let bytes = self.get_bytes(url)?;
        let size = bytes.len() as u64;
        on_progress(size, Some(size));
        Ok(bytes)
    }
}

/// Real HTTPS client (blocking) used at runtime.
pub struct UreqHttp {
    agent: ureq::Agent,
    authorization: Option<String>,
}

const MAX_DOWNLOAD: u64 = 300 * 1024 * 1024;
const MAX_TEXT_RESPONSE: u64 = 8 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const OVERALL_TIMEOUT: Duration = Duration::from_secs(60);

fn parsed_https_url(value: &str) -> Result<url::Url, ResolveError> {
    let parsed =
        url::Url::parse(value).map_err(|_| ResolveError::InsecureUrl(value.to_string()))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(ResolveError::InsecureUrl(value.to_string()));
    }
    Ok(parsed)
}

fn is_github_host(url: &str) -> bool {
    let Ok(parsed) = parsed_https_url(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or_default();
    host.eq_ignore_ascii_case("api.github.com")
        || host.eq_ignore_ascii_case("github.com")
        || host.eq_ignore_ascii_case("githubusercontent.com")
        || host
            .to_ascii_lowercase()
            .ends_with(".githubusercontent.com")
}

impl UreqHttp {
    pub fn new(token: Option<String>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .https_only(true)
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_read(IO_TIMEOUT)
            .timeout_write(IO_TIMEOUT)
            .timeout(OVERALL_TIMEOUT)
            .build();
        Self {
            agent,
            authorization: token.map(|token| format!("Bearer {token}")),
        }
    }

    fn req(&self, url: &str) -> Result<ureq::Request, ResolveError> {
        parsed_https_url(url)?;
        let mut request = self.agent.get(url).set("User-Agent", "perfect-sync");
        if is_github_host(url) {
            if let Some(authorization) = &self.authorization {
                request = request.set("Authorization", authorization);
            }
        }
        Ok(request)
    }

    fn call(&self, url: &str) -> Result<ureq::Response, ResolveError> {
        self.req(url)?.call().map_err(|error| match error {
            ureq::Error::Status(status, _) => ResolveError::HttpStatus(status),
            ureq::Error::Transport(error) => ResolveError::Http(error.to_string()),
        })
    }
}

impl Http for UreqHttp {
    fn get_text(&self, url: &str) -> Result<String, ResolveError> {
        let mut bytes = Vec::new();
        self.call(url)?
            .into_reader()
            .take(MAX_TEXT_RESPONSE + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| ResolveError::Http(error.to_string()))?;
        if bytes.len() as u64 > MAX_TEXT_RESPONSE {
            return Err(ResolveError::Http("text response too large".into()));
        }
        String::from_utf8(bytes).map_err(|error| ResolveError::Parse(error.to_string()))
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, ResolveError> {
        self.get_bytes_with_progress(url, &mut |_, _| {})
    }

    fn get_bytes_with_progress(
        &self,
        url: &str,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<Vec<u8>, ResolveError> {
        let response = self.call(url)?;
        let total = response
            .header("Content-Length")
            .and_then(|value| value.parse::<u64>().ok());
        let mut reader = response.into_reader().take(MAX_DOWNLOAD + 1);
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 64 * 1024];
        on_progress(0, total);
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|error| ResolveError::Http(error.to_string()))?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            on_progress(bytes.len() as u64, total);
        }
        if bytes.len() as u64 > MAX_DOWNLOAD {
            return Err(ResolveError::Http("download too large".into()));
        }
        Ok(bytes)
    }
}

/// Download and verify a resolved release asset. Raw `Http::get_bytes` remains
/// available for non-release resources and controlled tests.
pub fn download_resolved(
    http: &dyn Http,
    resolved: &ResolvedDownload,
) -> Result<Vec<u8>, ResolveError> {
    parsed_https_url(&resolved.url)?;
    let bytes = http.get_bytes(&resolved.url)?;
    let actual = bytes.len() as u64;
    if actual != resolved.size.bytes {
        return Err(ResolveError::SizeMismatch {
            expected: resolved.size.bytes,
            actual,
        });
    }
    if let Some(expected) = resolved.size.sha256 {
        let actual: [u8; 32] = Sha256::digest(&bytes).into();
        if actual != expected {
            return Err(ResolveError::DigestMismatch);
        }
    }
    Ok(bytes)
}

pub fn parse_release(json: &str) -> Result<Release, ResolveError> {
    serde_json::from_str(json).map_err(|error| ResolveError::Parse(error.to_string()))
}

fn normalize_slug(value: &str) -> Option<String> {
    let (owner, mut repo) = value.split_once('/')?;
    if repo
        .get(repo.len().saturating_sub(4)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".git"))
    {
        repo = &repo[..repo.len() - 4];
    }
    let normalized = format!("{owner}/{repo}");
    catalog::valid_repo_slug(&normalized).then_some(normalized)
}

/// Normalize a strict `owner/repo` slug or an exact HTTPS github.com repository
/// URL into `owner/repo`. Lookalike hosts, extra path components and credentials
/// are rejected.
pub fn parse_repo(input: &str) -> Option<String> {
    let input = input.trim();
    if input.contains("://") {
        let parsed = url::Url::parse(input).ok()?;
        if parsed.scheme() != "https"
            || !parsed.host_str()?.eq_ignore_ascii_case("github.com")
            || parsed.port().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return None;
        }
        let path = parsed.path().strip_prefix('/')?;
        let path = path.strip_suffix('/').unwrap_or(path);
        if path.is_empty()
            || path.starts_with('/')
            || path.ends_with('/')
            || path.matches('/').count() != 1
        {
            return None;
        }
        normalize_slug(path)
    } else {
        normalize_slug(input)
    }
}

/// Choose an asset only when the architecture is known and the catalog rule,
/// configured DLL name, or conservative fallback identifies exactly one DLL.
pub fn pick_asset<'a>(rel: &'a Release, rules: &AssetRules, arch: &str) -> Option<&'a Asset> {
    if arch != "x86" && arch != "x64" {
        return None;
    }
    if let Some(dll) = &rules.dll_name {
        if let Some(asset) = rel
            .assets
            .iter()
            .find(|asset| asset.name.eq_ignore_ascii_case(dll))
        {
            return Some(asset);
        }
    }
    let names: Vec<String> = rel
        .assets
        .iter()
        .filter(|asset| asset.name.to_ascii_lowercase().ends_with(".dll"))
        .map(|asset| asset.name.clone())
        .collect();
    if let Some(name) = catalog::select_asset(rules, arch, &names) {
        return rel.assets.iter().find(|asset| asset.name == *name);
    }
    if rules.per_arch.contains_key(arch) {
        return None;
    }
    let mut candidates = rel
        .assets
        .iter()
        .filter(|asset| asset.name.to_ascii_lowercase().ends_with(".dll"));
    let selected = candidates.next()?;
    candidates.next().is_none().then_some(selected)
}

fn canonical_repo(repo: &str) -> Result<String, ResolveError> {
    parse_repo(repo).ok_or_else(|| ResolveError::InvalidRepo(repo.to_string()))
}

fn ensure_not_draft(repo: &str, release: Release) -> Result<Release, ResolveError> {
    if release.draft {
        Err(ResolveError::NoRelease(format!(
            "{repo} release {} is a draft",
            release.tag
        )))
    } else {
        Ok(release)
    }
}

pub fn fetch_latest_release(http: &dyn Http, repo: &str) -> Result<Release, ResolveError> {
    let repo = canonical_repo(repo)?;
    let latest = format!("https://api.github.com/repos/{repo}/releases/latest");
    match http.get_text(&latest) {
        Ok(text) => return ensure_not_draft(&repo, parse_release(&text)?),
        Err(ResolveError::HttpStatus(404)) => {}
        Err(error) => return Err(error),
    }

    // GitHub's latest endpoint returns 404 when a repository has no stable
    // release. Only that condition permits falling back to prereleases.
    let list = format!("https://api.github.com/repos/{repo}/releases?per_page=100");
    let text = http.get_text(&list)?;
    let releases: Vec<Release> =
        serde_json::from_str(&text).map_err(|error| ResolveError::Parse(error.to_string()))?;
    releases
        .into_iter()
        .find(|release| !release.draft)
        .ok_or_else(|| ResolveError::NoRelease(format!("no non-draft releases for {repo}")))
}

pub fn fetch_release_by_tag(
    http: &dyn Http,
    repo: &str,
    tag: &str,
) -> Result<Release, ResolveError> {
    if tag.is_empty() || tag.len() > 255 || tag.chars().any(char::is_control) {
        return Err(ResolveError::Parse(
            "release tag must be 1..=255 non-control bytes".into(),
        ));
    }
    let repo = canonical_repo(repo)?;
    let tag = utf8_percent_encode(tag, NON_ALPHANUMERIC);
    let url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
    ensure_not_draft(&repo, parse_release(&http.get_text(&url)?)?)
}

fn resolved(rel: &Release, asset: &Asset) -> Result<ResolvedDownload, ResolveError> {
    parsed_https_url(&asset.url)?;
    if asset.size.bytes > MAX_DOWNLOAD {
        return Err(ResolveError::NoAsset(format!(
            "release asset {} exceeds the maximum download size",
            asset.name
        )));
    }
    Ok(ResolvedDownload {
        url: asset.url.clone(),
        asset_name: asset.name.clone(),
        version: rel.tag.clone(),
        size: asset.size,
    })
}

/// Resolve an exact release `tag` to a concrete download for `arch`.
pub fn resolve_tag(
    http: &dyn Http,
    repo: &str,
    tag: &str,
    rules: &AssetRules,
    arch: &str,
) -> Result<ResolvedDownload, ResolveError> {
    let canonical = canonical_repo(repo)?;
    let rel = fetch_release_by_tag(http, &canonical, tag)?;
    let asset = pick_asset(&rel, rules, arch).ok_or_else(|| no_asset_err(&canonical, &rel))?;
    resolved(&rel, asset)
}

/// Resolve the latest release of `repo` to a concrete download for `arch`.
pub fn resolve_latest(
    http: &dyn Http,
    repo: &str,
    rules: &AssetRules,
    arch: &str,
) -> Result<ResolvedDownload, ResolveError> {
    let canonical = canonical_repo(repo)?;
    let rel = fetch_latest_release(http, &canonical)?;
    let asset = pick_asset(&rel, rules, arch).ok_or_else(|| no_asset_err(&canonical, &rel))?;
    resolved(&rel, asset)
}

fn no_asset_err(repo: &str, rel: &Release) -> ResolveError {
    let names: Vec<&str> = rel.assets.iter().map(|asset| asset.name.as_str()).collect();
    if names.is_empty() {
        ResolveError::NoAsset(format!(
            "{repo} release {} has no downloadable files (only source). Create a GitHub release with the mod .dll, or pick a file manually.",
            rel.tag
        ))
    } else {
        ResolveError::NoAsset(format!(
            "{repo}: could not auto-pick a file from release {} (files: {}). Use the file picker to choose one.",
            rel.tag,
            names.join(", ")
        ))
    }
}

/// List a repo's recent non-draft releases (for the manual picker).
pub fn fetch_releases(
    http: &dyn Http,
    repo: &str,
    per_page: u32,
) -> Result<Vec<Release>, ResolveError> {
    if !(1..=100).contains(&per_page) {
        return Err(ResolveError::Parse(
            "per_page must be between 1 and 100".into(),
        ));
    }
    let repo = canonical_repo(repo)?;
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page={per_page}");
    let releases: Vec<Release> = serde_json::from_str(&http.get_text(&url)?)
        .map_err(|error| ResolveError::Parse(error.to_string()))?;
    Ok(releases
        .into_iter()
        .filter(|release| !release.draft)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::parse;
    use std::cell::RefCell;

    const CATALOG: &str = include_str!("../fixtures/catalog.sample.json");
    const RELEASE_JSON: &str = r#"{
        "tag_name": "1.6.3", "draft": false, "prerelease": true,
        "assets": [
            {"name":"TouMira-v1.6.3-x86-steam-itch.zip","browser_download_url":"https://x/x86.zip","size":100},
            {"name":"TouMira-v1.6.3-x64-epic-msstore.zip","browser_download_url":"https://x/x64.zip","size":200},
            {"name":"TownOfUsMira.dll","browser_download_url":"https://x/dll","size":50}
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

    struct RecordingHttp {
        status: u16,
        body: String,
        urls: RefCell<Vec<String>>,
    }
    impl Http for RecordingHttp {
        fn get_text(&self, url: &str) -> Result<String, ResolveError> {
            self.urls.borrow_mut().push(url.to_string());
            if self.status == 200 {
                Ok(self.body.clone())
            } else {
                Err(ResolveError::HttpStatus(self.status))
            }
        }
        fn get_bytes(&self, _url: &str) -> Result<Vec<u8>, ResolveError> {
            Ok(self.body.as_bytes().to_vec())
        }
    }

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.into(),
            url: format!("https://x/{name}"),
            size: AssetSize::new(1),
        }
    }

    fn release(assets: Vec<Asset>) -> Release {
        Release {
            tag: "1.0.0".into(),
            assets,
            draft: false,
            prerelease: false,
        }
    }

    #[test]
    fn preserves_release_flags() {
        let release = parse_release(RELEASE_JSON).unwrap();
        assert!(!release.draft);
        assert!(release.prerelease);
    }

    #[test]
    fn parses_only_exact_github_url_and_strict_slug() {
        assert_eq!(
            parse_repo("https://github.com/AU-Avengers/TOU-Mira.git/").as_deref(),
            Some("AU-Avengers/TOU-Mira")
        );
        assert_eq!(
            parse_repo("NuclearPowered/Reactor").as_deref(),
            Some("NuclearPowered/Reactor")
        );
        assert!(parse_repo("http://github.com/A/B").is_none());
        assert!(parse_repo("https://github.evil.com/A/B").is_none());
        assert!(parse_repo("https://github.com/A/B/releases").is_none());
        assert!(parse_repo("https://github.com//A/B").is_none());
        assert!(parse_repo("https://github.com/A/B//").is_none());
        assert!(parse_repo("A--B/Repo").is_none());
    }

    #[test]
    fn github_token_allowlist_requires_https() {
        assert!(is_github_host("https://api.github.com/repos/x/y/releases"));
        assert!(is_github_host(
            "https://raw.githubusercontent.com/x/y/main/f.dll"
        ));
        assert!(!is_github_host("http://github.com/x/y"));
        assert!(!is_github_host("https://github.evil.com/x"));
        assert!(matches!(
            UreqHttp::new(Some("secret".into())).req("http://github.com/x/y"),
            Err(ResolveError::InsecureUrl(_))
        ));
    }

    #[test]
    fn picks_only_dll_assets_for_known_arch() {
        let cat = parse(CATALOG).unwrap();
        let rules = &cat.get("AU-Avengers/TOU-Mira").unwrap().asset_rules;
        let rel = parse_release(RELEASE_JSON).unwrap();
        assert_eq!(
            pick_asset(&rel, rules, "x86").unwrap().name,
            "TownOfUsMira.dll"
        );
        assert!(pick_asset(&rel, rules, "arm64").is_none());

        let empty_rules = AssetRules {
            per_arch: Default::default(),
            dll_name: None,
            bundles_loader: false,
        };
        assert!(pick_asset(
            &release(vec![asset("a.dll"), asset("b.dll")]),
            &empty_rules,
            "x86"
        )
        .is_none());
        assert_eq!(
            pick_asset(
                &release(vec![asset("a.dll"), asset("a.zip")]),
                &empty_rules,
                "x86"
            )
            .unwrap()
            .name,
            "a.dll"
        );
        assert!(pick_asset(&release(vec![asset("bundle.zip")]), &empty_rules, "x64").is_none());
    }

    #[test]
    fn latest_falls_back_only_for_no_stable_release_and_skips_drafts() {
        for status in [429, 500] {
            let http = RecordingHttp {
                status,
                body: String::new(),
                urls: RefCell::new(Vec::new()),
            };
            assert!(
                matches!(fetch_latest_release(&http, "A/Repo"), Err(ResolveError::HttpStatus(code)) if code == status)
            );
            assert_eq!(http.urls.borrow().len(), 1);
        }

        struct Fallback(RefCell<Vec<String>>);
        impl Http for Fallback {
            fn get_text(&self, url: &str) -> Result<String, ResolveError> {
                self.0.borrow_mut().push(url.into());
                if url.ends_with("/latest") {
                    Err(ResolveError::HttpStatus(404))
                } else {
                    Ok(r#"[{"tag_name":"draft","draft":true,"prerelease":false,"assets":[]},{"tag_name":"beta","draft":false,"prerelease":true,"assets":[]}]"#.into())
                }
            }
            fn get_bytes(&self, _: &str) -> Result<Vec<u8>, ResolveError> {
                unreachable!()
            }
        }
        let http = Fallback(RefCell::new(Vec::new()));
        let release = fetch_latest_release(&http, "A/Repo").unwrap();
        assert_eq!(release.tag, "beta");
        assert!(release.prerelease);
    }

    #[test]
    fn release_tag_is_one_encoded_path_segment() {
        let http = RecordingHttp {
            status: 200,
            body: r#"{"tag_name":"a/b","assets":[]}"#.into(),
            urls: RefCell::new(Vec::new()),
        };
        fetch_release_by_tag(&http, "A/Repo", "a/b").unwrap();
        assert!(http.urls.borrow()[0].ends_with("/tags/a%2Fb"));
    }

    #[test]
    fn verified_download_rejects_length_and_digest_mismatches() {
        let digest: [u8; 32] = Sha256::digest(b"good").into();
        let digest = format_sha256(&digest);
        let json = format!(
            r#"{{"tag_name":"1","assets":[{{"name":"a.dll","browser_download_url":"https://x/a","size":4,"digest":"{digest}"}}]}}"#
        );
        let release = parse_release(&json).unwrap();
        let resolved = resolved(&release, &release.assets[0]).unwrap();

        let short = MockHttp { body: "bad".into() };
        assert!(matches!(
            download_resolved(&short, &resolved),
            Err(ResolveError::SizeMismatch { .. })
        ));
        let long = MockHttp {
            body: "extra".into(),
        };
        assert!(matches!(
            download_resolved(&long, &resolved),
            Err(ResolveError::SizeMismatch { .. })
        ));
        let wrong = MockHttp {
            body: "evil".into(),
        };
        assert!(matches!(
            download_resolved(&wrong, &resolved),
            Err(ResolveError::DigestMismatch)
        ));
        let good = MockHttp {
            body: "good".into(),
        };
        assert_eq!(download_resolved(&good, &resolved).unwrap(), b"good");
    }

    #[test]
    fn mock_download_progress_reports_verified_byte_count() {
        let http = MockHttp {
            body: "good".into(),
        };
        let mut updates = Vec::new();
        let bytes = http
            .get_bytes_with_progress("https://x/a", &mut |received, total| {
                updates.push((received, total));
            })
            .unwrap();
        assert_eq!(bytes, b"good");
        assert_eq!(updates, vec![(4, Some(4))]);
    }

    #[test]
    fn resolve_latest_via_mock() {
        let cat = parse(CATALOG).unwrap();
        let rules = &cat.get("AU-Avengers/TOU-Mira").unwrap().asset_rules;
        let http = MockHttp {
            body: RELEASE_JSON.to_string(),
        };
        let result = resolve_latest(&http, "AU-Avengers/TOU-Mira", rules, "x86").unwrap();
        assert_eq!(result.version, "1.6.3");
        assert_eq!(result.asset_name, "TownOfUsMira.dll");
    }
}
