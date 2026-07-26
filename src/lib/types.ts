// Mirrors the spec data model (UI-facing subset). The Tauri/Rust core will
// supply these via commands; for now they are populated from mock data.

export type Arch = "x86" | "x64";
export type Store = "steam" | "epic" | "itch" | "msstore" | "manual";
export type Runtime = "native" | "proton" | "wine" | "crossover" | "whisky" | "bottles";
export type Trust = "trusted" | "community" | "flagged";
export type GithubTokenAction =
  | { kind: "unchanged" }
  | { kind: "set"; token: string }
  | { kind: "clear" };
export type ModTag =
  | "role"
  | "all-client"
  | "host-only"
  | "map"
  | "cosmetic"
  | "library"
  | "loader";
export type ModSource = "catalog" | "github" | "file";

export interface ProfileMod {
  packageId: string;
  name: string;
  repo?: string;
  version: string;
  /** available versions for the upgrade/downgrade picker (newest first) */
  versions: string[];
  enabled: boolean;
  source: ModSource;
  tags: ModTag[];
  /** dependencies + the loader are auto-managed and rendered dimmed */
  managed?: boolean;
  /** a newer release exists; value is the newer version */
  update?: string;
  /** installed plugin file name (backend-tracked) */
  file?: string;
  /** exact release asset selected for this installed mod */
  asset?: string;
}

export interface Profile {
  id: string;
  name: string;
  crewColor: string;
  /** reference info only; the app does not change the game version in v1 */
  gameBuild?: string;
  /** globally configured Among Us instance used by this profile */
  gameInstanceId?: string;
  mods: ProfileMod[];
  /** exact LevelImposter maps installed for this profile */
  levelImposterMaps?: string[];
}

export interface CatalogItem {
  id: string;
  name: string;
  repo: string;
  summary: string;
  tags: ModTag[];
  latest: string;
  /** catalog package ids installed automatically unless excluded during review */
  dependencies?: string[];
  /** package components supplied by the selected release bundle, not separate downloads */
  included?: string[];
  /** vetting tier: trusted (curated) | community (listed) | flagged (unknown) */
  trust?: Trust;
}

export interface ModInstallOption {
  tag: string;
  assetName: string;
  size: number;
}

export interface ModInstallSelection {
  id: string;
  repo: string;
  name: string;
  tag: string;
  assetName: string;
  /** true when this selection is included only as an auto-managed dependency */
  managed: boolean;
}

export interface OperationProgress {
  phase: "preparing" | "resolving" | "downloading" | "finalizing";
  message: string;
  bytesReceived?: number;
  bytesTotal?: number;
}

export interface LevelImposterMap {
  id: string;
  name: string;
  thumbnailUrl?: string;
  authorName: string;
  description: string;
}

/** one line in the lobby-code apply diff */
export interface DiffItem {
  name: string;
  repo?: string;
  tags: ModTag[];
  action: "install" | "change" | "ok";
  from?: string;
  to?: string;
  /** Exact release asset requested by the lobby manifest. */
  asset?: string;
  detail: string;
  trust?: Trust;
}

export interface GameStatus {
  store: Store;
  arch: Arch;
  running: boolean;
}

/** A detected Among Us install (from the backend `detect_games`). */
export interface GameInstall {
  path: string;
  store: Store;
  arch: Arch;
  /** how the game runs: native (Windows) or via Proton/Wine/CrossOver */
  runtime?: Runtime;
  /** detected Among Us build/version when readable */
  build?: string;
  /** whether Perfect-Sync can safely modify this folder */
  writable?: boolean;
}

export interface GameInstance extends GameInstall {
  id: string;
  name: string;
  runtime: Runtime;
}

export interface PersonalLocalMod {
  path: string;
  name: string;
  enabled?: boolean;
}

export interface PersonalMod {
  repo: string;
  tag: string;
  asset: string;
  name?: string;
  /** when false, skipped from lobby merges; defaults to enabled */
  enabled?: boolean;
}

export interface Settings {
  gameInstances: GameInstance[];
  personalMods: PersonalMod[];
  /** local DLLs that should be installed into every profile */
  personalLocalMods?: PersonalLocalMod[];
  setupComplete: boolean;
  /** don't warn on launch when BepInEx isn't fully installed */
  skipLaunchWarning?: boolean;
  /** id of the profile to re-select on startup */
  activeProfile?: string;
  /** whether a GitHub token is stored in the native credential store */
  hasGithubToken: boolean;
  /** warning returned after malformed settings were quarantined */
  recoveryWarning?: string;
}
