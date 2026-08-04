import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  ArrowsClockwise,
  CaretDown,
  CheckCircle,
  FolderOpen,
  FileCode,
  FileArrowDown,
  GameController,
  GithubLogo,
  HardDrives,
  Plus,
  TrashSimple,
  X,
  XCircle,
} from "@phosphor-icons/react";
import {
  inspectGame,
  loaderStatus,
  pickFolder,
  pickLocalDll,
  reinstallLoader,
  type LoaderStatus,
} from "../lib/bridge";
import { useModalFocus } from "../lib/useModalFocus";
import { TrustBadge } from "./TrustBadge";
import { ReleasePicker } from "./ReleasePicker";
import type { GameInstall, GameInstance, GithubTokenAction, Settings, Store, Trust } from "../lib/types";
import { displayPath } from "../lib/displayPath";

interface SettingsModalProps {
  open: boolean;
  settings: Settings;
  profileId: string;
  profileGameInstanceId?: string;
  profileUsesTou: boolean;
  onClose: () => void;
  onSave: (settings: Settings, tokenAction: GithubTokenAction) => Promise<void>;
  onRunSetup: () => void;
  onMoveStorage: (storagePath?: string) => Promise<void>;
  onSaveErrorLog: () => Promise<void>;
  trustOf: (repo: string) => Trust;
}

type TokenIntent = GithubTokenAction["kind"];
type LoaderView =
  | { kind: "idle" }
  | { kind: "loading"; path: string; profileId: string }
  | { kind: "ready"; path: string; profileId: string; value: LoaderStatus }
  | { kind: "missing"; path: string; profileId: string; value: LoaderStatus | null }
  | { kind: "error"; path: string; profileId: string; message: string };

type RequestIdentity = { session: number; path: string; profileId: string };

const MAX_INSTANCE_NAME = 64;

function normalizedSourcePath(path: string): string {
  return path.trim().replaceAll("\\", "/").replace(/\/+$/u, "").toLocaleLowerCase();
}

function refreshedInstance(instance: GameInstance, game: GameInstall): GameInstance {
  const sameSource = normalizedSourcePath(instance.path) === normalizedSourcePath(game.path);
  return {
    ...instance,
    path: game.path,
    arch: game.arch,
    store: game.store,
    runtime: game.runtime ?? "native",
    build: game.build ?? (sameSource ? instance.build : undefined),
    writable: game.writable ?? (sameSource ? instance.writable : undefined),
    sourceClean: game.sourceClean ?? (sameSource ? instance.sourceClean : undefined),
    sourceModArtifacts:
      game.sourceModArtifacts ?? (sameSource ? instance.sourceModArtifacts : undefined),
    sourceFingerprint:
      game.sourceFingerprint ?? (sameSource ? instance.sourceFingerprint : undefined),
    sourceFileCount:
      game.sourceFileCount ?? (sameSource ? instance.sourceFileCount : undefined),
    sourceByteCount:
      game.sourceByteCount ?? (sameSource ? instance.sourceByteCount : undefined),
  };
}

export function SettingsModal({
  open,
  settings,
  profileId,
  profileGameInstanceId,
  profileUsesTou,
  onClose,
  onSave,
  onRunSetup,
  onMoveStorage,
  onSaveErrorLog,
  trustOf,
}: SettingsModalProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const tokenInputRef = useRef<HTMLInputElement>(null);
  const [token, setToken] = useState("");
  const [tokenIntent, setTokenIntent] = useState<TokenIntent>("unchanged");
  const [instances, setInstances] = useState<GameInstance[]>(settings.gameInstances ?? []);
  const [personalMods, setPersonalMods] = useState(settings.personalMods ?? []);
  const [personalLocalMods, setPersonalLocalMods] = useState(settings.personalLocalMods ?? []);
  const [personalPicker, setPersonalPicker] = useState<{
    repo: string;
    name: string;
    currentVersion?: string;
  } | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loaderView, setLoaderView] = useState<LoaderView>({ kind: "idle" });
  const [loaderRetry, setLoaderRetry] = useState(0);
  const [folderPending, setFolderPending] = useState(false);
  const [reinstalling, setReinstalling] = useState(false);
  const [saving, setSaving] = useState(false);
  const [storagePending, setStoragePending] = useState(false);
  const [errorLogSaving, setErrorLogSaving] = useState(false);
  const [draftError, setDraftError] = useState("");
  const [sourceMessage, setSourceMessage] = useState("");
  const [personalError, setPersonalError] = useState("");
  const [loaderNotice, setLoaderNotice] = useState<{ path: string; profileId: string; text: string } | null>(null);
  const [applyDoorstopFix, setApplyDoorstopFix] = useState(false);
  const [personalUrl, setPersonalUrl] = useState("");

  const sessionRef = useRef(0);
  const wasOpenRef = useRef(false);
  const openRef = useRef(open);
  const profileIdRef = useRef(profileId);
  const closeRef = useRef(onClose);
  const latestOpenDataRef = useRef({ settings, profileGameInstanceId });
  const loaderRequestRef = useRef(0);
  const installRequestRef = useRef(0);
  const reinstallPendingRef = useRef(false);
  const folderPendingRef = useRef(false);
  const savePendingRef = useRef(false);
  const storagePendingRef = useRef(false);
  const errorLogSavingRef = useRef(false);

  const selected = instances.find((instance) => instance.id === selectedId) ?? null;
  const selectedRef = useRef<GameInstance | null>(selected);

  openRef.current = open;
  profileIdRef.current = profileId;
  closeRef.current = onClose;
  latestOpenDataRef.current = { settings, profileGameInstanceId };
  selectedRef.current = selected;

  const hasPendingWork = folderPending || reinstalling || saving || storagePending || errorLogSaving;
  const canDismissRef = useRef(!hasPendingWork);
  canDismissRef.current = !hasPendingWork;

  const requestClose = useCallback(() => {
    if (
      canDismissRef.current &&
      !folderPendingRef.current &&
      !reinstallPendingRef.current &&
      !savePendingRef.current
      && !storagePendingRef.current
      && !errorLogSavingRef.current
    ) closeRef.current();
  }, []);
  useModalFocus(open, modalRef, requestClose);

  useEffect(() => {
    if (open && !wasOpenRef.current) {
      sessionRef.current += 1;
      const opening = latestOpenDataRef.current;
      const next = opening.settings.gameInstances ?? [];
      setToken("");
      setTokenIntent("unchanged");
      setInstances(next);
      setPersonalMods(opening.settings.personalMods ?? []);
      setPersonalLocalMods(opening.settings.personalLocalMods ?? []);
      setPersonalPicker(null);
      setSelectedId(
        next.some((instance) => instance.id === opening.profileGameInstanceId)
          ? (opening.profileGameInstanceId ?? null)
          : (next[0]?.id ?? null),
      );
      setLoaderView({ kind: "idle" });
      setFolderPending(false);
      setReinstalling(false);
      setSaving(false);
      setStoragePending(false);
      setErrorLogSaving(false);
      setDraftError("");
      setSourceMessage("");
      setPersonalError("");
      setLoaderNotice(null);
      setApplyDoorstopFix(false);
      setPersonalUrl("");
      folderPendingRef.current = false;
      reinstallPendingRef.current = false;
      savePendingRef.current = false;
      storagePendingRef.current = false;
      errorLogSavingRef.current = false;
    } else if (!open && wasOpenRef.current) {
      sessionRef.current += 1;
      loaderRequestRef.current += 1;
      installRequestRef.current += 1;
      folderPendingRef.current = false;
      reinstallPendingRef.current = false;
      savePendingRef.current = false;
      storagePendingRef.current = false;
      errorLogSavingRef.current = false;
    }
    wasOpenRef.current = open;
  }, [open]);

  useEffect(() => {
    const path = selected?.path ?? "";
    const request = ++loaderRequestRef.current;
    if (!open || !path) {
      setLoaderView({ kind: "idle" });
      return;
    }

    const identity: RequestIdentity = { session: sessionRef.current, path, profileId };
    setLoaderView({ kind: "loading", path, profileId });
    loaderStatus(path, profileId)
      .then((value) => {
        if (!isCurrent(identity, request, loaderRequestRef, openRef, sessionRef, profileIdRef, selectedRef)) return;
        setLoaderView(
          value?.current &&
            value.runtimeReady &&
            (!applyDoorstopFix || value.doorstopFix)
            ? { kind: "ready", path, profileId, value }
            : { kind: "missing", path, profileId, value },
        );
      })
      .catch((error: unknown) => {
        if (!isCurrent(identity, request, loaderRequestRef, openRef, sessionRef, profileIdRef, selectedRef)) return;
        setLoaderView({ kind: "error", path, profileId, message: errorMessage(error) });
      });
  }, [applyDoorstopFix, loaderRetry, open, profileId, selected?.path]);

  const beginFolderWork = () => {
    if (folderPendingRef.current) return false;
    folderPendingRef.current = true;
    setFolderPending(true);
    setDraftError("");
    setSourceMessage("");
    return true;
  };

  const endFolderWork = (session: number) => {
    if (!openRef.current || sessionRef.current !== session) return;
    folderPendingRef.current = false;
    setFolderPending(false);
  };

  const changeStorage = async (restoreDefault = false) => {
    if (hasPendingWork || storagePendingRef.current) return;
    const session = sessionRef.current;
    const requestProfileId = profileIdRef.current;
    let path: string | undefined;
    if (!restoreDefault) {
      const selectedPath = await pickFolder("Choose a Perfect Sync storage folder");
      if (!selectedPath || !sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef)) return;
      path = selectedPath;
    }
    storagePendingRef.current = true;
    setStoragePending(true);
    setDraftError("");
    try {
      await onMoveStorage(path);
    } catch (error) {
      if (sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef)) {
        setDraftError(actionableSettingsError(error));
      }
    } finally {
      if (sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef)) {
        storagePendingRef.current = false;
        setStoragePending(false);
      }
    }
  };

  const saveErrorLog = async () => {
    if (hasPendingWork || errorLogSavingRef.current) return;
    const session = sessionRef.current;
    const requestProfileId = profileIdRef.current;
    errorLogSavingRef.current = true;
    setErrorLogSaving(true);
    setDraftError("");
    try {
      await onSaveErrorLog();
    } catch (error) {
      if (sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef)) {
        setDraftError(errorMessage(error));
      }
    } finally {
      if (sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef)) {
        errorLogSavingRef.current = false;
        setErrorLogSaving(false);
      }
    }
  };

  const addInstance = async () => {
    if (!beginFolderWork()) return;
    const session = sessionRef.current;
    const requestProfileId = profileIdRef.current;
    try {
      const path = await pickFolder();
      if (!path || !sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef)) return;
      const game = await inspectGame(path);
      if (!sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef)) return;

      const existing = instances.find(
        (instance) => normalizedSourcePath(instance.path) === normalizedSourcePath(game.path),
      );
      if (existing) {
        setInstances((current) =>
          current.map((instance) =>
            instance.id === existing.id ? refreshedInstance(instance, game) : instance,
          ),
        );
        setSelectedId(existing.id);
        setSourceMessage("Original source checked. Save Settings to keep its refreshed source record.");
      } else {
        const id = `game-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
        const baseName = STORE_NAMES[game.store];
        setInstances((current) => [
          ...current,
          {
            ...game,
            id,
            name: uniqueInstanceName(baseName, current),
            runtime: game.runtime ?? "native",
          },
        ]);
        setSelectedId(id);
        setSourceMessage("Original source inspected. Save Settings to add its source record.");
      }
    } catch (error) {
      if (sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef)) {
        setDraftError(actionableSettingsError(error));
      }
    } finally {
      endFolderWork(session);
    }
  };

  const changeSelectedFolder = async () => {
    const target = selectedRef.current;
    if (!target || !beginFolderWork()) return;
    const session = sessionRef.current;
    const requestProfileId = profileIdRef.current;
    const targetId = target.id;
    const originalPath = target.path;
    try {
      const path = await pickFolder();
      if (!path || !sameSelectedSession(session, requestProfileId, targetId, originalPath, openRef, sessionRef, profileIdRef, selectedRef)) return;
      const game = await inspectGame(path);
      if (!sameSelectedSession(session, requestProfileId, targetId, originalPath, openRef, sessionRef, profileIdRef, selectedRef)) return;
      const duplicate = instances.find(
        (instance) =>
          instance.id !== targetId &&
          normalizedSourcePath(instance.path) === normalizedSourcePath(game.path),
      );
      if (duplicate) {
        throw new Error(`That original source is already saved as “${duplicate.name}”.`);
      }
      setInstances((current) =>
        current.map((instance) =>
          instance.id === targetId ? refreshedInstance(instance, game) : instance,
        ),
      );
      setSourceMessage(
        normalizedSourcePath(originalPath) === normalizedSourcePath(game.path)
          ? "Original source checked. Save Settings to keep its refreshed source record."
          : "Original source changed. Save Settings to bind this direct instance to the new source record.",
      );
    } catch (error) {
      if (sameSelectedSession(session, requestProfileId, targetId, originalPath, openRef, sessionRef, profileIdRef, selectedRef)) {
        setDraftError(actionableSettingsError(error));
      }
    } finally {
      endFolderWork(session);
    }
  };

  const refreshSelectedSource = async () => {
    const target = selectedRef.current;
    if (!target || !beginFolderWork()) return;
    const session = sessionRef.current;
    const requestProfileId = profileIdRef.current;
    const targetId = target.id;
    const originalPath = target.path;
    try {
      const game = await inspectGame(originalPath);
      if (!sameSelectedSession(session, requestProfileId, targetId, originalPath, openRef, sessionRef, profileIdRef, selectedRef)) return;
      setInstances((current) =>
        current.map((instance) =>
          instance.id === targetId ? refreshedInstance(instance, game) : instance,
        ),
      );
      setSourceMessage("Original source checked. Save Settings to keep its refreshed source record.");
    } catch (error) {
      if (sameSelectedSession(session, requestProfileId, targetId, originalPath, openRef, sessionRef, profileIdRef, selectedRef)) {
        setDraftError(actionableSettingsError(error));
      }
    } finally {
      endFolderWork(session);
    }
  };


  const removeInstance = (id: string) => {
    if (hasPendingWork) return;
    if (id === profileGameInstanceId) {
      setDraftError("This profile uses that instance. Select it and use Change to move the profile to another folder.");
      return;
    }
    const next = instances.filter((instance) => instance.id !== id);
    setInstances(next);
    if (selectedId === id) setSelectedId(next[0]?.id ?? null);
    setDraftError("");
  };

  const reinstall = async (useLatestLoader = false) => {
    const target = selectedRef.current;
    if (!target || reinstallPendingRef.current) return;
    const identity: RequestIdentity = {
      session: sessionRef.current,
      path: target.path,
      profileId: profileIdRef.current,
    };
    const includeFix = applyDoorstopFix;
    const sourceName = useLatestLoader ? "latest experimental BepInEx build" : "BepInEx be.753";
    const request = ++installRequestRef.current;
    reinstallPendingRef.current = true;
    setReinstalling(true);
    setLoaderNotice({
      path: identity.path,
      profileId: identity.profileId,
      text: `Installing ${sourceName}${includeFix ? " with the compatibility fix" : ""}.`,
    });
    try {
      const warning = await reinstallLoader(
        target.path,
        identity.profileId,
        target.arch,
        includeFix,
        useLatestLoader,
      );
      if (!isCurrent(identity, request, installRequestRef, openRef, sessionRef, profileIdRef, selectedRef)) return;
      setLoaderNotice({
        path: identity.path,
        profileId: identity.profileId,
        text:
          warning ??
          `${sourceName} installed${includeFix ? " with the compatibility fix" : " without the compatibility fix"}.`,
      });
      setLoaderRetry((value) => value + 1);
    } catch (error) {
      if (isCurrent(identity, request, installRequestRef, openRef, sessionRef, profileIdRef, selectedRef)) {
        setLoaderNotice({ path: identity.path, profileId: identity.profileId, text: `Reinstall failed: ${errorMessage(error)}` });
      }
    } finally {
      if (
        openRef.current &&
        sessionRef.current === identity.session &&
        installRequestRef.current === request
      ) {
        reinstallPendingRef.current = false;
        setReinstalling(false);
      }
    }
  };


  const submitPersonal = () => {
    const match = personalUrl.match(/github\.com\/([^/]+)\/([^/#?]+)/i);
    const repo = (match ? `${match[1]}/${match[2]}` : personalUrl).trim().replace(/\.git$/i, "");
    if (!/^[^/\s]+\/[^/\s]+$/.test(repo)) {
      setPersonalError("Enter an owner/repository name or GitHub repository URL.");
      return;
    }
    setPersonalError("");
    setPersonalPicker({
      repo,
      name: match ? match[2].replace(/\.git$/i, "") : (repo.split("/").at(-1) ?? repo),
    });
  };

  const addLocalLobbyDefault = async () => {
    if (hasPendingWork) return;
    try {
      const path = await pickLocalDll();
      if (!path) return;
      if (personalLocalMods.some((candidate) => candidate.path.toLowerCase() === path.toLowerCase())) {
        setPersonalError("That local DLL is already a lobby default.");
        return;
      }
      const fileName = path.split(/[\\/]/u).at(-1) ?? "Local DLL";
      setPersonalLocalMods((current) => [
        ...current,
        { path, name: fileName.replace(/\.dll$/iu, ""), enabled: true },
      ]);
      setPersonalError("");
    } catch (error) {
      setPersonalError(errorMessage(error));
    }
  };

  const startTokenReplacement = () => {
    setToken("");
    setTokenIntent("set");
    window.requestAnimationFrame(() => tokenInputRef.current?.focus());
  };

  const save = async () => {
    if (
      savePendingRef.current ||
      folderPendingRef.current ||
      reinstallPendingRef.current ||
      hasPendingWork
    ) return;
    const validation = validateInstances(instances);
    if (validation) {
      setDraftError(validation);
      return;
    }
    const trimmedToken = token.trim();
    if (tokenIntent === "set" && !trimmedToken) {
      setDraftError("Enter the replacement token, or choose Keep current token.");
      tokenInputRef.current?.focus();
      return;
    }

    const session = sessionRef.current;
    const requestProfileId = profileIdRef.current;
    const tokenAction: GithubTokenAction =
      tokenIntent === "set"
        ? { kind: "set", token: trimmedToken }
        : tokenIntent === "clear"
          ? { kind: "clear" }
          : { kind: "unchanged" };
    savePendingRef.current = true;
    setSaving(true);
    setDraftError("");
    try {
      await onSave(
        {
          ...settings,
          gameInstances: instances.map((instance) => ({ ...instance, name: instance.name.trim() })),
          personalMods,
          personalLocalMods,
        },
        tokenAction,
      );
      if (sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef)) {
        setToken("");
        closeRef.current();
      }
    } catch (error) {
      if (sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef)) {
        setDraftError(actionableSettingsError(error));
      }
    } finally {
      if (openRef.current && sessionRef.current === session && savePendingRef.current) {
        savePendingRef.current = false;
        setSaving(false);
      }
    }
  };

  const selectedNameError = selected ? instanceNameError(selected, instances) : "";
  const hasInstanceDrafts = JSON.stringify(instances) !== JSON.stringify(settings.gameInstances ?? []);
  const hasPersonalDrafts = JSON.stringify(personalMods) !== JSON.stringify(settings.personalMods ?? []);
  const hasPersonalLocalDrafts =
    JSON.stringify(personalLocalMods) !== JSON.stringify(settings.personalLocalMods ?? []);
  const hasDraftChanges =
    hasInstanceDrafts || hasPersonalDrafts || hasPersonalLocalDrafts || tokenIntent !== "unchanged";
  const visibleLoaderView: LoaderView =
    !selected
      ? { kind: "idle" }
      : loaderView.kind !== "idle" &&
          loaderView.path === selected.path &&
          loaderView.profileId === profileId
        ? loaderView
        : { kind: "loading", path: selected.path, profileId };
  const visibleLoaderNotice =
    loaderNotice && selected?.path === loaderNotice.path && profileId === loaderNotice.profileId
      ? loaderNotice.text
      : "";

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-50 grid place-items-center p-6 max-[600px]:p-0"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18 }}
          onClick={(event) => {
            if (event.target === event.currentTarget) requestClose();
          }}
        >
          <div
            aria-hidden="true"
            className="pointer-events-none absolute inset-0 bg-[rgba(6,4,18,0.5)]"
            style={{ backdropFilter: "blur(2px)" }}
          />
          <motion.div
            ref={modalRef}
            role="dialog"
            aria-modal="true"
            aria-label="Settings"
            aria-busy={hasPendingWork}
            tabIndex={-1}
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex max-h-[90vh] w-[520px] max-w-full flex-col rounded-3xl p-6 max-[600px]:h-[100dvh] max-[600px]:max-h-none max-[600px]:w-full max-[600px]:rounded-none max-[600px]:p-4"
          >
            <button
              type="button"
              onClick={requestClose}
              disabled={hasPendingWork}
              aria-label="Close settings"
              className="ring-focus absolute top-4 right-4 grid h-9 w-9 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink disabled:opacity-40"
            >
              <X size={16} weight="bold" />
            </button>

            <h2 className="text-[20px] font-semibold text-ink">Settings</h2>

            <div className="scroll-region min-h-0 flex-1 overflow-x-hidden overflow-y-auto pr-2">
              <div className="mt-5 mb-2 text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
                Managed storage
              </div>
              <div className="rounded-xl border border-white/9 bg-white/[0.035] p-3.5">
                <div className="flex min-w-0 items-center gap-3">
                  <div className="grid h-9 w-9 shrink-0 place-items-center rounded-lg bg-[#9b7bff]/12 text-accent-2">
                    <HardDrives size={18} />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="text-[12.5px] font-semibold text-ink">
                      {settings.storagePath ? "Custom location" : "Local app data"}
                    </div>
                    <div
                      className="truncate font-mono text-[11.5px] text-ink-faint"
                      title={settings.activeStoragePath}
                    >
                      {displayPath(settings.activeStoragePath)}
                    </div>
                  </div>
                </div>
                <p className="mt-2.5 text-[11.5px] leading-relaxed text-ink-faint">
                  Direct profile instances and downloaded packages use this folder. Source records, profiles, and settings remain in AppData.
                </p>
                {settings.storageWarning && (
                  <p role="alert" className="mt-2 text-[11.5px] leading-relaxed text-[#ffb4b4]">
                    {actionableSettingsError(settings.storageWarning)}
                  </p>
                )}
                <div className="mt-3 flex flex-wrap gap-2">
                  <button
                    type="button"
                    onClick={() => void changeStorage(false)}
                    disabled={hasPendingWork}
                    className="ring-focus glass rounded-lg px-3 py-2 text-[12px] font-semibold text-ink-dim hover:text-ink disabled:opacity-50"
                  >
                    {storagePending ? "Moving storage…" : "Change location"}
                  </button>
                  {settings.storagePath && (
                    <button
                      type="button"
                      onClick={() => void changeStorage(true)}
                      disabled={hasPendingWork}
                      className="ring-focus rounded-lg px-3 py-2 text-[12px] text-ink-faint hover:bg-white/10 hover:text-ink disabled:opacity-50"
                    >
                      Restore default
                    </button>
                  )}
                </div>
              </div>

              <div className="mt-5 mb-2 flex items-center justify-between">
                <span className="text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
                  Among Us sources
                </span>
                <button
                  type="button"
                  onClick={() => void addInstance()}
                  disabled={hasPendingWork}
                  className="ring-focus flex items-center gap-1 rounded-lg px-2 py-1 text-[11.5px] font-semibold text-ink-dim hover:bg-white/10 hover:text-ink disabled:opacity-50"
                >
                  <Plus size={12} weight="bold" /> {folderPending ? "Inspecting" : "Add source"}
                </button>
              </div>
              <div className="flex flex-col gap-1.5">
                {instances.map((instance) => {
                  const active = instance.id === selectedId;
                  const inUse = instance.id === profileGameInstanceId;
                  return (
                    <div
                      key={instance.id}
                      className={`flex items-center gap-1 rounded-xl border transition ${
                        active ? "border-accent/40 bg-accent/10" : "border-white/8 bg-white/[0.035]"
                      }`}
                    >
                      <button
                        type="button"
                        onClick={() => setSelectedId(instance.id)}
                        disabled={reinstalling || saving || folderPending}
                        aria-label={`Select ${instance.name || "unnamed instance"}`}
                        className="ring-focus flex min-w-0 flex-1 items-center gap-2.5 rounded-xl px-3 py-2.5 text-left disabled:opacity-50"
                      >
                        <GameController size={17} className={active ? "text-accent-2" : "text-ink-faint"} />
                        <span className="min-w-0 flex-1">
                          <span className="flex items-center gap-2">
                            <span className="truncate text-[13px] font-semibold text-ink">
                              {instance.name || "Unnamed instance"}
                            </span>
                            <span className="font-mono text-[12px] text-ink-faint">
                              {instance.store} · {instance.arch} · {instance.runtime}
                            </span>
                          </span>
                          <span className="block truncate font-mono text-[12px] text-ink-faint">
                            {displayPath(instance.path)}
                          </span>
                        </span>
                      </button>
                      <button
                        type="button"
                        onClick={() => removeInstance(instance.id)}
                        disabled={hasPendingWork || inUse}
                        aria-label={
                          inUse
                            ? `${instance.name || "Unnamed instance"} is used by this profile; use Change to move it`
                            : `Remove ${instance.name || "unnamed instance"}`
                        }
                        title={inUse ? "Used by this profile. Select it and use Change to move it." : undefined}
                        className="ring-focus mr-2 grid h-7 w-7 shrink-0 place-items-center rounded-md text-ink-faint hover:bg-white/10 hover:text-[#ff8a8a] disabled:opacity-40"
                      >
                        <TrashSimple size={14} />
                      </button>
                    </div>
                  );
                })}
                {instances.length === 0 && (
                  <div className="glass rounded-xl px-3 py-4 text-center text-[12px] text-ink-faint">
                    Add each Steam, Epic, itch.io, or Microsoft Store source used by your profiles. Perfect Sync never modifies these folders.
                  </div>
                )}
              </div>
              {selected && (
                <div className="mt-2 grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2">
                  <label className="glass flex min-w-0 items-center gap-2 rounded-xl px-3 py-2.5 text-ink-dim focus-within:text-ink">
                    <GameController size={16} className="opacity-75" />
                    <input
                      value={selected.name}
                      maxLength={MAX_INSTANCE_NAME}
                      disabled={saving || reinstalling}
                      onChange={(event) => {
                        const name = event.target.value;
                        setInstances((current) =>
                          current.map((instance) => (instance.id === selected.id ? { ...instance, name } : instance)),
                        );
                        setDraftError("");
                      }}
                      placeholder="Instance name"
                      aria-label="Instance name"
                      aria-invalid={!!selectedNameError}
                      aria-describedby={selectedNameError ? "instance-name-error" : undefined}
                      className="min-w-0 flex-1 bg-transparent text-[12.5px] text-ink placeholder:text-ink-faint focus:outline-none disabled:opacity-50"
                    />
                  </label>
                  {selectedNameError && (
                    <p id="instance-name-error" className="px-1 text-[12px] text-[#ffb4b4]">
                      {selectedNameError}
                    </p>
                  )}
                  <div className="flex min-w-0 items-center gap-2">
                    <div className="glass flex min-w-0 flex-1 items-center gap-2 rounded-xl px-3 py-2.5 text-ink-dim">
                      <FolderOpen size={16} className="shrink-0 opacity-75" />
                      <span className="truncate font-mono text-[12px] text-ink">
                        {displayPath(selected.path)}
                      </span>
                    </div>
                    <button
                      type="button"
                      onClick={() => void refreshSelectedSource()}
                      disabled={hasPendingWork}
                      className="ring-focus glass flex items-center gap-1.5 rounded-xl px-3 py-2.5 text-[12.5px] text-ink-dim hover:text-ink disabled:opacity-50"
                    >
                      <ArrowsClockwise size={14} className={folderPending ? "animate-spin" : undefined} />
                      Check original
                    </button>
                    <button
                      type="button"
                      onClick={() => void changeSelectedFolder()}
                      disabled={hasPendingWork}
                      className="ring-focus glass rounded-xl px-3 py-2.5 text-[12.5px] text-ink-dim hover:text-ink disabled:opacity-50"
                    >
                      Change
                    </button>
                  </div>
                  {sourceMessage && (
                    <p role="status" className="px-1 text-[11.5px] leading-relaxed text-[#aef3d8]">
                      {sourceMessage}
                    </p>
                  )}
                  {selected.sourceFingerprint ? (
                    <div className="rounded-xl border border-white/9 bg-white/[0.035] px-3.5 py-3 text-[11.5px] leading-relaxed text-ink-faint">
                      <p className="font-semibold text-ink-dim">Recorded original source</p>
                      <p className="mt-1 font-mono">
                        {selected.build ? `Build ${selected.build} · ` : ""}
                        {selected.sourceFileCount?.toLocaleString() ?? "Unknown"} files
                        {selected.sourceByteCount !== undefined
                          ? ` · ${selected.sourceByteCount.toLocaleString()} bytes`
                          : ""}
                      </p>
                      <p className="mt-0.5 truncate font-mono" title={selected.sourceFingerprint}>
                        {selected.sourceFingerprint}
                      </p>
                    </div>
                  ) : (
                    <div role="alert" className="rounded-xl border border-[#ffd23f]/25 bg-[#ffd23f]/8 px-3.5 py-3 text-[12px] leading-relaxed text-[#ffe49a]">
                      This source has no complete source record. Check the original folder, then save Settings before building or repairing a direct instance.
                    </div>
                  )}
                  {selected.id === profileGameInstanceId && (
                    <p className="px-1 text-[11.5px] leading-relaxed text-ink-faint">
                      This profile uses this source. Checking the same original folder refreshes its source record without changing the profile, selected instance, or installed mods.
                    </p>
                  )}
                  {selected.sourceClean === false && (
                    <div role="alert" className="rounded-xl border border-[#ff8a8a]/30 bg-[#ff8a8a]/10 px-3.5 py-3 text-[12px] leading-relaxed text-[#ffb4b4]">
                      This original source is invalid because it contains mod-loader files
                      {selected.sourceModArtifacts?.length
                        ? ` (${selected.sourceModArtifacts.slice(0, 4).join(", ")}${selected.sourceModArtifacts.length > 4 ? ", …" : ""})`
                        : ""}. Verify or repair Among Us in its store, then check the original folder again.
                    </div>
                  )}
                  {selected.store === "msstore" && selected.writable === false && (
                    <div className="rounded-xl border border-[#ffd23f]/25 bg-[#ffd23f]/8 px-3.5 py-3">
                      <div className="flex items-start gap-3">
                        <HardDrives size={18} className="mt-0.5 shrink-0 text-crew-gold" />
                        <div className="min-w-0 flex-1">
                          <p className="text-[12.5px] font-semibold text-ink">
                            This Microsoft Store source is read-only
                          </p>
                          <p className="mt-1 text-[12px] leading-relaxed text-ink-dim">
                            Supported. Perfect Sync builds writable direct profile instances from this source and never modifies the original folder.
                          </p>
                        </div>
                      </div>
                    </div>
                  )}
                </div>
              )}

              <span className="mt-5 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
                GitHub token (optional)
              </span>
              <div className="glass rounded-xl px-3 py-3">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="flex items-center gap-2 text-[12.5px] text-ink-dim">
                    <GithubLogo size={16} className="opacity-75" />
                    {tokenIntent === "clear"
                      ? "The stored token will be cleared when you save."
                      : settings.hasGithubToken
                        ? "A token is stored. Its value is never shown here."
                        : "No GitHub token is stored."}
                  </div>
                  <div className="flex items-center gap-1.5">
                    {tokenIntent === "clear" ? (
                      <button
                        type="button"
                        onClick={() => setTokenIntent("unchanged")}
                        disabled={saving}
                        className="ring-focus rounded-lg bg-white/10 px-2.5 py-1.5 text-[12px] font-semibold text-ink disabled:opacity-50"
                      >
                        Keep stored token
                      </button>
                    ) : (
                      <>
                        <button
                          type="button"
                          onClick={startTokenReplacement}
                          disabled={saving}
                          className="ring-focus rounded-lg bg-white/10 px-2.5 py-1.5 text-[12px] font-semibold text-ink disabled:opacity-50"
                        >
                          {settings.hasGithubToken ? "Replace" : "Add token"}
                        </button>
                        {settings.hasGithubToken && (
                          <button
                            type="button"
                            onClick={() => {
                              setToken("");
                              setTokenIntent("clear");
                            }}
                            disabled={saving}
                            className="ring-focus rounded-lg px-2.5 py-1.5 text-[12px] text-[#ffb4b4] hover:bg-white/10 disabled:opacity-50"
                          >
                            Clear
                          </button>
                        )}
                      </>
                    )}
                  </div>
                </div>
                {tokenIntent === "set" && (
                  <label className="mt-2 flex items-center gap-2 rounded-lg bg-white/[0.055] px-3 py-2.5 text-ink-dim focus-within:text-ink">
                    <GithubLogo size={15} className="opacity-75" />
                    <input
                      ref={tokenInputRef}
                      value={token}
                      maxLength={512}
                      onChange={(event) => setToken(event.target.value)}
                      type="password"
                      autoComplete="new-password"
                      spellCheck={false}
                      placeholder={settings.hasGithubToken ? "Enter replacement token" : "Enter token"}
                      aria-label={settings.hasGithubToken ? "Replacement GitHub token" : "New GitHub token"}
                      className="w-full bg-transparent font-mono text-[13px] text-ink placeholder:text-ink-faint focus:outline-none"
                    />
                    <button
                      type="button"
                      onClick={() => {
                        setToken("");
                        setTokenIntent("unchanged");
                      }}
                      className="ring-focus shrink-0 rounded-md px-2 py-1 text-[11.5px] text-ink-faint hover:bg-white/10 hover:text-ink"
                    >
                      Cancel replacement
                    </button>
                  </label>
                )}
              </div>
              <p className="mt-2 px-1 text-[12px] text-ink-faint">
                Stored securely by the desktop app. Normal catalog release checks use API-free GitHub pages; this token is only used for authenticated GitHub traffic.
              </p>

              <span className="mt-5 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
                Lobby defaults
              </span>
              <p className="mb-2 px-1 text-[12.5px] text-ink-faint">
                These personal mods join every lobby profile. All changes are saved together with the rest of Settings.
              </p>
              <div className="flex flex-col gap-1.5">
                {personalMods.map((personalMod) => {
                  const enabled = personalMod.enabled !== false;
                  return (
                    <div key={personalMod.repo} className="surface-row flex items-center gap-2 rounded-lg px-3 py-2 text-[12.5px]">
                      <button
                        type="button"
                        role="switch"
                        aria-checked={enabled}
                        aria-label={`${enabled ? "Disable" : "Enable"} ${personalMod.name ?? personalMod.repo}`}
                        onClick={() =>
                          setPersonalMods((current) =>
                            current.map((candidate) =>
                              candidate.repo === personalMod.repo
                                ? { ...candidate, enabled: !enabled }
                                : candidate,
                            ),
                          )
                        }
                        disabled={hasPendingWork}
                        className={`ring-focus relative h-5 w-9 shrink-0 rounded-full transition-colors disabled:opacity-50 ${
                          enabled ? "accent-grad" : "bg-white/15"
                        }`}
                      >
                        <span
                          className={`absolute top-1/2 h-4 w-4 -translate-y-1/2 rounded-full bg-white transition-all ${
                            enabled ? "left-[18px]" : "left-0.5"
                          }`}
                        />
                      </button>
                      <span className={`min-w-0 flex-1 truncate ${enabled ? "text-ink" : "text-ink-faint"}`}>
                        {personalMod.name ?? personalMod.repo}
                      </span>
                      <TrustBadge trust={trustOf(personalMod.repo)} compact />
                      <button
                        type="button"
                        onClick={() =>
                          setPersonalPicker({
                            repo: personalMod.repo,
                            name: personalMod.name ?? personalMod.repo,
                            currentVersion: personalMod.tag,
                          })
                        }
                        disabled={hasPendingWork}
                        aria-label={`Change ${personalMod.name ?? personalMod.repo} version`}
                        title="Change version"
                        className="ring-focus glass-2 flex shrink-0 items-center gap-1 rounded-md px-2 py-1 font-mono text-[12px] text-ink-dim hover:text-ink disabled:opacity-50"
                      >
                        {personalMod.tag}
                        <CaretDown size={11} weight="bold" />
                      </button>
                      <button
                        type="button"
                        onClick={() =>
                          setPersonalMods((current) =>
                            current.filter((candidate) => candidate.repo !== personalMod.repo),
                          )
                        }
                        disabled={hasPendingWork}
                        aria-label={`Remove ${personalMod.repo} from lobby defaults`}
                        className="ring-focus grid h-8 w-8 place-items-center rounded-md text-ink-faint hover:bg-white/10 hover:text-[#ff8a8a] disabled:opacity-50"
                      >
                        <TrashSimple size={14} />
                      </button>
                    </div>
                  );
                })}
                {personalMods.length === 0 && (
                  <p className="px-1 text-[12.5px] text-ink-faint">No personal mods are added to every lobby.</p>
                )}
              </div>
              <label className="glass mt-2 flex items-center gap-2 rounded-xl px-3 py-2 text-ink-dim focus-within:text-ink">
                <GithubLogo size={15} className="opacity-75" />
                <input
                  value={personalUrl}
                  maxLength={300}
                  disabled={hasPendingWork}
                  onChange={(event) => {
                    setPersonalUrl(event.target.value);
                    if (personalError) setPersonalError("");
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      submitPersonal();
                    }
                  }}
                  placeholder="owner/repository or GitHub URL"
                  aria-label="Personal mod repository"
                  className="w-full min-w-0 bg-transparent text-[12.5px] text-ink placeholder:text-ink-faint focus:outline-none disabled:opacity-50"
                />
                <button
                  type="button"
                  onClick={submitPersonal}
                  disabled={hasPendingWork || !personalUrl.trim()}
                  className="ring-focus flex shrink-0 items-center gap-1 rounded-lg bg-white/10 px-2.5 py-1 text-[12px] font-semibold text-ink disabled:opacity-50"
                >
                  <Plus size={12} weight="bold" /> Add
                </button>
              </label>
              {personalError && (
                <p role="alert" className="mt-2 px-1 text-[12px] text-[#ffb4b4]">
                  {personalError}
                </p>
              )}

              <div className="mt-3 flex items-center justify-between gap-3">
                <div>
                  <p className="text-[12.5px] font-semibold text-ink">Local DLL defaults</p>
                  <p className="mt-0.5 text-[12px] text-ink-faint">
                    Installed locally for new lobby profiles. Never included in shared lobby codes.
                  </p>
                </div>
                <button
                  type="button"
                  onClick={() => void addLocalLobbyDefault()}
                  disabled={hasPendingWork}
                  className="ring-focus glass flex shrink-0 items-center gap-1.5 rounded-lg px-3 py-2 text-[12px] font-semibold text-ink-dim hover:text-ink disabled:opacity-50"
                >
                  <FileCode size={14} /> Add local DLL
                </button>
              </div>
              <div className="mt-2 flex flex-col gap-1.5">
                {personalLocalMods.map((local) => {
                  const enabled = local.enabled !== false;
                  return (
                    <div key={local.path} className="surface-row flex items-center gap-2 rounded-lg px-3 py-2 text-[12.5px]">
                      <button
                        type="button"
                        role="switch"
                        aria-checked={enabled}
                        aria-label={`${enabled ? "Disable" : "Enable"} ${local.name}`}
                        onClick={() =>
                          setPersonalLocalMods((current) =>
                            current.map((candidate) =>
                              candidate.path === local.path ? { ...candidate, enabled: !enabled } : candidate,
                            ),
                          )
                        }
                        disabled={hasPendingWork}
                        className={`ring-focus relative h-5 w-9 shrink-0 rounded-full transition-colors disabled:opacity-50 ${
                          enabled ? "accent-grad" : "bg-white/15"
                        }`}
                      >
                        <span
                          className={`absolute top-1/2 h-4 w-4 -translate-y-1/2 rounded-full bg-white transition-all ${
                            enabled ? "left-[18px]" : "left-0.5"
                          }`}
                        />
                      </button>
                      <span className="min-w-0 flex-1">
                        <span className={`block truncate ${enabled ? "text-ink" : "text-ink-faint"}`}>{local.name}</span>
                        <span className="block truncate font-mono text-[11px] text-ink-faint">{displayPath(local.path)}</span>
                      </span>
                      <button
                        type="button"
                        onClick={() =>
                          setPersonalLocalMods((current) =>
                            current.filter((candidate) => candidate.path !== local.path),
                          )
                        }
                        disabled={hasPendingWork}
                        aria-label={`Remove ${local.name} from local lobby defaults`}
                        className="ring-focus grid h-8 w-8 shrink-0 place-items-center rounded-md text-ink-faint hover:bg-white/10 hover:text-[#ff8a8a] disabled:opacity-50"
                      >
                        <TrashSimple size={14} />
                      </button>
                    </div>
                  );
                })}
              </div>

              <span className="mt-5 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
                BepInEx loader
              </span>
              <div className="glass rounded-xl px-3.5 py-3 text-[12.5px]">
                <LoaderStatusView state={visibleLoaderView} selected={!!selected} />
                {visibleLoaderView.kind === "error" && (
                  <button
                    type="button"
                    onClick={() => setLoaderRetry((value) => value + 1)}
                    disabled={reinstalling}
                    className="ring-focus mt-3 rounded-lg bg-white/10 px-3 py-2 text-[12.5px] font-semibold text-ink disabled:opacity-50"
                  >
                    Retry status check
                  </button>
                )}
                {profileUsesTou ? (
                  <div className="mt-3 rounded-lg border border-[#9b7bff]/20 bg-[#9b7bff]/8 px-3 py-2.5 text-[12px] leading-relaxed text-ink-dim">
                    Town of Us owns this profile’s BepInEx and fixed UnityDoorstop build. Change or reinstall the Town of Us release from the profile instead.
                  </div>
                ) : (
                  <>
                <label className="mt-3 flex cursor-pointer items-start gap-2 text-[12px] text-ink-dim">
                  <input
                    type="checkbox"
                    checked={applyDoorstopFix}
                    onChange={(event) => setApplyDoorstopFix(event.target.checked)}
                    disabled={hasPendingWork}
                    className="mt-0.5 h-4 w-4 shrink-0 accent-[#9b7bff]"
                  />
                  <span>
                    Apply optional UnityDoorstop 4.5.1 compatibility fix
                    <span className="block text-[11px] text-ink-faint">
                      Off by default. Enable only if the standard loader has startup problems.
                    </span>
                  </span>
                </label>
                <button
                  type="button"
                  onClick={() => void reinstall()}
                  disabled={hasPendingWork || !selected || visibleLoaderView.kind === "loading"}
                  className="ring-focus glass-2 mt-3 flex items-center gap-1.5 rounded-lg px-3 py-2 text-[12.5px] text-ink-dim hover:text-ink disabled:opacity-50"
                >
                  <ArrowsClockwise size={14} className={reinstalling ? "animate-spin" : ""} />
                  {reinstalling ? "Reinstalling BepInEx" : "Reinstall BepInEx be.753"}
                </button>
                <details className="mt-2 text-[11.5px] text-ink-faint">
                  <summary className="ring-focus w-fit cursor-pointer rounded-md px-1 py-0.5 hover:text-ink-dim">
                    Advanced
                  </summary>
                  <div className="mt-2 border-l border-white/10 pl-2.5">
                    <p>Resolve and install the newest experimental BepInEx build instead of pinned be.753.</p>
                    <button
                      type="button"
                      onClick={() => void reinstall(true)}
                      disabled={hasPendingWork || !selected || visibleLoaderView.kind === "loading"}
                      className="ring-focus mt-2 rounded-md bg-white/8 px-2.5 py-1.5 text-[11.5px] font-medium text-ink-dim hover:bg-white/12 hover:text-ink disabled:opacity-50"
                    >
                      Install latest experimental build
                    </button>
                  </div>
                </details>
                  </>
                )}
                {visibleLoaderNotice && (
                  <p aria-live="polite" className="mt-2 text-[12px] text-ink-dim">
                    {visibleLoaderNotice}
                  </p>
                )}
              </div>

              <span className="mt-5 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
                Setup assistant
              </span>
              <div className="glass flex items-center justify-between gap-3 rounded-xl px-3.5 py-3 max-[480px]:items-start">
                <div>
                  <p className="text-[12.5px] text-ink">Run the first-time setup again</p>
                  <p className="mt-0.5 text-[11px] leading-relaxed text-ink-faint">
                    Choose another game install, Town of Us release, or BepInEx-only setup.
                  </p>
                </div>
                <button
                  type="button"
                  onClick={onRunSetup}
                  disabled={hasPendingWork || hasDraftChanges}
                  title={hasDraftChanges ? "Save or discard your changes first" : undefined}
                  className="ring-focus glass-2 shrink-0 rounded-lg px-3 py-2 text-[12px] font-semibold text-ink-dim hover:text-ink disabled:opacity-50"
                >
                  Run setup
                </button>
              </div>

              <span className="mt-5 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
                Support
              </span>
              <div className="glass flex items-center justify-between gap-3 rounded-xl px-3.5 py-3 max-[480px]:items-start">
                <div>
                  <p className="text-[12.5px] text-ink">BepInEx error log</p>
                  <p className="mt-0.5 text-[11px] leading-relaxed text-ink-faint">
                    Save LogOutput.log from the active direct profile instance for troubleshooting.
                  </p>
                </div>
                <button
                  type="button"
                  onClick={() => void saveErrorLog()}
                  disabled={hasPendingWork}
                  title="Save LogOutput.log from the active direct profile instance"
                  className="ring-focus glass-2 flex shrink-0 items-center gap-1.5 rounded-lg px-3 py-2 text-[12px] font-semibold text-ink-dim hover:text-ink disabled:opacity-50"
                >
                  <FileArrowDown size={15} />
                  {errorLogSaving ? "Saving…" : "Save error log"}
                </button>
              </div>

              {draftError && (
                <p role="alert" className="mt-4 rounded-xl border border-[#ff8a8a]/30 bg-[#ff8a8a]/10 px-3 py-2.5 text-[12.5px] text-[#ffb4b4]">
                  {draftError}
                </p>
              )}
            </div>

            <div className="mt-4 flex items-center justify-between gap-2.5 border-t border-white/10 pt-4 max-[520px]:flex-col max-[520px]:items-stretch">
              <p className="max-w-[250px] text-[12px] text-ink-faint max-[520px]:max-w-none" aria-live="polite">
                {hasDraftChanges ? "Your changes are ready to save together." : "No unsaved changes."}
              </p>
              <div className="flex gap-2.5 max-[520px]:w-full">
                <button
                  type="button"
                  onClick={requestClose}
                  disabled={hasPendingWork}
                  className="ring-focus glass rounded-xl px-4 py-2.5 text-[14px] text-ink disabled:opacity-50 max-[520px]:flex-1"
                >
                  {hasDraftChanges ? "Discard changes" : "Close"}
                </button>
                <button
                  type="button"
                  onClick={() => void save()}
                  disabled={hasPendingWork || !hasDraftChanges}
                  className="ring-focus accent-grad rounded-xl px-5 py-2.5 text-[14px] font-bold text-[#0d0820] disabled:opacity-50 max-[520px]:flex-1"
                >
                  {saving ? "Saving" : "Save changes"}
                </button>
              </div>
            </div>
          </motion.div>
          <ReleasePicker
            open={personalPicker !== null}
            repo={personalPicker?.repo ?? ""}
            modName={personalPicker?.name ?? ""}
            trust={personalPicker ? trustOf(personalPicker.repo) : "flagged"}
            busy={saving}
            profileId={profileId}
            currentVersion={personalPicker?.currentVersion}
            onClose={() => setPersonalPicker(null)}
            onPick={(repo, tag, assetName) => {
              const target = personalPicker;
              if (!target || target.repo !== repo) return;
              setPersonalMods((current) => {
                const previous = current.find((candidate) => candidate.repo === repo);
                return [
                  ...current.filter((candidate) => candidate.repo !== repo),
                  {
                    repo,
                    tag,
                    asset: assetName,
                    name: target.name,
                    enabled: previous?.enabled ?? true,
                  },
                ];
              });
              setPersonalPicker(null);
              setPersonalUrl("");
            }}
          />
        </motion.div>
      )}
    </AnimatePresence>
  );
}

const STORE_NAMES: Record<Store, string> = {
  steam: "Steam",
  epic: "Epic Games",
  itch: "itch.io",
  msstore: "Microsoft Store",
  manual: "Among Us",
};

function LoaderStatusView({ state, selected }: { state: LoaderView; selected: boolean }) {
  if (state.kind === "idle") {
    return <div className="text-ink-faint">{selected ? "Direct instance status has not been checked." : "Select a source above to check its direct instance."}</div>;
  }
  if (state.kind === "loading") {
    return <div role="status" className="text-ink-faint">Checking loader status</div>;
  }
  if (state.kind === "error") {
    return (
      <div role="alert" className="text-[#ffb4b4]">
        Loader status check failed: {state.message}
      </div>
    );
  }
  if (state.kind === "missing") {
    return state.value ? (
      <LoaderDetails status={state.value} incomplete />
    ) : (
      <div className="text-[#ffe49a]">This profile has not prepared a direct instance yet.</div>
    );
  }
  return <LoaderDetails status={state.value} />;
}

function LoaderDetails({ status, incomplete = false }: { status: LoaderStatus; incomplete?: boolean }) {
  return (
    <div className="flex flex-col gap-1">
      {incomplete && (
        <div className="mb-1 text-[#ffe49a]">The direct profile instance needs to be prepared or repaired.</div>
      )}
      <StatusRow ok={status.workspaceReady} label="Direct profile instance" />
      <StatusRow ok={status.winhttp} label="Doorstop (winhttp.dll)" />
      <StatusRow ok={status.preloader} label="BepInEx core" />
      <StatusRow
        ok={status.current}
        label={status.current && status.installedVersion ? `BepInEx installed (${status.installedVersion})` : "BepInEx loader installed"}
      />
      <StatusRow ok={status.doorstopFix} label="Latest-game compatibility fix (optional)" />
      <StatusRow ok={status.dotnet} label=".NET runtime" />
      <StatusRow ok={status.steamAppid} label="Steam launch fix" />
      {status.runtime !== "native" && <StatusRow ok={status.runtimeReady} label={`${status.runtime} winhttp override`} />}
      <div className="mt-1 text-ink-faint">
        plugins: {status.profilePlugins} in profile · {status.gamePlugins} in direct instance
        {status.workspacePath ? <span className="mt-0.5 block truncate font-mono">{displayPath(status.workspacePath)}</span> : null}
      </div>
    </div>
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

function uniqueInstanceName(base: string, instances: GameInstance[]): string {
  const names = new Set(instances.map((instance) => instance.name.trim().toLocaleLowerCase()));
  if (!names.has(base.toLocaleLowerCase())) return base;
  let suffix = 2;
  while (names.has(`${base} ${suffix}`.toLocaleLowerCase())) suffix += 1;
  return `${base} ${suffix}`;
}

function instanceNameError(instance: GameInstance, instances: GameInstance[]): string {
  const name = instance.name.trim();
  if (!name) return "Enter a name for this instance.";
  if (name.length > MAX_INSTANCE_NAME) return `Instance names must be ${MAX_INSTANCE_NAME} characters or fewer.`;
  const duplicate = instances.find(
    (candidate) => candidate.id !== instance.id && candidate.name.trim().toLocaleLowerCase() === name.toLocaleLowerCase(),
  );
  return duplicate ? `“${name}” is already used. Give each instance a distinct name.` : "";
}

function validateInstances(instances: GameInstance[]): string {
  for (const instance of instances) {
    const error = instanceNameError(instance, instances);
    if (error) return error;
  }
  return "";
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function actionableSettingsError(error: unknown): string {
  const message = errorMessage(error);
  if (/unique folder/i.test(message)) {
    return `${message} Choose a different original folder for one instance, or remove the duplicate source record before saving.`;
  }
  if (/unique id/i.test(message)) {
    return `${message} Remove the duplicate instance and add its original folder again.`;
  }
  if (/(source|original).*(unavailable|not found|does not exist|cannot be reached)|cannot access.*(source|original)/i.test(message)) {
    return `The recorded original Among Us source is unavailable. Reconnect its drive or use Change to choose the exact original folder again. ${message}`;
  }
  if (/(source.*(fingerprint|build).*(changed|differ|mismatch)|source changed)/i.test(message)) {
    return `The original Among Us source changed since its source record was saved. Check the original folder and save Settings to accept its new fingerprint and build. ${message}`;
  }
  if (/(storage.*(inside|overlap|contain).*(source|among us)|(source|among us).*(inside|overlap|contain).*storage|storage.*(source|among us).*cannot contain|cannot contain one another|unsafe storage)/i.test(message)) {
    return `Perfect Sync storage overlaps the original Among Us source. Move storage to the recommended or another non-overlapping location before building a direct instance. ${message}`;
  }
  if (/(invalid.*(source|among us)|(source|among us).*(invalid|not (?:a )?valid)|mod-loader artifacts|among us executable|non-link directory|regular.*directory)/i.test(message)) {
    return `The selected original Among Us source is invalid. Verify or repair the game in its store, then check the original folder again. ${message}`;
  }
  return message;
}

function sameOpenSession(
  session: number,
  profileId: string,
  openRef: { current: boolean },
  sessionRef: { current: number },
  profileIdRef: { current: string },
): boolean {
  return openRef.current && sessionRef.current === session && profileIdRef.current === profileId;
}

function sameSelectedSession(
  session: number,
  profileId: string,
  instanceId: string,
  path: string,
  openRef: { current: boolean },
  sessionRef: { current: number },
  profileIdRef: { current: string },
  selectedRef: { current: GameInstance | null },
): boolean {
  return (
    sameOpenSession(session, profileId, openRef, sessionRef, profileIdRef) &&
    selectedRef.current?.id === instanceId &&
    selectedRef.current.path === path
  );
}

function isCurrent(
  identity: RequestIdentity,
  request: number,
  requestRef: { current: number },
  openRef: { current: boolean },
  sessionRef: { current: number },
  profileIdRef: { current: string },
  selectedRef: { current: GameInstance | null },
): boolean {
  return (
    openRef.current &&
    sessionRef.current === identity.session &&
    requestRef.current === request &&
    profileIdRef.current === identity.profileId &&
    selectedRef.current?.path === identity.path
  );
}
