import { useCallback, useEffect, useRef, useState } from "react";
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
import { inspectGame, loaderStatus, pickFolder, reinstallLoader, type LoaderStatus } from "../lib/bridge";
import { useModalFocus } from "../lib/useModalFocus";
import { TrustBadge } from "./TrustBadge";
import type { GameInstance, GithubTokenAction, Settings, Store, Trust } from "../lib/types";

interface SettingsModalProps {
  open: boolean;
  settings: Settings;
  profileId: string;
  profileGameInstanceId?: string;
  onClose: () => void;
  onSave: (settings: Settings, tokenAction: GithubTokenAction) => Promise<void>;
  onAddPersonal: (repo: string, name: string) => Promise<void>;
  onRemovePersonal: (repo: string) => Promise<void>;
  onTogglePersonal: (repo: string, enabled: boolean) => Promise<void>;
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

export function SettingsModal({
  open,
  settings,
  profileId,
  profileGameInstanceId,
  onClose,
  onSave,
  onAddPersonal,
  onRemovePersonal,
  onTogglePersonal,
  trustOf,
}: SettingsModalProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const tokenInputRef = useRef<HTMLInputElement>(null);
  const [token, setToken] = useState("");
  const [tokenIntent, setTokenIntent] = useState<TokenIntent>("unchanged");
  const [instances, setInstances] = useState<GameInstance[]>(settings.gameInstances ?? []);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loaderView, setLoaderView] = useState<LoaderView>({ kind: "idle" });
  const [loaderRetry, setLoaderRetry] = useState(0);
  const [folderPending, setFolderPending] = useState(false);
  const [reinstalling, setReinstalling] = useState(false);
  const [saving, setSaving] = useState(false);
  const [personalPending, setPersonalPending] = useState<string | null>(null);
  const [draftError, setDraftError] = useState("");
  const [personalError, setPersonalError] = useState("");
  const [loaderNotice, setLoaderNotice] = useState<{ path: string; profileId: string; text: string } | null>(null);
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
  const personalPendingRef = useRef<string | null>(null);
  const savePendingRef = useRef(false);

  const selected = instances.find((instance) => instance.id === selectedId) ?? null;
  const selectedRef = useRef<GameInstance | null>(selected);

  openRef.current = open;
  profileIdRef.current = profileId;
  closeRef.current = onClose;
  latestOpenDataRef.current = { settings, profileGameInstanceId };
  selectedRef.current = selected;

  const hasPendingWork = folderPending || reinstalling || saving || personalPending !== null;
  const canDismissRef = useRef(!hasPendingWork);
  canDismissRef.current = !hasPendingWork;

  const requestClose = useCallback(() => {
    if (
      canDismissRef.current &&
      !folderPendingRef.current &&
      !reinstallPendingRef.current &&
      !personalPendingRef.current &&
      !savePendingRef.current
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
      setSelectedId(
        next.some((instance) => instance.id === opening.profileGameInstanceId)
          ? (opening.profileGameInstanceId ?? null)
          : (next[0]?.id ?? null),
      );
      setLoaderView({ kind: "idle" });
      setFolderPending(false);
      setReinstalling(false);
      setSaving(false);
      setPersonalPending(null);
      setDraftError("");
      setPersonalError("");
      setLoaderNotice(null);
      setPersonalUrl("");
      folderPendingRef.current = false;
      reinstallPendingRef.current = false;
      personalPendingRef.current = null;
      savePendingRef.current = false;
    } else if (!open && wasOpenRef.current) {
      sessionRef.current += 1;
      loaderRequestRef.current += 1;
      installRequestRef.current += 1;
      folderPendingRef.current = false;
      reinstallPendingRef.current = false;
      personalPendingRef.current = null;
      savePendingRef.current = false;
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
          value?.current && value.runtimeReady
            ? { kind: "ready", path, profileId, value }
            : { kind: "missing", path, profileId, value },
        );
      })
      .catch((error: unknown) => {
        if (!isCurrent(identity, request, loaderRequestRef, openRef, sessionRef, profileIdRef, selectedRef)) return;
        setLoaderView({ kind: "error", path, profileId, message: errorMessage(error) });
      });
  }, [loaderRetry, open, profileId, selected?.path]);

  const beginFolderWork = () => {
    if (folderPendingRef.current) return false;
    folderPendingRef.current = true;
    setFolderPending(true);
    setDraftError("");
    return true;
  };

  const endFolderWork = (session: number) => {
    if (!openRef.current || sessionRef.current !== session) return;
    folderPendingRef.current = false;
    setFolderPending(false);
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

      setInstances((current) => {
        const baseName = STORE_NAMES[game.store];
        const instance: GameInstance = {
          id: `game-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`,
          name: uniqueInstanceName(baseName, current),
          path: game.path,
          arch: game.arch,
          store: game.store,
          runtime: game.runtime ?? "native",
        };
        setSelectedId(instance.id);
        return [...current, instance];
      });
    } catch (error) {
      if (sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef)) {
        setDraftError(errorMessage(error));
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
      setInstances((current) =>
        current.map((instance) =>
          instance.id === targetId
            ? { ...instance, path: game.path, arch: game.arch, store: game.store, runtime: game.runtime ?? "native" }
            : instance,
        ),
      );
    } catch (error) {
      if (sameSelectedSession(session, requestProfileId, targetId, originalPath, openRef, sessionRef, profileIdRef, selectedRef)) {
        setDraftError(errorMessage(error));
      }
    } finally {
      endFolderWork(session);
    }
  };

  const removeInstance = (id: string) => {
    if (hasPendingWork) return;
    const next = instances.filter((instance) => instance.id !== id);
    setInstances(next);
    if (selectedId === id) setSelectedId(next[0]?.id ?? null);
    setDraftError("");
  };

  const reinstall = async () => {
    const target = selectedRef.current;
    if (!target || reinstallPendingRef.current) return;
    const identity: RequestIdentity = {
      session: sessionRef.current,
      path: target.path,
      profileId: profileIdRef.current,
    };
    const request = ++installRequestRef.current;
    reinstallPendingRef.current = true;
    setReinstalling(true);
    setLoaderNotice({ path: identity.path, profileId: identity.profileId, text: "Reinstalling BepInEx…" });
    try {
      const warning = await reinstallLoader(target.path, identity.profileId, target.arch);
      if (!isCurrent(identity, request, installRequestRef, openRef, sessionRef, profileIdRef, selectedRef)) return;
      setLoaderNotice({ path: identity.path, profileId: identity.profileId, text: warning ?? "BepInEx reinstalled (latest)." });
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

  const runPersonal = async (key: string, action: () => Promise<void>, clearInput = false) => {
    if (personalPendingRef.current) return;
    const session = sessionRef.current;
    const requestProfileId = profileIdRef.current;
    personalPendingRef.current = key;
    setPersonalPending(key);
    setPersonalError("");
    try {
      await action();
      if (
        !sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef) ||
        personalPendingRef.current !== key
      ) return;
      if (clearInput) setPersonalUrl("");
    } catch (error) {
      if (
        sameOpenSession(session, requestProfileId, openRef, sessionRef, profileIdRef) &&
        personalPendingRef.current === key
      ) {
        setPersonalError(errorMessage(error));
      }
    } finally {
      if (
        openRef.current &&
        sessionRef.current === session &&
        personalPendingRef.current === key
      ) {
        personalPendingRef.current = null;
        setPersonalPending(null);
      }
    }
  };

  const submitPersonal = () => {
    const match = personalUrl.match(/github\.com\/([^/]+)\/([^/#?]+)/i);
    const repo = (match ? `${match[1]}/${match[2]}` : personalUrl).trim().replace(/\.git$/i, "");
    if (!repo) {
      setPersonalError("Enter an owner/repository name or GitHub repository URL.");
      return;
    }
    void runPersonal(`add:${repo}`, () => onAddPersonal(repo, match ? match[2].replace(/\.git$/i, "") : repo), true);
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
      personalPendingRef.current ||
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
  const personalBusy = personalPending !== null;
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
          className="fixed inset-0 z-50 grid place-items-center p-6"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18 }}
        >
          <div
            className="absolute inset-0 bg-[rgba(6,4,18,0.5)]"
            style={{ backdropFilter: "blur(2px)" }}
            onClick={requestClose}
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
            className="glass-strong relative flex max-h-[90vh] w-[520px] max-w-full flex-col rounded-3xl p-6"
          >
            <button
              type="button"
              onClick={requestClose}
              disabled={hasPendingWork}
              aria-label="Close settings"
              className="ring-focus absolute top-4 right-4 grid h-8 w-8 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink disabled:opacity-40"
            >
              <X size={16} weight="bold" />
            </button>

            <h2 className="text-[20px] font-semibold text-ink">Settings</h2>

            <div className="scroll-region -mr-2 min-h-0 flex-1 overflow-y-auto pr-2">
              <div className="mt-5 mb-2 flex items-center justify-between">
                <span className="text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
                  Among Us instances
                </span>
                <button
                  type="button"
                  onClick={() => void addInstance()}
                  disabled={hasPendingWork}
                  className="ring-focus flex items-center gap-1 rounded-lg px-2 py-1 text-[11.5px] font-semibold text-ink-dim hover:bg-white/10 hover:text-ink disabled:opacity-50"
                >
                  <Plus size={12} weight="bold" /> {folderPending ? "Inspecting…" : "Add folder"}
                </button>
              </div>
              <div className="flex flex-col gap-1.5">
                {instances.map((instance) => {
                  const active = instance.id === selectedId;
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
                            <span className="font-mono text-[10.5px] text-ink-faint">
                              {instance.store} · {instance.arch} · {instance.runtime}
                            </span>
                          </span>
                          <span className="block truncate font-mono text-[10.5px] text-ink-faint">
                            {instance.path}
                          </span>
                        </span>
                      </button>
                      <button
                        type="button"
                        onClick={() => removeInstance(instance.id)}
                        disabled={hasPendingWork}
                        aria-label={`Remove ${instance.name || "unnamed instance"}`}
                        className="ring-focus mr-2 grid h-7 w-7 shrink-0 place-items-center rounded-md text-ink-faint hover:bg-white/10 hover:text-[#ff8a8a] disabled:opacity-40"
                      >
                        <TrashSimple size={14} />
                      </button>
                    </div>
                  );
                })}
                {instances.length === 0 && (
                  <div className="glass rounded-xl px-3 py-4 text-center text-[12px] text-ink-faint">
                    Add every Among Us folder you want to use with profiles.
                  </div>
                )}
              </div>
              {selected && (
                <div className="mt-2 grid gap-2">
                  <label className="glass flex items-center gap-2 rounded-xl px-3 py-2.5 text-ink-dim focus-within:text-ink">
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
                      className="w-full bg-transparent text-[12.5px] text-ink placeholder:text-ink-faint focus:outline-none disabled:opacity-50"
                    />
                  </label>
                  {selectedNameError && (
                    <p id="instance-name-error" className="px-1 text-[12px] text-[#ffb4b4]">
                      {selectedNameError}
                    </p>
                  )}
                  <div className="flex items-center gap-2">
                    <div className="glass flex min-w-0 flex-1 items-center gap-2 rounded-xl px-3 py-2.5 text-ink-dim">
                      <FolderOpen size={16} className="shrink-0 opacity-75" />
                      <span className="truncate font-mono text-[11.5px] text-ink">{selected.path}</span>
                    </div>
                    <button
                      type="button"
                      onClick={() => void changeSelectedFolder()}
                      disabled={hasPendingWork}
                      className="ring-focus glass rounded-xl px-3 py-2.5 text-[12.5px] text-ink-dim hover:text-ink disabled:opacity-50"
                    >
                      Change
                    </button>
                  </div>
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
                Stored securely by the desktop app. It raises the GitHub API rate limit without exposing the saved value to this form.
              </p>

              <span className="mt-5 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
                Always add to lobbies · saves immediately
              </span>
              <p className="mb-2 px-1 text-[12px] text-ink-faint">
                Adding, changing, enabling, or removing a personal mod saves immediately. Cancel only discards token and instance drafts.
              </p>
              <div className="flex flex-col gap-1.5">
                {(settings.personalMods ?? []).map((personalMod) => {
                  const enabled = personalMod.enabled !== false;
                  const rowBusy = personalPending?.endsWith(`:${personalMod.repo}`) ?? false;
                  return (
                    <div key={personalMod.repo} className="glass flex items-center gap-2 rounded-lg px-3 py-2 text-[12.5px]">
                      <button
                        type="button"
                        role="switch"
                        aria-checked={enabled}
                        aria-label={`${enabled ? "Disable" : "Enable"} ${personalMod.name ?? personalMod.repo}; saves immediately`}
                        onClick={() => void runPersonal(`toggle:${personalMod.repo}`, () => onTogglePersonal(personalMod.repo, !enabled))}
                        disabled={personalBusy}
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
                        onClick={() => void runPersonal(`version:${personalMod.repo}`, () => onAddPersonal(personalMod.repo, personalMod.name ?? personalMod.repo))}
                        disabled={personalBusy}
                        aria-label={`Change ${personalMod.name ?? personalMod.repo} version; saves immediately`}
                        title="Change version (saves immediately)"
                        className="ring-focus glass-2 flex shrink-0 items-center gap-1 rounded-md px-2 py-1 font-mono text-[11.5px] text-ink-dim hover:text-ink disabled:opacity-50"
                      >
                        {rowBusy ? "Saving…" : personalMod.tag}
                        <CaretDown size={11} weight="bold" />
                      </button>
                      <button
                        type="button"
                        onClick={() => void runPersonal(`remove:${personalMod.repo}`, () => onRemovePersonal(personalMod.repo))}
                        disabled={personalBusy}
                        aria-label={`Remove ${personalMod.repo}; saves immediately`}
                        className="ring-focus grid h-7 w-7 place-items-center rounded-md text-ink-faint hover:bg-white/10 hover:text-[#ff8a8a] disabled:opacity-50"
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
                  maxLength={300}
                  disabled={personalBusy}
                  onChange={(event) => setPersonalUrl(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      submitPersonal();
                    }
                  }}
                  placeholder="Paste a GitHub repo to always include"
                  aria-label="Always-include repo; selection saves immediately"
                  className="w-full min-w-0 bg-transparent text-[12.5px] text-ink placeholder:text-ink-faint focus:outline-none disabled:opacity-50"
                />
                <button
                  type="button"
                  onClick={submitPersonal}
                  disabled={personalBusy}
                  className="ring-focus flex shrink-0 items-center gap-1 rounded-lg bg-white/10 px-2.5 py-1 text-[12px] font-semibold text-ink disabled:opacity-50"
                >
                  <Plus size={12} weight="bold" /> {personalBusy ? "Saving…" : "Add"}
                </button>
              </label>
              {personalError && (
                <p role="alert" className="mt-2 px-1 text-[12px] text-[#ffb4b4]">
                  {personalError}
                </p>
              )}

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
                <button
                  type="button"
                  onClick={() => void reinstall()}
                  disabled={hasPendingWork || !selected || visibleLoaderView.kind === "loading"}
                  className="ring-focus glass-2 mt-3 flex items-center gap-1.5 rounded-lg px-3 py-2 text-[12.5px] text-ink-dim hover:text-ink disabled:opacity-50"
                >
                  <ArrowsClockwise size={14} className={reinstalling ? "animate-spin" : ""} />
                  {reinstalling ? "Reinstalling BepInEx…" : "Reinstall BepInEx (latest)"}
                </button>
                {visibleLoaderNotice && (
                  <p aria-live="polite" className="mt-2 text-[12px] text-ink-dim">
                    {visibleLoaderNotice}
                  </p>
                )}
              </div>

              {draftError && (
                <p role="alert" className="mt-4 rounded-xl border border-[#ff8a8a]/30 bg-[#ff8a8a]/10 px-3 py-2.5 text-[12.5px] text-[#ffb4b4]">
                  {draftError}
                </p>
              )}
            </div>

            <div className="mt-4 flex items-center justify-between gap-2.5 border-t border-white/10 pt-4">
              <p className="max-w-[250px] text-[11.5px] text-ink-faint">
                Cancel discards only unsaved token and instance changes.
              </p>
              <div className="flex gap-2.5">
                <button
                  type="button"
                  onClick={requestClose}
                  disabled={hasPendingWork}
                  className="ring-focus glass rounded-xl px-4 py-2.5 text-[14px] text-ink disabled:opacity-50"
                >
                  Cancel drafts
                </button>
                <button
                  type="button"
                  onClick={() => void save()}
                  disabled={hasPendingWork}
                  className="ring-focus accent-grad rounded-xl px-5 py-2.5 text-[14px] font-bold text-[#0d0820] disabled:opacity-50"
                >
                  {saving ? "Saving…" : "Save"}
                </button>
              </div>
            </div>
          </motion.div>
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
    return <div className="text-ink-faint">{selected ? "Loader status has not been checked." : "Select an instance above to check the loader."}</div>;
  }
  if (state.kind === "loading") {
    return <div role="status" className="text-ink-faint">Checking loader status…</div>;
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
      <div className="text-[#ffe49a]">BepInEx is not installed for this game folder.</div>
    );
  }
  return <LoaderDetails status={state.value} />;
}

function LoaderDetails({ status, incomplete = false }: { status: LoaderStatus; incomplete?: boolean }) {
  return (
    <div className="flex flex-col gap-1">
      {incomplete && (
        <div className="mb-1 text-[#ffe49a]">BepInEx setup is incomplete for this game folder.</div>
      )}
      <StatusRow ok={status.winhttp} label="Doorstop (winhttp.dll)" />
      <StatusRow ok={status.preloader} label="BepInEx core" />
      <StatusRow
        ok={status.current}
        label={status.current && status.installedVersion ? `BepInEx installed (${status.installedVersion})` : "BepInEx loader installed"}
      />
      <StatusRow ok={status.dotnet} label=".NET runtime" />
      <StatusRow ok={status.steamAppid} label="Steam launch fix" />
      {status.runtime !== "native" && <StatusRow ok={status.runtimeReady} label={`${status.runtime} winhttp override`} />}
      <div className="mt-1 text-ink-faint">
        plugins: {status.profilePlugins} in profile · {status.gamePlugins} synced to game
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
    return `${message} Choose a different folder for one instance, or remove the duplicate before saving.`;
  }
  if (/unique id/i.test(message)) {
    return `${message} Remove the duplicate instance and add its folder again.`;
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
