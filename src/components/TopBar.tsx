import { useState } from "react";
import { GearSix, MagnifyingGlass, Minus, Plus, Square, X } from "@phosphor-icons/react";
import { extractLobbyCode, winClose, winMinimize, winToggleMaximize } from "../lib/bridge";

interface TopBarProps {
  onAddMod: () => void;
  onPasteCode: (code: string) => void;
  onOpenSettings: () => void;
}

export function TopBar({ onAddMod, onPasteCode, onOpenSettings }: TopBarProps) {
  const [q, setQ] = useState("");

  const submit = () => {
    const code = extractLobbyCode(q);
    if (code) {
      onPasteCode(code);
      setQ("");
    }
  };

  return (
    <header data-tauri-drag-region className="glass-2 flex items-center gap-4 px-4 py-3">
      <div className="font-semibold tracking-tight">Perfect-Sync</div>

      <label className="glass relative flex max-w-[460px] min-w-[200px] flex-1 items-center gap-2 rounded-xl px-3 py-2 text-ink-dim focus-within:text-ink">
        <MagnifyingGlass size={16} className="shrink-0 opacity-70" />
        <input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && submit()}
          placeholder="Search or paste a code…"
          className="w-full min-w-0 bg-transparent text-[13.5px] text-ink placeholder:text-ink-faint focus:outline-none"
          aria-label="Search mods or paste a lobby code"
        />
      </label>

      <div className="flex-1" />

      <button
        type="button"
        onClick={onAddMod}
        className="ring-focus accent-grad flex items-center gap-1.5 rounded-xl px-3.5 py-2 text-[13px] font-semibold text-[#0d0820] transition-transform active:scale-[0.97]"
      >
        <Plus size={15} weight="bold" /> Add mod
      </button>

      <button
        type="button"
        aria-label="Settings"
        onClick={onOpenSettings}
        className="ring-focus glass grid h-[34px] w-[34px] place-items-center rounded-[10px] text-ink-dim transition-colors hover:text-ink"
      >
        <GearSix size={17} />
      </button>

      <div className="ml-1 flex items-center gap-1">
        <button
          type="button"
          aria-label="Minimize"
          onClick={() => winMinimize()}
          className="ring-focus grid h-[34px] w-[34px] place-items-center rounded-[10px] text-ink-dim transition-colors hover:bg-white/10 hover:text-ink"
        >
          <Minus size={15} weight="bold" />
        </button>
        <button
          type="button"
          aria-label="Maximize"
          onClick={() => winToggleMaximize()}
          className="ring-focus grid h-[34px] w-[34px] place-items-center rounded-[10px] text-ink-dim transition-colors hover:bg-white/10 hover:text-ink"
        >
          <Square size={13} weight="bold" />
        </button>
        <button
          type="button"
          aria-label="Close"
          onClick={() => winClose()}
          className="ring-focus grid h-[34px] w-[34px] place-items-center rounded-[10px] text-ink-dim transition-colors hover:bg-[#e23b3b] hover:text-white"
        >
          <X size={16} weight="bold" />
        </button>
      </div>
    </header>
  );
}
