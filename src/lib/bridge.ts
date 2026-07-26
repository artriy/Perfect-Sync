import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { getCurrent as getCurrentDeepLinks, onOpenUrl } from "@tauri-apps/plugin-deep-link";
import type {
  CatalogItem,
  DiffItem,
  Arch,
  GameInstall,
  GithubTokenAction,
  LevelImposterMap,
  ModTag,
  ModInstallOption,
  ModInstallSelection,
  OperationProgress,
  ModSource,
  Profile,
  ProfileMod,
  Runtime,
  Store,
  Settings,
  Trust,
} from "./types";
import { CATALOG, PROFILES } from "../data/mock";

/** True when running inside the Tauri shell (vs a plain browser via `pnpm dev`). */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await tauriInvoke<T>(cmd, args);
  } catch (reason: unknown) {
    const message = reason instanceof Error ? reason.message : String(reason);
    if (/^HTTP status 403$/i.test(message.trim())) {
      throw "HTTP 403: GitHub's API limit is exhausted. Add a GitHub token in Settings, or retry after the rate-limit reset.";
    }
    throw reason;
  }
}

type ProgressHandler = (progress: OperationProgress) => void;

function progressChannel(onProgress?: ProgressHandler): Channel<OperationProgress> {
  return new Channel<OperationProgress>((progress) => onProgress?.(progress));
}


async function simulateBrowserTransfers(
  files: string[],
  onProgress?: ProgressHandler,
): Promise<void> {
  if (!onProgress) return;
  onProgress?.({ phase: "resolving", message: "Resolving exact releases and dependencies" });
  await new Promise<void>((resolve) => {
    window.setTimeout(resolve, 160);
  });
  for (const [fileIndex, file] of files.entries()) {
    const bytesTotal = (3 + fileIndex) * 1024 * 1024;
    for (let chunk = 0; chunk <= 5; chunk += 1) {
      await new Promise<void>((resolve) => {
        window.setTimeout(resolve, 90);
      });
      onProgress?.({
        phase: "downloading",
        message: `Downloading ${file}`,
        bytesReceived: Math.round(bytesTotal * (chunk / 5)),
        bytesTotal,
      });
    }
  }
  onProgress?.({ phase: "finalizing", message: "Verifying files and saving the profile" });
  await new Promise<void>((resolve) => {
    window.setTimeout(resolve, 180);
  });
}

export interface Preview {
  name: string;
  items: DiffItem[];
  gameBuild?: string;
  levelImposterMaps: string[];
}

export interface SaveBackupInfo {
  id: string;
  createdAt: number;
  files: number;
  bytes: number;
}

export interface DiagnosticGame {
  name: string;
  store: Store;
  arch: Arch;
  runtime: Runtime;
  build?: string;
  writable: boolean;
}

export interface DiagnosticLoader {
  current: boolean;
  installedVersion?: string;
  winhttp: boolean;
  preloader: boolean;
  dotnet: boolean;
  profilePlugins: number;
  gamePlugins: number;
}

export interface DiagnosticAsset {
  name: string;
  version: string;
  file?: string;
  enabled: boolean;
  source: ModSource;
}

export interface DiagnosticsReport {
  generatedAt: number;
  appVersion: string;
  profileName?: string;
  game?: DiagnosticGame;
  loader?: DiagnosticLoader;
  assets: DiagnosticAsset[];
  logErrors: string[];
  gameRunning?: boolean;
  warnings: string[];
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
  const assetNames = normalized.toLowerCase() === "au-avengers/tou-mira"
    ? ["TownOfUsMira.dll", "MiraAPI.dll"]
    : normalized.toLowerCase() === "theotherrolesau/theotherroles"
      ? ["TheOtherRoles.zip"]
      : [`${assetStem}.dll`];
  return fixtureVersions(normalized).map((version) => ({
    tag_name: version,
    assets: assetNames.map((name) => ({
      name,
      browser_download_url: `https://github.com/${normalized}/releases/download/${encodeURIComponent(version)}/${encodeURIComponent(name)}`,
      size: 1024 * 1024,
    })),
  }));
}

/** List installable release assets, with the catalog default first. */
export async function listInstallOptions(repo: string, profileId: string): Promise<ModInstallOption[]> {
  if (inTauri) return invoke<ModInstallOption[]>("list_install_options", { repo, profileId });
  return (await listReleases(repo)).flatMap((release) =>
    release.assets
      .filter((asset) => /\.(?:dll|zip)$/i.test(asset.name))
      .map((asset) => ({ tag: release.tag_name, assetName: asset.name, size: asset.size })),
  );
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
  onProgress?: ProgressHandler,
): Promise<Profile> {
  if (!confirmed) throw new Error("Confirm the exact release asset before installing.");
  if (!/\.(?:dll|zip)$/i.test(assetName)) throw new Error("Only .dll and catalog-selected .zip mod files can be installed.");
  if (inTauri) {
    return invoke<Profile>("install_asset", {
      profileId: profile.id,
      repo,
      tag,
      assetName,
      arch,
      confirmed,
      onProgress: progressChannel(onProgress),
    });
  }
  await simulateBrowserTransfers([assetName], onProgress);
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

/** Install an exact, reviewed set of release assets as one profile mutation. */
export async function installAssets(
  profile: Profile,
  selections: ModInstallSelection[],
  confirmed: boolean,
  onProgress?: ProgressHandler,
): Promise<Profile> {
  if (!confirmed) throw new Error("Review the selected versions before installing.");
  if (selections.some((selection) => !/\.(?:dll|zip)$/i.test(selection.assetName))) {
    throw new Error("Only .dll and catalog-selected .zip mod files can be installed.");
  }
  if (inTauri) {
    return invoke<Profile>("install_assets", {
      profileId: profile.id,
      selections,
      confirmed,
      onProgress: progressChannel(onProgress),
    });
  }
  await simulateBrowserTransfers(selections.map((selection) => selection.assetName), onProgress);
  const nextMods = [...profile.mods];
  for (const selection of selections) {
    const catalog = browserCatalog.find(
      (item) => item.id === selection.id || item.repo === selection.repo,
    );
    const position = nextMods.findIndex(
      (mod) => mod.packageId === selection.id || mod.repo === selection.repo,
    );
    const previous = position >= 0 ? nextMods[position] : undefined;
    const versions = previous?.versions.filter((version) => version !== selection.tag) ?? [];
    versions.unshift(selection.tag);
    const installed: ProfileMod = {
      packageId: catalog?.id ?? selection.id,
      name: catalog?.name ?? selection.name,
      repo: selection.repo,
      version: selection.tag,
      versions,
      enabled: true,
      source: catalog ? "catalog" : "github",
      tags: catalog?.tags ?? [],
      managed: selection.managed,
      asset: selection.assetName,
    };
    if (position >= 0) nextMods[position] = installed;
    else nextMods.push(installed);
  }
  return replaceBrowserProfile({ ...profile, mods: nextMods });
}
/** Pick and install one bare local DLL. Local files are intentionally non-shareable. */
export async function installLocalMod(profile: Profile): Promise<Profile | null> {
  if (!inTauri) throw new Error("Local DLL import is available in the desktop app.");
  const selected = await openDialog({
    directory: false,
    multiple: false,
    title: "Select a mod DLL",
    filters: [{ name: "Among Us mod DLL", extensions: ["dll"] }],
  });
  if (selected === null) return null;
  const path = Array.isArray(selected) ? selected[0] : selected;
  if (!path?.toLowerCase().endsWith(".dll")) {
    throw new Error("Only .dll mod files can be installed.");
  }
  return invoke<Profile>("install_local_mod", { profileId: profile.id, path });
}


const browserInstalledMaps = new Map<string, Set<string>>();

interface BrowserLevelImposterCallback {
  v: number;
  error: string;
  data?: Array<{
    id: string;
    name: string;
    authorName?: string;
    description?: string;
    thumbnailURL?: string;
  }>;
}

interface BrowserLevelImposterSearch {
  hits: Array<{
    objectID: string;
    name: string;
    authorName?: string;
    description?: string;
    thumbnailURL?: string;
  }>;
}

/** Retry a blocked native WebView banner through the bounded Rust proxy. */
export async function fetchLevelImposterBanner(url: string): Promise<string> {
  if (!inTauri) throw new Error("Banner proxy is available only in the desktop app.");
  return invoke<string>("fetch_levelimposter_banner", { url });
}

export async function searchLevelImposterMaps(query: string): Promise<LevelImposterMap[]> {
  if (inTauri) return invoke<LevelImposterMap[]>("search_levelimposter_maps", { query });
  const normalized = query.trim();
  if (!normalized) {
    const response = await fetch("/levelimposter-api/maps/top");
    if (!response.ok) throw new Error(`LevelImposter search returned HTTP ${response.status}.`);
    const payload = await response.json() as BrowserLevelImposterCallback;
    if (payload.v !== 1 || payload.error || !Array.isArray(payload.data)) {
      throw new Error(payload.error || "LevelImposter returned invalid map data.");
    }
    return payload.data.map((map) => ({
      id: map.id,
      name: map.name,
      authorName: map.authorName ?? "",
      description: map.description ?? "",
      thumbnailUrl: map.thumbnailURL,
    }));
  }

  const params = new URLSearchParams({
    query: normalized,
    hitsPerPage: "40",
    "x-algolia-application-id": "T5IVXJGKB9",
    "x-algolia-api-key": "14062d24b40e0b3689a899fc36abd756",
  });
  const response = await fetch(`/levelimposter-search?${params}`);
  if (!response.ok) throw new Error(`LevelImposter search returned HTTP ${response.status}.`);
  const payload = await response.json() as BrowserLevelImposterSearch;
  if (!Array.isArray(payload.hits)) throw new Error("LevelImposter returned invalid search data.");
  return payload.hits.map((hit) => ({
    id: hit.objectID,
    name: hit.name,
    authorName: hit.authorName ?? "",
    description: hit.description ?? "",
    thumbnailUrl: hit.thumbnailURL,
  }));
}

export async function listLevelImposterMaps(profileId: string): Promise<string[]> {
  if (inTauri) return invoke<string[]>("list_levelimposter_maps", { profileId });
  const profile = browserProfiles.find((candidate) => candidate.id === profileId);
  return [...(profile?.levelImposterMaps ?? browserInstalledMaps.get(profileId) ?? [])];
}

export async function installLevelImposterMaps(
  profile: Profile,
  mapIds: string[],
  onProgress?: ProgressHandler,
): Promise<Profile> {
  if (inTauri) {
    return invoke<Profile>("install_levelimposter_maps", {
      profileId: profile.id,
      mapIds,
      onProgress: progressChannel(onProgress),
    });
  }
  await simulateBrowserTransfers(
    [
      ...(profile.mods.some((mod) => mod.packageId === "DigiWorm0/LevelImposter")
        ? []
        : ["LevelImposter.dll"]),
      ...mapIds.map((id) => `${id}.lim`),
    ],
    onProgress,
  );
  let installed = profile;
  if (!profile.mods.some((mod) => mod.packageId === "DigiWorm0/LevelImposter")) {
    installed = await installAssets(
      profile,
      [{
        id: "DigiWorm0/LevelImposter",
        repo: "DigiWorm0/LevelImposter",
        name: "LevelImposter",
        tag: "v0.21.2-beta",
        assetName: "LevelImposter.dll",
        managed: false,
      }],
      true,
    );
  }
  const current = new Set(installed.levelImposterMaps ?? browserInstalledMaps.get(profile.id) ?? []);
  for (const id of mapIds) current.add(id);
  const levelImposterMaps = [...current].sort();
  browserInstalledMaps.set(profile.id, new Set(levelImposterMaps));
  return replaceBrowserProfile({ ...installed, levelImposterMaps });
}

export async function removeLevelImposterMaps(
  profile: Profile,
  mapIds: string[],
): Promise<Profile> {
  if (inTauri) {
    return invoke<Profile>("remove_levelimposter_maps", {
      profileId: profile.id,
      mapIds,
    });
  }
  const current = new Set(profile.levelImposterMaps ?? browserInstalledMaps.get(profile.id) ?? []);
  for (const id of mapIds) current.delete(id);
  const levelImposterMaps = [...current].sort();
  browserInstalledMaps.set(profile.id, new Set(levelImposterMaps));
  return replaceBrowserProfile({ ...profile, levelImposterMaps });
}

/** Native folder picker (Tauri only). Returns the chosen path or null. */
export async function pickFolder(): Promise<string | null> {
  if (!inTauri) return null;
  const picked = await openDialog({ directory: true, multiple: false, title: "Select your Among Us folder" });
  return typeof picked === "string" ? picked : null;
}

/** Native DLL picker for local profile and lobby-default mods. */
export async function pickLocalDll(): Promise<string | null> {
  if (!inTauri) return "C:/Mods/LocalUtility.dll";
  const picked = await openDialog({
    directory: false,
    multiple: false,
    title: "Choose a local mod DLL",
    filters: [{ name: "BepInEx plugin", extensions: ["dll"] }],
  });
  return typeof picked === "string" ? picked : null;
}

/** Validate and classify a manually selected Among Us folder. */
export async function inspectGame(gamePath: string): Promise<GameInstall> {
  if (inTauri) return invoke<GameInstall>("inspect_game", { gamePath });
  if (!gamePath.trim()) throw new Error("Choose an Among Us folder.");
  return { path: gamePath.trim(), store: "manual", arch: "x86", runtime: "native" };
}

/** Create a writable, managed copy of an existing Among Us installation. */
export async function createManagedGameCopy(
  sourcePath: string,
  destinationParent: string,
): Promise<GameInstall> {
  if (inTauri) {
    return invoke<GameInstall>("create_managed_game_copy", { sourcePath, destinationParent });
  }
  if (!sourcePath.trim() || !destinationParent.trim()) {
    throw new Error("Choose the source game and a destination folder.");
  }
  return {
    path: `${destinationParent.replace(/[\\/]+$/u, "")}/Perfect-Sync Among Us`,
    store: "msstore",
    arch: "x64",
    runtime: "native",
    build: "2026.3.31",
    writable: true,
  };
}

export async function pickManagedCopyDestination(): Promise<string | null> {
  if (!inTauri) return "C:/Games";
  const picked = await openDialog({
    directory: true,
    multiple: false,
    title: "Choose where to create the managed Among Us copy",
  });
  return typeof picked === "string" ? picked : null;
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
      build: "2026.3.31",
      writable: true,
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
    personalLocalMods: structuredClone(settings.personalLocalMods ?? []),
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

let browserBackups: SaveBackupInfo[] = [];

export async function backupSaveData(): Promise<SaveBackupInfo> {
  if (inTauri) return invoke<SaveBackupInfo>("backup_save_data");
  const createdAt = Date.now();
  const backup = { id: `${createdAt}-1`, createdAt, files: 18, bytes: 94_208 };
  browserBackups = [backup, ...browserBackups].slice(0, 25);
  return structuredClone(backup);
}

export async function listSaveBackups(): Promise<SaveBackupInfo[]> {
  if (inTauri) return invoke<SaveBackupInfo[]>("list_save_backups");
  return structuredClone(browserBackups);
}

export async function restoreSaveData(backupId: string): Promise<SaveBackupInfo> {
  if (inTauri) return invoke<SaveBackupInfo>("restore_save_data", { backupId });
  const backup = browserBackups.find((candidate) => candidate.id === backupId);
  if (!backup) throw new Error("Save backup was not found.");
  return structuredClone(backup);
}

export async function collectDiagnostics(profileId?: string): Promise<DiagnosticsReport> {
  if (inTauri) return invoke<DiagnosticsReport>("collect_diagnostics", { profileId });
  const profile = profileId
    ? browserProfiles.find((candidate) => candidate.id === profileId)
    : browserProfiles[0];
  const instance = browserSettings.gameInstances.find(
    (candidate) => candidate.id === profile?.gameInstanceId,
  ) ?? browserSettings.gameInstances[0];
  return {
    generatedAt: Date.now(),
    appVersion: "0.1.2",
    profileName: profile?.name,
    game: instance
      ? {
          name: instance.name,
          store: instance.store,
          arch: instance.arch,
          runtime: instance.runtime,
          build: instance.build ?? profile?.gameBuild ?? "2026.3.31",
          writable: instance.writable ?? true,
        }
      : undefined,
    loader: {
      current: true,
      installedVersion: "6.0.0-be.735",
      winhttp: true,
      preloader: true,
      dotnet: true,
      profilePlugins: profile?.mods.length ?? 0,
      gamePlugins: 0,
    },
    assets: (profile?.mods ?? []).map((mod) => ({
      name: mod.name,
      version: mod.version,
      file: mod.file,
      enabled: mod.enabled,
      source: mod.source,
    })),
    logErrors: [],
    gameRunning: browserRunning,
    warnings: [],
  };
}

export async function exportSupportBundle(profileId?: string): Promise<string | null> {
  if (!inTauri) {
    await collectDiagnostics(profileId);
    return "Perfect-Sync-support.zip";
  }
  const destination = await saveDialog({
    title: "Export Perfect-Sync support bundle",
    defaultPath: "Perfect-Sync-support.zip",
    filters: [{ name: "ZIP archive", extensions: ["zip"] }],
  });
  if (typeof destination !== "string") return null;
  return invoke<string>("export_support_bundle", { destination, profileId });
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

export async function applyModUpdates(
  profile: Profile,
  packageIds: string[],
  arch: string,
  onProgress?: ProgressHandler,
): Promise<Profile> {
  if (inTauri) {
    return invoke<Profile>("apply_mod_updates", {
      profileId: profile.id,
      packageIds,
      arch,
      onProgress: progressChannel(onProgress),
    });
  }
  if (packageIds.length === 0) throw new Error("Choose at least one reviewed mod update.");
  onProgress?.({ phase: "resolving", message: "Resolving reviewed mod updates" });
  const selected = new Set(packageIds);
  const updated = replaceBrowserProfile({
    ...profile,
    mods: profile.mods.map((mod) =>
      selected.has(mod.packageId) && mod.update
        ? { ...mod, version: mod.update, update: undefined }
        : mod,
    ),
  });
  onProgress?.({ phase: "finalizing", message: "Saving the reviewed mod update batch" });
  return updated;
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
  maps?: string[];
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
  if (manifestKeys.some((key) => !["v", "name", "platform", "gameBuild", "mods", "maps", "loader"].includes(key))) {
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
  if (!Array.isArray(manifest.maps ?? []) || (manifest.maps?.length ?? 0) > 4_096) {
    throw new Error("Malformed lobby manifest.");
  }
  const mapIds = new Set<string>();
  for (const id of manifest.maps ?? []) {
    if (typeof id !== "string" || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu.test(id)) {
      throw new Error("Malformed lobby manifest.");
    }
    const identity = id.toLowerCase();
    if (mapIds.has(identity)) throw new Error("Lobby contains a duplicate LevelImposter map.");
    mapIds.add(identity);
  }
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
  if (mapIds.size > 0 && !identities.has("digiworm0/levelimposter")) {
    throw new Error("LevelImposter maps require the LevelImposter mod.");
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
  requireCodecApi();
  const levelImposterEnabled = profile.mods.some((mod) =>
    mod.enabled && (mod.packageId.toLowerCase() === "digiworm0/levelimposter"
      || mod.repo?.toLowerCase() === "digiworm0/levelimposter"),
  );
  const manifest: BrowserManifest = {
    v: 1,
    name: profile.name,
    mods: profile.mods
      .filter((mod) => mod.enabled && mod.source !== "file")
      .map((mod) => ({ id: mod.repo ?? mod.packageId, v: mod.version, ...(mod.asset ? { a: mod.asset } : {}) })),
    ...(levelImposterEnabled && profile.levelImposterMaps?.length ? { maps: profile.levelImposterMaps } : {}),
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
    gameBuild: undefined,
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
    levelImposterMaps: manifest.maps ?? [],
  };
}

/** Apply a code into a new/refreshed profile. */
export async function applyLobbyCode(
  code: string,
  arch: string,
  gameInstanceId: string | undefined,
  onProgress?: ProgressHandler,
): Promise<Profile> {
  if (inTauri) {
    return invoke<Profile>("apply_lobby_code", {
      code,
      arch,
      gameInstanceId,
      onProgress: progressChannel(onProgress),
    });
  }
  if (!gameInstanceId) throw new Error("Choose an Among Us instance before applying a lobby.");
  const manifest = await decodeBrowserCode(code);
  await simulateBrowserTransfers(
    [
      ...manifest.mods.map((requested) => requested.a ?? `${requested.id.split("/").at(-1) ?? "mod"}.dll`),
      ...(manifest.maps ?? []).map((id) => `${id}.lim`),
    ],
    onProgress,
  );
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
    gameBuild: undefined,
    gameInstanceId,
    mods: manifest.mods.map((requested) => {
      const catalog = browserCatalog.find(
        (item) => item.id.toLowerCase() === requested.id.toLowerCase() || item.repo.toLowerCase() === requested.id.toLowerCase(),
      );
      const versions = fixtureVersions(requested.id);
      const version = requested.v;
      return {
        packageId: catalog?.id ?? requested.id,
        name: catalog?.name ?? requested.id.split("/").at(-1) ?? requested.id,
        repo: catalog?.repo ?? requested.id,
        version,
        versions: versions.includes(version) ? versions : [version, ...versions],
        enabled: true,
        source: catalog ? "catalog" : "github",
        tags: catalog?.tags ?? [],
        asset: requested.a,
      };
    }),
    levelImposterMaps: manifest.maps ?? [],
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
  browserInstalledMaps.set(profile.id, new Set(profile.levelImposterMaps));
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
