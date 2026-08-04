//! ModSourceResolver: turn a GitHub repo (catalog id or pasted URL) into a
//! concrete downloadable asset for the detected architecture.
//!
//! HTTP is behind the `Http` trait so resolution is unit-testable with a mock.

use crate::catalog::{self, AssetRules};
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{Read, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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

    pub const fn sha256(self) -> Option<[u8; 32]> {
        self.sha256
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextResponse {
    pub body: String,
    pub final_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadResponse {
    pub final_url: String,
    pub content_length: Option<u64>,
}

/// Abstracts raw HTTP so callers and resolver tests can use controlled sources.
pub trait Http {
    fn get_text(&self, url: &str) -> Result<String, ResolveError>;

    fn get_text_fresh(&self, url: &str) -> Result<String, ResolveError> {
        Ok(self.get_text_with_url_fresh(url)?.body)
    }

    fn get_text_with_url(&self, url: &str) -> Result<TextResponse, ResolveError> {
        Ok(TextResponse {
            body: self.get_text(url)?,
            final_url: url.to_string(),
        })
    }

    fn get_text_with_url_fresh(&self, url: &str) -> Result<TextResponse, ResolveError> {
        self.get_text_with_url(url)
    }

    fn head(&self, _url: &str) -> Result<HeadResponse, ResolveError> {
        Err(ResolveError::Http(
            "this HTTP client does not support HEAD requests".into(),
        ))
    }
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

    fn download_to(
        &self,
        url: &str,
        output: &mut dyn Write,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<u64, ResolveError> {
        let bytes = self.get_bytes(url)?;
        output
            .write_all(&bytes)
            .map_err(|error| ResolveError::Http(error.to_string()))?;
        let size = bytes.len() as u64;
        on_progress(size, Some(size));
        Ok(size)
    }
}

/// Real HTTPS client (blocking) used at runtime.
#[derive(Clone)]
pub struct UreqHttp {
    agent: ureq::Agent,
    download_agent: ureq::Agent,
    authorization: Option<String>,
    metadata_cache: Arc<MetadataCache>,
}

#[derive(Default)]
struct MetadataCacheState {
    responses: HashMap<String, CachedTextResponse>,
    in_flight: HashSet<String>,
    active_requests: usize,
}

struct CachedTextResponse {
    response: TextResponse,
    cached_at: Instant,
}

#[derive(Default)]
struct MetadataCache {
    state: Mutex<MetadataCacheState>,
    ready: Condvar,
}

const MAX_DOWNLOAD: u64 = 300 * 1024 * 1024;
const MAX_TEXT_RESPONSE: u64 = 8 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const METADATA_IO_TIMEOUT: Duration = Duration::from_secs(30);
const METADATA_OVERALL_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_IO_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const METADATA_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_METADATA_CACHE_ENTRIES: usize = 256;
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

fn build_agent(io_timeout: Duration, overall_timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .https_only(true)
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(io_timeout)
        .timeout_write(io_timeout)
        .timeout(overall_timeout)
        .build()
}
fn build_download_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(DOWNLOAD_CONNECT_TIMEOUT)
        .timeout_read(DOWNLOAD_IO_TIMEOUT)
        .timeout_write(DOWNLOAD_IO_TIMEOUT)
        .build()
}

impl UreqHttp {
    pub fn new(token: Option<String>) -> Self {
        Self {
            agent: build_agent(METADATA_IO_TIMEOUT, METADATA_OVERALL_TIMEOUT),
            download_agent: build_download_agent(),
            authorization: token.map(|token| format!("Bearer {token}")),
            metadata_cache: Arc::new(MetadataCache::default()),
        }
    }

    fn request_with_agent(
        &self,
        agent: &ureq::Agent,
        method: &str,
        url: &str,
    ) -> Result<ureq::Request, ResolveError> {
        parsed_https_url(url)?;
        let mut request = agent.request(method, url).set("User-Agent", "perfect-sync");
        if is_github_host(url) {
            if let Some(authorization) = &self.authorization {
                request = request.set("Authorization", authorization);
            }
        }
        Ok(request)
    }

    fn request(&self, method: &str, url: &str) -> Result<ureq::Request, ResolveError> {
        self.request_with_agent(&self.agent, method, url)
    }

    #[cfg(test)]
    fn req(&self, url: &str) -> Result<ureq::Request, ResolveError> {
        self.request("GET", url)
    }

    fn call_method(&self, method: &str, url: &str) -> Result<ureq::Response, ResolveError> {
        self.request(method, url)?
            .call()
            .map_err(|error| match error {
                ureq::Error::Status(status, _) => ResolveError::HttpStatus(status),
                ureq::Error::Transport(error) => ResolveError::Http(error.to_string()),
            })
    }

    fn call(&self, url: &str) -> Result<ureq::Response, ResolveError> {
        self.call_method("GET", url)
    }

    fn call_download(&self, url: &str) -> Result<ureq::Response, ResolveError> {
        self.request_with_agent(&self.download_agent, "GET", url)?
            .call()
            .map_err(|error| match error {
                ureq::Error::Status(status, _) => ResolveError::HttpStatus(status),
                ureq::Error::Transport(error) => ResolveError::Http(error.to_string()),
            })
    }

    fn fetch_text(&self, url: &str) -> Result<TextResponse, ResolveError> {
        let response = self.call(url)?;
        let final_url = response.get_url().to_string();
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_TEXT_RESPONSE + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| ResolveError::Http(error.to_string()))?;
        if bytes.len() as u64 > MAX_TEXT_RESPONSE {
            return Err(ResolveError::Http("text response too large".into()));
        }
        let body =
            String::from_utf8(bytes).map_err(|error| ResolveError::Parse(error.to_string()))?;
        Ok(TextResponse { body, final_url })
    }

    fn cached_text(
        &self,
        url: &str,
        fresh_after: Option<Instant>,
    ) -> Result<TextResponse, ResolveError> {
        loop {
            let mut state = self
                .metadata_cache
                .state
                .lock()
                .map_err(|_| ResolveError::Http("metadata cache lock is poisoned".into()))?;
            if let Some(cached) = state.responses.get(url) {
                let current = fresh_after.map_or_else(
                    || cached.cached_at.elapsed() < METADATA_CACHE_TTL,
                    |started| cached.cached_at >= started,
                );
                if current {
                    return Ok(cached.response.clone());
                }
            }
            state.responses.remove(url);
            if state.in_flight.insert(url.to_string()) {
                break;
            }
            drop(
                self.metadata_cache
                    .ready
                    .wait(state)
                    .map_err(|_| ResolveError::Http("metadata cache lock is poisoned".into()))?,
            );
        }

        {
            let mut state = self
                .metadata_cache
                .state
                .lock()
                .map_err(|_| ResolveError::Http("metadata cache lock is poisoned".into()))?;
            while state.active_requests >= 4 {
                state =
                    self.metadata_cache.ready.wait(state).map_err(|_| {
                        ResolveError::Http("metadata cache lock is poisoned".into())
                    })?;
            }
            state.active_requests += 1;
        }
        let result = self.fetch_text(url);
        let mut state = self
            .metadata_cache
            .state
            .lock()
            .map_err(|_| ResolveError::Http("metadata cache lock is poisoned".into()))?;
        state.in_flight.remove(url);
        state.active_requests = state.active_requests.saturating_sub(1);
        if let Ok(response) = &result {
            state
                .responses
                .retain(|_, cached| cached.cached_at.elapsed() < METADATA_CACHE_TTL);
            if state.responses.len() >= MAX_METADATA_CACHE_ENTRIES {
                if let Some(oldest) = state
                    .responses
                    .iter()
                    .max_by_key(|(_, cached)| cached.cached_at.elapsed())
                    .map(|(key, _)| key.clone())
                {
                    state.responses.remove(&oldest);
                }
            }
            state.responses.insert(
                url.to_string(),
                CachedTextResponse {
                    response: response.clone(),
                    cached_at: Instant::now(),
                },
            );
        }
        self.metadata_cache.ready.notify_all();
        result
    }
}

impl Http for UreqHttp {
    fn get_text(&self, url: &str) -> Result<String, ResolveError> {
        Ok(self.get_text_with_url(url)?.body)
    }

    fn get_text_with_url(&self, url: &str) -> Result<TextResponse, ResolveError> {
        parsed_https_url(url)?;
        self.cached_text(url, None)
    }

    fn get_text_with_url_fresh(&self, url: &str) -> Result<TextResponse, ResolveError> {
        parsed_https_url(url)?;
        self.cached_text(url, Some(Instant::now()))
    }

    fn head(&self, url: &str) -> Result<HeadResponse, ResolveError> {
        let response = self.call_method("HEAD", url)?;
        let final_url = response.get_url().to_string();
        let content_length = response
            .header("Content-Length")
            .and_then(|value| value.parse::<u64>().ok());
        Ok(HeadResponse {
            final_url,
            content_length,
        })
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, ResolveError> {
        self.get_bytes_with_progress(url, &mut |_, _| {})
    }

    fn get_bytes_with_progress(
        &self,
        url: &str,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<Vec<u8>, ResolveError> {
        let mut bytes = Vec::new();
        self.download_to(url, &mut bytes, on_progress)?;
        Ok(bytes)
    }

    fn download_to(
        &self,
        url: &str,
        output: &mut dyn Write,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<u64, ResolveError> {
        let response = self.call_download(url)?;
        let total = response
            .header("Content-Length")
            .and_then(|value| value.parse::<u64>().ok());
        let mut reader = response.into_reader().take(MAX_DOWNLOAD + 1);
        let mut buffer = [0_u8; 64 * 1024];
        let mut received = 0_u64;
        on_progress(0, total);
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|error| ResolveError::Http(error.to_string()))?;
            if count == 0 {
                break;
            }
            received += count as u64;
            if received > MAX_DOWNLOAD {
                return Err(ResolveError::Http("download too large".into()));
            }
            output
                .write_all(&buffer[..count])
                .map_err(|error| ResolveError::Http(error.to_string()))?;
            on_progress(received, total);
        }
        Ok(received)
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

struct VerifyingWriter<'a> {
    output: &'a mut dyn Write,
    hasher: Sha256,
    bytes: u64,
}

impl Write for VerifyingWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.output.write(buffer)?;
        self.hasher.update(&buffer[..written]);
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

pub fn download_resolved_to_writer(
    http: &dyn Http,
    resolved: &ResolvedDownload,
    output: &mut dyn Write,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(), ResolveError> {
    parsed_https_url(&resolved.url)?;
    let mut verifying = VerifyingWriter {
        output,
        hasher: Sha256::new(),
        bytes: 0,
    };
    let actual = http.download_to(&resolved.url, &mut verifying, on_progress)?;
    if actual != verifying.bytes || actual != resolved.size.bytes {
        return Err(ResolveError::SizeMismatch {
            expected: resolved.size.bytes,
            actual,
        });
    }
    if let Some(expected) = resolved.size.sha256 {
        let actual: [u8; 32] = verifying.hasher.finalize().into();
        if actual != expected {
            return Err(ResolveError::DigestMismatch);
        }
    }
    Ok(())
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
    let names: Vec<String> = rel.assets.iter().map(|asset| asset.name.clone()).collect();
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

fn validate_tag(tag: &str) -> Result<(), ResolveError> {
    if tag.is_empty() || tag.len() > 255 || tag.chars().any(char::is_control) {
        Err(ResolveError::Parse(
            "release tag must be 1..=255 non-control bytes".into(),
        ))
    } else {
        Ok(())
    }
}

fn decoded_segment(segment: &str) -> Result<String, ResolveError> {
    percent_decode_str(segment)
        .decode_utf8()
        .map(String::from)
        .map_err(|error| ResolveError::Parse(error.to_string()))
}

fn release_tag_from_url(repo: &str, value: &str) -> Option<String> {
    let parsed = url::Url::parse(value).ok()?;
    if parsed.scheme() != "https" || !parsed.host_str()?.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let segments: Vec<&str> = parsed.path_segments()?.collect();
    let (owner, name) = repo.split_once('/')?;
    if segments.len() != 5
        || !segments[0].eq_ignore_ascii_case(owner)
        || !segments[1].eq_ignore_ascii_case(name)
        || segments[2] != "releases"
        || segments[3] != "tag"
    {
        return None;
    }
    decoded_segment(segments[4]).ok()
}

fn release_tag_from_response(repo: &str, response: &TextResponse) -> Option<String> {
    release_tag_from_url(repo, &response.final_url).or_else(|| {
        let marker = format!("/{repo}/releases/tag/");
        response.body.split(&marker).nth(1).and_then(|suffix| {
            let encoded = suffix
                .split(['"', '\'', '?', '#', '<'])
                .next()
                .unwrap_or_default();
            decoded_segment(encoded).ok()
        })
    })
}

fn parse_expanded_assets(repo: &str, tag: &str, html: &str) -> Result<Vec<Asset>, ResolveError> {
    let link_pattern = Regex::new(r#"href="([^"]*/releases/download/[^"]+)""#)
        .map_err(|error| ResolveError::Parse(error.to_string()))?;
    let digest_pattern = Regex::new(r"sha256:[0-9a-fA-F]{64}")
        .map_err(|error| ResolveError::Parse(error.to_string()))?;
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| ResolveError::InvalidRepo(repo.to_string()))?;
    let mut seen = HashSet::new();
    let mut assets = Vec::new();

    for capture in link_pattern.captures_iter(html) {
        let href = capture[1].replace("&amp;", "&");
        let absolute = if href.starts_with("https://") {
            href
        } else {
            format!("https://github.com{href}")
        };
        let parsed = parsed_https_url(&absolute)?;
        if !parsed
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
        {
            continue;
        }
        let segments: Vec<&str> = parsed
            .path_segments()
            .ok_or_else(|| ResolveError::Parse("release asset URL has no path".into()))?
            .collect();
        if segments.len() != 6
            || !segments[0].eq_ignore_ascii_case(owner)
            || !segments[1].eq_ignore_ascii_case(name)
            || segments[2] != "releases"
            || segments[3] != "download"
            || !decoded_segment(segments[4])?.eq_ignore_ascii_case(tag)
        {
            continue;
        }
        let asset_name = decoded_segment(segments[5])?;
        if asset_name.is_empty() || !seen.insert(asset_name.to_ascii_lowercase()) {
            continue;
        }
        let match_start = capture.get(0).map_or(0, |matched| matched.start());
        let block_start = html[..match_start].rfind("<li").unwrap_or(match_start);
        let block_end = html[match_start..]
            .find("</li>")
            .map_or(html.len(), |offset| match_start + offset);
        let sha256 = digest_pattern
            .find(&html[block_start..block_end])
            .and_then(|matched| parse_sha256(matched.as_str()));
        assets.push(Asset {
            name: asset_name,
            url: absolute,
            size: AssetSize { bytes: 0, sha256 },
        });
    }
    Ok(assets)
}

fn fetch_expanded_release(
    http: &dyn Http,
    repo: &str,
    tag: &str,
    fresh: bool,
) -> Result<Release, ResolveError> {
    validate_tag(tag)?;
    let encoded = utf8_percent_encode(tag, NON_ALPHANUMERIC);
    let url = format!("https://github.com/{repo}/releases/expanded_assets/{encoded}");
    let html = if fresh {
        http.get_text_fresh(&url)?
    } else {
        http.get_text(&url)?
    };
    let assets = parse_expanded_assets(repo, tag, &html)?;
    Ok(Release {
        tag: tag.to_string(),
        assets,
        draft: false,
        prerelease: false,
    })
}

fn fetch_latest_release_with_freshness(
    http: &dyn Http,
    repo: &str,
    fresh: bool,
) -> Result<Release, ResolveError> {
    let repo = canonical_repo(repo)?;
    let latest = format!("https://github.com/{repo}/releases/latest");
    let response = if fresh {
        http.get_text_with_url_fresh(&latest)
    } else {
        http.get_text_with_url(&latest)
    };
    match response {
        Ok(response) => {
            let tag = release_tag_from_response(&repo, &response).ok_or_else(|| {
                ResolveError::Parse(format!(
                    "GitHub's latest release page did not identify a tag for {repo}"
                ))
            })?;
            fetch_expanded_release(http, &repo, &tag, fresh)
        }
        Err(primary_error) => {
            let no_stable_release = matches!(primary_error, ResolveError::HttpStatus(404));
            match fetch_releases(http, &repo, 1) {
                Ok(mut releases) => {
                    let mut release = releases.pop().ok_or_else(|| {
                        ResolveError::NoRelease(format!("no releases for {repo}"))
                    })?;
                    release.prerelease = no_stable_release;
                    Ok(release)
                }
                Err(_) => Err(primary_error),
            }
        }
    }
}

pub fn fetch_latest_release(http: &dyn Http, repo: &str) -> Result<Release, ResolveError> {
    fetch_latest_release_with_freshness(http, repo, false)
}

pub fn fetch_latest_release_fresh(http: &dyn Http, repo: &str) -> Result<Release, ResolveError> {
    fetch_latest_release_with_freshness(http, repo, true)
}

pub fn fetch_release_by_tag(
    http: &dyn Http,
    repo: &str,
    tag: &str,
) -> Result<Release, ResolveError> {
    let repo = canonical_repo(repo)?;
    fetch_expanded_release(http, &repo, tag, false)
}

pub fn resolved_asset(
    http: &dyn Http,
    rel: &Release,
    asset: &Asset,
) -> Result<ResolvedDownload, ResolveError> {
    parsed_https_url(&asset.url)?;
    let mut size = asset.size;
    if size.bytes == 0 {
        let metadata = http.head(&asset.url)?;
        parsed_https_url(&metadata.final_url)?;
        size.bytes = metadata.content_length.ok_or_else(|| {
            ResolveError::NoAsset(format!(
                "GitHub did not report a byte length for release asset {}",
                asset.name
            ))
        })?;
    }
    if size.bytes == 0 || size.bytes > MAX_DOWNLOAD {
        return Err(ResolveError::NoAsset(format!(
            "release asset {} has an invalid download size",
            asset.name
        )));
    }
    Ok(ResolvedDownload {
        url: asset.url.clone(),
        asset_name: asset.name.clone(),
        version: rel.tag.clone(),
        size,
    })
}

pub fn hydrate_release_assets(
    http: &dyn Http,
    mut release: Release,
) -> Result<Release, ResolveError> {
    let metadata_release = Release {
        tag: release.tag.clone(),
        assets: Vec::new(),
        draft: release.draft,
        prerelease: release.prerelease,
    };
    for asset in &mut release.assets {
        asset.size = resolved_asset(http, &metadata_release, asset)?.size;
    }
    Ok(release)
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
    resolved_asset(http, &rel, asset)
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
    resolved_asset(http, &rel, asset)
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

/// List a repo's recent release tags from its API-free Atom feed.
pub fn fetch_release_tags(
    http: &dyn Http,
    repo: &str,
    per_page: u32,
) -> Result<Vec<String>, ResolveError> {
    if !(1..=100).contains(&per_page) {
        return Err(ResolveError::Parse(
            "per_page must be between 1 and 100".into(),
        ));
    }
    let repo = canonical_repo(repo)?;
    let feed_url = format!("https://github.com/{repo}/releases.atom");
    let feed = http.get_text(&feed_url)?;
    let href_pattern =
        Regex::new(r#"href="([^"]+)""#).map_err(|error| ResolveError::Parse(error.to_string()))?;
    let mut seen = HashSet::new();
    let mut tags = Vec::new();
    for capture in href_pattern.captures_iter(&feed) {
        if let Some(tag) = release_tag_from_url(&repo, &capture[1]) {
            if seen.insert(tag.to_ascii_lowercase()) {
                tags.push(tag);
                if tags.len() == per_page as usize {
                    break;
                }
            }
        }
    }
    if tags.is_empty() {
        return Err(ResolveError::NoRelease(format!(
            "GitHub's release feed contains no releases for {repo}"
        )));
    }
    Ok(tags)
}

/// List a repo's recent releases from its API-free Atom feed.
pub fn fetch_releases(
    http: &dyn Http,
    repo: &str,
    per_page: u32,
) -> Result<Vec<Release>, ResolveError> {
    let repo = canonical_repo(repo)?;
    fetch_release_tags(http, &repo, per_page)?
        .into_iter()
        .map(|tag| fetch_expanded_release(http, &repo, &tag, false))
        .collect()
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

        let archive_rules = AssetRules {
            per_arch: std::collections::HashMap::from([(
                "x86".into(),
                crate::catalog::AssetArchRule {
                    pat: r"(?i)^package\.zip$".into(),
                    prefer: Some("zip".into()),
                },
            )]),
            dll_name: Some("Mod.dll".into()),
            bundles_loader: false,
        };
        assert_eq!(
            pick_asset(
                &release(vec![asset("package.zip"), asset("source.zip")]),
                &archive_rules,
                "x86"
            )
            .unwrap()
            .name,
            "package.zip"
        );
    }

    #[test]
    fn fresh_latest_release_bypasses_cached_discovery_and_assets() {
        struct FreshHttp(RefCell<Vec<&'static str>>);

        impl Http for FreshHttp {
            fn get_text(&self, _url: &str) -> Result<String, ResolveError> {
                self.0.borrow_mut().push("cached assets");
                Ok(r#"<a href="/A/Repo/releases/download/1.6.3-beta2/mod.dll">mod.dll</a>"#.into())
            }

            fn get_text_fresh(&self, _url: &str) -> Result<String, ResolveError> {
                self.0.borrow_mut().push("fresh assets");
                Ok(r#"<a href="/A/Repo/releases/download/1.7.0/mod.dll">mod.dll</a>"#.into())
            }

            fn get_text_with_url(&self, _url: &str) -> Result<TextResponse, ResolveError> {
                self.0.borrow_mut().push("cached latest");
                Ok(TextResponse {
                    body: String::new(),
                    final_url: "https://github.com/A/Repo/releases/tag/1.6.3-beta2".into(),
                })
            }

            fn get_text_with_url_fresh(&self, _url: &str) -> Result<TextResponse, ResolveError> {
                self.0.borrow_mut().push("fresh latest");
                Ok(TextResponse {
                    body: String::new(),
                    final_url: "https://github.com/A/Repo/releases/tag/1.7.0".into(),
                })
            }

            fn get_bytes(&self, _url: &str) -> Result<Vec<u8>, ResolveError> {
                unreachable!()
            }
        }

        let http = FreshHttp(RefCell::new(Vec::new()));
        assert_eq!(
            fetch_latest_release(&http, "A/Repo").unwrap().tag,
            "1.6.3-beta2"
        );
        assert_eq!(
            fetch_latest_release_fresh(&http, "A/Repo").unwrap().tag,
            "1.7.0"
        );
        assert_eq!(
            http.0.into_inner(),
            [
                "cached latest",
                "cached assets",
                "fresh latest",
                "fresh assets"
            ]
        );
    }

    #[test]
    fn latest_uses_atom_fallback_and_preserves_primary_errors() {
        for status in [429, 500] {
            let http = RecordingHttp {
                status,
                body: String::new(),
                urls: RefCell::new(Vec::new()),
            };
            assert!(
                matches!(fetch_latest_release(&http, "A/Repo"), Err(ResolveError::HttpStatus(code)) if code == status)
            );
            assert_eq!(http.urls.borrow().len(), 2);
        }

        struct Fallback(RefCell<Vec<String>>);
        impl Http for Fallback {
            fn get_text(&self, url: &str) -> Result<String, ResolveError> {
                self.0.borrow_mut().push(url.into());
                if url.ends_with("/latest") {
                    Err(ResolveError::HttpStatus(404))
                } else if url.ends_with("releases.atom") {
                    Ok(r#"<feed><entry><link href="https://github.com/A/Repo/releases/tag/beta"/></entry></feed>"#.into())
                } else {
                    Ok(String::new())
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
        assert!(http.urls.borrow()[0].ends_with("/expanded_assets/a%2Fb"));
    }

    #[test]
    fn verified_download_rejects_length_and_digest_mismatches() {
        let digest: [u8; 32] = Sha256::digest(b"good").into();
        let digest = format_sha256(&digest);
        let json = format!(
            r#"{{"tag_name":"1","assets":[{{"name":"a.dll","browser_download_url":"https://x/a","size":4,"digest":"{digest}"}}]}}"#
        );
        let release = parse_release(&json).unwrap();
        let resolved = resolved_asset(
            &MockHttp {
                body: String::new(),
            },
            &release,
            &release.assets[0],
        )
        .unwrap();

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
        let mut streamed = Vec::new();
        let mut progress = Vec::new();
        download_resolved_to_writer(&good, &resolved, &mut streamed, &mut |received, total| {
            progress.push((received, total))
        })
        .unwrap();
        assert_eq!(streamed, b"good");
        assert_eq!(progress, vec![(4, Some(4))]);
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
    fn resolves_latest_from_redirect_and_expanded_assets_without_api() {
        struct WebHttp {
            urls: RefCell<Vec<String>>,
        }
        impl Http for WebHttp {
            fn get_text(&self, url: &str) -> Result<String, ResolveError> {
                self.urls.borrow_mut().push(url.into());
                Ok(r#"
                    <li>
                      <a href="/AU-Avengers/TOU-Mira/releases/download/1.6.3/TownOfUsMira.dll">
                        TownOfUsMira.dll
                      </a>
                      <span>sha256:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef</span>
                    </li>
                "#.into())
            }

            fn get_text_with_url(&self, url: &str) -> Result<TextResponse, ResolveError> {
                self.urls.borrow_mut().push(url.into());
                Ok(TextResponse {
                    body: String::new(),
                    final_url: "https://github.com/AU-Avengers/TOU-Mira/releases/tag/1.6.3".into(),
                })
            }

            fn head(&self, url: &str) -> Result<HeadResponse, ResolveError> {
                self.urls.borrow_mut().push(url.into());
                Ok(HeadResponse {
                    final_url: url.into(),
                    content_length: Some(50),
                })
            }

            fn get_bytes(&self, _: &str) -> Result<Vec<u8>, ResolveError> {
                unreachable!()
            }
        }

        let cat = parse(CATALOG).unwrap();
        let rules = &cat.get("AU-Avengers/TOU-Mira").unwrap().asset_rules;
        let http = WebHttp {
            urls: RefCell::new(Vec::new()),
        };
        let result = resolve_latest(&http, "AU-Avengers/TOU-Mira", rules, "x86").unwrap();
        assert_eq!(result.version, "1.6.3");
        assert_eq!(result.asset_name, "TownOfUsMira.dll");
        assert_eq!(result.size.bytes(), 50);
        assert!(result.size.sha256.is_some());
        assert!(http
            .urls
            .borrow()
            .iter()
            .all(|url| !url.contains("api.github.com")));
    }
}
