import { useEffect, useState } from "react";
import { Copy, GearSix, LinkSimple, Minus, Plus, Square, X } from "@phosphor-icons/react";
import {
  extractLobbyCode,
  onWindowResized,
  winClose,
  winIsMaximized,
  winMinimize,
  winToggleMaximize,
} from "../lib/bridge";

interface TopBarProps {
  onAddMod: () => void;
  onPasteCode: (code: string) => void;
  onOpenSettings: () => void;
}

export function TopBar({ onAddMod, onPasteCode, onOpenSettings }: TopBarProps) {
  const [q, setQ] = useState("");
  const [inputError, setInputError] = useState("");
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

  const submit = () => {
    if (!q.trim()) {
      setInputError("Paste a lobby code to continue.");
      return;
    }
    const code = extractLobbyCode(q);
    if (!code) {
      setInputError("That text does not contain a valid PERFECT lobby code.");
      return;
    }
    onPasteCode(code);
    setQ("");
    setInputError("");
  };

  const toggleMaximize = async () => {
    await winToggleMaximize();
    setIsMaximized(await winIsMaximized());
  };

  return (
    <header data-tauri-drag-region className="glass-2 flex min-w-0 items-center gap-3 px-4 py-3">
      <div data-tauri-drag-region className="flex shrink-0 items-baseline gap-1.5 font-semibold tracking-tight">
        <span data-tauri-drag-region>Perfect-Sync</span>
        <span data-tauri-drag-region className="font-mono text-[9.5px] font-medium text-ink-faint">v0.1.1</span>
      </div>

      <div className="relative min-w-[180px] max-w-[460px] flex-1">
        <form
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
          className="glass flex min-w-0 items-center gap-2 rounded-xl px-3 py-2 text-ink-dim focus-within:text-ink focus-within:ring-2 focus-within:ring-[#9b7bff]/70"
        >
          <LinkSimple size={16} className="shrink-0 opacity-70" aria-hidden="true" />
          <input
            value={q}
            maxLength={4096}
            onChange={(event) => {
              setQ(event.target.value);
              if (inputError) setInputError("");
            }}
            placeholder="Paste a lobby code or link…"
            className="w-full min-w-0 bg-transparent text-[13.5px] text-ink placeholder:text-ink-faint focus:outline-none"
            aria-label="Lobby code or link"
            aria-invalid={inputError ? "true" : undefined}
            aria-describedby={inputError ? "lobby-code-error" : undefined}
          />
          <button
            type="submit"
            className="ring-focus shrink-0 rounded-md px-1.5 py-0.5 text-[12px] font-semibold text-accent-2 hover:text-ink"
          >
            Apply
          </button>
        </form>
        {inputError && (
          <p
            id="lobby-code-error"
            role="alert"
            className="glass-strong absolute top-full left-1 z-50 mt-1 max-w-[calc(100vw-2rem)] rounded-lg px-2.5 py-1.5 text-[12px] text-[#ffabab]"
          >
            {inputError}
          </p>
        )}
      </div>

      <div data-tauri-drag-region className="min-w-0 flex-1 self-stretch" />

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
          aria-label={isMaximized ? "Restore" : "Maximize"}
          title={isMaximized ? "Restore" : "Maximize"}
          onClick={() => void toggleMaximize().catch(() => {})}
          className="ring-focus grid h-[34px] w-[34px] place-items-center rounded-[10px] text-ink-dim transition-colors hover:bg-white/10 hover:text-ink"
        >
          {isMaximized ? <Copy size={13} weight="bold" /> : <Square size={13} weight="bold" />}
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
