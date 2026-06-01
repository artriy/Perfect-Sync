import { TAG_STYLE } from "../lib/palette";
import type { ModTag } from "../lib/types";

/** Shows the most meaningful tag for a mod (role/all-client/host-only/map/library). */
export function Pill({ tag }: { tag: ModTag }) {
  const s = TAG_STYLE[tag];
  return (
    <span
      className="rounded-full px-2.5 py-[3px] text-[11px] font-medium leading-none whitespace-nowrap"
      style={{ color: s.fg, background: s.bg }}
    >
      {s.label}
    </span>
  );
}

/** Picks the tag a player most needs to see first. */
export function primaryTag(tags: ModTag[]): ModTag {
  const order: ModTag[] = ["all-client", "host-only", "role", "map", "cosmetic", "library", "loader"];
  for (const t of order) if (tags.includes(t)) return t;
  return tags[0];
}
