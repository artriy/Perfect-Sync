# Perfect-Sync — Design Spec

**Date:** 2026-06-02
**Status:** Approved (brainstorming) — ready for implementation planning
**Type:** Desktop app (Windows) — modded Among Us mod manager + launcher

---

## 1. Problem

Installing and coordinating modded Among Us is chaotic. To play in a modded lobby, every
player must run the **same mods at the same versions** (the Reactor handshake matches mod
`Id` + `Version` *exactly*, or you get kicked). Today this is fully manual: a host posts
"this lobby uses Town of Us 1.6.3 + Submerged" in Discord, and each player hunts down each
GitHub release, drops DLLs into `BepInEx/plugins`, and hopes the versions line up. Switching
to a different lobby means redoing it all. This is hostile to newcomers and tedious for
everyone.

## 2. Goals

- **One-click mod management** — add any mod, then enable / disable / update / downgrade it.
- **Add any GitHub mod** — paste a repo URL and install it, not just a fixed list.
- **Isolated profiles** — separate mod sets per lobby/playstyle, switch instantly, game stays clean.
- **One-click lobby sync** — paste a shared code → app sets up the exact required mods + versions → launch. This is the headline feature.
- **One-click launch** — start Among Us with the active profile's mods.
- **Newcomer-friendly** — the whole flow should be approachable for a first-time modder.

## 3. Non-goals (v1)

- **No game-version management** — the app does not detect, downgrade, or change the Among Us
  build. It records the build a lobby code was made for as *reference info only*.
- **No custom server / region picker** — dropped entirely.
- **No Linux/macOS** — Windows only.
- **No cloud profile sync** — sharing is via self-contained codes.
- **No Thunderstore integration** — we maintain our own curated catalog.

(See §16 for the deferred roadmap.)

## 4. Locked decisions

| Area | Decision |
|---|---|
| Platform | Windows desktop; multi-store aware (Steam/Epic/itch solid, MS Store best-effort) |
| Game version | Not managed in v1 (recorded as info only) |
| Mod ingestion | Hybrid: our curated catalog (1-click) + paste-any-GitHub-URL + local file import |
| Catalog source | Our own curated JSON catalog (we maintain dependency data). No Thunderstore. |
| Lobby sharing | Self-contained `PERFECT-` share code; no backend |
| Profiles | Isolated, r2modman-style; game launched pointed at the active profile |
| Tech stack | **Tauri 2** (Rust core) + **React + TypeScript** frontend |
| Visual direction | **Aurora Glass** (see §13) |

## 5. Domain background (essential for implementers)

Among Us is a Unity **IL2CPP** game — there is no managed `Assembly-CSharp.dll`; game logic
lives in native `GameAssembly.dll`. Mods run via a loader stack:

1. **UnityDoorstop** — bootstrapper. A proxy DLL named **`winhttp.dll`** placed next to
   `Among Us.exe` is loaded first by Windows' DLL search order, handing control to managed
   code before Unity starts. Configured via `doorstop_config.ini` **or environment variables**.
2. **BepInEx** (IL2CPP 6.0.0 "bleeding-edge" build, the community `BepInExPack_AmongUs`) —
   the plugin framework. Ships its own CoreCLR (.NET) runtime (`dotnet/` folder). Generates
   per-game-version **interop assemblies** into `BepInEx/interop/` on first launch (slow, one-time).
3. **Reactor** — the de-facto modding API + modded handshake (`Reactor.dll` in `BepInEx/plugins`).
4. **Mod DLLs** — dropped into `BepInEx/plugins`.

**Modded install layout** (root = where `Among Us.exe` lives):

```
Among Us/
├── Among Us.exe            ← launched directly; NOT modified
├── GameAssembly.dll        ← native game logic
├── winhttp.dll             ← Doorstop proxy (the only file we add to the game dir)  ★
├── doorstop_config.ini     ← (we drive Doorstop via env vars instead at launch)
├── dotnet/                 ← bundled CoreCLR runtime
└── BepInEx/
    ├── core/               ← framework DLLs (preloader, chainloader, etc.)
    ├── plugins/            ← ★ MODS GO HERE: Reactor.dll + each mod's .dll
    ├── interop/            ← generated per-game-version (preserve assembly-hash.txt)
    ├── config/             ← per-plugin .cfg (e.g. gg.reactor.api.cfg)
    └── cache/
```

**Critical constraints (drive the design):**

- **Architecture is store-specific:** Steam/Epic/itch = **x86** BepInEx; MS Store = **x64**.
  Wrong arch silently fails to load. Store detection must pick the loader build.
- **Mods are version-locked to a game build** by construction (interop encodes exact memory
  layout). We don't manage this in v1, but it's *why* lobby codes must reproduce exact versions.
- **Two role mods cannot coexist** — they fight over the same Harmony patches / RPC space.
  Enforce **one role mod per profile**.
- **Host-only vs all-client mods** (Reactor `ModFlags`): all-client mods (Town of Us, The
  Other Roles) must match on *every* client; host-only mods (TOHE, TownOfHost) only the host
  needs. The handshake only enforces all-client mods. The UI must distinguish them.
- **Dependency graph is real and layered:** `mod → MiraAPI → Reactor → BepInEx`. Reactor must
  load first. **TOHE is an exception** — no Reactor, deps merged into one fat DLL (Costura.Fody).
- **Can't modify mods while the game runs** (file locks) — gate all writes on the game being closed.
- **GitHub release assets are unstructured** — platform encoded in zip names (e.g.
  `Submerged+dependencies.Steam.Itch.zip` vs `.Epic.MSStore.zip`; `TheOtherRoles.zip` vs
  `TheOtherRoles_MSStore.zip`). The catalog encodes per-mod asset-selection rules.
- Reactor/these mods are **not affiliated with Innersloth**; custom RPCs on official servers
  risk bans. The app does not encourage that; it just manages mods.

> **[UNCERTAIN — verify during implementation]** Exact Doorstop v4 env-var names used by the
> AU BepInEx pack (`DOORSTOP_ENABLED`, `DOORSTOP_TARGET_ASSEMBLY`, runtime path overrides);
> whether current Reactor still needs a non-Steam DLL swap for Epic/itch; MS Store
> (`WindowsApps`) write feasibility.

## 6. Architecture

Tauri app: **Rust core** owns all privileged work (filesystem, process launch, network);
**React/TS frontend** owns UI and calls the core via Tauri commands. Each module below has one
clear purpose and a typed command interface; all are independently testable.

```
┌─────────────────────────── React + TS (frontend) ───────────────────────────┐
│  Views: First-run · Home (profiles+mods) · Add-mod · Lobby-code · Settings    │
│  State: profiles, catalog, active profile, game status                        │
└──────────────────────────────── Tauri commands ─────────────────────────────┘
┌──────────────────────────────── Rust core ──────────────────────────────────┐
│  GameLocator · LoaderManager · ModSourceResolver · DependencyResolver         │
│  ProfileManager · SyncCodec · CatalogClient · GameProcess                     │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Modules:**

1. **GameLocator** — detect Among Us installs and their store → architecture.
   - Steam: parse `libraryfolders.vdf` + `appmanifest_945360.acf` (App ID **945360**).
   - Epic: parse `%ProgramData%/Epic/EpicGamesLauncher/Data/Manifests/*.item`.
   - itch: itch library / default path.
   - MS Store: detect package under `WindowsApps` (best-effort, flagged).
   - Output: `{ path, store, arch (x86|x64) }`. User can also point manually.

2. **LoaderManager** — install/repair the Doorstop + BepInEx bootstrap of the correct arch;
   wire the active profile's BepInEx at launch (see §10). Idempotent install/repair.

3. **ModSourceResolver** — turn an input into an installable `ResolvedMod` (download URL + the
   right asset). Three input kinds: catalog id, GitHub URL, local file. Applies the catalog's
   asset-selection rules (or generic heuristics + user confirm for unknown repos). Reads GitHub
   Releases API; honors an optional user PAT to dodge rate limits.

4. **DependencyResolver** — expand a mod set into the full graph (`mod → MiraAPI → Reactor →
   BepInEx`), order it (Reactor first), dedupe, and reconcile version pins. Handles the TOHE
   "no Reactor / fat DLL" exception. Flags **conflicts** (2+ role mods).

5. **ProfileManager** — CRUD profiles; per-mod enable/disable, version pin, update/downgrade;
   builds a profile's on-disk `BepInEx/plugins`. Stores profile metadata as JSON.

6. **SyncCodec** — encode a profile to a `PERFECT-` code and decode a code to a `LobbyManifest`,
   then compute the **apply-diff** vs current state (§9). Pure, fully unit-testable, no I/O.

7. **CatalogClient** — fetch + cache the curated catalog JSON (ETag/conditional GET), expose
   search/lookup. Falls back to last-cached copy offline.

8. **GameProcess** — detect whether Among Us is running (gate writes/launch); launch the game
   directly with the Doorstop env vars pointing at the active profile.

## 7. Data model

```ts
type Arch = 'x86' | 'x64';
type Store = 'steam' | 'epic' | 'itch' | 'msstore' | 'manual';
type ModTag = 'role' | 'all-client' | 'host-only' | 'map' | 'cosmetic' | 'library' | 'loader';
type ModSource = 'catalog' | 'github' | 'file';

interface CatalogEntry {            // one record in the curated catalog (§8)
  id: string;                       // stable id, e.g. "AU-Avengers/TOU-Mira"
  name: string;
  summary: string;
  repo?: string;                    // "owner/repo" for GitHub releases
  tags: ModTag[];
  dependencies: string[];           // catalog ids, e.g. ["NuclearPowered/Reactor"]
  assetRules: AssetRules;           // how to pick the right release asset per arch
  icon?: string;
  homepage?: string;
}

interface AssetRules {
  perArch: Record<Arch, { match: string /* regex over asset name */; prefer?: 'zip'|'dll' }>;
  dllName?: string;                 // expected plugin DLL inside a zip / bare asset
  bundlesLoader?: boolean;          // true if the "full" zip already contains BepInEx/Reactor
}

interface ProfileMod {
  packageId: string;                // catalog id or "owner/repo"
  version: string;                  // exact tag/version (pinned)
  enabled: boolean;
  source: ModSource;
  ref?: string;                     // explicit source URL for non-catalog mods
}

interface Profile {
  id: string;
  name: string;
  crewColor: string;                // functional accent (Aurora Glass crew palette)
  gameBuild?: string;               // reference info only (not enforced)
  mods: ProfileMod[];               // user-chosen mods; deps resolved at build time
}

interface LobbyManifest {           // payload encoded in a PERFECT- code
  v: 1;                             // manifest schema version
  name?: string;
  platform?: { store: Store; arch: Arch };
  gameBuild?: string;               // info only
  mods: { id: string; v: string; src: ModSource; ref?: string }[];
  loader?: { bepinex?: string; reactor?: string };
}

interface ApplyDiffItem {
  mod: { id: string; name: string; tags: ModTag[] };
  action: 'no-change' | 'install' | 'change-version' | 'disable';
  from?: string; to?: string;
}
```

## 8. Curated catalog format

The catalog is **versioned JSON hosted in its own public GitHub repo** (e.g.
`perfect-sync/catalog`). The app fetches it with conditional GET and caches it; the community
can contribute entries via PRs. This keeps us independent of Thunderstore while staying
low-maintenance.

```json
{
  "schema": 1,
  "updated": "2026-06-02",
  "mods": [
    {
      "id": "AU-Avengers/TOU-Mira",
      "name": "Town of Us — Mira",
      "summary": "The Mira-API rebuild of Town of Us. Many custom roles.",
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
      },
      "icon": "https://…",
      "homepage": "https://github.com/AU-Avengers/TOU-Mira"
    }
  ]
}
```

**Unknown (any-URL) mods:** ModSourceResolver fetches the repo's latest (or chosen) release,
applies generic heuristics (prefer an asset matching the detected arch; else the lone `.dll`;
else ask which asset), and shows a confirm step. The resolved choice can be saved as a **local
catalog override** so it's 1-click next time. Default dependency assumption for unknown mods:
`Reactor + BepInEx` unless the user marks it host-only/standalone.

## 9. The `PERFECT-` share code

- **Format:** `PERFECT-` + base64url( gzip( JSON(LobbyManifest) ) ), with a trailing 4-char
  CRC for quick validity check. Fully self-contained — decoding needs **no network**.
- **Encode (Copy lobby code):** serialize the active profile's enabled mods (id + exact
  version + source) plus resolved loader pins and reference `gameBuild`.
- **Decode + apply:** parse → `LobbyManifest` → compute `ApplyDiff` against the user's installed
  versions/cache → render the diff (install / change-version / already-have) → on confirm,
  build a **new profile** (default name `Lobby — <name>`) with bit-identical versions, then
  optionally launch.
- **Compatibility guarantee:** because all-client mods are pinned to exact versions, the Reactor
  handshake will pass. The UI states this explicitly.
- **Validation/UX:** malformed/old-schema codes produce a friendly error; unknown mod ids that
  aren't in the catalog fall back to their embedded `ref` (GitHub URL) so codes still resolve.

## 10. Profile isolation & launch

Mirrors the proven r2modman/Gale approach so the **real game install stays clean**:

- Each profile lives under app data, e.g.
  `%APPDATA%/Perfect-Sync/profiles/<id>/BepInEx/…` with its own `plugins`.
- The **only** thing added to the game directory is the one-time Doorstop bootstrap
  (`winhttp.dll`). All mods/BepInEx content live per-profile, outside the game dir.
- **Launch** = `GameProcess` starts `Among Us.exe` **directly** with Doorstop **environment
  variables** pointing `target_assembly` / BepInEx paths at the active profile's BepInEx. No
  file copying into the game dir to switch profiles → instant switching.
- **Trade-off:** launching the exe directly (not via `steam://`) means Steam playtime/overlay
  may not register. Acceptable for v1; note in UI.
- **Interop cache:** generated per game version; first launch of each profile regenerates it
  (slow) — surface a "first launch may take a minute" state so users don't think it hung. When
  cleaning `BepInEx/interop`, **delete only `.dll`, preserve `assembly-hash.txt`**. *(Later
  optimization: a shared interop cache keyed by game-build hash across profiles.)*

## 11. Key user flows

1. **First run / setup:** detect game (auto, or manual pick) → confirm store/arch → install
   loader bootstrap → land on Home with a starter empty profile.
2. **Add a mod:** search catalog *or* paste GitHub URL *or* import file → resolver picks the
   right asset → dependency resolver shows what else is needed → confirm → installed into the
   active profile.
3. **Apply a lobby code:** paste `PERFECT-…` → diff → **Apply & Launch** (or Apply only).
4. **Copy a lobby code:** any profile → ⧉ Copy lobby code → share in Discord.
5. **Update / downgrade:** per mod, version picker lists release tags; pick one (gated on game
   closed). Profile-level "update all".
6. **Switch profile:** click in sidebar → becomes active → Launch uses it.
7. **Launch:** confirm game closed → set env vars → run.

## 12. Conflict, compatibility & arch handling

- **One role mod per profile:** adding a second role-tagged mod is blocked with a clear message
  offering to replace or open a new profile.
- **all-client vs host-only:** shown as tags; host-only mods don't force guests to match (UI
  explains this for lobby codes).
- **Arch correctness:** detected store → required arch → resolver only selects matching assets
  and LoaderManager installs the matching BepInEx; mismatches are prevented, not warned.
- **Dependency reconciliation:** if two mods need different Reactor versions, surface a conflict
  with the highest compatible pin (or block with explanation).

## 13. Visual design — Aurora Glass

**Vibe:** glossy, premium-consumer, friendly. Frosted-glass panels floating over a deep aurora
gradient. Distinct from every existing AU mod manager (all flat/utilitarian).

- **Background:** layered radial aurora — violet `#3a1d6e` / magenta `#6b2db8` / blue `#0e4a8a`
  blobs over `linear-gradient(135deg,#150a2c,#0a1430)`.
- **Surfaces:** `rgba(255,255,255,0.05–0.08)` + `backdrop-filter: blur(14–22px)`, 1px
  `rgba(255,255,255,0.12–0.2)` borders, radius 11–18px, soft deep shadows.
- **Accent:** gradient **violet `#9b7bff` → cyan `#5bc0ff`** (buttons, active states, toggles).
- **Crew palette (functional):** each profile/mod gets a crewmate hue — `#9b7bff` `#5bc0ff`
  `#5be3b0` `#ff5b5b` `#ffd23f` `#7aa2ff` `#b66bff` — used for dots/identity.
- **Type:** Inter; uppercase tracked micro-labels for section headers; monospace only for codes.
- **Tag colors:** all-client = violet, map = mint, library/loader = neutral glass, host-only = amber.
- **Text:** `#f1ecff` primary, `~0.6` opacity for secondary.

**Screens:** First-run setup · **Home** (profile sidebar + mod list + footer Launch) ·
Add-mod (catalog grid + paste-URL) · **Lobby-code apply** (diff modal) · Settings (game path,
GitHub PAT, theme, reset game settings utility). Mockups captured in
`.superpowers/brainstorm/…` (home + lobby-code flow approved).

## 14. Error handling & edge cases

- **Game running:** block installs/launches that would write; show status, offer to wait.
- **GitHub rate limit (60/hr unauth):** prefer catalog (hosted JSON) for browsing; cache
  release metadata; optional PAT in settings; clear message on 403.
- **Network down:** serve last-cached catalog; queue/deny downloads with explanation.
- **Bad/old code:** validate CRC + schema; friendly errors; resolve unknown ids via embedded `ref`.
- **Wrong-arch asset:** prevented by store detection; if no matching asset exists, explain.
- **Disk space:** check before large downloads (Submerged ~46 MB, ToU ~31 MB).
- **`settings.amogus` black screen** (after game changes): a Settings utility to delete
  `%userprofile%/AppData/LocalLow/Innersloth/Among Us/settings.amogus`.
- **MS Store `WindowsApps`:** if writes fail, flag as unsupported with guidance (best-effort).
- **Download integrity:** verify asset size; hash where the source provides one.

## 15. Testing strategy (TDD)

- **Unit (Rust + TS):** `SyncCodec` encode/decode round-trip + diff; catalog parsing;
  `AssetRules` regex selection across real asset-name samples; dependency resolution + ordering;
  conflict detection; version comparison handling **semver + date-based (`2025.11.20`) +
  BepInEx `-be.NNN`** (no single scheme).
- **Integration:** mocked GitHub Releases API; profile build → expected `BepInEx/plugins`
  contents; GameLocator against fixture store-manifest files; launch-arg/env construction.
- **Manual E2E checklist:** real Steam install → add mod → launch modded → apply a `PERFECT-`
  code → confirm two machines pass the Reactor handshake in a lobby.

## 16. v1 scope vs roadmap

- **v1:** mod management (add/enable/disable/update/downgrade), isolated profiles, catalog +
  any-GitHub-URL + file ingestion, dependency + conflict + arch handling, `PERFECT-` codes,
  one-click launch. Steam/Epic/itch solid; MS Store best-effort.
- **Later:** game-version detection + auto-downgrade (DepotDownloader/Steam depots), shared
  interop cache, deep-link `perfectsync://` install, Linux/macOS, cloud profile sync,
  signed-binary auto-update.

## 17. Distribution

- **Code signing** to avoid SmartScreen warnings (a known killer of prior AU managers).
- Tauri updater for in-app auto-update.

## 18. Open questions / uncertainties

1. Exact Doorstop v4 env-var names for the AU BepInEx pack (verify against `BepInExPack_AmongUs`).
2. Whether modern Reactor needs a non-Steam DLL swap on Epic/itch, or is now cross-platform.
3. MS Store `WindowsApps` write feasibility — may stay unsupported.
4. Catalog hosting/governance (repo, PR review, who maintains).
5. Whether to launch via the exe directly (env vars; loses Steam overlay) or seek a Steam-aware path.
