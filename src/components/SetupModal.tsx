import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { CheckCircle, FolderOpen, GameController, GearSix, Package, Warning } from "@phosphor-icons/react";
import {
  inspectGame,
  listTouSetupOptions,
  loaderStatus,
  pickFolder,
  type LoaderStatus,
} from "../lib/bridge";
import { useModalFocus } from "../lib/useModalFocus";
import type { GameInstall, ModInstallOption, Runtime } from "../lib/types";
import { displayPath } from "../lib/displayPath";

export type SetupSelection =
  | { kind: "bepinex"; applyDoorstopFix: boolean }
  | { kind: "tou"; tag: string; assetName: string };

interface SetupModalProps {
  open: boolean;
  detected: GameInstall[];
  profileId: string;
  onFinish: (
    gamePath?: string,
    arch?: string,
    store?: string,
    runtime?: Runtime,
    selection?: SetupSelection,
  ) => Promise<void>;
  onInstallLoader: (
    gamePath: string,
    profileId: string,
    arch: string,
    applyDoorstopFix: boolean,
  ) => Promise<string | null>;
  onDismiss: () => Promise<void>;
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
export function SetupModal({ open, detected, profileId, onFinish, onDismiss, onInstallLoader }: SetupModalProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const [chosen, setChosen] = useState<string | null>(null);
  const [inspected, setInspected] = useState<GameInstall | null>(null);
  const [status, setStatus] = useState<StatusState>({ kind: "idle" });
  const [retry, setRetry] = useState(0);
  const [browsing, setBrowsing] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [message, setMessage] = useState("");
  const [applyDoorstopFix, setApplyDoorstopFix] = useState(false);
  const [setupKind, setSetupKind] = useState<SetupSelection["kind"] | null>(null);
  const [touOptions, setTouOptions] = useState<ModInstallOption[]>([]);
  const [touOptionKey, setTouOptionKey] = useState("");
  const [touOptionsLoading, setTouOptionsLoading] = useState(false);
  const [touOptionsError, setTouOptionsError] = useState("");

  const sessionRef = useRef(0);
  const wasOpenRef = useRef(false);
  const openRef = useRef(open);
  const profileIdRef = useRef(profileId);
  const chosenRef = useRef(chosen);
  const finishRef = useRef(onFinish);
  const dismissRef = useRef(onDismiss);
  const installingRef = useRef(false);
  const browsingRef = useRef(false);
  const applyDoorstopFixRef = useRef(false);
  const statusRequestRef = useRef(0);
  const installRequestRef = useRef(0);
  const touOptionsRequestRef = useRef(0);

  const selectedInstall =
    detected.find((game) => game.path === chosen) ??
    (inspected?.path === chosen ? inspected : null);
  const selectedInstallRef = useRef<GameInstall | null>(selectedInstall);

  openRef.current = open;
  profileIdRef.current = profileId;
  chosenRef.current = chosen;
  finishRef.current = onFinish;
  dismissRef.current = onDismiss;
  selectedInstallRef.current = selectedInstall;
  installingRef.current = installing;
  applyDoorstopFixRef.current = applyDoorstopFix;

  const requestDismiss = useCallback(() => {
    if (installingRef.current) return;
    void dismissRef.current().catch((error: unknown) => {
      if (openRef.current) {
        setMessage(`Could not close setup: ${error instanceof Error ? error.message : String(error)}`);
      }
    });
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
      setApplyDoorstopFix(false);
      setSetupKind(null);
      setTouOptions([]);
      setTouOptionKey("");
      setTouOptionsLoading(false);
      setTouOptionsError("");
      applyDoorstopFixRef.current = false;
      browsingRef.current = false;
      installingRef.current = false;
    } else if (!open && wasOpenRef.current) {
      sessionRef.current += 1;
      statusRequestRef.current += 1;
      installRequestRef.current += 1;
      touOptionsRequestRef.current += 1;
      browsingRef.current = false;
    }
    wasOpenRef.current = open;
  }, [open]);

  useEffect(() => {
    const path = chosen ?? "";
    const request = ++statusRequestRef.current;
    if (!open || !path || setupKind !== "bepinex") {
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
          value?.current &&
            value.runtimeReady &&
            (!applyDoorstopFix || value.doorstopFix)
            ? { kind: "ready", path, profileId, value }
            : { kind: "missing", path, profileId, value },
        );
      })
      .catch((error: unknown) => {
        if (!requestIsCurrent(identity, request, statusRequestRef, openRef, sessionRef, profileIdRef, chosenRef)) return;
        setStatus({ kind: "error", path, profileId, message: error instanceof Error ? error.message : String(error) });
      });
  }, [applyDoorstopFix, chosen, open, profileId, retry, setupKind]);

  useEffect(() => {
    const request = ++touOptionsRequestRef.current;
    const game = selectedInstall;
    if (!open || setupKind !== "tou" || !game) {
      setTouOptions([]);
      setTouOptionKey("");
      setTouOptionsLoading(false);
      setTouOptionsError("");
      return;
    }
    setTouOptions([]);
    setTouOptionKey("");
    setTouOptionsLoading(true);
    setTouOptionsError("");
    listTouSetupOptions(game.arch, game.store, game.runtime ?? "native")
      .then((options) => {
        if (
          request !== touOptionsRequestRef.current ||
          !openRef.current ||
          chosenRef.current !== game.path
        ) {
          return;
        }
        setTouOptions(options);
        setTouOptionKey(options[0] ? `${options[0].tag}\0${options[0].assetName}` : "");
        if (options.length === 0) {
          setTouOptionsError("No Town of Us release supports this game installation.");
        }
      })
      .catch((error: unknown) => {
        if (
          request === touOptionsRequestRef.current &&
          openRef.current &&
          chosenRef.current === game.path
        ) {
          setTouOptionsError(error instanceof Error ? error.message : String(error));
        }
      })
      .finally(() => {
        if (
          request === touOptionsRequestRef.current &&
          openRef.current &&
          chosenRef.current === game.path
        ) {
          setTouOptionsLoading(false);
        }
      });
  }, [
    chosen,
    open,
    selectedInstall?.arch,
    selectedInstall?.path,
    selectedInstall?.runtime,
    selectedInstall?.store,
    setupKind,
  ]);

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
    setMessage(
      applyDoorstopFixRef.current
        ? "Installing BepInEx and the compatibility fix. Downloads about 30 MB once."
        : "Installing BepInEx. Downloads about 30 MB once.",
    );

    try {
      const warning = await onInstallLoader(
        path,
        identity.profileId,
        game?.arch ?? "x86",
        applyDoorstopFixRef.current,
      );
      if (!requestIsCurrent(identity, request, installRequestRef, openRef, sessionRef, profileIdRef, chosenRef)) return;

      try {
        const value = await loaderStatus(path, identity.profileId);
        if (!requestIsCurrent(identity, request, installRequestRef, openRef, sessionRef, profileIdRef, chosenRef)) return;
        const ready =
          !!value &&
          value.current &&
          value.runtimeReady &&
          (!applyDoorstopFixRef.current || value.doorstopFix);
        if (ready) {
          setStatus({ kind: "ready", path, profileId: identity.profileId, value });
        } else {
          setStatus({ kind: "missing", path, profileId: identity.profileId, value });
        }
        setMessage(
          warning ??
            (ready
              ? applyDoorstopFixRef.current
                ? "BepInEx and the compatibility fix are installed."
                : "BepInEx installed."
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

  const selectedTouOption = touOptions.find(
    (option) => `${option.tag}\0${option.assetName}` === touOptionKey,
  );

  const finishSetup = async () => {
    const path = chosenRef.current;
    const game = selectedInstallRef.current;
    if (!path || !game || !setupKind || installingRef.current) return;
    const selection: SetupSelection =
      setupKind === "tou"
        ? {
            kind: "tou",
            tag: selectedTouOption?.tag ?? "",
            assetName: selectedTouOption?.assetName ?? "",
          }
        : { kind: "bepinex", applyDoorstopFix: applyDoorstopFixRef.current };
    if (selection.kind === "tou" && (!selection.tag || !selection.assetName)) return;

    const session = sessionRef.current;
    installingRef.current = true;
    setInstalling(true);
    setMessage(
      selection.kind === "tou"
        ? `Installing Town of Us ${selection.tag} and its complete BepInEx package…`
        : "Saving setup…",
    );
    try {
      await finishRef.current(game.path, game.arch, game.store, game.runtime, selection);
    } catch (error) {
      if (openRef.current && sessionRef.current === session) {
        setMessage(`Setup failed: ${error instanceof Error ? error.message : String(error)}`);
      }
    } finally {
      if (openRef.current && sessionRef.current === session) {
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
    setupKind === "bepinex" &&
    (visibleStatus.kind === "loading" ||
      visibleStatus.kind === "error" ||
      visibleStatus.kind === "idle");
  const visibleMessage =
    setupKind === "tou"
      ? message
      : !chosen || (status.kind !== "idle" && status.path === chosen && status.profileId === profileId)
        ? message
        : "";

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-[55] grid place-items-center p-6 max-[600px]:p-0"
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
            className="glass-strong relative flex max-h-[90vh] w-[560px] max-w-full flex-col rounded-3xl p-6 max-[600px]:h-[100dvh] max-[600px]:max-h-none max-[600px]:w-full max-[600px]:rounded-none max-[600px]:p-4"
          >
            <h2 className="text-[20px] font-semibold text-ink">Welcome to Perfect-Sync</h2>
            <p className="mt-0.5 text-[13px] text-ink-dim">
              {chosen ? "Step 2 of 2: choose your mod setup." : "Step 1 of 2: find your Among Us install."}
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
                              <div className="truncate text-[13px] text-ink">{displayPath(game.path)}</div>
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
                    <FolderOpen size={16} /> {browsing ? "Inspecting folder" : "Browse for your Among Us folder"}
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
                    <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] text-ink">
                      {displayPath(chosen)}
                    </span>
                    <button
                      type="button"
                      onClick={() => {
                        setChosen(null);
                        setStatus({ kind: "idle" });
                        setMessage("");
                        setSetupKind(null);
                        setTouOptions([]);
                        setTouOptionKey("");
                        setTouOptionsError("");
                      }}
                      disabled={installing}
                      className="ring-focus shrink-0 rounded-md px-2 py-1 text-[12px] text-ink-faint hover:bg-white/10 hover:text-ink disabled:opacity-50"
                    >
                      Change
                    </button>
                  </div>

                  <span className={`${LABEL} mt-5`}>Choose your setup</span>
                  <div className="grid grid-cols-2 gap-2 max-[480px]:grid-cols-1" role="radiogroup" aria-label="Mod setup">
                    <button
                      type="button"
                      role="radio"
                      aria-checked={setupKind === "tou"}
                      disabled={installing}
                      onClick={() => {
                        setSetupKind("tou");
                        setMessage("");
                      }}
                      className={`ring-focus rounded-xl border p-3 text-left transition-colors disabled:opacity-50 ${
                        setupKind === "tou"
                          ? "border-[#9b7bff]/60 bg-[#9b7bff]/16"
                          : "border-white/10 bg-white/[0.04] hover:bg-white/[0.08]"
                      }`}
                    >
                      <span className="flex items-center gap-2 text-[13px] font-semibold text-ink">
                        <Package size={16} className="text-accent-2" />
                        Town of Us — Mira
                      </span>
                      <span className="mt-1 block text-[11px] leading-relaxed text-ink-faint">
                        Complete release ZIP, including its matching BepInEx, dependencies, configs, and cosmetics.
                      </span>
                    </button>
                    <button
                      type="button"
                      role="radio"
                      aria-checked={setupKind === "bepinex"}
                      disabled={installing}
                      onClick={() => {
                        setSetupKind("bepinex");
                        setMessage("");
                      }}
                      className={`ring-focus rounded-xl border p-3 text-left transition-colors disabled:opacity-50 ${
                        setupKind === "bepinex"
                          ? "border-[#9b7bff]/60 bg-[#9b7bff]/16"
                          : "border-white/10 bg-white/[0.04] hover:bg-white/[0.08]"
                      }`}
                    >
                      <span className="flex items-center gap-2 text-[13px] font-semibold text-ink">
                        <GearSix size={16} className="text-accent-2" />
                        BepInEx only
                      </span>
                      <span className="mt-1 block text-[11px] leading-relaxed text-ink-faint">
                        Set up the loader now and add mods later.
                      </span>
                    </button>
                  </div>

                  {setupKind === "bepinex" ? (
                    <>
                  <span className={`${LABEL} mt-5`}>Mod loader (BepInEx)</span>
                  <label className="mb-2 flex cursor-pointer items-start gap-2 px-1 text-[12px] text-ink-dim">
                    <input
                      type="checkbox"
                      checked={applyDoorstopFix}
                      onChange={(event) => setApplyDoorstopFix(event.target.checked)}
                      disabled={installing}
                      className="mt-0.5 h-4 w-4 shrink-0 accent-[#9b7bff]"
                    />
                    <span>
                      Apply optional UnityDoorstop 4.5.1 compatibility fix
                      <span className="block text-[11px] text-ink-faint">
                        Off by default. Enable only if the standard BepInEx loader has startup problems.
                      </span>
                    </span>
                  </label>
                  {visibleStatus.kind === "loading" ? (
                    <div role="status" className="glass rounded-xl px-3.5 py-3 text-[13px] text-ink-faint">
                      {installing ? "Installing and verifying BepInEx" : "Checking loader status"}
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
                  ) : visibleStatus.kind === "missing" &&
                    visibleStatus.value?.current &&
                    visibleStatus.value.runtimeReady &&
                    applyDoorstopFix &&
                    !visibleStatus.value.doorstopFix ? (
                    <div
                      className="rounded-xl px-3.5 py-3 text-[13px]"
                      style={{ background: "rgba(255,210,63,0.12)", border: "1px solid rgba(255,210,63,0.32)", color: "#ffe49a" }}
                    >
                      <div className="flex items-center gap-2">
                        <Warning size={16} weight="fill" /> BepInEx is ready. The compatibility fix has not been applied.
                      </div>
                      <button
                        type="button"
                        onClick={() => void install()}
                        disabled={installing}
                        className="ring-focus accent-grad mt-3 flex items-center gap-1.5 rounded-lg px-3 py-2 text-[12.5px] font-semibold text-[#0d0820] disabled:opacity-50"
                      >
                        <GearSix size={14} className={installing ? "animate-spin" : ""} />
                        {installing ? "Applying" : "Apply compatibility fix"}
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
                        {installing ? "Checking" : "Retry runtime setup"}
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
                        {installing ? "Installing" : "Install BepInEx"}
                      </button>
                    </div>
                  ) : (
                    <div className="glass rounded-xl px-3.5 py-3 text-[13px] text-ink-faint">Waiting to check loader status</div>
                  )}
                  {visibleMessage && (
                    <p aria-live="polite" className="mt-2 px-1 text-[12px] text-ink-dim">
                      {visibleMessage}
                    </p>
                  )}
                    </>
                  ) : setupKind === "tou" ? (
                    <>
                      <span className={`${LABEL} mt-5`}>Town of Us version</span>
                      {touOptionsLoading ? (
                        <div role="status" className="glass rounded-xl px-3.5 py-3 text-[13px] text-ink-faint">
                          Loading compatible releases…
                        </div>
                      ) : touOptionsError ? (
                        <div role="alert" className="rounded-xl border border-[#ff8a8a]/30 bg-[#ff8a8a]/10 px-3.5 py-3 text-[13px] text-[#ffb4b4]">
                          {touOptionsError}
                        </div>
                      ) : (
                        <label className="glass block rounded-xl px-3.5 py-3">
                          <span className="sr-only">Town of Us version</span>
                          <select
                            value={touOptionKey}
                            disabled={installing || touOptions.length === 0}
                            onChange={(event) => setTouOptionKey(event.target.value)}
                            className="ring-focus w-full bg-transparent text-[13px] font-semibold text-ink focus:outline-none disabled:opacity-50"
                          >
                            {touOptions.map((option, index) => (
                              <option
                                key={`${option.tag}\0${option.assetName}`}
                                value={`${option.tag}\0${option.assetName}`}
                                className="bg-[#171225] text-ink"
                              >
                                {option.tag}
                                {index === 0 ? " — Latest" : ""}
                                {` · ${(option.size / (1024 * 1024)).toFixed(1)} MB`}
                              </option>
                            ))}
                          </select>
                          {selectedTouOption && (
                            <span className="mt-1.5 block truncate font-mono text-[10.5px] text-ink-faint" title={selectedTouOption.assetName}>
                              {selectedTouOption.assetName}
                            </span>
                          )}
                        </label>
                      )}

                      <div className="mt-2 rounded-xl border border-[#9b7bff]/20 bg-[#9b7bff]/8 px-3.5 py-3 text-[11.5px] leading-relaxed text-ink-dim">
                        This ZIP supplies its matching BepInEx and fixed UnityDoorstop, plus MiraAPI, Reactor, Mini.RegionInstall, configs, and cosmetics. Those components stay owned by Town of Us while it is enabled.
                      </div>

                      {visibleMessage && (
                        <p aria-live="polite" className={`mt-2 px-1 text-[12px] ${visibleMessage.startsWith("Setup failed:") ? "text-[#ffb4b4]" : "text-ink-dim"}`}>
                          {visibleMessage}
                        </p>
                      )}
                    </>
                  ) : (
                    <div className="mt-3 rounded-xl border border-white/10 bg-white/[0.03] px-3.5 py-3 text-[12px] text-ink-faint">
                      Choose Town of Us or BepInEx only to continue.
                    </div>
                  )}
                </>
              )}
            </div>

            <div className="mt-4 flex items-center justify-between gap-2.5 border-t border-white/10 pt-4 max-[420px]:flex-wrap">
              <button
                type="button"
                onClick={requestDismiss}
                disabled={installing}
                className="ring-focus rounded-lg px-2 py-1 text-[13px] text-ink-faint hover:text-ink disabled:opacity-50"
              >
                Skip setup
              </button>
              <button
                type="button"
                disabled={
                  !chosen ||
                  !setupKind ||
                  installing ||
                  statusBlocksFinish ||
                  (setupKind === "tou" &&
                    (touOptionsLoading || !!touOptionsError || !selectedTouOption))
                }
                onClick={() => void finishSetup()}
                className="ring-focus accent-grad rounded-xl px-5 py-2.5 text-[14px] font-bold text-[#0d0820] disabled:opacity-50"
              >
                {installing
                  ? setupKind === "tou"
                    ? "Installing Town of Us…"
                    : "Saving…"
                  : setupKind === "tou"
                    ? "Install Town of Us"
                    : visibleStatus.kind === "ready"
                      ? "Finish"
                      : "Finish without loader"}
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
