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
  "all-client": { label: "all-client", fg: "#d4c6ff", bg: "rgba(155,123,255,0.26)" },
  role: { label: "role", fg: "#d4c6ff", bg: "rgba(155,123,255,0.26)" },
  "host-only": { label: "host-only", fg: "#ffe49a", bg: "rgba(255,210,63,0.20)" },
  map: { label: "map", fg: "#aef3d8", bg: "rgba(91,227,176,0.24)" },
  cosmetic: { label: "cosmetic", fg: "#a8c2ff", bg: "rgba(122,162,255,0.22)" },
  library: { label: "library", fg: "#d7d2ee", bg: "rgba(255,255,255,0.12)" },
  loader: { label: "loader", fg: "#d7d2ee", bg: "rgba(255,255,255,0.12)" },
};
