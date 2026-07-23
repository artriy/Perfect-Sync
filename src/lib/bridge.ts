import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getCurrent as getCurrentDeepLinks, onOpenUrl } from "@tauri-apps/plugin-deep-link";
import type {
  CatalogItem,
  DiffItem,
  GameInstall,
  GithubTokenAction,
  ModTag,
  Profile,
  ProfileMod,
  Runtime,
  Settings,
  Trust,
} from "./types";
import { CATALOG, PROFILES } from "../data/mock";

/** True when running inside the Tauri shell (vs a plain browser via `pnpm dev`). */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return tauriInvoke<T>(cmd, args);
}

export interface Preview {
  name: string;
  items: DiffItem[];
}

// ----------------------------------------------------------- window controls

export async function winMinimize(): Promise<void> {
  if (inTauri) await getCurrentWindow().minimize();
}

export async function winToggleMaximize(): Promise<void> {
  if (inTauri) await getCurrentWindow().toggleMaximize();
}

export async function winIsMaximized(): Promise<boolean> {
  return inTauri ? getCurrentWindow().isMaximized() : false;
}

export async function onWindowResized(cb: () => void): Promise<() => void> {
  if (!inTauri) return () => {};
  return getCurrentWindow().onResized(() => cb());
}

export async function winClose(): Promise<void> {
  if (inTauri) await getCurrentWindow().close();
}

export interface GhAsset {
  name: string;
  browser_download_url: string;
  size: number;
}

export interface GhRelease {
  tag_name: string;
  assets: GhAsset[];
}

function fixtureVersions(repo: string): string[] {
  const versions = new Set<string>();
  const catalogVersion = browserCatalog.find((item) => item.repo === repo || item.id === repo)?.latest;
  if (catalogVersion) versions.add(catalogVersion);
  for (const profile of browserProfiles) {
    const mod = profile.mods.find((item) => item.repo === repo || item.packageId === repo);
    if (!mod) continue;
    for (const version of mod.versions) versions.add(version);
    versions.add(mod.version);
  }
  if (versions.size === 0) {
    versions.add("1.0.0");
    versions.add("0.9.0");
  }
  return [...versions];
}

/** List a repo's releases + asset files (for manual selection). */
export async function listReleases(repo: string): Promise<GhRelease[]> {
  if (inTauri) return invoke<GhRelease[]>("list_releases", { repo });
  const parsed = repo.match(/(?:github\.com\/)?([^/\s]+)\/([^/#?\s]+)/i);
  if (!parsed) throw new Error("Enter a valid GitHub repository or URL.");
  const normalized = `${parsed[1]}/${parsed[2]}`;
  const assetStem = parsed[2].replace(/\.git$/i, "") || "mod";
  return fixtureVersions(normalized).map((version) => ({
    tag_name: version,
    assets: [
      {
        name: `${assetStem}.dll`,
        browser_download_url: `https://github.com/${normalized}/releases/download/${encodeURIComponent(version)}/${encodeURIComponent(assetStem)}.dll`,
        size: 1024 * 1024,
      },
    ],
  }));
}

function replaceBrowserProfile(profile: Profile): Profile {
  const saved = structuredClone(profile);
  const index = browserProfiles.findIndex((candidate) => candidate.id === saved.id);
  if (index >= 0) browserProfiles[index] = saved;
  else browserProfiles.push(saved);
  return structuredClone(saved);
}

/** Install a specific chosen release asset into the active profile. */
export async function installAsset(
  profile: Profile,
  repo: string,
  tag: string,
  assetName: string,
  arch: string,
  confirmed: boolean,
): Promise<Profile> {
  if (!confirmed) throw new Error("Confirm the exact release asset before installing.");
  if (inTauri) {
    return invoke<Profile>("install_asset", { profileId: profile.id, repo, tag, assetName, arch, confirmed });
  }
  const catalog = browserCatalog.find((item) => item.repo === repo || item.id === repo);
  const existing = profile.mods.find((mod) => mod.repo === repo || mod.packageId === repo);
  const versions = fixtureVersions(repo);
  if (!versions.includes(tag)) versions.unshift(tag);
  const mod: ProfileMod = existing
    ? { ...existing, version: tag, versions, asset: assetName, update: undefined }
    : {
        packageId: catalog?.id ?? repo,
        name: catalog?.name ?? repo.split("/").at(-1) ?? repo,
        repo,
        version: tag,
        versions,
        enabled: true,
        source: catalog ? "catalog" : "github",
        tags: catalog?.tags ?? [],
        asset: assetName,
      };
  return replaceBrowserProfile({
    ...profile,
    mods: existing
      ? profile.mods.map((candidate) => (candidate === existing ? mod : candidate))
      : [mod, ...profile.mods],
  });
}

/** Native folder picker (Tauri only). Returns the chosen path or null. */
export async function pickFolder(): Promise<string | null> {
  if (!inTauri) return null;
  const picked = await openDialog({ directory: true, multiple: false, title: "Select your Among Us folder" });
  return typeof picked === "string" ? picked : null;
}

/** Validate and classify a manually selected Among Us folder. */
export async function inspectGame(gamePath: string): Promise<GameInstall> {
  if (inTauri) return invoke<GameInstall>("inspect_game", { gamePath });
  if (!gamePath.trim()) throw new Error("Choose an Among Us folder.");
  return { path: gamePath.trim(), store: "manual", arch: "x86", runtime: "native" };
}

// ------------------------------------------------------------------ catalog
let browserCatalog = structuredClone(CATALOG);

export async function loadCatalog(): Promise<CatalogItem[]> {
  if (inTauri) return invoke<CatalogItem[]>("get_catalog");
  return structuredClone(browserCatalog);
}

/** Pull the hosted catalog into the local cache. */
export async function refreshCatalog(): Promise<number> {
  if (inTauri) return invoke<number>("refresh_catalog");
  return browserCatalog.length;
}

/** Add a custom repo to the persistent catalog (returns the updated list). */
export async function addCatalogMod(_list: CatalogItem[], repo: string, name: string): Promise<CatalogItem[]> {
  if (inTauri) return invoke<CatalogItem[]>("add_catalog_mod", { repo, name });
  if (!browserCatalog.some((item) => item.id === repo || item.repo === repo)) {
    browserCatalog.push({ id: repo, name, repo, summary: "", tags: [], latest: "", trust: "flagged" });
  }
  return structuredClone(browserCatalog);
}

/** Remove a mod from the persistent catalog (returns the updated list). */
export async function removeCatalogMod(_list: CatalogItem[], id: string): Promise<CatalogItem[]> {
  if (inTauri) return invoke<CatalogItem[]>("remove_catalog_mod", { id });
  browserCatalog = browserCatalog.filter((item) => item.id !== id);
  return structuredClone(browserCatalog);
}

/** Persist a new catalog order (returns the updated list). */
export async function reorderCatalog(_list: CatalogItem[], ids: string[]): Promise<CatalogItem[]> {
  if (inTauri) return invoke<CatalogItem[]>("reorder_catalog", { ids });
  const byId = new Map(browserCatalog.map((item) => [item.id, item] as const));
  browserCatalog = ids.map((id) => byId.get(id)).filter((item): item is CatalogItem => !!item);
  return structuredClone(browserCatalog);
}

// ---------------------------------------------------------------- detection
export async function detectGames(): Promise<GameInstall[]> {
  if (inTauri) return invoke<GameInstall[]>("detect_games");
  return [
    {
      path: "C:/Program Files (x86)/Steam/steamapps/common/Among Us",
      store: "steam",
      arch: "x86",
      runtime: "native",
    },
  ];
}

let browserSettings: Settings = {
  setupComplete: true,
  gameInstances: [
    {
      id: "steam-demo",
      name: "Steam",
      path: "C:/Program Files (x86)/Steam/steamapps/common/Among Us",
      store: "steam",
      arch: "x86",
      runtime: "native",
    },
    {
      id: "epic-demo",
      name: "Epic Games",
      path: "C:/Program Files/Epic Games/AmongUs",
      store: "epic",
      arch: "x86",
      runtime: "native",
    },
  ],
  personalMods: [],
  hasGithubToken: false,
};

function settingsPayload(settings: Settings) {
  const { hasGithubToken: _hasGithubToken, recoveryWarning: _recoveryWarning, ...payload } = settings;
  return payload;
}

function normalizeBrowserSettings(settings: Settings): Settings {
  return {
    ...structuredClone(settings),
    gameInstances: structuredClone(settings.gameInstances ?? []),
    personalMods: structuredClone(settings.personalMods ?? []),
    setupComplete: !!settings.setupComplete,
    hasGithubToken: browserSettings.hasGithubToken,
    recoveryWarning: undefined,
  };
}

export async function getSettings(): Promise<Settings> {
  if (inTauri) return invoke<Settings>("get_settings");
  return structuredClone(browserSettings);
}

export async function saveSettings(
  settings: Settings,
  tokenAction: GithubTokenAction = { kind: "unchanged" },
): Promise<Settings> {
  if (inTauri) {
    return invoke<Settings>("save_settings", {
      settings: settingsPayload(settings),
      tokenAction,
    });
  }
  if (tokenAction.kind === "set" && !tokenAction.token.trim()) {
    throw new Error("GitHub token cannot be blank.");
  }
  const hasGithubToken =
    tokenAction.kind === "set"
      ? true
      : tokenAction.kind === "clear"
        ? false
        : browserSettings.hasGithubToken;
  browserSettings = {
    ...normalizeBrowserSettings(settings),
    hasGithubToken,
  };
  return structuredClone(browserSettings);
}

let browserRunning = false;

export async function gameRunning(): Promise<boolean> {
  if (inTauri) return invoke<boolean>("game_running");
  return browserRunning;
}

// ------------------------------------------------------------------ profiles
let browserProfiles = structuredClone(PROFILES);

export async function loadProfiles(): Promise<Profile[]> {
  if (inTauri) return invoke<Profile[]>("list_profiles");
  return structuredClone(browserProfiles);
}

export async function saveProfile(profile: Profile): Promise<Profile> {
  if (inTauri) return invoke<Profile>("save_profile", { profile });
  const saved = { ...structuredClone(profile), name: profile.name.trim(), crewColor: profile.crewColor.trim() };
  if (!saved.id || !saved.name || !saved.crewColor) throw new Error("Profile name and crew color are required.");
  return replaceBrowserProfile(saved);
}

export async function deleteProfile(id: string): Promise<void> {
  if (inTauri) {
    await invoke<void>("delete_profile", { id });
    return;
  }
  if (!browserProfiles.some((profile) => profile.id === id)) throw new Error("Profile not found.");
  browserProfiles = browserProfiles.filter((profile) => profile.id !== id);
  if (browserSettings.activeProfile === id) browserSettings = { ...browserSettings, activeProfile: undefined };
}

// ------------------------------------------------------- mod mutations
export async function setModEnabled(profile: Profile, packageId: string, enabled: boolean): Promise<Profile> {
  if (inTauri) return invoke<Profile>("set_mod_enabled", { profileId: profile.id, packageId, enabled });
  return replaceBrowserProfile({
    ...profile,
    mods: profile.mods.map((mod) => (mod.packageId === packageId ? { ...mod, enabled } : mod)),
  });
}

export async function setModVersion(
  profile: Profile,
  packageId: string,
  version: string,
  arch: string,
): Promise<Profile> {
  if (inTauri) return invoke<Profile>("set_mod_version", { profileId: profile.id, packageId, version, arch });
  return replaceBrowserProfile({
    ...profile,
    mods: profile.mods.map((mod) =>
      mod.packageId === packageId
        ? { ...mod, version, update: mod.update === version ? undefined : mod.update }
        : mod,
    ),
  });
}

export async function removeMod(profile: Profile, packageId: string): Promise<Profile> {
  if (inTauri) return invoke<Profile>("remove_mod", { profileId: profile.id, packageId });
  return replaceBrowserProfile({ ...profile, mods: profile.mods.filter((mod) => mod.packageId !== packageId) });
}

/** Add a mod by repo/URL. `browserMod` is the locally-constructed entry used in the browser demo. */
export async function addMod(
  profile: Profile,
  repo: string,
  arch: string,
  browserMod: ProfileMod,
): Promise<Profile> {
  if (inTauri) return invoke<Profile>("add_mod", { profileId: profile.id, repo, arch });
  if (profile.mods.some((mod) => mod.packageId === browserMod.packageId)) return structuredClone(profile);
  return replaceBrowserProfile({ ...profile, mods: [browserMod, ...profile.mods] });
}

export async function checkModUpdates(profileId: string, arch: string): Promise<Profile> {
  if (inTauri) return invoke<Profile>("check_mod_updates", { profileId, arch });
  const profile = browserProfiles.find((candidate) => candidate.id === profileId);
  if (!profile) throw new Error("Profile not found.");
  return replaceBrowserProfile({
    ...profile,
    mods: profile.mods.map((mod) => {
      const latest = browserCatalog.find((item) => item.id === mod.packageId || item.repo === mod.repo)?.latest;
      return { ...mod, update: latest && latest !== mod.version ? latest : undefined };
    }),
  });
}

// --------------------------------------------------------------- lobby codes
interface BrowserManifestMod {
  id: string;
  v: string;
  a?: string;
}

interface BrowserManifest {
  v: number;
  name?: string;
  gameBuild?: string;
  mods: BrowserManifestMod[];
  platform?: unknown;
  loader?: unknown;
}

const MAX_CODE_LENGTH = 64 * 1024;
const MAX_MANIFEST_LENGTH = 1024 * 1024;

function requireCodecApi(): void {
  if (
    typeof CompressionStream === "undefined" ||
    typeof DecompressionStream === "undefined" ||
    typeof TextEncoder === "undefined" ||
    typeof TextDecoder === "undefined" ||
    typeof Blob === "undefined" ||
    typeof Response === "undefined" ||
    typeof btoa === "undefined" ||
    typeof atob === "undefined"
  ) {
    throw new Error("This browser does not provide the compression APIs required for lobby codes.");
  }
}

function crc32Ascii(value: string): number {
  let crc = 0xffffffff;
  for (let index = 0; index < value.length; index += 1) {
    crc ^= value.charCodeAt(index);
    for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function bytesToBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/u, "");
}

function base64UrlToBytes(value: string): Uint8Array {
  if (!/^[A-Za-z0-9_-]+$/u.test(value)) throw new Error("Malformed lobby code.");
  const padded = value.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
  let decoded: string;
  try {
    decoded = atob(padded);
  } catch {
    throw new Error("Malformed lobby code.");
  }
  return Uint8Array.from(decoded, (character) => character.charCodeAt(0));
}

async function gzip(bytes: Uint8Array): Promise<Uint8Array> {
  requireCodecApi();
  const stream = new Blob([bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer])
    .stream()
    .pipeThrough(new CompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

async function gunzip(bytes: Uint8Array): Promise<Uint8Array> {
  requireCodecApi();
  const stream = new Blob([bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer])
    .stream()
    .pipeThrough(new DecompressionStream("gzip"));
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (length + value.byteLength > MAX_MANIFEST_LENGTH) {
        try {
          await reader.cancel();
        } catch {
          // Preserve the size violation even if the decompressor is already closed.
        }
        throw new Error("Lobby manifest is too large.");
      }
      chunks.push(value);
      length += value.byteLength;
    }
  } catch (error) {
    try {
      await reader.cancel();
    } catch {
      // The decompressor may already have errored and closed the stream.
    }
    if (error instanceof Error && error.message === "Lobby manifest is too large.") throw error;
    throw new Error("Malformed lobby code.");
  } finally {
    reader.releaseLock();
  }

  const output = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    output.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return output;
}

function validateManifest(value: unknown): BrowserManifest {
  if (!value || typeof value !== "object") throw new Error("Malformed lobby manifest.");
  const manifest = value as Partial<BrowserManifest>;
  const manifestKeys = Object.keys(manifest);
  if (manifestKeys.some((key) => !["v", "name", "platform", "gameBuild", "mods", "loader"].includes(key))) {
    throw new Error("Malformed lobby manifest.");
  }
  if (
    (manifest.name != null && (typeof manifest.name !== "string" || manifest.name.length > 128)) ||
    (manifest.gameBuild != null && typeof manifest.gameBuild !== "string")
  ) {
    throw new Error("Malformed lobby manifest.");
  }
  if (manifest.v !== 1) throw new Error(`Unsupported lobby schema version ${String(manifest.v)}.`);
  if (manifest.platform != null || manifest.loader != null) throw new Error("This lobby uses an unsupported feature.");
  if (!Array.isArray(manifest.mods) || manifest.mods.length > 64) throw new Error("Malformed lobby manifest.");
  const identities = new Set<string>();
  for (const mod of manifest.mods) {
    if (!mod || typeof mod !== "object" || Object.keys(mod).some((key) => !["id", "v", "a"].includes(key))) {
      throw new Error("Malformed lobby manifest.");
    }
    if (
      typeof mod.id !== "string" ||
      !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/u.test(mod.id) ||
      mod.id.length > 140 ||
      typeof mod.v !== "string" ||
      !mod.v.trim() ||
      mod.v.length > 128 ||
      (mod.a != null &&
        (typeof mod.a !== "string" ||
          !mod.a.trim() ||
          mod.a.length > 255 ||
          mod.a.includes("/") ||
          mod.a.includes("\\") ||
          /[\u0000-\u001f\u007f]/u.test(mod.a)))
    ) {
      throw new Error("Malformed lobby manifest.");
    }
    const identity = mod.id.toLowerCase();
    if (identities.has(identity)) throw new Error("Lobby contains a duplicate mod repository.");
    identities.add(identity);
  }
  return manifest as BrowserManifest;
}

async function decodeBrowserCode(code: string): Promise<BrowserManifest> {
  requireCodecApi();
  if (code.length > MAX_CODE_LENGTH) throw new Error("Lobby code is too large.");
  const match = code.match(/^PERFECT-([A-Za-z0-9_-]+)\.([0-9a-fA-F]{4})$/u);
  if (!match) throw new Error("Malformed lobby code.");
  const body = match[1];
  const checksum = Number.parseInt(match[2], 16);
  if ((crc32Ascii(body) & 0xffff) !== checksum) throw new Error("Lobby code checksum mismatch.");
  const json = new TextDecoder("utf-8", { fatal: true }).decode(await gunzip(base64UrlToBytes(body)));
  let decoded: unknown;
  try {
    decoded = JSON.parse(json);
  } catch {
    throw new Error("Malformed lobby manifest.");
  }
  return validateManifest(decoded);
}

export async function encodeLobbyCode(profile: Profile): Promise<string> {
  if (inTauri) return invoke<string>("encode_lobby_code", { profile });
  requireCodecApi();
  const manifest: BrowserManifest = {
    v: 1,
    name: profile.name,
    gameBuild: profile.gameBuild,
    mods: profile.mods
      .filter((mod) => mod.enabled)
      .map((mod) => ({ id: mod.repo ?? mod.packageId, v: mod.version, ...(mod.asset ? { a: mod.asset } : {}) })),
  };
  const body = bytesToBase64Url(await gzip(new TextEncoder().encode(JSON.stringify(manifest))));
  return `PERFECT-${body}.${(crc32Ascii(body) & 0xffff).toString(16).padStart(4, "0")}`;
}

function resolvedPreviewTrust(repo: string | undefined, name: string, trust: Trust | undefined): Trust {
  const identity = (repo ?? name).toLowerCase();
  return identity === "bepinex/bepinex" ? "trusted" : (trust ?? "flagged");
}

export async function previewCode(code: string, installed: [string, string][]): Promise<Preview> {
  if (inTauri) {
    const preview = await invoke<Preview>("preview_code", { code, installed });
    return {
      ...preview,
      items: preview.items.map((item) => ({
        ...item,
        trust: resolvedPreviewTrust(item.repo, item.name, item.trust),
      })),
    };
  }
  const manifest = await decodeBrowserCode(code);
  const installedVersions = new Map(installed.map(([id, version]) => [id.toLowerCase(), version]));
  return {
    name: manifest.name ?? "Imported lobby",
    items: manifest.mods.map((requested) => {
      const catalog = browserCatalog.find(
        (item) => item.id.toLowerCase() === requested.id.toLowerCase() || item.repo.toLowerCase() === requested.id.toLowerCase(),
      );
      const current = installedVersions.get(requested.id.toLowerCase());
      const action: DiffItem["action"] = current == null ? "install" : current === requested.v ? "ok" : "change";
      return {
        name: catalog?.name ?? requested.id.split("/").at(-1) ?? requested.id,
        repo: requested.id,
        tags: catalog?.tags ?? ([] as ModTag[]),
        action,
        from: current,
        to: requested.v,
        asset: requested.a,
        detail:
          action === "install"
            ? "not in this set yet"
            : action === "ok"
              ? `${requested.v}, already installed`
              : `you have ${current}, lobby needs ${requested.v}`,
        trust: resolvedPreviewTrust(requested.id, catalog?.name ?? requested.id, catalog?.trust),
      };
    }),
  };
}

/** Apply a code into a new/refreshed profile. */
export async function applyLobbyCode(
  code: string,
  arch: string,
  gameInstanceId: string | undefined,
): Promise<Profile> {
  if (inTauri) return invoke<Profile>("apply_lobby_code", { code, arch, gameInstanceId });
  if (!gameInstanceId) throw new Error("Choose an Among Us instance before applying a lobby.");
  const manifest = await decodeBrowserCode(code);
  const slug = (manifest.name ?? "imported-lobby")
    .toLowerCase()
    .replace(/[^a-z0-9]+/gu, "-")
    .replace(/^-|-$/gu, "")
    .slice(0, 16) || "imported-lobby";
  const body = code.slice("PERFECT-".length, code.lastIndexOf("."));
  const profile: Profile = {
    id: `lobby-${slug}-${body.slice(0, 16).toLowerCase()}`,
    name: manifest.name ?? "Imported lobby",
    crewColor: "#ffd23f",
    gameBuild: manifest.gameBuild,
    gameInstanceId,
    mods: manifest.mods.map((requested) => {
      const catalog = browserCatalog.find(
        (item) => item.id.toLowerCase() === requested.id.toLowerCase() || item.repo.toLowerCase() === requested.id.toLowerCase(),
      );
      const versions = fixtureVersions(requested.id);
      return {
        packageId: catalog?.id ?? requested.id,
        name: catalog?.name ?? requested.id.split("/").at(-1) ?? requested.id,
        repo: catalog?.repo ?? requested.id,
        version: requested.v,
        versions: versions.includes(requested.v) ? versions : [requested.v, ...versions],
        enabled: true,
        source: catalog ? "catalog" : "github",
        tags: catalog?.tags ?? [],
        asset: requested.a,
      };
    }),
  };
  for (const personal of browserSettings.personalMods.filter((candidate) => candidate.enabled !== false)) {
    const identity = personal.repo.toLowerCase();
    const existing = profile.mods.find(
      (mod) => mod.packageId.toLowerCase() === identity || mod.repo?.toLowerCase() === identity,
    );
    if (existing) {
      existing.managed = false;
      continue;
    }
    const catalog = browserCatalog.find(
      (item) => item.id.toLowerCase() === identity || item.repo.toLowerCase() === identity,
    );
    const versions = fixtureVersions(personal.repo);
    profile.mods.push({
      packageId: personal.repo,
      name: personal.name ?? catalog?.name ?? personal.repo,
      repo: personal.repo,
      version: personal.tag,
      versions: versions.includes(personal.tag) ? versions : [personal.tag, ...versions],
      enabled: true,
      source: catalog ? "catalog" : "github",
      tags: catalog?.tags ?? [],
      managed: false,
      asset: personal.asset,
    });
  }
  return replaceBrowserProfile(profile);
}

// ------------------------------------------------------------ loader + launch
export interface LoaderStatus {
  gameFound: boolean;
  winhttp: boolean;
  preloader: boolean;
  current: boolean;
  installedVersion?: string | null;
  dotnet: boolean;
  steamAppid: boolean;
  profilePlugins: number;
  gamePlugins: number;
  runtime: Runtime;
  runtimeReady: boolean;
}

export async function loaderStatus(gamePath: string, profileId: string): Promise<LoaderStatus> {
  if (inTauri) return invoke<LoaderStatus>("loader_status", { gamePath, profileId });
  if (!gamePath.trim()) throw new Error("Choose an Among Us instance first.");
  return {
    gameFound: true,
    winhttp: true,
    preloader: true,
    current: true,
    installedVersion: "6.0.0-be.735",
    dotnet: true,
    steamAppid: true,
    profilePlugins: browserProfiles.find((profile) => profile.id === profileId)?.mods.length ?? 0,
    gamePlugins: 0,
    runtime: "native",
    runtimeReady: true,
  };
}

export async function ensureLoader(gamePath: string, profileId: string, arch: string): Promise<string | null> {
  if (inTauri) return invoke<string | null>("ensure_loader", { gamePath, profileId, arch });
  if (!gamePath.trim()) throw new Error("Choose an Among Us instance first.");
  return null;
}

/** Force-wipe and reinstall the BepInEx engine (fixes a stale/broken loader). */
export async function reinstallLoader(
  gamePath: string,
  profileId: string,
  arch: string,
): Promise<string | null> {
  if (inTauri) return invoke<string | null>("reinstall_loader", { gamePath, profileId, arch });
  if (!gamePath.trim()) throw new Error("Choose an Among Us instance first.");
  return null;
}

function beginBrowserLaunch(): void {
  if (browserRunning) throw new Error("Among Us is already running.");
  browserRunning = true;
  window.setTimeout(() => {
    browserRunning = false;
  }, 2800);
}

export async function launchProfile(gamePath: string, profileId: string): Promise<void> {
  if (inTauri) {
    await invoke<void>("launch_profile", { gamePath, profileId });
    return;
  }
  if (!gamePath.trim()) throw new Error("Choose an Among Us instance first.");
  beginBrowserLaunch();
}

export async function launchVanilla(gamePath: string): Promise<void> {
  if (inTauri) {
    await invoke<void>("launch_vanilla", { gamePath });
    return;
  }
  if (!gamePath.trim()) throw new Error("Choose an Among Us instance first.");
  beginBrowserLaunch();
}

/** Synchronize the active profile into the game folder. Returns optional runtime guidance. */
export async function syncProfile(gamePath: string, profileId: string): Promise<string | null> {
  if (inTauri) return invoke<string | null>("sync_profile", { gamePath, profileId });
  if (!gamePath.trim()) throw new Error("Choose an Among Us instance first.");
  return null;
}

export interface UpdateInfo {
  version: string;
  url: string;
}

/** Check GitHub Releases for a newer version (null if up to date). */
export async function checkUpdate(): Promise<UpdateInfo | null> {
  if (!inTauri) return null;
  return invoke<UpdateInfo | null>("check_update");
}

/** Open an https URL in the user's default browser. */
export async function openUrl(url: string): Promise<void> {
  if (inTauri) {
    await invoke<void>("open_url", { url });
    return;
  }
  const opened = window.open(url, "_blank", "noopener,noreferrer");
  if (!opened) throw new Error("The browser blocked the download page. Allow pop-ups and try again.");
}

// ----------------------------------------------------------- lobby sharing
/** Custom URI scheme the Tauri shell registers for one-click lobby links. */
export const LOBBY_SCHEME = "perfectsync";

/** A clickable deep link that opens Perfect-Sync straight onto this lobby. */
export function lobbyDeepLink(code: string): string {
  return `${LOBBY_SCHEME}://lobby/${code}`;
}

export const LOBBY_WEB_BASE = "https://artriy.github.io/Perfect-Sync/";

export function webLobbyLink(name: string, code: string): string {
  return `${LOBBY_WEB_BASE}#lobby=${encodeURIComponent(name.trim())}&code=${encodeURIComponent(code)}`;
}

function escapeDiscordLabel(value: string): string {
  return value
    .replace(/[\u0000-\u001f\u007f-\u009f]/gu, " ")
    .replace(/([\\`*_{}\[\]()<>#+\-.!|~])/gu, "\\$1");
}

export function discordShare(name: string, code: string): string {
  return `[${escapeDiscordLabel(name)}](${webLobbyLink(name, code)})`;
}

/** Pull a PERFECT- code out of a raw code, a deep link, or a markdown link. */
export function extractLobbyCode(input: string): string | null {
  const match = input.match(/PERFECT-[A-Za-z0-9_-]+\.[0-9a-fA-F]{4}/u);
  return match ? match[0] : null;
}

/** Subscribe to incoming perfectsync:// links (cold start + while running). */
export async function onLobbyLink(cb: (code: string) => void): Promise<(() => void) | void> {
  const DUPLICATE_EVENT_WINDOW_MS = 1_000;
  let lastDelivery: { code: string; at: number } | null = null;
  const deliver = (urls: string[]) => {
    const code = urls.map(extractLobbyCode).find((candidate): candidate is string => !!candidate);
    if (!code) return;
    const now = Date.now();
    if (lastDelivery?.code === code && now - lastDelivery.at < DUPLICATE_EVENT_WINDOW_MS) return;
    lastDelivery = { code, at: now };
    cb(code);
  };

  if (!inTauri) {
    const deliverLocation = () => deliver([window.location.href]);
    window.addEventListener("hashchange", deliverLocation);
    window.addEventListener("popstate", deliverLocation);
    deliverLocation();
    return () => {
      window.removeEventListener("hashchange", deliverLocation);
      window.removeEventListener("popstate", deliverLocation);
    };
  }

  const unlisten = await onOpenUrl(deliver);
  try {
    deliver((await getCurrentDeepLinks()) ?? []);
  } catch (error) {
    unlisten();
    throw new Error(`Could not read the current lobby link: ${String(error)}`);
  }
  return unlisten;
}
