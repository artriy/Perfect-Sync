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
const miraApi = {
  packageId: "All-Of-Us-Mods/MiraAPI",
  name: "MiraAPI",
  repo: "All-Of-Us-Mods/MiraAPI",
  version: "0.3.9",
  versions: ["0.4.0", "0.3.9", "0.3.8"],
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
      miraApi,
      reactor,
      bepinex,
    ],
  },
  {
    id: "tohe-chaos",
    name: "TOHE chaos",
    crewColor: CREW.red,
    gameBuild: "17.0.1",
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
    mods: [
      {
        packageId: "DigiWorm0/LevelImposter",
        name: "LevelImposter",
        repo: "DigiWorm0/LevelImposter",
        version: "0.7.2",
        versions: ["0.7.2", "0.7.0"],
        enabled: true,
        source: "github",
        tags: ["cosmetic"],
      },
      reactor,
      bepinex,
    ],
  },
];

export const GAME: GameStatus = { store: "steam", arch: "x86", running: false };

export const CATALOG: CatalogItem[] = [
  { id: "AU-Avengers/TOU-Mira", name: "Town of Us - Mira", repo: "AU-Avengers/TOU-Mira", summary: "The Mira-API rebuild of Town of Us. Dozens of custom roles.", tags: ["role", "all-client"], latest: "1.6.3", trust: "trusted" },
  { id: "TheOtherRolesAU/TheOtherRoles", name: "The Other Roles", repo: "TheOtherRolesAU/TheOtherRoles", summary: "Classic all-client role mod with a deep options menu.", tags: ["role", "all-client"], latest: "4.8.0", trust: "trusted" },
  { id: "EnhancedNetwork/TownofHost-Enhanced", name: "Town of Host - Enhanced", repo: "EnhancedNetwork/TownofHost-Enhanced", summary: "Host-only chaos modes. Guests can stay vanilla.", tags: ["role", "host-only"], latest: "2.4.1", trust: "trusted" },
  { id: "SubmergedAmongUs/Submerged", name: "Submerged", repo: "SubmergedAmongUs/Submerged", summary: "The underwater map, with elevators and verticality.", tags: ["map"], latest: "2025.11.20", trust: "trusted" },
  { id: "All-Of-Us-Mods/LaunchpadReloaded", name: "Launchpad Reloaded", repo: "All-Of-Us-Mods/LaunchpadReloaded", summary: "A fresh roster of roles built on Mira API.", tags: ["role", "all-client"], latest: "0.3.8", trust: "trusted" },
  { id: "DigiWorm0/LevelImposter", name: "LevelImposter", repo: "DigiWorm0/LevelImposter", summary: "Load community-built custom maps from files.", tags: ["map", "cosmetic"], latest: "0.7.2", trust: "trusted" },
  { id: "NuclearPowered/Reactor", name: "Reactor", repo: "NuclearPowered/Reactor", summary: "Shared runtime library for Reactor-based Among Us mods.", tags: ["library"], latest: "2.5.0", trust: "trusted" },
  { id: "All-Of-Us-Mods/MiraAPI", name: "MiraAPI", repo: "All-Of-Us-Mods/MiraAPI", summary: "Shared API and runtime library for Mira-based mods.", tags: ["library"], latest: "0.3.9", trust: "trusted" },
];

// A valid checksum-bearing lobby code used when a static fixture is needed.
export const SAMPLE_CODE =
  "PERFECT-H4sIAAAAAAAC_0WLTQuCMBiA_0q8Z33dFiV4M7oElYccHaKD4VoDt8Wm6yD-9yZIXZ-PEQIUNAHTaAEF1PZjqif3q7OSrx4SkJHvBtW1UdIcCdIItW09FLcR1IxLnpZBGCmcz-qKpyflmhiF-cAtrmFKlvIyPLRwUrSltkZyn_3A0jPCNkgpMvKf9kqqq3WaZEcRRHfQb-t74ZaDYI4Mpvv0BSvE0SvJAAAA.86b7";
