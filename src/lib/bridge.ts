import type { CatalogItem, DiffItem, GameInstall, Profile, ProfileMod, Settings } from "./types";
import { CATALOG, PROFILES, SAMPLE_CODE, SAMPLE_DIFF } from "../data/mock";

/** True when running inside the Tauri shell (vs a plain browser via `pnpm dev`). */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export interface Preview {
  name: string;
  items: DiffItem[];
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

/** List a repo's releases + asset files (for manual selection). */
export async function listReleases(repo: string): Promise<GhRelease[]> {
  if (inTauri) return invoke<GhRelease[]>("list_releases", { repo });
  return [];
}

/** Install a specific chosen release asset into the active profile. */
export async function installAsset(
  profile: Profile,
  repo: string,
  tag: string,
  assetName: string,
  arch: string,
): Promise<Profile> {
  if (inTauri)
    return invoke<Profile>("install_asset", { profileId: profile.id, repo, tag, assetName, arch });
  return {
    ...profile,
    mods: profile.mods.map((m) => (m.repo === repo || m.packageId === repo ? { ...m, version: tag } : m)),
  };
}

/** Native folder picker (Tauri only). Returns the chosen path or null. */
export async function pickFolder(): Promise<string | null> {
  if (!inTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ directory: true, multiple: false, title: "Select your Among Us folder" });
  return typeof picked === "string" ? picked : null;
}

// ------------------------------------------------------------------ catalog
export async function loadCatalog(): Promise<CatalogItem[]> {
  if (inTauri) return invoke<CatalogItem[]>("get_catalog");
  return CATALOG;
}

/** Best-effort pull of the hosted catalog into the local cache. */
export async function refreshCatalog(): Promise<number> {
  if (inTauri) return invoke<number>("refresh_catalog");
  return CATALOG.length;
}

// ---------------------------------------------------------------- detection
export async function detectGames(): Promise<GameInstall[]> {
  if (inTauri) return invoke<GameInstall[]>("detect_games");
  return [
    { path: "C:/Program Files (x86)/Steam/steamapps/common/Among Us", store: "steam", arch: "x86" },
  ];
}

export async function getSettings(): Promise<Settings> {
  if (inTauri) return invoke<Settings>("get_settings");
  return {};
}

export async function saveSettings(settings: Settings): Promise<void> {
  if (inTauri) await invoke("save_settings", { settings });
}

export async function gameRunning(): Promise<boolean> {
  if (inTauri) return invoke<boolean>("game_running");
  return false;
}

// ------------------------------------------------------------------ profiles
export async function loadProfiles(): Promise<Profile[]> {
  if (inTauri) return invoke<Profile[]>("list_profiles");
  return structuredClone(PROFILES);
}

export async function saveProfile(profile: Profile): Promise<void> {
  if (inTauri) await invoke("save_profile", { profile });
}

export async function deleteProfile(id: string): Promise<void> {
  if (inTauri) await invoke("delete_profile", { id });
}

// ------------------------------------------------------- mod mutations
// Each returns the updated profile. Under Tauri the backend is authoritative;
// in the browser we apply the same change to a local copy for the demo.

export async function setModEnabled(profile: Profile, packageId: string, enabled: boolean): Promise<Profile> {
  if (inTauri) return invoke<Profile>("set_mod_enabled", { profileId: profile.id, packageId, enabled });
  return { ...profile, mods: profile.mods.map((m) => (m.packageId === packageId ? { ...m, enabled } : m)) };
}

export async function setModVersion(profile: Profile, packageId: string, version: string, arch: string): Promise<Profile> {
  if (inTauri) return invoke<Profile>("set_mod_version", { profileId: profile.id, packageId, version, arch });
  return {
    ...profile,
    mods: profile.mods.map((m) =>
      m.packageId === packageId ? { ...m, version, update: m.update === version ? undefined : m.update } : m,
    ),
  };
}

export async function removeMod(profile: Profile, packageId: string): Promise<Profile> {
  if (inTauri) return invoke<Profile>("remove_mod", { profileId: profile.id, packageId });
  return { ...profile, mods: profile.mods.filter((m) => m.packageId !== packageId) };
}

/** Add a mod by repo/URL. `browserMod` is the locally-constructed entry used in the browser demo. */
export async function addMod(profile: Profile, repo: string, arch: string, browserMod: ProfileMod): Promise<Profile> {
  if (inTauri) return invoke<Profile>("add_mod", { profileId: profile.id, repo, arch });
  if (profile.mods.some((m) => m.packageId === browserMod.packageId)) return profile;
  return { ...profile, mods: [browserMod, ...profile.mods] };
}

// --------------------------------------------------------------- lobby codes
export async function encodeLobbyCode(profile: Profile): Promise<string> {
  if (inTauri) return invoke<string>("encode_lobby_code", { profile });
  return SAMPLE_CODE;
}

export async function previewCode(code: string, installed: [string, string][]): Promise<Preview> {
  if (inTauri) return invoke<Preview>("preview_code", { code, installed });
  await new Promise((r) => setTimeout(r, 500));
  return { name: "Lobby - TownOfUs Night", items: SAMPLE_DIFF };
}

/** Apply a code into a new/refreshed profile. `browserProfile` is the demo fallback. */
export async function applyLobbyCode(code: string, arch: string, browserProfile: Profile): Promise<Profile> {
  if (inTauri) return invoke<Profile>("apply_lobby_code", { code, arch });
  return browserProfile;
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
}

export async function loaderStatus(gamePath: string, profileId: string): Promise<LoaderStatus | null> {
  if (!inTauri) return null;
  return invoke<LoaderStatus>("loader_status", { gamePath, profileId });
}

export async function ensureLoader(gamePath: string, profileId: string, arch: string): Promise<void> {
  if (inTauri) await invoke("ensure_loader", { gamePath, profileId, arch });
}

/** Force-wipe and reinstall the BepInEx engine (fixes a stale/broken loader). */
export async function reinstallLoader(gamePath: string, profileId: string, arch: string): Promise<void> {
  if (inTauri) await invoke("reinstall_loader", { gamePath, profileId, arch });
}

export async function launchProfile(gamePath: string, profileId: string): Promise<void> {
  if (inTauri) await invoke("launch_profile", { gamePath, profileId });
}
