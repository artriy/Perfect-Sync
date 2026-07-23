import { LinkSimple, Plus } from "@phosphor-icons/react";
import type { Profile } from "../lib/types";

interface SidebarProps {
  profiles: Profile[];
  activeId: string;
  busy: boolean;
  onSelect: (id: string) => void;
  onNewProfile: () => void;
  onPasteCode: () => void;
}

export function Sidebar({ profiles, activeId, busy, onSelect, onNewProfile, onPasteCode }: SidebarProps) {
  return (
    <aside className="glass-2 flex min-h-0 w-[244px] shrink-0 flex-col gap-2 overflow-hidden p-3.5" aria-busy={busy}>
      <div className="flex items-center justify-between px-1 pb-1">
        <span className="text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
          Profiles
        </span>
        <button
          type="button"
          disabled={busy}
          onClick={onNewProfile}
          className="ring-focus flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[12px] font-semibold text-ink-dim hover:text-ink disabled:cursor-not-allowed disabled:opacity-50"
        >
          <Plus size={13} weight="bold" /> New
        </button>
      </div>

      <nav aria-label="Profiles" className="scroll-region flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto pr-1">
        {profiles.map((p) => {
          const active = p.id === activeId;
          const updates = p.mods.some((m) => m.update && !m.managed);
          return (
            <button
              key={p.id}
              type="button"
              disabled={busy}
              onClick={() => onSelect(p.id)}
              aria-current={active}
              title={p.name}
              className={`ring-focus flex shrink-0 items-center gap-2.5 rounded-xl px-2.5 py-2.5 text-left text-[14px] transition-colors disabled:cursor-not-allowed disabled:opacity-50 ${
                active
                  ? "border border-white/[0.18] bg-white/[0.13] text-ink"
                  : "text-ink-dim hover:bg-white/[0.06]"
              }`}
            >
              <span className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ background: p.crewColor }} />
              <span className="min-w-0 flex-1 truncate">{p.name}</span>
              {updates && (
                <span
                  className="h-[7px] w-[7px] rounded-full bg-[#ffd23f]"
                  style={{ boxShadow: "0 0 8px #ffd23f" }}
                  title="Update available"
                />
              )}
              <span className="text-[12px] text-ink-faint">{p.mods.filter((m) => !m.managed).length}</span>
            </button>
          );
        })}
      </nav>


      <button
        type="button"
        onClick={onPasteCode}
        className="ring-focus shrink-0 rounded-2xl border border-white/20 p-3.5 text-left transition-transform active:scale-[0.985]"
        style={{
          background:
            "linear-gradient(135deg, rgba(155,123,255,0.28), rgba(91,192,255,0.18))",
        }}
      >
        <div className="flex items-center gap-2 text-[14px] font-semibold text-ink">
          <LinkSimple size={16} weight="bold" /> Paste lobby code
        </div>
        <p className="mt-1 text-[12px] leading-snug text-ink-dim">
          Drop a friend's code to set up the exact mods and versions their lobby needs, in one click.
        </p>
      </button>
    </aside>
  );
}
