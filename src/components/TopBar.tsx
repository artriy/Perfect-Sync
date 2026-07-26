import { useEffect, useState } from "react";
import perfectSyncLogo from "../assets/perfect-sync-logo.svg";
import { Copy, GearSix, LinkSimple, Minus, Plus, Square, X } from "@phosphor-icons/react";
import {
  onWindowResized,
  winClose,
  winIsMaximized,
  winMinimize,
  winToggleMaximize,
} from "../lib/bridge";

interface TopBarProps {
  onAddMod: () => void;
  onJoinLobby: () => void;
  onOpenSettings: () => void;
}

export function TopBar({ onAddMod, onJoinLobby, onOpenSettings }: TopBarProps) {
  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    let active = true;
    let unlisten = () => {};
    const refresh = () => {
      void winIsMaximized()
        .then((maximized) => {
          if (active) setIsMaximized(maximized);
        })
        .catch(() => {});
    };

    refresh();
    void onWindowResized(refresh)
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      })
      .catch(() => {});

    return () => {
      active = false;
      unlisten();
    };
  }, []);


  const toggleMaximize = async () => {
    await winToggleMaximize();
    setIsMaximized(await winIsMaximized());
  };

  return (
    <header data-tauri-drag-region className="glass-2 flex min-w-0 items-center gap-2 px-4 py-2.5">
      <div data-tauri-drag-region className="flex shrink-0 items-center gap-2">
        <img
          data-tauri-drag-region
          src={perfectSyncLogo}
          alt=""
          aria-hidden="true"
          className="h-7 w-7 shrink-0 rounded-[6px]"
        />
        <div data-tauri-drag-region className="flex items-baseline gap-1.5 font-semibold tracking-tight">
          <span data-tauri-drag-region>Perfect-Sync</span>
          <span data-tauri-drag-region className="font-mono text-[11px] font-medium text-ink-faint max-[600px]:hidden">v0.1.3</span>
        </div>
      </div>

      <div data-tauri-drag-region className="min-w-0 flex-1 self-stretch" />

      <button
        type="button"
        onClick={onJoinLobby}
        className="ring-focus glass flex h-10 shrink-0 items-center gap-1.5 rounded-xl px-3.5 text-[13px] font-semibold text-ink-dim transition-colors hover:text-ink active:scale-[0.97] max-[600px]:w-10 max-[600px]:justify-center max-[600px]:px-0"
      >
        <LinkSimple size={16} weight="bold" aria-hidden="true" />
        <span className="max-[600px]:sr-only">Join lobby</span>
      </button>

      <button
        type="button"
        onClick={onAddMod}
        className="ring-focus glass flex h-10 shrink-0 items-center gap-1.5 rounded-xl px-3.5 text-[13px] font-semibold text-ink transition-colors hover:bg-white/[0.08] active:scale-[0.97] max-[600px]:w-10 max-[600px]:justify-center max-[600px]:px-0"
      >
        <Plus size={15} weight="bold" className="text-accent-2" aria-hidden="true" />
        <span className="max-[600px]:sr-only">Add mod</span>
      </button>

      <button
        type="button"
        aria-label="Settings"
        onClick={onOpenSettings}
        className="ring-focus glass grid h-10 w-10 shrink-0 place-items-center rounded-xl text-ink-dim transition-colors hover:text-ink"
      >
        <GearSix size={17} />
      </button>

      <div className="ml-1 flex shrink-0 items-center gap-1 max-[520px]:ml-0">
        <button
          type="button"
          aria-label="Minimize"
          onClick={() => winMinimize()}
          className="ring-focus grid h-10 w-10 place-items-center rounded-xl text-ink-dim transition-colors hover:bg-white/10 hover:text-ink max-[520px]:h-9 max-[520px]:w-9"
        >
          <Minus size={15} weight="bold" />
        </button>
        <button
          type="button"
          aria-label={isMaximized ? "Restore" : "Maximize"}
          title={isMaximized ? "Restore" : "Maximize"}
          onClick={() => void toggleMaximize().catch(() => {})}
          className="ring-focus grid h-10 w-10 place-items-center rounded-xl text-ink-dim transition-colors hover:bg-white/10 hover:text-ink max-[520px]:h-9 max-[520px]:w-9"
        >
          {isMaximized ? <Copy size={13} weight="bold" /> : <Square size={13} weight="bold" />}
        </button>
        <button
          type="button"
          aria-label="Close"
          onClick={() => winClose()}
          className="ring-focus grid h-10 w-10 place-items-center rounded-xl text-ink-dim transition-colors hover:bg-[#e23b3b] hover:text-white max-[520px]:h-9 max-[520px]:w-9"
        >
          <X size={16} weight="bold" />
        </button>
      </div>
    </header>
  );
}
