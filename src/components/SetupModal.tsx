import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { CheckCircle, FolderOpen, GameController, GearSix, HardDrives, Package, WarningCircle } from "@phosphor-icons/react";
import { inspectGame, listTouSetupOptions, pickFolder } from "../lib/bridge";
import { useModalFocus } from "../lib/useModalFocus";
import type { GameInstall, ModInstallOption, Runtime, Store } from "../lib/types";
import { displayPath } from "../lib/displayPath";

export type SetupSelection =
  | { kind: "bepinex"; applyDoorstopFix: boolean }
  | { kind: "tou"; tag: string; assetName: string };

interface SetupModalProps {
  open: boolean;
  migrationRequired?: boolean;
  detected: GameInstall[];
  activeStoragePath: string;
  defaultStoragePath: string;
  onMoveStorage: (storagePath?: string) => Promise<void>;
  onFinish: (
    gamePath?: string,
    arch?: string,
    store?: string,
    runtime?: Runtime,
    selection?: SetupSelection,
  ) => Promise<boolean>;
  onDismiss: () => Promise<void>;
}


const LABEL = "mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase";

const STORE_CHOICES: Array<{ id: Store; label: string }> = [
  { id: "steam", label: "Steam" },
  { id: "epic", label: "Epic Games" },
  { id: "msstore", label: "Microsoft Store" },
  { id: "itch", label: "itch.io" },
  { id: "manual", label: "Direct executable" },
];

function sameGamePath(left: string, right: string): boolean {
  const normalize = (value: string) =>
    value.replaceAll("\\", "/").replace(/\/+$/u, "").toLowerCase();
  return normalize(left) === normalize(right);
}

/** First-run onboarding: select a read-only Among Us source, then choose the managed profile setup. */
export function SetupModal({
  open,
  migrationRequired = false,
  detected,
  activeStoragePath,
  defaultStoragePath,
  onMoveStorage,
  onFinish,
  onDismiss,
}: SetupModalProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const [chosen, setChosen] = useState<string | null>(null);
  const [inspected, setInspected] = useState<GameInstall | null>(null);
  const [selectedStore, setSelectedStore] = useState<Store | null>(null);
  const [browsing, setBrowsing] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [storagePending, setStoragePending] = useState(false);
  const [storageMessage, setStorageMessage] = useState("");
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
  const chosenRef = useRef(chosen);
  const finishRef = useRef(onFinish);
  const dismissRef = useRef(onDismiss);
  const installingRef = useRef(false);
  const moveStorageRef = useRef(onMoveStorage);
  const storagePendingRef = useRef(false);
  const browsingRef = useRef(false);
  const touOptionsRequestRef = useRef(0);

  const selectedInstall =
    detected.find((game) => !!chosen && sameGamePath(game.path, chosen)) ??
    (inspected && chosen && sameGamePath(inspected.path, chosen) ? inspected : null);
  const effectiveStore = selectedStore ?? selectedInstall?.store ?? "manual";

  openRef.current = open;
  chosenRef.current = chosen;
  finishRef.current = onFinish;
  dismissRef.current = onDismiss;
  moveStorageRef.current = onMoveStorage;
  installingRef.current = installing;

  const requestDismiss = useCallback(() => {
    if (installingRef.current || storagePendingRef.current) return;
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
      setSelectedStore(null);
      setBrowsing(false);
      setInstalling(false);
      setMessage("");
      setStoragePending(false);
      setStorageMessage("");
      setApplyDoorstopFix(false);
      setSetupKind(null);
      setTouOptions([]);
      setTouOptionKey("");
      setTouOptionsLoading(false);
      setTouOptionsError("");
      browsingRef.current = false;
      installingRef.current = false;
      storagePendingRef.current = false;
    } else if (!open && wasOpenRef.current) {
      sessionRef.current += 1;
      touOptionsRequestRef.current += 1;
      browsingRef.current = false;
    }
      storagePendingRef.current = false;
    wasOpenRef.current = open;
  }, [open]);

  useEffect(() => {
    if (
      !open ||
      !chosen ||
      installingRef.current ||
      browsingRef.current ||
      (inspected && sameGamePath(inspected.path, chosen)) ||
      detected.some((game) => sameGamePath(game.path, chosen))
    ) {
      return;
    }
    const replacement = detected.length === 1 ? detected[0] : null;
    setChosen(replacement?.path ?? null);
    setInspected(null);
    setSelectedStore(replacement?.store ?? null);
    setMessage(
      replacement
        ? ""
        : "The selected folder is no longer available. Choose the current Among Us folder.",
    );
  }, [chosen, detected, inspected, open]);


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
    listTouSetupOptions(game.arch, effectiveStore, game.runtime ?? "native")
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
    effectiveStore,
    setupKind,
  ]);

  const browse = async () => {
    if (browsingRef.current || installingRef.current || storagePendingRef.current) return;
    browsingRef.current = true;
    setBrowsing(true);
    setMessage("");
    const session = sessionRef.current;
    try {
      const path = await pickFolder();
      if (!path || !openRef.current || sessionRef.current !== session) return;
      const game = await inspectGame(path);
      if (!openRef.current || sessionRef.current !== session) return;
      setInspected(game);
      setChosen(game.path);
      setSelectedStore(game.store);
    } catch (error) {
      if (openRef.current && sessionRef.current === session) {
        setMessage(`Folder inspection failed: ${error instanceof Error ? error.message : String(error)}`);
      }
    } finally {
      if (openRef.current && sessionRef.current === session) {
        browsingRef.current = false;
        setBrowsing(false);
      }
    }
  };


  const moveStorage = async (restoreDefault = false) => {
    if (storagePendingRef.current || installingRef.current) return;
    storagePendingRef.current = true;
    setStoragePending(true);
    setStorageMessage("");
    const session = sessionRef.current;
    try {
      let path: string | undefined;
      if (!restoreDefault) {
        const selected = await pickFolder("Choose a Perfect Sync storage folder");
        if (!selected || !openRef.current || sessionRef.current !== session) return;
        path = selected;
      }
      await moveStorageRef.current(path);
      if (openRef.current && sessionRef.current === session) {
        setStorageMessage("Storage location updated.");
      }
    } catch (error) {
      if (openRef.current && sessionRef.current === session) {
        setStorageMessage(`Storage move failed: ${error instanceof Error ? error.message : String(error)}`);
      }
    } finally {
      if (openRef.current && sessionRef.current === session) {
        storagePendingRef.current = false;
        setStoragePending(false);
      }
    }
  };

  const selectedTouOption = touOptions.find(
    (option) => `${option.tag}\0${option.assetName}` === touOptionKey,
  );

  const finishSetup = async () => {
    const path = chosenRef.current;
    const game = selectedInstall;
    if (!path || !game || !setupKind || installingRef.current || storagePendingRef.current) return;
    const selection: SetupSelection =
      setupKind === "tou"
        ? {
            kind: "tou",
            tag: selectedTouOption?.tag ?? "",
            assetName: selectedTouOption?.assetName ?? "",
          }
        : { kind: "bepinex", applyDoorstopFix };
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
      const completed = await finishRef.current(game.path, game.arch, effectiveStore, game.runtime, selection);
      if (!completed && openRef.current && sessionRef.current === session) {
        setMessage("Setup canceled. No changes were made.");
      }
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


  const visibleMessage = message;

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
            aria-label="Set up Perfect Sync"
            aria-busy={installing || browsing || storagePending}
            tabIndex={-1}
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex max-h-[90vh] w-[560px] max-w-full flex-col rounded-3xl p-6 max-[600px]:h-[100dvh] max-[600px]:max-h-none max-[600px]:w-full max-[600px]:rounded-none max-[600px]:p-4"
          >
            <h2 className="text-[20px] font-semibold text-ink">
              {migrationRequired ? "Select a fresh Among Us installation" : "Welcome to Perfect Sync"}
            </h2>
            <p className="mt-0.5 text-[13px] text-ink-dim">
              {chosen
                ? "Step 2 of 2: choose storage and your isolated mod setup."
                : "Step 1 of 2: find your fresh Among Us source."}
            </p>
            {migrationRequired && (
              <div
                role="alert"
                className="mt-4 flex items-start gap-2.5 rounded-xl border border-[#ffbf69]/35 bg-[#ffbf69]/10 px-3.5 py-3 text-[12.5px] leading-relaxed text-[#ffd7a0]"
              >
                <WarningCircle size={18} weight="fill" className="mt-0.5 shrink-0" />
                <span>
                  <strong className="block font-semibold text-[#ffe4bd]">
                    v0.1.6 requires a fresh source for the new exact-base workflow.
                  </strong>
                  Verify or reinstall Among Us, then select that untouched folder here. Perfect Sync
                  will preserve your profiles and mods, import a private clean base, and leave the
                  original game folder unchanged.
                </span>
              </div>
            )}

            <div className="scroll-region mt-4 min-h-0 flex-1 overflow-y-auto pr-1">
              {!chosen ? (
                <>
                  {detected.length > 0 && (
                    <>
                      <span className={LABEL}>Detected sources</span>
                      <div className="flex flex-col gap-2">
                        {detected.map((game) => (
                          <button
                            key={game.path}
                            type="button"
                            disabled={browsing}
                            onClick={() => {
                              setInspected(null);
                              setChosen(game.path);
                              setSelectedStore(game.store);
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
                    <FolderOpen size={16} /> {browsing ? "Inspecting source" : "Browse for your Among Us source"}
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
                  <span className={LABEL}>Among Us source</span>
                  <div className="glass flex items-center gap-2 rounded-xl px-3.5 py-3">
                    <GameController size={18} className="shrink-0 text-ink-dim" />
                    <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] text-ink">
                      {displayPath(chosen)}
                    </span>
                    <button
                      type="button"
                      onClick={() => {
                        setChosen(null);
                        setSelectedStore(null);
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
                  <span className={`${LABEL} mt-5`}>Storefront</span>
                  <div
                    className="grid grid-cols-2 gap-2 max-[480px]:grid-cols-1"
                    role="radiogroup"
                    aria-label="Among Us storefront"
                  >
                    {STORE_CHOICES.map((store) => (
                      <button
                        key={store.id}
                        type="button"
                        role="radio"
                        aria-checked={effectiveStore === store.id}
                        disabled={installing}
                        onClick={() => {
                          setSelectedStore(store.id);
                          setMessage("");
                        }}
                        className={`ring-focus rounded-xl border px-3 py-2.5 text-left text-[12.5px] font-semibold transition-colors disabled:opacity-50 ${
                          effectiveStore === store.id
                            ? "border-[#9b7bff]/60 bg-[#9b7bff]/16 text-ink"
                            : "border-white/10 bg-white/[0.04] text-ink-dim hover:bg-white/[0.08] hover:text-ink"
                        }`}
                      >
                        {store.label}
                      </button>
                    ))}
                  </div>
                  <p
                    className={`mt-2 px-1 text-[11.5px] leading-relaxed ${
                      effectiveStore === "manual" ? "text-[#ffd7a0]" : "text-ink-faint"
                    }`}
                  >
                    {effectiveStore === "epic"
                      ? "Epic profiles use the verified EpicGamesStarter authentication helper."
                      : effectiveStore === "manual"
                        ? "Direct launch skips storefront authentication. If this copy came from Epic, select Epic Games."
                        : "This controls package selection and the storefront-specific launch path."}
                  </p>
                  <div className="mt-2 flex items-start gap-2 rounded-xl border border-[#5be3b0]/25 bg-[#5be3b0]/8 px-3.5 py-3 text-[12px] leading-relaxed text-ink-dim">
                    <CheckCircle size={17} weight="fill" className="mt-0.5 shrink-0 text-[#5be3b0]" />
                    <span>
                      {selectedInstall?.sourceClean === false
                        ? "Perfect Sync leaves this folder untouched. Setup can only reuse a compatible private base that was created earlier."
                        : "Perfect Sync leaves this folder untouched. The first setup imports an exact private base, and every profile runs from one disposable isolated workspace."}
                    </span>
                  </div>
                  {selectedInstall?.sourceClean === false && (
                    <div role="alert" className="mt-2 flex items-start gap-2 rounded-xl border border-[#ff8a8a]/30 bg-[#ff8a8a]/10 px-3.5 py-3 text-[12px] leading-relaxed text-[#ffb4b4]">
                      <WarningCircle size={17} weight="fill" className="mt-0.5 shrink-0" />
                      <span>
                        This source contains existing mod-loader files (
                        {selectedInstall.sourceModArtifacts?.slice(0, 4).join(", ") || "unknown artifacts"}
                        {(selectedInstall.sourceModArtifacts?.length ?? 0) > 4 ? ", …" : ""}). Perfect Sync will not remove them or call this a fresh base. Verify or reinstall a clean game first, unless this source already has a compatible private base.
                      </span>
                    </div>
                  )}

                  <span className={`${LABEL} mt-5`}>Managed storage</span>
                  <div className="rounded-xl border border-white/10 bg-white/[0.035] p-3.5">
                    <div className="flex min-w-0 items-center gap-3">
                      <div className="grid h-9 w-9 shrink-0 place-items-center rounded-lg bg-[#9b7bff]/12 text-accent-2">
                        <HardDrives size={18} />
                      </div>
                      <div className="min-w-0 flex-1">
                        <div className="text-[12.5px] font-semibold text-ink">
                          {sameGamePath(activeStoragePath, defaultStoragePath) ? "Local app data" : "Custom location"}
                        </div>
                        <div className="truncate font-mono text-[11.5px] text-ink-faint" title={activeStoragePath}>
                          {displayPath(activeStoragePath)}
                        </div>
                      </div>
                    </div>
                    <p className="mt-2.5 text-[11.5px] leading-relaxed text-ink-faint">
                      The clean game base, isolated workspace, and download cache live here. Profiles and settings stay in AppData.
                    </p>
                    <div className="mt-3 flex flex-wrap gap-2">
                      <button
                        type="button"
                        onClick={() => void moveStorage(false)}
                        disabled={installing || storagePending}
                        className="ring-focus glass rounded-lg px-3 py-2 text-[12px] font-semibold text-ink-dim hover:text-ink disabled:opacity-50"
                      >
                        {storagePending ? "Moving storage…" : "Choose location"}
                      </button>
                      {!sameGamePath(activeStoragePath, defaultStoragePath) && (
                        <button
                          type="button"
                          onClick={() => void moveStorage(true)}
                          disabled={installing || storagePending}
                          className="ring-focus rounded-lg px-3 py-2 text-[12px] text-ink-faint hover:bg-white/10 hover:text-ink disabled:opacity-50"
                        >
                          Restore default
                        </button>
                      )}
                    </div>
                    {storageMessage && (
                      <p
                        role={storageMessage.startsWith("Storage move failed:") ? "alert" : "status"}
                        className={`mt-2 text-[11.5px] leading-relaxed ${
                          storageMessage.startsWith("Storage move failed:") ? "text-[#ffb4b4]" : "text-[#83efc7]"
                        }`}
                      >
                        {storageMessage}
                      </p>
                    )}
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
                      <span className={`${LABEL} mt-5`}>Managed loader</span>
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
                      <div className="glass mt-3 flex items-start gap-2 rounded-xl px-3.5 py-3 text-[12px] leading-relaxed text-ink-dim">
                        <CheckCircle size={16} weight="fill" className="mt-0.5 shrink-0 text-[#5be3b0]" />
                        <span>
                          Continue to import the clean game base, install BepInEx, and verify the private workspace.
                        </span>
                      </div>
                      {visibleMessage && (
                        <p aria-live="polite" className={`mt-2 px-1 text-[12px] ${visibleMessage.startsWith("Setup failed:") ? "text-[#ffb4b4]" : "text-ink-dim"}`}>
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
                disabled={installing || storagePending}
                className="ring-focus rounded-lg px-2 py-1 text-[13px] text-ink-faint hover:text-ink disabled:opacity-50"
              >
                {migrationRequired ? "Do this later" : "Skip setup"}
              </button>
              <button
                type="button"
                disabled={
                  !chosen ||
                  !setupKind ||
                  installing ||
                  storagePending ||
                  (setupKind === "tou" &&
                    (touOptionsLoading || !!touOptionsError || !selectedTouOption))
                }
                onClick={() => void finishSetup()}
                className="ring-focus accent-grad rounded-xl px-5 py-2.5 text-[14px] font-bold text-[#0d0820] disabled:opacity-50"
              >
                {installing
                  ? setupKind === "tou"
                    ? "Installing Town of Us…"
                    : "Preparing workspace…"
                  : setupKind === "tou"
                    ? "Install Town of Us"
                    : "Set up BepInEx"}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
