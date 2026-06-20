import { useEffect, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  ArrowsClockwise,
  CaretDown,
  CheckCircle,
  FolderOpen,
  GameController,
  GithubLogo,
  Plus,
  TrashSimple,
  X,
  XCircle,
} from "@phosphor-icons/react";
import { loaderStatus, pickFolder, reinstallLoader, type LoaderStatus } from "../lib/bridge";
import { TrustBadge } from "./TrustBadge";
import type { Arch, GameInstall, Settings, Trust } from "../lib/types";

interface SettingsModalProps {
  open: boolean;
  settings: Settings;
  game: GameInstall | null;
  profileId: string;
  onClose: () => void;
  onSave: (s: Settings) => void;
  onAddPersonal: (repo: string, name: string) => void;
  onRemovePersonal: (repo: string) => void;
  onTogglePersonal: (repo: string, enabled: boolean) => void;
  trustOf: (repo: string) => Trust;
}

export function SettingsModal({
  open,
  settings,
  game,
  profileId,
  onClose,
  onSave,
  onAddPersonal,
  onRemovePersonal,
  onTogglePersonal,
  trustOf,
}: SettingsModalProps) {
  const reduce = useReducedMotion();
  const [token, setToken] = useState(settings.githubToken ?? "");
  const [gamePath, setGamePath] = useState(settings.gamePath ?? "");
  const [arch, setArch] = useState<Arch>(settings.arch ?? "x86");
  const [status, setStatus] = useState<LoaderStatus | null>(null);
  const [working, setWorking] = useState(false);
  const [msg, setMsg] = useState("");
  const [personalUrl, setPersonalUrl] = useState("");

  const submitPersonal = () => {
    const m = personalUrl.match(/github\.com\/([^/]+)\/([^/#?]+)/i);
    const repo = m ? `${m[1]}/${m[2]}` : personalUrl.trim();
    if (!repo) return;
    setPersonalUrl("");
    onAddPersonal(repo, m ? m[2] : repo);
  };

  const refreshStatus = (path: string) => {
    if (!path.trim()) {
      setStatus(null);
      return;
    }
    loaderStatus(path.trim(), profileId)
      .then(setStatus)
      .catch(() => setStatus(null));
  };

  useEffect(() => {
    if (!open) return;
    setToken(settings.githubToken ?? "");
    setGamePath(settings.gamePath ?? game?.path ?? "");
    setArch(settings.arch ?? game?.arch ?? "x86");
    refreshStatus(settings.gamePath ?? game?.path ?? "");
  }, [open, settings.githubToken, settings.gamePath, settings.arch, game, profileId]);

  const reinstall = async () => {
    if (!gamePath.trim()) return;
    setWorking(true);
    setMsg("");
    try {
      await reinstallLoader(gamePath.trim(), profileId, arch);
      setMsg("BepInEx reinstalled (latest).");
    } catch (e) {
      setMsg(String(e));
    } finally {
      setWorking(false);
      refreshStatus(gamePath);
    }
  };

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  const save = () =>
    onSave({
      ...settings,
      githubToken: token.trim() || undefined,
      gamePath: gamePath.trim() || game?.path || undefined,
      arch,
    });

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-50 grid place-items-center p-6"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18 }}
        >
          <div
            className="absolute inset-0 bg-[rgba(6,4,18,0.5)]"
            style={{ backdropFilter: "blur(2px)" }}
            onClick={onClose}
          />
          <motion.div
            role="dialog"
            aria-modal="true"
            aria-label="Settings"
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex max-h-[90vh] w-[520px] max-w-full flex-col rounded-3xl p-6"
          >
            <button
              type="button"
              onClick={onClose}
              aria-label="Close"
              className="ring-focus absolute top-4 right-4 grid h-8 w-8 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink"
            >
              <X size={16} weight="bold" />
            </button>

            <h2 className="text-[20px] font-semibold text-ink">Settings</h2>

            <div className="scroll-region -mr-2 min-h-0 flex-1 overflow-y-auto pr-2">
            <span className="mt-3 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
              Detected game
            </span>
            <div className="glass flex items-center gap-3 rounded-xl px-3.5 py-3">
              <GameController size={20} className="text-ink-dim" />
              {game ? (
                <div className="min-w-0">
                  <div className="truncate text-[13.5px] text-ink">{game.path}</div>
                  <div className="text-[12px] text-ink-faint">
                    {game.store} · {game.arch}
                    {game.runtime && game.runtime !== "native" ? ` · ${game.runtime}` : ""}
                  </div>
                </div>
              ) : (
                <div className="text-[13px] text-ink-dim">
                  No Among Us install detected. Open the game once, or set the path manually later.
                </div>
              )}
            </div>

            <span className="mt-5 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
              Game folder (override)
            </span>
            <div className="flex items-center gap-2">
              <label className="glass flex flex-1 items-center gap-2 rounded-xl px-3 py-2.5 text-ink-dim focus-within:text-ink">
                <GameController size={16} className="opacity-75" />
                <input
                  value={gamePath}
                  onChange={(e) => setGamePath(e.target.value)}
                  placeholder="C:\\Program Files (x86)\\Steam\\steamapps\\common\\Among Us"
                  aria-label="Game folder"
                  className="w-full bg-transparent font-mono text-[12.5px] text-ink placeholder:text-ink-faint focus:outline-none"
                />
              </label>
              <button
                type="button"
                onClick={async () => {
                  const p = await pickFolder();
                  if (p) setGamePath(p);
                }}
                className="ring-focus glass flex items-center gap-1.5 rounded-xl px-3 py-2.5 text-[12.5px] text-ink-dim hover:text-ink"
              >
                <FolderOpen size={15} /> Browse
              </button>
            </div>

            <span className="mt-5 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
              GitHub token (optional)
            </span>
            <label className="glass flex items-center gap-2 rounded-xl px-3 py-2.5 text-ink-dim focus-within:text-ink">
              <GithubLogo size={16} className="opacity-75" />
              <input
                value={token}
                onChange={(e) => setToken(e.target.value)}
                type="password"
                placeholder="ghp_… raises the GitHub rate limit"
                aria-label="GitHub token"
                className="w-full bg-transparent font-mono text-[13px] text-ink placeholder:text-ink-faint focus:outline-none"
              />
            </label>
            <p className="mt-2 px-1 text-[12px] text-ink-faint">
              Stored locally. Lets the app fetch more mod releases per hour without hitting the
              anonymous GitHub limit.
            </p>

            <span className="mt-5 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
              Always add to lobbies
            </span>
            <p className="mb-2 px-1 text-[12px] text-ink-faint">
              Your personal must-haves. Added to every lobby code you apply.
            </p>
            <div className="flex flex-col gap-1.5">
              {(settings.personalMods ?? []).map((pm) => {
                const on = pm.enabled !== false;
                return (
                  <div
                    key={pm.repo}
                    className="glass flex items-center gap-2 rounded-lg px-3 py-2 text-[12.5px]"
                  >
                    <button
                      type="button"
                      role="switch"
                      aria-checked={on}
                      aria-label={`${on ? "Disable" : "Enable"} ${pm.name ?? pm.repo}`}
                      onClick={() => onTogglePersonal(pm.repo, !on)}
                      className={`ring-focus relative h-5 w-9 shrink-0 rounded-full transition-colors ${
                        on ? "accent-grad" : "bg-white/15"
                      }`}
                    >
                      <span
                        className={`absolute top-1/2 h-4 w-4 -translate-y-1/2 rounded-full bg-white transition-all ${
                          on ? "left-[18px]" : "left-0.5"
                        }`}
                      />
                    </button>
                    <span className={`min-w-0 flex-1 truncate ${on ? "text-ink" : "text-ink-faint"}`}>
                      {pm.name ?? pm.repo}
                    </span>
                    <TrustBadge trust={trustOf(pm.repo)} compact />
                    <button
                      type="button"
                      onClick={() => onAddPersonal(pm.repo, pm.name ?? pm.repo)}
                      title="Change version"
                      className="ring-focus glass-2 flex shrink-0 items-center gap-1 rounded-md px-2 py-1 font-mono text-[11.5px] text-ink-dim hover:text-ink"
                    >
                      {pm.tag}
                      <CaretDown size={11} weight="bold" />
                    </button>
                    <button
                      type="button"
                      onClick={() => onRemovePersonal(pm.repo)}
                      aria-label={`Remove ${pm.repo}`}
                      className="ring-focus grid h-7 w-7 place-items-center rounded-md text-ink-faint hover:bg-white/10 hover:text-[#ff8a8a]"
                    >
                      <TrashSimple size={14} />
                    </button>
                  </div>
                );
              })}
              {(settings.personalMods ?? []).length === 0 && (
                <p className="px-1 text-[12px] text-ink-faint">None yet.</p>
              )}
            </div>
            <label className="glass mt-2 flex items-center gap-2 rounded-xl px-3 py-2 text-ink-dim focus-within:text-ink">
              <GithubLogo size={15} className="opacity-75" />
              <input
                value={personalUrl}
                onChange={(e) => setPersonalUrl(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && submitPersonal()}
                placeholder="Paste a GitHub repo to always include"
                aria-label="Always-include repo"
                className="w-full min-w-0 bg-transparent text-[12.5px] text-ink placeholder:text-ink-faint focus:outline-none"
              />
              <button
                type="button"
                onClick={submitPersonal}
                className="ring-focus flex shrink-0 items-center gap-1 rounded-lg bg-white/10 px-2.5 py-1 text-[12px] font-semibold text-ink"
              >
                <Plus size={12} weight="bold" /> Add
              </button>
            </label>

            <span className="mt-5 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
              BepInEx loader
            </span>
            <div className="glass rounded-xl px-3.5 py-3 text-[12.5px]">
              {status ? (
                <div className="flex flex-col gap-1">
                  <StatusRow ok={status.winhttp} label="Doorstop (winhttp.dll)" />
                  <StatusRow ok={status.preloader} label="BepInEx core" />
                  <StatusRow
                    ok={status.current}
                    label={
                      status.current && status.installedVersion
                        ? `BepInEx installed (${status.installedVersion})`
                        : "BepInEx loader installed"
                    }
                  />
                  <StatusRow ok={status.dotnet} label=".NET runtime" />
                  <StatusRow ok={status.steamAppid} label="Steam launch fix" />
                  <div className="mt-1 text-ink-faint">
                    plugins: {status.profilePlugins} in profile · {status.gamePlugins} synced to game
                  </div>
                </div>
              ) : (
                <div className="text-ink-faint">Set your game folder above to check the loader.</div>
              )}
              <button
                type="button"
                onClick={reinstall}
                disabled={working || !gamePath.trim()}
                className="ring-focus glass-2 mt-3 flex items-center gap-1.5 rounded-lg px-3 py-2 text-[12.5px] text-ink-dim hover:text-ink disabled:opacity-50"
              >
                <ArrowsClockwise size={14} className={working ? "animate-spin" : ""} />
                {working ? "Reinstalling BepInEx…" : "Reinstall BepInEx (latest)"}
              </button>
              {msg && <p className="mt-2 text-[12px] text-ink-dim">{msg}</p>}
            </div>

            </div>

            <div className="mt-4 flex justify-end gap-2.5 border-t border-white/10 pt-4">
              <button
                type="button"
                onClick={onClose}
                className="ring-focus glass rounded-xl px-4 py-2.5 text-[14px] text-ink"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={save}
                className="ring-focus accent-grad rounded-xl px-5 py-2.5 text-[14px] font-bold text-[#0d0820]"
              >
                Save
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

function StatusRow({ ok, label }: { ok: boolean; label: string }) {
  return (
    <div className="flex items-center gap-2">
      {ok ? (
        <CheckCircle size={14} weight="fill" className="text-[#5be3b0]" />
      ) : (
        <XCircle size={14} weight="fill" className="text-[#ff8a8a]" />
      )}
      <span className={ok ? "text-ink-dim" : "text-[#ffb4b4]"}>{label}</span>
    </div>
  );
}
