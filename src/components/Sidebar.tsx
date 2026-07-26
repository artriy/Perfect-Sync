import { Plus } from "@phosphor-icons/react";
import type { Profile } from "../lib/types";

interface SidebarProps {
  profiles: Profile[];
  activeId: string;
  busy: boolean;
  onSelect: (id: string) => void;
  onNewProfile: () => void;
}

export function Sidebar({ profiles, activeId, busy, onSelect, onNewProfile }: SidebarProps) {
  return (
    <aside className="glass-2 flex min-h-0 w-[244px] shrink-0 flex-col gap-2 overflow-hidden p-3.5 max-[720px]:w-full max-[720px]:flex-row max-[720px]:items-center max-[720px]:p-2.5" aria-busy={busy}>
      <div className="flex items-center justify-between px-1 pb-1 max-[720px]:shrink-0 max-[720px]:gap-2 max-[720px]:pb-0">
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

      <nav aria-label="Profiles" className="scroll-region flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto pr-1 max-[720px]:flex-row max-[720px]:overflow-x-auto max-[720px]:overflow-y-hidden max-[720px]:pr-0">
        {profiles.map((p) => {
          const active = p.id === activeId;
          const updates = p.mods.filter((mod) => mod.update && !mod.managed).length;
          const mods = p.mods.filter((mod) => !mod.managed).length;
          return (
            <button
              key={p.id}
              type="button"
              disabled={busy}
              onClick={() => onSelect(p.id)}
              aria-current={active}
              title={p.name}
              className={`ring-focus flex shrink-0 items-center gap-2.5 rounded-xl px-2.5 py-2.5 text-left text-[14px] transition-colors disabled:cursor-not-allowed disabled:opacity-50 max-[720px]:min-w-[10rem] ${
                active
                  ? "border border-white/[0.18] bg-white/[0.13] text-ink"
                  : "text-ink-dim hover:bg-white/[0.06]"
              }`}
            >
              <span className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ background: p.crewColor }} aria-hidden="true" />
              <span className="min-w-0 flex-1 truncate">{p.name}</span>
              {updates > 0 && (
                <span
                  className="rounded-md bg-[rgba(255,210,63,0.14)] px-1.5 py-0.5 text-[11px] font-semibold text-[#ffe49a]"
                  title={`${updates} ${updates === 1 ? "update" : "updates"} available`}
                  aria-label={`${updates} ${updates === 1 ? "update" : "updates"} available`}
                >
                  ↑{updates}
                </span>
              )}
              <span className="text-[12.5px] text-ink-faint" aria-label={`${mods} ${mods === 1 ? "mod" : "mods"}`}>
                {mods}
              </span>
            </button>
          );
        })}
      </nav>

    </aside>
  );
}
