import { motion, useReducedMotion } from "motion/react";
import {
  ArrowUp,
  CaretDown,
  CircleNotch,
  GearSix,
  MapTrifold,
  PuzzlePiece,
  Sparkle,
  TrashSimple,
  UsersThree,
  type Icon,
} from "@phosphor-icons/react";
import { Pill, primaryTag } from "./Pill";
import { TrustBadge } from "./TrustBadge";
import { Toggle } from "./Toggle";
import type { ModTag, ProfileMod, Trust } from "../lib/types";

const ICON: Partial<Record<ModTag, Icon>> = {
  role: UsersThree,
  "all-client": UsersThree,
  "host-only": UsersThree,
  map: MapTrifold,
  cosmetic: Sparkle,
  library: PuzzlePiece,
  loader: GearSix,
};

function iconBg(tag: ModTag | null): string {
  if (tag === "map") return "linear-gradient(135deg,#5be3b0,#28b8d0)";
  if (tag === "library" || tag === "loader") return "rgba(255,255,255,0.12)";
  if (tag === "cosmetic") return "linear-gradient(135deg,#7aa2ff,#5bc0ff)";
  return "linear-gradient(135deg,#9b7bff,#7a5bff)";
}

interface ModRowProps {
  mod: ProfileMod;
  busy?: boolean;
  onToggle: () => void;
  onRemove: () => void;
  onPickRelease: () => void;
  trust?: Trust;
}

export function ModRow({ mod, busy, trust, onToggle, onRemove, onPickRelease }: ModRowProps) {
  const reduce = useReducedMotion();
  const tag = mod.tags.length ? primaryTag(mod.tags) : null;
  const Glyph = (tag && ICON[tag]) || PuzzlePiece;

  return (
    <motion.div
      layout={!reduce}
      initial={reduce ? false : { opacity: 0, y: 10 }}
      animate={{ opacity: mod.managed ? 0.62 : 1, y: 0 }}
      transition={{ duration: 0.28, ease: [0.16, 1, 0.3, 1] }}
      className="glass flex items-center gap-3.5 rounded-2xl px-3.5 py-3"
    >
      <span
        className="grid h-9 w-9 shrink-0 place-items-center rounded-[11px] text-[#0d0820]"
        style={{ background: iconBg(tag) }}
      >
        <Glyph size={18} weight="bold" />
      </span>

      <div className="min-w-0">
        <div className="truncate text-[15px] font-semibold text-ink">{mod.name}</div>
        {mod.managed ? (
          <div className="truncate text-[12px] text-ink-faint">
            {mod.tags.includes("loader") ? "loader · auto-managed" : "dependency · auto-managed"}
            {mod.file ? ` · ${mod.file}` : ""}
          </div>
        ) : (
          <div className="truncate text-[12px] text-ink-faint" title={mod.repo}>
            {mod.file ? (
              <span className="font-mono text-ink-dim">{mod.file}</span>
            ) : (
              mod.repo
            )}
          </div>
        )}
      </div>

      <div className="flex-1" />

      {tag && <Pill tag={tag} />}
      {!mod.managed && trust && <TrustBadge trust={trust} compact />}

      {mod.update && !mod.managed && (
        <span className="flex items-center gap-1 rounded-lg border border-[rgba(255,210,63,0.35)] bg-[rgba(255,210,63,0.16)] px-2 py-1 text-[11.5px] font-medium text-[#ffe49a]">
          <ArrowUp size={11} weight="bold" /> {mod.update}
        </span>
      )}

      {mod.managed ? (
        <span className="glass-2 rounded-lg px-2.5 py-1.5 font-mono text-[12.5px] text-ink-faint">
          {mod.version}
        </span>
      ) : busy ? (
        <span className="glass-2 flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[12px] text-ink-dim">
          <CircleNotch size={13} className="animate-spin" /> working
        </span>
      ) : (
        <button
          type="button"
          onClick={onPickRelease}
          aria-label={`Choose version and file for ${mod.name}`}
          title="Choose version / file"
          className="ring-focus glass-2 flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 font-mono text-[12.5px] text-ink-dim transition-colors hover:text-ink"
        >
          {mod.version} <CaretDown size={12} weight="bold" className="opacity-70" />
        </button>
      )}

      <Toggle on={mod.enabled} onChange={onToggle} disabled={busy} label={`Enable ${mod.name}`} />

      {!mod.managed && (
        <button
          type="button"
          onClick={onRemove}
          disabled={busy}
          aria-label={`Remove ${mod.name}`}
          className="ring-focus grid h-8 w-8 place-items-center rounded-lg text-ink-faint transition-colors hover:bg-white/10 hover:text-[#ff8a8a] disabled:opacity-50"
        >
          <TrashSimple size={16} />
        </button>
      )}
    </motion.div>
  );
}
