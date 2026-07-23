import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { CheckCircle, FolderOpen, GameController, GearSix, Warning } from "@phosphor-icons/react";
import { ensureLoader, inspectGame, loaderStatus, pickFolder, type LoaderStatus } from "../lib/bridge";
import { useModalFocus } from "../lib/useModalFocus";
import type { GameInstall, Runtime } from "../lib/types";

interface SetupModalProps {
  open: boolean;
  detected: GameInstall[];
  profileId: string;
  onFinish: (gamePath?: string, arch?: string, store?: string, runtime?: Runtime) => void;
}

type StatusState =
  | { kind: "idle" }
  | { kind: "loading"; path: string; profileId: string }
  | { kind: "ready"; path: string; profileId: string; value: LoaderStatus }
  | { kind: "missing"; path: string; profileId: string; value: LoaderStatus | null }
  | { kind: "error"; path: string; profileId: string; message: string };

type RequestIdentity = { session: number; path: string; profileId: string };

const LABEL = "mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase";

/** First-run onboarding: pick the Among Us folder (detected or browsed), then optionally install the loader. */
export function SetupModal({ open, detected, profileId, onFinish }: SetupModalProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const [chosen, setChosen] = useState<string | null>(null);
  const [inspected, setInspected] = useState<GameInstall | null>(null);
  const [status, setStatus] = useState<StatusState>({ kind: "idle" });
  const [retry, setRetry] = useState(0);
  const [browsing, setBrowsing] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [message, setMessage] = useState("");

  const sessionRef = useRef(0);
  const wasOpenRef = useRef(false);
  const openRef = useRef(open);
  const profileIdRef = useRef(profileId);
  const chosenRef = useRef(chosen);
  const finishRef = useRef(onFinish);
  const installingRef = useRef(false);
  const browsingRef = useRef(false);
  const statusRequestRef = useRef(0);
  const installRequestRef = useRef(0);

  const selectedInstall =
    detected.find((game) => game.path === chosen) ??
    (inspected?.path === chosen ? inspected : null);
  const selectedInstallRef = useRef<GameInstall | null>(selectedInstall);

  openRef.current = open;
  profileIdRef.current = profileId;
  chosenRef.current = chosen;
  finishRef.current = onFinish;
  selectedInstallRef.current = selectedInstall;
  installingRef.current = installing;

  const requestDismiss = useCallback(() => {
    if (!installingRef.current) finishRef.current(undefined);
  }, []);
  useModalFocus(open, modalRef, requestDismiss);

  useEffect(() => {
    if (open && !wasOpenRef.current) {
      sessionRef.current += 1;
      setChosen(null);
      setInspected(null);
      setStatus({ kind: "idle" });
      setBrowsing(false);
      setInstalling(false);
      setMessage("");
      browsingRef.current = false;
      installingRef.current = false;
    } else if (!open && wasOpenRef.current) {
      sessionRef.current += 1;
      statusRequestRef.current += 1;
      installRequestRef.current += 1;
      browsingRef.current = false;
    }
    wasOpenRef.current = open;
  }, [open]);

  useEffect(() => {
    const path = chosen ?? "";
    const request = ++statusRequestRef.current;
    if (!open || !path) {
      setStatus({ kind: "idle" });
      return;
    }

    const identity: RequestIdentity = { session: sessionRef.current, path, profileId };
    setStatus({ kind: "loading", path, profileId });
    setMessage("");
    loaderStatus(path, profileId)
      .then((value) => {
        if (!requestIsCurrent(identity, request, statusRequestRef, openRef, sessionRef, profileIdRef, chosenRef)) return;
        setStatus(
          value?.current && value.runtimeReady
            ? { kind: "ready", path, profileId, value }
            : { kind: "missing", path, profileId, value },
        );
      })
      .catch((error: unknown) => {
        if (!requestIsCurrent(identity, request, statusRequestRef, openRef, sessionRef, profileIdRef, chosenRef)) return;
        setStatus({ kind: "error", path, profileId, message: error instanceof Error ? error.message : String(error) });
      });
  }, [chosen, open, profileId, retry]);

  const browse = async () => {
    if (browsingRef.current || installingRef.current) return;
    browsingRef.current = true;
    setBrowsing(true);
    setMessage("");
    const session = sessionRef.current;
    const requestProfileId = profileIdRef.current;
    try {
      const path = await pickFolder();
      if (!path || !openRef.current || sessionRef.current !== session || profileIdRef.current !== requestProfileId) return;
      const game = await inspectGame(path);
      if (!openRef.current || sessionRef.current !== session || profileIdRef.current !== requestProfileId) return;
      setInspected(game);
      setChosen(game.path);
    } catch (error) {
      if (openRef.current && sessionRef.current === session && profileIdRef.current === requestProfileId) {
        setMessage(`Folder inspection failed: ${error instanceof Error ? error.message : String(error)}`);
      }
    } finally {
      if (openRef.current && sessionRef.current === session && profileIdRef.current === requestProfileId) {
        browsingRef.current = false;
        setBrowsing(false);
      }
    }
  };

  const install = async () => {
    const path = chosenRef.current;
    const game = selectedInstallRef.current;
    if (!path || installingRef.current) return;

    const identity: RequestIdentity = {
      session: sessionRef.current,
      path,
      profileId: profileIdRef.current,
    };
    const request = ++installRequestRef.current;
    statusRequestRef.current += 1;
    installingRef.current = true;
    setInstalling(true);
    setStatus({ kind: "loading", path, profileId: identity.profileId });
    setMessage("Installing BepInEx… (downloads about 30 MB once)");

    try {
      const warning = await ensureLoader(path, identity.profileId, game?.arch ?? "x86");
      if (!requestIsCurrent(identity, request, installRequestRef, openRef, sessionRef, profileIdRef, chosenRef)) return;

      try {
        const value = await loaderStatus(path, identity.profileId);
        if (!requestIsCurrent(identity, request, installRequestRef, openRef, sessionRef, profileIdRef, chosenRef)) return;
        const ready = !!value && value.current && value.runtimeReady;
        if (ready) {
          setStatus({ kind: "ready", path, profileId: identity.profileId, value });
        } else {
          setStatus({ kind: "missing", path, profileId: identity.profileId, value });
        }
        setMessage(
          warning ??
            (ready
              ? "BepInEx installed."
              : "BepInEx still needs runtime setup. Launch Among Us once without mods, close it, then retry."),
        );
      } catch (error) {
        if (!requestIsCurrent(identity, request, installRequestRef, openRef, sessionRef, profileIdRef, chosenRef)) return;
        const detail = error instanceof Error ? error.message : String(error);
        setStatus({ kind: "error", path, profileId: identity.profileId, message: detail });
        setMessage(`BepInEx installation completed, but verification failed: ${detail}`);
      }
    } catch (error) {
      if (requestIsCurrent(identity, request, installRequestRef, openRef, sessionRef, profileIdRef, chosenRef)) {
        const detail = error instanceof Error ? error.message : String(error);
        setStatus({ kind: "error", path, profileId: identity.profileId, message: detail });
        setMessage(`Install failed: ${detail}`);
      }
    } finally {
      if (
        openRef.current &&
        sessionRef.current === identity.session &&
        installRequestRef.current === request
      ) {
        installingRef.current = false;
        setInstalling(false);
      }
    }
  };

  const visibleStatus: StatusState =
    !chosen
      ? { kind: "idle" }
      : status.kind !== "idle" && status.path === chosen && status.profileId === profileId
        ? status
        : { kind: "loading", path: chosen, profileId };
  const statusBlocksFinish =
    visibleStatus.kind === "loading" || visibleStatus.kind === "error" || visibleStatus.kind === "idle";
  const visibleMessage =
    !chosen || (status.kind !== "idle" && status.path === chosen && status.profileId === profileId)
      ? message
      : "";

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
            ref={modalRef}
            role="dialog"
            aria-modal="true"
            aria-label="Set up Perfect-Sync"
            aria-busy={installing || browsing || visibleStatus.kind === "loading"}
            tabIndex={-1}
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
                        {detected.map((game) => (
                          <button
                            key={game.path}
                            type="button"
                            disabled={browsing}
                            onClick={() => {
                              setInspected(game);
                              setChosen(game.path);
                              setMessage("");
                            }}
                            className="ring-focus glass flex items-center gap-3 rounded-xl px-3.5 py-3 text-left hover:bg-white/10 disabled:opacity-50"
                          >
                            <GameController size={18} className="shrink-0 text-ink-dim" />
                            <div className="min-w-0">
                              <div className="truncate text-[13px] text-ink">{game.path}</div>
                              <div className="text-[12px] text-ink-faint">
                                {game.store} · {game.arch}
                                {game.runtime && game.runtime !== "native" ? ` · ${game.runtime}` : ""}
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
                    onClick={() => void browse()}
                    disabled={browsing}
                    className="ring-focus glass flex w-full items-center justify-center gap-2 rounded-xl px-3 py-3 text-[13px] text-ink-dim hover:text-ink disabled:opacity-50"
                  >
                    <FolderOpen size={16} /> {browsing ? "Inspecting folder…" : "Browse for your Among Us folder…"}
                  </button>
                  {detected.length === 0 && (
                    <p className="mt-2 px-1 text-[12px] text-ink-faint">
                      No installs auto-detected. Browse to the folder that contains “Among Us.exe”.
                    </p>
                  )}
                  {visibleMessage && (
                    <p role="alert" className="mt-2 px-1 text-[12px] text-[#ffb4b4]">
                      {visibleMessage}
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
                      onClick={() => {
                        setChosen(null);
                        setStatus({ kind: "idle" });
                        setMessage("");
                      }}
                      disabled={installing}
                      className="ring-focus shrink-0 rounded-md px-2 py-1 text-[12px] text-ink-faint hover:bg-white/10 hover:text-ink disabled:opacity-50"
                    >
                      Change
                    </button>
                  </div>

                  <span className={`${LABEL} mt-5`}>Mod loader (BepInEx)</span>
                  {visibleStatus.kind === "loading" ? (
                    <div role="status" className="glass rounded-xl px-3.5 py-3 text-[13px] text-ink-faint">
                      {installing ? "Installing and verifying BepInEx…" : "Checking loader status…"}
                    </div>
                  ) : visibleStatus.kind === "ready" ? (
                    <div className="glass flex items-center gap-2 rounded-xl px-3.5 py-3 text-[13px] text-[#aef3d8]">
                      <CheckCircle size={16} weight="fill" /> BepInEx is installed and ready.
                    </div>
                  ) : visibleStatus.kind === "error" ? (
                    <div className="rounded-xl border border-[#ff8a8a]/30 bg-[#ff8a8a]/10 px-3.5 py-3 text-[13px] text-[#ffb4b4]">
                      <div role="alert">Could not check BepInEx: {visibleStatus.message}</div>
                      <button
                        type="button"
                        onClick={() => setRetry((value) => value + 1)}
                        disabled={installing}
                        className="ring-focus mt-3 rounded-lg bg-white/10 px-3 py-2 text-[12.5px] font-semibold text-ink disabled:opacity-50"
                      >
                        Retry status check
                      </button>
                    </div>
                  ) : visibleStatus.kind === "missing" && visibleStatus.value?.current ? (
                    <div
                      className="rounded-xl px-3.5 py-3 text-[13px]"
                      style={{ background: "rgba(255,210,63,0.12)", border: "1px solid rgba(255,210,63,0.32)", color: "#ffe49a" }}
                    >
                      <div className="flex items-center gap-2">
                        <Warning size={16} weight="fill" /> BepInEx files are installed. {visibleStatus.value.runtime} still needs its winhttp override.
                      </div>
                      <button
                        type="button"
                        onClick={() => void install()}
                        disabled={installing}
                        className="ring-focus accent-grad mt-3 flex items-center gap-1.5 rounded-lg px-3 py-2 text-[12.5px] font-semibold text-[#0d0820] disabled:opacity-50"
                      >
                        <GearSix size={14} className={installing ? "animate-spin" : ""} />
                        {installing ? "Checking…" : "Retry runtime setup"}
                      </button>
                    </div>
                  ) : visibleStatus.kind === "missing" ? (
                    <div
                      className="rounded-xl px-3.5 py-3 text-[13px]"
                      style={{ background: "rgba(255,210,63,0.12)", border: "1px solid rgba(255,210,63,0.32)", color: "#ffe49a" }}
                    >
                      <div className="flex items-center gap-2">
                        <Warning size={16} weight="fill" /> BepInEx isn’t set up. Mods won’t load until it is.
                      </div>
                      <button
                        type="button"
                        onClick={() => void install()}
                        disabled={installing}
                        className="ring-focus accent-grad mt-3 flex items-center gap-1.5 rounded-lg px-3 py-2 text-[12.5px] font-semibold text-[#0d0820] disabled:opacity-50"
                      >
                        <GearSix size={14} className={installing ? "animate-spin" : ""} />
                        {installing ? "Installing…" : "Install BepInEx"}
                      </button>
                    </div>
                  ) : (
                    <div className="glass rounded-xl px-3.5 py-3 text-[13px] text-ink-faint">Waiting to check loader status…</div>
                  )}
                  {visibleMessage && (
                    <p aria-live="polite" className="mt-2 px-1 text-[12px] text-ink-dim">
                      {visibleMessage}
                    </p>
                  )}
                </>
              )}
            </div>

            <div className="mt-4 flex items-center justify-between gap-2.5 border-t border-white/10 pt-4">
              <button
                type="button"
                onClick={() => finishRef.current(undefined)}
                disabled={installing}
                className="ring-focus rounded-lg px-2 py-1 text-[13px] text-ink-faint hover:text-ink disabled:opacity-50"
              >
                Skip setup
              </button>
              <button
                type="button"
                disabled={!chosen || installing || statusBlocksFinish}
                onClick={() =>
                  finishRef.current(
                    chosen ?? undefined,
                    selectedInstall?.arch,
                    selectedInstall?.store,
                    selectedInstall?.runtime,
                  )
                }
                className="ring-focus accent-grad rounded-xl px-5 py-2.5 text-[14px] font-bold text-[#0d0820] disabled:opacity-50"
              >
                {visibleStatus.kind === "ready" ? "Finish" : "Finish without loader"}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

function requestIsCurrent(
  identity: RequestIdentity,
  request: number,
  requestRef: { current: number },
  openRef: { current: boolean },
  sessionRef: { current: number },
  profileIdRef: { current: string },
  chosenRef: { current: string | null },
): boolean {
  return (
    openRef.current &&
    sessionRef.current === identity.session &&
    requestRef.current === request &&
    profileIdRef.current === identity.profileId &&
    chosenRef.current === identity.path
  );
}
