import { useEffect, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { CheckCircle, FolderOpen, GameController, GearSix, Warning } from "@phosphor-icons/react";
import { ensureLoader, loaderStatus, pickFolder, type LoaderStatus } from "../lib/bridge";
import type { GameInstall } from "../lib/types";

interface SetupModalProps {
  open: boolean;
  detected: GameInstall[];
  profileId: string;
  onFinish: (gamePath?: string, arch?: string, store?: string) => void;
}

const LABEL = "mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase";

/** First-run onboarding: pick the Among Us folder (detected or browsed), then
 * optionally install the BepInEx loader. Skippable at every step. */
export function SetupModal({ open, detected, profileId, onFinish }: SetupModalProps) {
  const reduce = useReducedMotion();
  const [chosen, setChosen] = useState<string | null>(null);
  const [status, setStatus] = useState<LoaderStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [msg, setMsg] = useState("");

  useEffect(() => {
    if (!open) {
      setChosen(null);
      setStatus(null);
      setMsg("");
    }
  }, [open]);

  useEffect(() => {
    if (!chosen) {
      setStatus(null);
      return;
    }
    setChecking(true);
    setMsg("");
    loaderStatus(chosen, profileId)
      .then(setStatus)
      .catch(() => setStatus(null))
      .finally(() => setChecking(false));
  }, [chosen, profileId]);

  const archOf = (path: string) => detected.find((d) => d.path === path)?.arch ?? "x86";
  const storeOf = (path: string) => detected.find((d) => d.path === path)?.store;

  const browse = async () => {
    const p = await pickFolder();
    if (p) setChosen(p);
  };

  const install = async () => {
    if (!chosen) return;
    setInstalling(true);
    setMsg("Installing BepInEx… (downloads ~30 MB once)");
    try {
      await ensureLoader(chosen, profileId, archOf(chosen));
      const s = await loaderStatus(chosen, profileId);
      setStatus(s);
      setMsg(s?.current ? "BepInEx installed." : "BepInEx still not detected. Try again, or skip for now.");
    } catch (e) {
      setMsg(`Install failed: ${e}`);
    } finally {
      setInstalling(false);
    }
  };

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-[55] grid place-items-center p-6"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18 }}
        >
          <div className="absolute inset-0 bg-[rgba(6,4,18,0.6)]" style={{ backdropFilter: "blur(3px)" }} />

          <motion.div
            role="dialog"
            aria-modal="true"
            aria-label="Set up Perfect-Sync"
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex max-h-[90vh] w-[560px] max-w-full flex-col rounded-3xl p-6"
          >
            <h2 className="text-[20px] font-semibold text-ink">Welcome to Perfect-Sync</h2>
            <p className="mt-0.5 text-[13px] text-ink-dim">
              {chosen ? "Step 2 of 2 — set up the mod loader." : "Step 1 of 2 — find your Among Us install."}
            </p>

            <div className="scroll-region mt-4 min-h-0 flex-1 overflow-y-auto pr-1">
              {!chosen ? (
                <>
                  {detected.length > 0 && (
                    <>
                      <span className={LABEL}>Detected installs</span>
                      <div className="flex flex-col gap-2">
                        {detected.map((d) => (
                          <button
                            key={d.path}
                            type="button"
                            onClick={() => setChosen(d.path)}
                            className="ring-focus glass flex items-center gap-3 rounded-xl px-3.5 py-3 text-left hover:bg-white/10"
                          >
                            <GameController size={18} className="shrink-0 text-ink-dim" />
                            <div className="min-w-0">
                              <div className="truncate text-[13px] text-ink">{d.path}</div>
                              <div className="text-[12px] text-ink-faint">
                                {d.store} · {d.arch}
                                {d.runtime && d.runtime !== "native" ? ` · ${d.runtime}` : ""}
                              </div>
                            </div>
                          </button>
                        ))}
                      </div>
                      <span className={`${LABEL} mt-5`}>Or pick manually</span>
                    </>
                  )}
                  <button
                    type="button"
                    onClick={browse}
                    className="ring-focus glass flex w-full items-center justify-center gap-2 rounded-xl px-3 py-3 text-[13px] text-ink-dim hover:text-ink"
                  >
                    <FolderOpen size={16} /> Browse for your Among Us folder…
                  </button>
                  {detected.length === 0 && (
                    <p className="mt-2 px-1 text-[12px] text-ink-faint">
                      No installs auto-detected. Browse to the folder that contains "Among Us.exe".
                    </p>
                  )}
                </>
              ) : (
                <>
                  <span className={LABEL}>Among Us folder</span>
                  <div className="glass flex items-center gap-2 rounded-xl px-3.5 py-3">
                    <GameController size={18} className="shrink-0 text-ink-dim" />
                    <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] text-ink">{chosen}</span>
                    <button
                      type="button"
                      onClick={() => setChosen(null)}
                      className="ring-focus shrink-0 rounded-md px-2 py-1 text-[12px] text-ink-faint hover:bg-white/10 hover:text-ink"
                    >
                      Change
                    </button>
                  </div>

                  <span className={`${LABEL} mt-5`}>Mod loader (BepInEx)</span>
                  {checking ? (
                    <div className="glass rounded-xl px-3.5 py-3 text-[13px] text-ink-faint">Checking…</div>
                  ) : status?.current ? (
                    <div className="glass flex items-center gap-2 rounded-xl px-3.5 py-3 text-[13px] text-[#aef3d8]">
                      <CheckCircle size={16} weight="fill" /> BepInEx is installed and ready.
                    </div>
                  ) : (
                    <div
                      className="rounded-xl px-3.5 py-3 text-[13px]"
                      style={{ background: "rgba(255,210,63,0.12)", border: "1px solid rgba(255,210,63,0.32)", color: "#ffe49a" }}
                    >
                      <div className="flex items-center gap-2">
                        <Warning size={16} weight="fill" /> BepInEx isn't set up. Mods won't load until it is.
                      </div>
                      <button
                        type="button"
                        onClick={install}
                        disabled={installing}
                        className="ring-focus accent-grad mt-3 flex items-center gap-1.5 rounded-lg px-3 py-2 text-[12.5px] font-semibold text-[#0d0820] disabled:opacity-50"
                      >
                        <GearSix size={14} className={installing ? "animate-spin" : ""} />
                        {installing ? "Installing…" : "Install BepInEx"}
                      </button>
                    </div>
                  )}
                  {msg && <p className="mt-2 px-1 text-[12px] text-ink-dim">{msg}</p>}
                </>
              )}
            </div>

            <div className="mt-4 flex items-center justify-between gap-2.5 border-t border-white/10 pt-4">
              <button
                type="button"
                onClick={() => onFinish(undefined)}
                className="ring-focus rounded-lg px-2 py-1 text-[13px] text-ink-faint hover:text-ink"
              >
                Skip setup
              </button>
              <button
                type="button"
                disabled={!chosen}
                onClick={() => onFinish(chosen ?? undefined, chosen ? archOf(chosen) : undefined, chosen ? storeOf(chosen) : undefined)}
                className="ring-focus accent-grad rounded-xl px-5 py-2.5 text-[14px] font-bold text-[#0d0820] disabled:opacity-50"
              >
                {chosen && !status?.current ? "Finish without loader" : "Finish"}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
