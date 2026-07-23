import { ShieldCheck, UsersThree, Warning } from "@phosphor-icons/react";
import type { Trust } from "../lib/types";

const STYLE = {
  trusted: { label: "Trusted", fg: "#aef3d8", bg: "rgba(91,227,176,0.16)", Icon: ShieldCheck },
  community: { label: "Community dev", fg: "#bfe0ff", bg: "rgba(122,162,255,0.18)", Icon: UsersThree },
  flagged: { label: "Unverified", fg: "#ffd9a8", bg: "rgba(255,170,60,0.18)", Icon: Warning },
} as const;

/** Shows a mod's vetting tier. `compact` keeps the visible presentation to an icon. */
export function TrustBadge({ trust, compact }: { trust: Trust; compact?: boolean }) {
  const s = STYLE[trust];
  const accessibleLabel = `${s.label} trust tier`;
  return (
    <span
      className="inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-[3px] text-[11px] font-medium leading-none whitespace-nowrap"
      style={{ color: s.fg, background: s.bg }}
      title={accessibleLabel}
      aria-label={accessibleLabel}
    >
      <s.Icon size={11} weight="fill" aria-hidden="true" />
      {compact ? <span className="sr-only">{accessibleLabel}</span> : s.label}
    </span>
  );
}
