import bundledCatalog from "../../catalog/catalog.json";
import { CREW } from "../lib/palette";
import type { CatalogItem, GameStatus, Profile } from "../lib/types";

// Loader + common dependencies, reused across profiles. Versions reflect the
// ecosystem research (BepInEx 6.0.0-be.735, Reactor 2.5.0, MiraAPI 0.3.9).
const bepinex = {
  packageId: "BepInEx/BepInEx",
  name: "BepInEx",
  version: "6.0.0-be.735",
  versions: ["6.0.0-be.735", "6.0.0-be.725", "6.0.0-be.697"],
  enabled: true,
  source: "catalog" as const,
  tags: ["loader" as const],
  managed: true,
};
const reactor = {
  packageId: "NuclearPowered/Reactor",
  name: "Reactor",
  repo: "NuclearPowered/Reactor",
  version: "2.5.0",
  versions: ["2.5.0", "2.3.1", "2.2.0"],
  enabled: true,
  source: "catalog" as const,
  tags: ["library" as const],
  managed: true,
};

export const PROFILES: Profile[] = [
  {
    id: "tou-mira-night",
    name: "ToU Mira night",
    crewColor: CREW.violet,
    gameBuild: "17.0.1",
    gameInstanceId: "steam-demo",
    mods: [
      {
        packageId: "AU-Avengers/TOU-Mira",
        name: "Town of Us - Mira",
        repo: "AU-Avengers/TOU-Mira",
        version: "1.6.2",
        versions: ["1.6.3", "1.6.2", "1.6.1", "1.5.0"],
        enabled: true,
        source: "github",
        tags: ["role", "all-client"],
        update: "1.6.3",
      },
      {
        packageId: "SubmergedAmongUs/Submerged",
        name: "Submerged",
        repo: "SubmergedAmongUs/Submerged",
        version: "2025.11.20",
        versions: ["2025.11.20", "2025.9.4", "2025.6.1"],
        enabled: true,
        source: "github",
        tags: ["map"],
      },
      bepinex,
    ],
  },
  {
    id: "tohe-chaos",
    name: "TOHE chaos",
    crewColor: CREW.red,
    gameBuild: "17.0.1",
    gameInstanceId: "steam-demo",
    mods: [
      {
        packageId: "EnhancedNetwork/TownofHost-Enhanced",
        name: "Town of Host - Enhanced",
        repo: "EnhancedNetwork/TownofHost-Enhanced",
        version: "2.4.0",
        versions: ["2.4.1", "2.4.0", "2.3.5"],
        enabled: true,
        source: "github",
        tags: ["role", "host-only"],
        update: "2.4.1",
      },
      bepinex,
    ],
  },
  {
    id: "the-other-roles",
    name: "The Other Roles",
    crewColor: CREW.cyan,
    gameBuild: "16.0.5",
    gameInstanceId: "steam-demo",
    mods: [
      {
        packageId: "TheOtherRolesAU/TheOtherRoles",
        name: "The Other Roles",
        repo: "TheOtherRolesAU/TheOtherRoles",
        version: "4.8.0",
        versions: ["4.8.0", "4.7.2", "4.6.0"],
        enabled: true,
        source: "github",
        tags: ["role", "all-client"],
      },
      { ...reactor, version: "2.3.1" },
      { ...bepinex, version: "6.0.0-be.697" },
    ],
  },
  {
    id: "vanilla-qol",
    name: "Vanilla + QoL",
    crewColor: CREW.mint,
    gameBuild: "17.0.1",
    gameInstanceId: "steam-demo",
    mods: [
      {
        packageId: "DigiWorm0/LevelImposter",
        name: "LevelImposter",
        repo: "DigiWorm0/LevelImposter",
        version: "v0.21.2-beta",
        versions: ["v0.21.2-beta", "v0.21.1-beta"],
        enabled: true,
        source: "github",
        tags: ["map", "cosmetic"],
      },
      reactor,
      bepinex,
    ],
  },
];

export const GAME: GameStatus = { store: "steam", arch: "x86", running: false };

const PRIORITY_CATALOG_IDS = [
  "TheOtherRolesAU/TheOtherRoles",
  "AU-Avengers/TOU-Mira",
  "EnhancedNetwork/TownofHost-Enhanced",
  "Mehzxzz/TownOfExtra",
  "DigiWorm0/LevelImposter",
];
const BUNDLED_DEPENDENCY_IDS = [
  "All-Of-Us-Mods/MiraAPI",
  "NuclearPowered/Reactor",
  "miniduikboot/Mini.RegionInstall",
];

function catalogRank(item: CatalogItem): number {
  const priority = PRIORITY_CATALOG_IDS.indexOf(item.id);
  if (priority >= 0) return priority;
  return item.tags.includes("library") || BUNDLED_DEPENDENCY_IDS.includes(item.id)
    ? Number.MAX_SAFE_INTEGER
    : PRIORITY_CATALOG_IDS.length;
}

export const CATALOG: CatalogItem[] = bundledCatalog.mods
  .map((item) => ({
    id: item.id,
    name: item.name,
    repo: item.repo,
    summary: item.summary,
    tags: item.tags as CatalogItem["tags"],
    latest: "latest",
    dependencies: item.dependencies,
    provides: "provides" in item ? item.provides : undefined,
    dependencyVersions: "dependencyVersions" in item ? item.dependencyVersions : undefined,
    recommendedDependencies:
      "recommendedDependencies" in item ? item.recommendedDependencies : undefined,
    included:
      item.id === "AU-Avengers/TOU-Mira"
        ? ["MiraAPI", "Reactor", "Mini.RegionInstall with the Town of Us server config", "Town of Us cosmetics"]
        : undefined,
    trust: item.trust as CatalogItem["trust"],
  }))
  .sort((left, right) => catalogRank(left) - catalogRank(right));

// A valid checksum-bearing lobby code used when a static fixture is needed.
export const SAMPLE_CODE =
  "PERFECT-H4sIAAAAAAAC_0WLTQuCMBiA_0q8Z33dFiV4M7oElYccHaKD4VoDt8Wm6yD-9yZIXZ-PEQIUNAHTaAEF1PZjqif3q7OSrx4SkJHvBtW1UdIcCdIItW09FLcR1IxLnpZBGCmcz-qKpyflmhiF-cAtrmFKlvIyPLRwUrSltkZyn_3A0jPCNkgpMvKf9kqqq3WaZEcRRHfQb-t74ZaDYI4Mpvv0BSvE0SvJAAAA.86b7";
