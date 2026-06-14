// Mirrors the spec data model (UI-facing subset). The Tauri/Rust core will
// supply these via commands; for now they are populated from mock data.

export type Arch = "x86" | "x64";
export type Store = "steam" | "epic" | "itch" | "msstore" | "manual";
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
}

export interface Profile {
  id: string;
  name: string;
  crewColor: string;
  /** reference info only; the app does not change the game version in v1 */
  gameBuild?: string;
  mods: ProfileMod[];
}

export interface CatalogItem {
  id: string;
  name: string;
  repo: string;
  summary: string;
  tags: ModTag[];
  latest: string;
}

/** one line in the lobby-code apply diff */
export interface DiffItem {
  name: string;
  repo?: string;
  tags: ModTag[];
  action: "install" | "change" | "ok";
  from?: string;
  to?: string;
  detail: string;
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
}

export interface PersonalMod {
  repo: string;
  tag: string;
  asset: string;
  name?: string;
}

export interface Settings {
  githubToken?: string;
  gamePath?: string;
  arch?: Arch;
  catalogUrl?: string;
  personalMods?: PersonalMod[];
}
