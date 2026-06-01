# Perfect-Sync Phase 1: Tauri Shell + Core Logic Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wrap the existing Aurora Glass React UI in a Tauri 2 desktop shell and build the pure-logic Rust core (PERFECT- codec, version comparison, catalog parsing, dependency resolution, apply-diff), then wire it so the lobby-code modal decodes *real* codes.

**Architecture:** A Cargo workspace with two crates: `core` (pure logic, zero I/O, exhaustively unit-tested) and `src-tauri` (the Tauri binary that exposes `core` to the frontend via commands). The React app keeps running in the browser via `pnpm dev` (mock fallback) and natively via `pnpm tauri dev` (real commands). No filesystem/network/process work in this phase; that is Phases 2-4.

**Tech Stack:** Tauri 2, Rust (serde, serde_json, flate2, base64, crc32fast, regex, thiserror), React 19 + TypeScript + Vite (already in place), `@tauri-apps/api`.

---

## Phase decomposition (full project)

This plan is **Phase 1 of 4**. Each phase is its own plan and produces working, testable software:

- **Phase 1 (this plan):** Tauri shell + pure-logic core (codec, versioning, catalog, deps, diff) wired to the lobby modal.
- **Phase 2 (future plan):** `GameLocator` (Steam/Epic/itch/MS Store detection -> arch), `LoaderManager` (Doorstop + BepInEx bootstrap of correct arch), `GameProcess` (running-detection + env-var launch). Deliverable: launch a manually-populated profile.
- **Phase 3 (future plan):** `ModSourceResolver` (GitHub Releases fetch + asset selection + download), `ProfileManager` (build a profile's on-disk `BepInEx/plugins`, enable/disable/version pin). Deliverable: add a mod and have it physically installed.
- **Phase 4 (future plan):** Replace all remaining frontend mock data with real commands; first-run setup flow; Settings (GitHub PAT, reset game settings); packaging + code signing.

Do **not** start Phases 2-4 from this document. Write a dedicated plan for each when reached.

---

## File Structure (Phase 1)

```
Cargo.toml                      # NEW: workspace root (members: core, src-tauri)
core/
  Cargo.toml                    # NEW: pure-logic crate manifest
  src/
    lib.rs                      # NEW: module declarations + re-exports
    types.rs                    # NEW: domain types (LobbyManifest, enums) + serde
    version.rs                  # NEW: parse + compare versions (semver/date/be.NNN)
    codec.rs                    # NEW: PERFECT- encode/decode (gzip + base64url + crc)
    diff.rs                     # NEW: apply-diff (manifest vs installed)
    catalog.rs                  # NEW: catalog parse + asset-rule selection
    deps.rs                     # NEW: dependency expansion/order + conflict detection
    preview.rs                  # NEW: join codec+diff+catalog into UI-ready preview
  fixtures/
    catalog.sample.json         # NEW: small catalog fixture used by tests + the command
src-tauri/
  Cargo.toml                    # NEW (via tauri init), then add core dependency
  tauri.conf.json               # NEW (via tauri init)
  build.rs                      # NEW (via tauri init)
  capabilities/default.json     # NEW (via tauri init)
  src/
    main.rs                     # NEW (via tauri init)
    lib.rs                      # MODIFY: register commands
    commands.rs                 # NEW: Tauri command wrappers around core
src/
  lib/bridge.ts                 # NEW: frontend bridge (Tauri invoke OR browser mock)
  components/LobbyCodeModal.tsx # MODIFY: call bridge.previewCode instead of faking
package.json                    # MODIFY: add @tauri-apps/api + @tauri-apps/cli + tauri scripts
```

---

## Prerequisites (one-time, not committed)

The implementing engineer must have:
- **Rust** via rustup (stable toolchain): https://rustup.rs
- **Tauri 2 Windows prerequisites:** Microsoft C++ Build Tools (MSVC) and WebView2 runtime (preinstalled on Windows 11). See https://tauri.app/start/prerequisites/

Verify before starting:

Run: `rustc --version` and `cargo --version`
Expected: both print versions (e.g. `rustc 1.8x.x`).

---

### Task 0: Add Tauri 2 to the existing frontend

**Files:**
- Create: `src-tauri/` (generated), `Cargo.toml` (workspace root)
- Modify: `package.json`

- [ ] **Step 1: Add the Tauri npm packages**

Run:
```bash
pnpm add -D @tauri-apps/cli
pnpm add @tauri-apps/api
```
Expected: both appear in `package.json`.

- [ ] **Step 2: Initialize Tauri non-interactively**

Run (single line):
```bash
pnpm tauri init --app-name "Perfect-Sync" --window-title "Perfect-Sync" --frontend-dist ../dist --dev-url http://localhost:1420 --before-dev-command "pnpm dev" --before-build-command "pnpm build"
```
Expected: a `src-tauri/` directory is created containing `Cargo.toml`, `tauri.conf.json`, `build.rs`, `src/main.rs`, `src/lib.rs`, `capabilities/default.json`, and `icons/`.

- [ ] **Step 3: Add tauri scripts to package.json**

In `package.json`, add to `"scripts"`:
```json
"tauri": "tauri",
"tauri:dev": "tauri dev",
"tauri:build": "tauri build"
```

- [ ] **Step 4: Create the Cargo workspace root**

Create `Cargo.toml` (repo root):
```toml
[workspace]
members = ["core", "src-tauri"]
resolver = "2"

[profile.release]
strip = true
lto = true
```

- [ ] **Step 5: Ignore Rust build output**

Append to `.gitignore`:
```
/target
```

- [ ] **Step 6: Verify the shell launches**

Run: `pnpm tauri dev`
Expected: a native window opens showing the existing Aurora Glass UI (the Vite dev server is started automatically by the `beforeDevCommand`). Close the window to stop.

- [ ] **Step 7: Commit**

```bash
git add package.json pnpm-lock.yaml Cargo.toml .gitignore src-tauri
git commit -m "Add Tauri 2 desktop shell around the frontend"
```

---

### Task 1: Core crate skeleton + domain types

**Files:**
- Create: `core/Cargo.toml`, `core/src/lib.rs`, `core/src/types.rs`

- [ ] **Step 1: Create the core crate manifest**

Create `core/Cargo.toml`:
```toml
[package]
name = "perfect-sync-core"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
flate2 = "1"
base64 = "0.22"
crc32fast = "1"
regex = "1"
thiserror = "2"
```

- [ ] **Step 2: Declare modules**

Create `core/src/lib.rs`:
```rust
pub mod catalog;
pub mod codec;
pub mod deps;
pub mod diff;
pub mod preview;
pub mod types;
pub mod version;
```

- [ ] **Step 3: Write the domain types**

Create `core/src/types.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    X86,
    X64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Store {
    Steam,
    Epic,
    Itch,
    Msstore,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModTag {
    Role,
    AllClient,
    HostOnly,
    Map,
    Cosmetic,
    Library,
    Loader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModSource {
    Catalog,
    Github,
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Platform {
    pub store: Store,
    pub arch: Arch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestMod {
    pub id: String,
    pub v: String,
    pub src: ModSource,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub r#ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoaderPins {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bepinex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reactor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LobbyManifest {
    pub v: u8,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub platform: Option<Platform>,
    #[serde(rename = "gameBuild", skip_serializing_if = "Option::is_none", default)]
    pub game_build: Option<String>,
    pub mods: Vec<ManifestMod>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub loader: Option<LoaderPins>,
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p perfect-sync-core`
Expected: compiles (warnings about unused modules are fine until later tasks fill them). If empty module files do not yet exist, create them empty so `lib.rs` compiles: `catalog.rs`, `codec.rs`, `deps.rs`, `diff.rs`, `preview.rs`, `version.rs` each containing a single comment line `// implemented in a later task`.

- [ ] **Step 5: Commit**

```bash
git add core Cargo.toml
git commit -m "Add core crate skeleton and domain types"
```

---

### Task 2: Version parsing and comparison

Handles the three real schemes from the spec: semver (`1.6.3`), date-based (`2025.11.20`), and BepInEx (`6.0.0-be.735`). Used for update detection; the handshake itself uses exact string equality elsewhere.

**Files:**
- Create/replace: `core/src/version.rs`

- [ ] **Step 1: Write the failing tests**

Replace `core/src/version.rs` with:
```rust
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    parts: Vec<u64>,
    be: Option<u64>,
}

pub fn parse(s: &str) -> Version {
    let (main, be) = match s.split_once("-be.") {
        Some((m, b)) => (m, b.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()),
        None => (s, None),
    };
    let main = main.trim_start_matches('v');
    let parts = main
        .split(['.', '-'])
        .filter_map(|p| {
            let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().ok()
        })
        .collect();
    Version { parts, be }
}

pub fn cmp(a: &str, b: &str) -> Ordering {
    let (va, vb) = (parse(a), parse(b));
    va.parts.cmp(&vb.parts).then(va.be.cmp(&vb.be))
}

/// True if `candidate` is a strictly newer release than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    cmp(candidate, current) == Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_ordering() {
        assert!(is_newer("1.6.3", "1.6.2"));
        assert!(!is_newer("1.6.2", "1.6.3"));
        assert_eq!(cmp("1.6.2", "1.6.2"), Ordering::Equal);
    }

    #[test]
    fn strips_v_prefix() {
        assert_eq!(cmp("v4.8.0", "4.8.0"), Ordering::Equal);
        assert!(is_newer("v4.8.0", "v4.7.2"));
    }

    #[test]
    fn date_based_ordering() {
        // 2025.11.20 is newer than 2025.9.4 (numeric, not lexical)
        assert!(is_newer("2025.11.20", "2025.9.4"));
    }

    #[test]
    fn bepinex_be_builds() {
        assert!(is_newer("6.0.0-be.735", "6.0.0-be.697"));
        assert_eq!(cmp("6.0.0-be.735", "6.0.0-be.735"), Ordering::Equal);
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p perfect-sync-core version::`
Expected: 4 tests PASS. (The implementation is written alongside the tests here because version logic is trivial and the tests are the specification; if a test fails, fix `parse`/`cmp` until green.)

- [ ] **Step 3: Commit**

```bash
git add core/src/version.rs
git commit -m "Add version parsing and comparison (semver/date/be)"
```

---

### Task 3: PERFECT- codec

Format: `PERFECT-` + base64url(no-pad)( gzip( JSON(LobbyManifest) ) ) + `.` + 4-hex CRC of the body. Fully offline.

**Files:**
- Create/replace: `core/src/codec.rs`

- [ ] **Step 1: Write the failing test**

Replace `core/src/codec.rs` with:
```rust
use crate::types::LobbyManifest;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Read, Write};

const PREFIX: &str = "PERFECT-";

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CodecError {
    #[error("missing PERFECT- prefix")]
    BadPrefix,
    #[error("malformed code")]
    Malformed,
    #[error("checksum mismatch")]
    BadChecksum,
}

pub fn encode(m: &LobbyManifest) -> String {
    let json = serde_json::to_vec(m).expect("manifest serializes");
    let mut enc = GzEncoder::new(Vec::new(), Compression::best());
    enc.write_all(&json).expect("gzip write");
    let gz = enc.finish().expect("gzip finish");
    let body = URL_SAFE_NO_PAD.encode(gz);
    let crc = crc32fast::hash(body.as_bytes()) & 0xffff;
    format!("{PREFIX}{body}.{crc:04x}")
}

pub fn decode(code: &str) -> Result<LobbyManifest, CodecError> {
    let rest = code.strip_prefix(PREFIX).ok_or(CodecError::BadPrefix)?;
    let (body, crc_str) = rest.rsplit_once('.').ok_or(CodecError::Malformed)?;
    let want = u32::from_str_radix(crc_str, 16).map_err(|_| CodecError::Malformed)?;
    if crc32fast::hash(body.as_bytes()) & 0xffff != want {
        return Err(CodecError::BadChecksum);
    }
    let gz = URL_SAFE_NO_PAD
        .decode(body.as_bytes())
        .map_err(|_| CodecError::Malformed)?;
    let mut s = String::new();
    GzDecoder::new(&gz[..])
        .read_to_string(&mut s)
        .map_err(|_| CodecError::Malformed)?;
    serde_json::from_str(&s).map_err(|_| CodecError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ManifestMod, ModSource};

    fn sample() -> LobbyManifest {
        LobbyManifest {
            v: 1,
            name: Some("TownOfUs Night".into()),
            platform: None,
            game_build: Some("17.0.1".into()),
            mods: vec![ManifestMod {
                id: "AU-Avengers/TOU-Mira".into(),
                v: "1.6.3".into(),
                src: ModSource::Github,
                r#ref: None,
            }],
            loader: None,
        }
    }

    #[test]
    fn round_trip() {
        let code = encode(&sample());
        assert!(code.starts_with("PERFECT-"));
        assert_eq!(decode(&code).unwrap(), sample());
    }

    #[test]
    fn rejects_bad_prefix() {
        assert_eq!(decode("NOPE-abc.0000"), Err(CodecError::BadPrefix));
    }

    #[test]
    fn rejects_tampered_body() {
        let mut code = encode(&sample());
        // flip a character in the body (before the '.')
        let dot = code.rfind('.').unwrap();
        let bytes = unsafe { code.as_bytes_mut() };
        bytes[dot - 1] = if bytes[dot - 1] == b'A' { b'B' } else { b'A' };
        assert_eq!(decode(&code), Err(CodecError::BadChecksum));
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p perfect-sync-core codec::`
Expected: 3 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add core/src/codec.rs
git commit -m "Add PERFECT- code encode/decode with checksum"
```

---

### Task 4: Apply-diff

Given a decoded manifest and the user's installed `(id, version)` pairs, classify each required mod as `install` / `change` / `ok`.

**Files:**
- Create/replace: `core/src/diff.rs`

- [ ] **Step 1: Write the failing test**

Replace `core/src/diff.rs` with:
```rust
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
    use crate::types::{ManifestMod, ModSource};

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
                    src: ModSource::Github,
                    r#ref: None,
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
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p perfect-sync-core diff::`
Expected: 1 test PASS.

- [ ] **Step 3: Commit**

```bash
git add core/src/diff.rs
git commit -m "Add apply-diff for lobby manifests"
```

---

### Task 5: Catalog parsing + asset selection

Parses the curated catalog JSON and picks the correct release asset per architecture using the real naming conventions from the research (Steam/itch x86 vs Epic/MS Store x64).

**Files:**
- Create: `core/fixtures/catalog.sample.json`
- Create/replace: `core/src/catalog.rs`

- [ ] **Step 1: Create the catalog fixture**

Create `core/fixtures/catalog.sample.json`:
```json
{
  "schema": 1,
  "updated": "2026-06-02",
  "mods": [
    {
      "id": "AU-Avengers/TOU-Mira",
      "name": "Town of Us - Mira",
      "summary": "The Mira-API rebuild of Town of Us.",
      "repo": "AU-Avengers/TOU-Mira",
      "tags": ["role", "all-client"],
      "dependencies": ["All-Of-Us-Mods/MiraAPI", "NuclearPowered/Reactor"],
      "assetRules": {
        "perArch": {
          "x86": { "match": "(?i)x86.*(steam|itch)", "prefer": "zip" },
          "x64": { "match": "(?i)x64.*(epic|msstore)", "prefer": "zip" }
        },
        "dllName": "TownOfUsMira.dll",
        "bundlesLoader": true
      }
    },
    {
      "id": "All-Of-Us-Mods/MiraAPI",
      "name": "MiraAPI",
      "summary": "Shared API layer.",
      "repo": "All-Of-Us-Mods/MiraAPI",
      "tags": ["library"],
      "dependencies": ["NuclearPowered/Reactor"],
      "assetRules": { "perArch": {}, "dllName": "MiraAPI.dll" }
    },
    {
      "id": "NuclearPowered/Reactor",
      "name": "Reactor",
      "summary": "The modding API and handshake.",
      "repo": "NuclearPowered/Reactor",
      "tags": ["library"],
      "dependencies": [],
      "assetRules": { "perArch": {}, "dllName": "Reactor.dll" }
    },
    {
      "id": "EnhancedNetwork/TownofHost-Enhanced",
      "name": "Town of Host - Enhanced",
      "summary": "Host-only chaos. No Reactor (fat DLL).",
      "repo": "EnhancedNetwork/TownofHost-Enhanced",
      "tags": ["role", "host-only"],
      "dependencies": [],
      "assetRules": {
        "perArch": {
          "x86": { "match": "(?i)SteamItchio" },
          "x64": { "match": "(?i)EpicMsStore" }
        },
        "dllName": "TOHE.dll"
      }
    }
  ]
}
```

- [ ] **Step 2: Write the failing tests**

Replace `core/src/catalog.rs` with:
```rust
use crate::types::ModTag;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct AssetArchRule {
    #[serde(rename = "match")]
    pub pat: String,
    pub prefer: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetRules {
    #[serde(rename = "perArch", default)]
    pub per_arch: HashMap<String, AssetArchRule>,
    #[serde(rename = "dllName", default)]
    pub dll_name: Option<String>,
    #[serde(rename = "bundlesLoader", default)]
    pub bundles_loader: bool,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub schema: u32,
    pub mods: Vec<CatalogEntry>,
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
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p perfect-sync-core catalog::`
Expected: 2 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add core/src/catalog.rs core/fixtures/catalog.sample.json
git commit -m "Add catalog parsing and per-arch asset selection"
```

---

### Task 6: Dependency expansion + conflict detection

Expands the transitive dependency graph (deps before dependents, so Reactor installs first), and flags the "more than one role mod" conflict. TOHE needs no special-casing: its catalog entry simply lists no Reactor dependency.

**Files:**
- Create/replace: `core/src/deps.rs`

- [ ] **Step 1: Write the failing tests**

Replace `core/src/deps.rs` with:
```rust
use crate::catalog::Catalog;
use crate::types::ModTag;

#[derive(Debug, PartialEq, Eq)]
pub struct Resolved {
    /// install order: each dependency appears before the mod that needs it
    pub ordered: Vec<String>,
    /// ids of role mods when more than one is selected (cannot coexist)
    pub conflicts: Vec<String>,
}

fn visit(cat: &Catalog, id: &str, out: &mut Vec<String>) {
    if out.iter().any(|x| x == id) {
        return;
    }
    if let Some(entry) = cat.get(id) {
        for dep in &entry.dependencies {
            visit(cat, dep, out);
        }
    }
    out.push(id.to_string());
}

pub fn resolve(cat: &Catalog, selected: &[String]) -> Resolved {
    let mut ordered = Vec::new();
    for id in selected {
        visit(cat, id, &mut ordered);
    }
    let role_mods: Vec<String> = selected
        .iter()
        .filter(|id| {
            cat.get(id)
                .map(|e| e.tags.contains(&ModTag::Role))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    let conflicts = if role_mods.len() > 1 { role_mods } else { Vec::new() };
    Resolved { ordered, conflicts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::parse;

    const SAMPLE: &str = include_str!("../fixtures/catalog.sample.json");

    fn idx(v: &[String], id: &str) -> usize {
        v.iter().position(|x| x == id).expect("present")
    }

    #[test]
    fn expands_deps_before_dependent() {
        let cat = parse(SAMPLE).unwrap();
        let r = resolve(&cat, &["AU-Avengers/TOU-Mira".to_string()]);
        // Reactor before MiraAPI before TOU-Mira
        assert!(idx(&r.ordered, "NuclearPowered/Reactor") < idx(&r.ordered, "All-Of-Us-Mods/MiraAPI"));
        assert!(idx(&r.ordered, "All-Of-Us-Mods/MiraAPI") < idx(&r.ordered, "AU-Avengers/TOU-Mira"));
        assert!(r.conflicts.is_empty());
    }

    #[test]
    fn tohe_pulls_no_reactor() {
        let cat = parse(SAMPLE).unwrap();
        let r = resolve(&cat, &["EnhancedNetwork/TownofHost-Enhanced".to_string()]);
        assert!(!r.ordered.iter().any(|x| x == "NuclearPowered/Reactor"));
    }

    #[test]
    fn flags_two_role_mods() {
        let cat = parse(SAMPLE).unwrap();
        let r = resolve(
            &cat,
            &[
                "AU-Avengers/TOU-Mira".to_string(),
                "EnhancedNetwork/TownofHost-Enhanced".to_string(),
            ],
        );
        assert_eq!(r.conflicts.len(), 2);
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p perfect-sync-core deps::`
Expected: 3 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add core/src/deps.rs
git commit -m "Add dependency resolution and role-mod conflict detection"
```

---

### Task 7: Preview (UI-ready join of codec + diff + catalog)

Produces exactly what the lobby modal needs: a display name and rows with name/repo/tags/action/from/to/detail. This is the single function the Tauri command calls.

**Files:**
- Create/replace: `core/src/preview.rs`

- [ ] **Step 1: Write the failing test**

Replace `core/src/preview.rs` with:
```rust
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
    use crate::types::{LobbyManifest, ManifestMod, ModSource};

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
                src: ModSource::Github,
                r#ref: None,
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
```

- [ ] **Step 2: Run the full core test suite**

Run: `cargo test -p perfect-sync-core`
Expected: all tests across all modules PASS.

- [ ] **Step 3: Commit**

```bash
git add core/src/preview.rs
git commit -m "Add UI-ready lobby preview joining codec, diff, and catalog"
```

---

### Task 8: Expose core through Tauri commands

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Create: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add the core dependency**

In `src-tauri/Cargo.toml`, under `[dependencies]`, add:
```toml
perfect-sync-core = { path = "../core" }
serde = { version = "1", features = ["derive"] }
```
(`serde` and `serde_json` are already present from `tauri init`; do not duplicate keys.)

- [ ] **Step 2: Write the command wrappers**

Create `src-tauri/src/commands.rs`:
```rust
use perfect_sync_core::catalog::{parse, Catalog};
use perfect_sync_core::preview::{preview, Preview};

const CATALOG_JSON: &str = include_str!("../../core/fixtures/catalog.sample.json");

fn catalog() -> Catalog {
    parse(CATALOG_JSON).expect("bundled catalog parses")
}

/// Decode a PERFECT- code into a UI preview (diff vs the installed set).
#[tauri::command]
pub fn preview_code(code: String, installed: Vec<(String, String)>) -> Result<Preview, String> {
    preview(&code, &catalog(), &installed).map_err(|e| e.to_string())
}
```

- [ ] **Step 3: Register the command**

In `src-tauri/src/lib.rs`, add `mod commands;` near the top, and add the handler to the builder. The generated `run()` contains a `tauri::Builder::default()...`; add `.invoke_handler(tauri::generate_handler![commands::preview_code])` before `.run(...)`. Example resulting builder:
```rust
mod commands;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![commands::preview_code])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```
(Keep any plugins `tauri init` already added; only add the `invoke_handler` line and `mod commands;`.)

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p perfect-sync`
Expected: compiles. (`perfect-sync` is the src-tauri package name from `tauri init`; if different, use the name in `src-tauri/Cargo.toml`.)

- [ ] **Step 5: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/commands.rs src-tauri/src/lib.rs Cargo.lock
git commit -m "Expose preview_code Tauri command backed by core"
```

---

### Task 9: Wire the lobby modal to the real codec

The modal currently fakes decoding with a timeout. Add a bridge that calls the Tauri command when running natively and falls back to the existing mock in the browser, so both `pnpm dev` and `pnpm tauri dev` work.

**Files:**
- Create: `src/lib/bridge.ts`
- Modify: `src/components/LobbyCodeModal.tsx`

- [ ] **Step 1: Create the bridge**

Create `src/lib/bridge.ts`:
```ts
import type { DiffItem } from "./types";
import { SAMPLE_DIFF } from "../data/mock";

export interface Preview {
  name: string;
  items: DiffItem[];
}

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Decode a PERFECT- code into an apply-preview. Real codec under Tauri; mock in the browser. */
export async function previewCode(code: string, installed: [string, string][]): Promise<Preview> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<Preview>("preview_code", { code, installed });
  }
  // browser fallback so `pnpm dev` still demos the flow
  await new Promise((r) => setTimeout(r, 500));
  return { name: "Lobby - TownOfUs Night", items: SAMPLE_DIFF };
}
```

- [ ] **Step 2: Use the bridge in the modal**

In `src/components/LobbyCodeModal.tsx`:
1. Add import: `import { previewCode } from "../lib/bridge";`
2. Add state for fetched rows: `const [rows, setRows] = useState<DiffItem[]>(diff);` (keep the `diff` prop as the browser default).
3. Replace the two `setTimeout(() => setMode("diff"), …)` calls (in the open-effect and in `decode`) with a real call:
```tsx
const runDecode = (value: string) => {
  setMode("decoding");
  previewCode(value, [
    ["AU-Avengers/TOU-Mira", "1.6.2"],
    ["Dolfannn/LevelImposter", "0.7.2"],
  ])
    .then((p) => {
      setRows(p.items);
      setMode("diff");
    })
    .catch(() => setMode("diff"));
};
```
Call `runDecode(initialCode)` in the open-effect when a code is present, and `runDecode(code)` from the `decode()` handler. Pass `rows` (not the `diff` prop) into `ResultStep`.

- [ ] **Step 3: Typecheck and build**

Run: `pnpm typecheck && pnpm build`
Expected: both succeed.

- [ ] **Step 4: Manual verification (native)**

Run: `pnpm tauri dev`. In the app, open the lobby modal (sidebar card), paste a code generated by the core round-trip test, or any valid `PERFECT-` code, and Decode.
Expected: the diff renders from the **real** Rust decoder. An invalid code (e.g. `PERFECT-bad.0000`) leaves the flow gracefully (catch -> diff with browser default or empty); confirm no crash.

- [ ] **Step 5: Commit**

```bash
git add src/lib/bridge.ts src/components/LobbyCodeModal.tsx
git commit -m "Wire lobby modal to the real PERFECT- codec via Tauri bridge"
```

---

## Self-Review

**1. Spec coverage (Phase 1 scope):**
- PERFECT- format (`PERFECT-` + base64url(gzip(JSON)) + CRC, offline) -> Task 3. ✓
- Apply-diff (install/change/ok) -> Task 4, surfaced in Task 7/9. ✓
- Curated catalog JSON + per-arch asset rules (Steam/itch x86 vs Epic/MS x64) -> Task 5. ✓
- Dependency graph (`mod -> MiraAPI -> Reactor -> BepInEx`, Reactor first) + TOHE no-Reactor exception + one-role-mod conflict -> Task 6. ✓
- Version schemes (semver/date/be) -> Task 2. ✓
- Tauri shell, clean game dir untouched (no I/O this phase) -> Task 0/8. ✓
- Frontend stays runnable in browser AND native -> Task 9 bridge. ✓
- Out of Phase 1 (correctly deferred): GameLocator, LoaderManager, ModSourceResolver downloads, ProfileManager on disk, launch — all listed under the Phase roadmap.

**2. Placeholder scan:** No "TBD"/"add error handling here" steps; every code step contains complete code; every command lists expected output. ✓

**3. Type consistency:** Rust `Action` serializes lowercase (`install`/`change`/`ok`) to match the TS `DiffItem.action` union; `ModTag` uses kebab-case to match TS (`all-client`, `host-only`); `preview_code` returns `{ name, items }` consumed as `Preview` in `bridge.ts`; `PreviewItem` fields (name/repo/tags/action/from/to/detail) match the existing TS `DiffItem`. ✓

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-06-02-perfect-sync-phase1-core-and-shell.md`. Two execution options:**

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
