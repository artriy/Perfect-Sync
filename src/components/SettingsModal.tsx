import { useEffect, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  ArrowsClockwise,
  CheckCircle,
  FolderOpen,
  GameController,
  GithubLogo,
  X,
  XCircle,
} from "@phosphor-icons/react";
import { loaderStatus, pickFolder, reinstallLoader, type LoaderStatus } from "../lib/bridge";
import type { Arch, GameInstall, Settings } from "../lib/types";

interface SettingsModalProps {
  open: boolean;
  settings: Settings;
  game: GameInstall | null;
  profileId: string;
  onClose: () => void;
  onSave: (s: Settings) => void;
}

export function SettingsModal({ open, settings, game, profileId, onClose, onSave }: SettingsModalProps) {
  const reduce = useReducedMotion();
  const [token, setToken] = useState(settings.githubToken ?? "");
  const [gamePath, setGamePath] = useState(settings.gamePath ?? "");
  const [arch, setArch] = useState<Arch>(settings.arch ?? "x86");
  const [status, setStatus] = useState<LoaderStatus | null>(null);
  const [working, setWorking] = useState(false);
  const [msg, setMsg] = useState("");

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
            className="glass-strong relative w-[520px] max-w-full rounded-3xl p-6"
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

            <span className="mt-5 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
              Detected game
            </span>
            <div className="glass flex items-center gap-3 rounded-xl px-3.5 py-3">
              <GameController size={20} className="text-ink-dim" />
              {game ? (
                <div className="min-w-0">
                  <div className="truncate text-[13.5px] text-ink">{game.path}</div>
                  <div className="text-[12px] text-ink-faint">
                    {game.store} · {game.arch}
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
            <div className="mt-2 flex items-center gap-2">
              <span className="text-[12px] text-ink-faint">Build:</span>
              {(["x86", "x64"] as Arch[]).map((a) => (
                <button
                  key={a}
                  type="button"
                  onClick={() => setArch(a)}
                  className={`ring-focus rounded-lg px-3 py-1 text-[12.5px] ${
                    arch === a ? "accent-grad text-[#0d0820] font-semibold" : "glass text-ink-dim"
                  }`}
                >
                  {a === "x86" ? "x86 (Steam/Epic/itch)" : "x64 (MS Store)"}
                </button>
              ))}
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
              BepInEx loader
            </span>
            <div className="glass rounded-xl px-3.5 py-3 text-[12.5px]">
              {status ? (
                <div className="flex flex-col gap-1">
                  <StatusRow ok={status.winhttp} label="Doorstop (winhttp.dll)" />
                  <StatusRow ok={status.preloader} label="BepInEx core" />
                  <StatusRow ok={status.current} label="BepInEx up to date (6.0.700, metadata 31)" />
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

            <div className="mt-6 flex justify-end gap-2.5">
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
