import type { ModTag } from "./types";

export const CREW = {
  violet: "#9b7bff",
  cyan: "#5bc0ff",
  mint: "#5be3b0",
  red: "#ff5b5b",
  gold: "#ffd23f",
  blue: "#7aa2ff",
  purple: "#b66bff",
} as const;

/** tag -> {fg, bg} used by the Pill component. Locked to the Aurora accent family. */
export const TAG_STYLE: Record<ModTag, { label: string; fg: string; bg: string }> = {
  "all-client": { label: "all-client", fg: "#e2dafe", bg: "#30234f" },
  role: { label: "role", fg: "#e2dafe", bg: "#30234f" },
  "host-only": { label: "host-only", fg: "#ffe7a8", bg: "#4a3509" },
  map: { label: "map", fg: "#bdf7df", bg: "#123f35" },
  cosmetic: { label: "cosmetic", fg: "#c7d7ff", bg: "#1c315c" },
  library: { label: "library", fg: "#e3ddf4", bg: "#302b3e" },
  loader: { label: "loader", fg: "#e3ddf4", bg: "#302b3e" },
};
