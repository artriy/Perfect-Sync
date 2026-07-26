export interface MainMod {
  id: string;
  name: string;
}

export interface MainModCandidate {
  id?: string;
  packageId?: string;
  repo?: string;
}

const MAIN_MODS: readonly MainMod[] = [
  { id: "theotherrolesau/theotherroles", name: "The Other Roles" },
  { id: "au-avengers/tou-mira", name: "Town of Us - Mira" },
  { id: "all-of-us-mods/launchpadreloaded", name: "Launchpad Reloaded" },
  { id: "tukasa0001/townofhost", name: "Town of Host" },
  { id: "gurge44/endlesshostroles", name: "Endless Host Roles" },
  { id: "mr-fluuff/stellarrolesau", name: "Stellar Roles" },
  { id: "apemv/amongusrevamped", name: "Among Us Revamped" },
  { id: "slok7565/finalsuspect", name: "Final Suspect" },
  { id: "yukieiji/extremeroles", name: "Extreme Roles" },
  { id: "supernewroles/supernewroles", name: "Super New Roles" },
];

export function findMainMods(candidates: readonly MainModCandidate[]): MainMod[] {
  const identities = new Set<string>();
  for (const candidate of candidates) {
    for (const value of [candidate.id, candidate.packageId, candidate.repo]) {
      if (value) identities.add(value.trim().toLowerCase());
    }
  }
  return MAIN_MODS.filter((mod) => identities.has(mod.id));
}
