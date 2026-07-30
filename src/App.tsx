import { useEffect, useMemo, useRef, useState } from "react";
import { TopBar } from "./components/TopBar";
import { Sidebar } from "./components/Sidebar";
import { MainPanel } from "./components/MainPanel";
import { LobbyCodeModal } from "./components/LobbyCodeModal";
import { AddModPanel } from "./components/AddModPanel";
import { BatchInstallReview } from "./components/BatchInstallReview";
import { BatchUpdateReview } from "./components/BatchUpdateReview";
import { MapBrowserPanel } from "./components/MapBrowserPanel";
import { SettingsModal } from "./components/SettingsModal";
import { ReleasePicker } from "./components/ReleasePicker";
import { ShareModal } from "./components/ShareModal";
import { SetupModal, type SetupSelection } from "./components/SetupModal";
import { LaunchWarning } from "./components/LaunchWarning";
import { MainModWarning } from "./components/MainModWarning";
import { UnmanagedPluginsModal } from "./components/UnmanagedPluginsModal";
import { Toast, type ToastState } from "./components/Toast";
import {
  OperationProgressModal,
  type OperationActivity,
  type OperationScope,
} from "./components/OperationProgressModal";
import * as bridge from "./lib/bridge";
import { CREW } from "./lib/palette";
import { findMainMods, type MainMod, type MainModCandidate } from "./lib/mainMods";
import type {
  Arch,
  CatalogItem,
  GameInstall,
  GameInstance,
  LevelImposterMap,
  ModInstallSelection,
  OperationProgress,
  GithubTokenAction,
  Profile,
  Runtime,
  Settings,
  Store,
  Trust,
  UnmanagedPlugin,
} from "./lib/types";

const CREW_CYCLE = Object.values(CREW);
const OPERATION_BUSY = new Error("Another operation is already in progress.");
const UNMANAGED_REVIEW_CANCELLED = new Error("Unmanaged plugin review was canceled.");

const INSTANCE_NAMES: Record<Store, string> = {
  steam: "Steam",
  epic: "Epic Games",
  itch: "itch.io",
  msstore: "Microsoft Store",
  manual: "Among Us",
};

const EMPTY_SETTINGS: Settings = {
  gameInstances: [],
  personalMods: [],
  setupComplete: false,
  hasGithubToken: false,
  freshSourceSetupComplete: false,
  activeStoragePath: "",
  defaultStoragePath: "",
};
interface StartupResult {
  settings: Settings;
  games: GameInstall[];
  catalog: CatalogItem[];
  profiles: Profile[];
  errors: string[];
  freshSourceMigration: boolean;
}

interface TrackedOperation {
  scope: OperationScope;
  title: string;
  message: string;
}

interface UnmanagedPluginPrompt {
  profileId: string;
  profileName: string;
  instanceName: string;
  gamePath: string;
  plugins: UnmanagedPlugin[];
  continuation: boolean;
}

function messageFrom(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (/^HTTP status 403$/i.test(message.trim())) {
    return "HTTP 403: GitHub temporarily refused this web request. Normal catalog installs do not use REST API quota; retry shortly and verify that github.com is reachable.";
  }
  return message;
}

export function App() {
  const [loaded, setLoaded] = useState(false);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeId, setActiveId] = useState("");
  const [running, setRunningState] = useState(false);
  const [operationBusy, setOperationBusy] = useState(false);
  const [operationActivity, setOperationActivity] = useState<OperationActivity | null>(null);

  const operationRef = useRef(false);
  const runningRef = useRef(false);
  const launchSession = useRef(0);
  const startupPromiseRef = useRef<Promise<StartupResult> | null>(null);
  const operationActivityId = useRef(0);
  const initialWorkspacePreparationStarted = useRef(false);
  const automaticUpdateRef = useRef(false);
  const mainModWarningResolver = useRef<((confirmed: boolean) => void) | null>(null);
  const unmanagedPluginResolver = useRef<((resolved: boolean) => void) | null>(null);

  const [games, setGames] = useState<GameInstall[]>([]);
  const [settings, setSettings] = useState<Settings>(EMPTY_SETTINGS);
  const [catalog, setCatalog] = useState<CatalogItem[]>([]);
  const [update, setUpdate] = useState<bridge.UpdateInfo | null>(null);
  const [updateDismissed, setUpdateDismissed] = useState(false);
  const [startupError, setStartupError] = useState<string | null>(null);
  const [freshSourceMigration, setFreshSourceMigration] = useState(false);

  const [addOpen, setAddOpen] = useState(false);
  const [selectedCatalogIds, setSelectedCatalogIds] = useState<string[]>([]);
  const [batchTargets, setBatchTargets] = useState<CatalogItem[]>([]);
  const [mapsOpen, setMapsOpen] = useState(false);
  const [updateReviewOpen, setUpdateReviewOpen] = useState(false);
  const [mapsReturnToAdd, setMapsReturnToAdd] = useState(false);
  const [lobbyOpen, setLobbyOpen] = useState(false);
  const [lobbyCode, setLobbyCode] = useState<string | undefined>(undefined);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [setupOpen, setSetupOpen] = useState(false);
  const [shareOpen, setShareOpen] = useState(false);
  const [launchWarn, setLaunchWarn] = useState<Profile | null>(null);
  const [mainModWarning, setMainModWarning] = useState<{
    mods: MainMod[];
    actionLabel: string;
  } | null>(null);
  const [unmanagedPlugins, setUnmanagedPlugins] = useState<UnmanagedPlugin[]>([]);
  const [unmanagedLoading, setUnmanagedLoading] = useState(false);
  const [unmanagedScanError, setUnmanagedScanError] = useState<string | null>(null);
  const [unmanagedPrompt, setUnmanagedPrompt] = useState<UnmanagedPluginPrompt | null>(null);
  const [pickerTarget, setPickerTarget] = useState<{
    repo: string;
    name: string;
    trust: Trust;
    currentVersion?: string;
    recommendedVersion?: string;
    returnToAdd?: boolean;
  } | null>(null);

  const [toast, setToast] = useState<ToastState | null>(null);
  const toastId = useRef(0);
  const notify = (msg: string, kind: "success" | "error" = "success") => {
    toastId.current += 1;
    const id = toastId.current;
    setToast((current) => (current?.kind === "error" && kind === "success" ? current : { id, msg, kind }));
    if (kind === "success") {
      window.setTimeout(() => setToast((current) => (current?.id === id ? null : current)), 2600);
    }
  };

  const requestMainModConfirmation = (
    existing: readonly MainModCandidate[],
    incoming: readonly MainModCandidate[],
    actionLabel: string,
  ): Promise<boolean> => {
    if (findMainMods(incoming).length === 0) return Promise.resolve(true);
    const mods = findMainMods([...existing, ...incoming]);
    if (mods.length <= 1) return Promise.resolve(true);
    if (mainModWarningResolver.current) return Promise.resolve(false);

    const { promise, resolve } = Promise.withResolvers<boolean>();
    mainModWarningResolver.current = resolve;
    setMainModWarning({ mods, actionLabel });
    return promise;
  };

  const resolveMainModWarning = (confirmed: boolean) => {
    const resolve = mainModWarningResolver.current;
    mainModWarningResolver.current = null;
    setMainModWarning(null);
    resolve?.(confirmed);
  };

  const setRunning = (value: boolean) => {
    runningRef.current = value;
    setRunningState(value);
  };

  const beginOperation = (): boolean => {
    if (operationRef.current || runningRef.current) return false;
    operationRef.current = true;
    setOperationBusy(true);
    return true;
  };

  const endOperation = () => {
    operationRef.current = false;
    setOperationBusy(false);
  };

  const exclusive = async <T,>(action: () => Promise<T>): Promise<T> => {
    if (!beginOperation()) throw OPERATION_BUSY;
    try {
      return await action();
    } finally {
      endOperation();
    }
  };

  const trackedExclusive = async <T,>(
    descriptor: TrackedOperation,
    action: (report: (progress: OperationProgress) => void) => Promise<T>,
  ): Promise<T> => {
    if (!beginOperation()) throw OPERATION_BUSY;
    const id = ++operationActivityId.current;
    setOperationActivity({
      id,
      scope: descriptor.scope,
      title: descriptor.title,
      phase: "preparing",
      message: descriptor.message,
      startedAt: Date.now(),
    });
    const report = (progress: OperationProgress) => {
      setOperationActivity((current) => current?.id === id ? { ...current, ...progress } : current);
    };
    try {
      return await action(report);
    } finally {
      setOperationActivity((current) => current?.id === id ? null : current);
      endOperation();
    }
  };

  const installApplicationUpdate = async (available: bridge.UpdateInfo): Promise<void> => {
    automaticUpdateRef.current = true;
    try {
      await trackedExclusive(
        {
          scope: "release",
          title: `Updating Perfect Sync to ${available.version}`,
          message: "Preparing the signed application update",
        },
        (report) => bridge.installUpdate(report),
      );
    } catch (error) {
      automaticUpdateRef.current = false;
      throw error;
    }
  };

  useEffect(() => {
    const root = document.documentElement;
    const usePointerFocus = () => {
      root.dataset.inputModality = "pointer";
    };
    const useKeyboardFocus = () => {
      root.dataset.inputModality = "keyboard";
    };
    usePointerFocus();
    window.addEventListener("pointerdown", usePointerFocus, true);
    window.addEventListener("keydown", useKeyboardFocus, true);
    return () => {
      window.removeEventListener("pointerdown", usePointerFocus, true);
      window.removeEventListener("keydown", useKeyboardFocus, true);
      delete root.dataset.inputModality;
    };
  }, []);

  useEffect(() => () => {
    const mainModResolve = mainModWarningResolver.current;
    mainModWarningResolver.current = null;
    mainModResolve?.(false);
    const unmanagedResolve = unmanagedPluginResolver.current;
    unmanagedPluginResolver.current = null;
    unmanagedResolve?.(false);
  }, []);

  // StrictMode replays effects. The startup promise is created synchronously
  // once, so every replay observes the same reads and persistence work while
  // only the currently mounted subscriber commits the result.
  useEffect(() => {
    let current = true;
    if (!startupPromiseRef.current) {
      if (!beginOperation()) return;
      startupPromiseRef.current = (async (): Promise<StartupResult> => {
        const [loadedSettings, detectedGames, loadedProfiles, loadedCatalog] = await Promise.all([
          bridge.getSettings(),
          bridge.detectGames(),
          bridge.loadProfiles(),
          bridge.loadCatalog(),
        ]);

        let nextProfiles = loadedProfiles;
        const defaultGameId = loadedSettings.gameInstances[0]?.id;
        if (nextProfiles.length === 0) {
          const starter = await bridge.saveProfile({
            id: "my-mods",
            name: "My mods",
            crewColor: CREW.violet,
            gameInstanceId: defaultGameId,
            mods: [],
          });
          nextProfiles = [starter];
        } else if (defaultGameId) {
          const validGameIds = new Set(loadedSettings.gameInstances.map((instance) => instance.id));
          const migrated: Profile[] = [];
          for (const profile of nextProfiles) {
            if (profile.gameInstanceId && validGameIds.has(profile.gameInstanceId)) {
              migrated.push(profile);
            } else {
              migrated.push(await bridge.saveProfile({ ...profile, gameInstanceId: defaultGameId }));
            }
          }
          nextProfiles = migrated;
        }

        const errors: string[] = [];
        let nextCatalog = loadedCatalog;
        try {
          await bridge.refreshCatalog();
          nextCatalog = await bridge.loadCatalog();
        } catch (error) {
          errors.push(`Catalog refresh failed: ${messageFrom(error)}`);
        }

        const refreshedProfiles: Profile[] = [];
        for (const profile of nextProfiles) {
          const instance =
            loadedSettings.gameInstances.find((candidate) => candidate.id === profile.gameInstanceId) ??
            loadedSettings.gameInstances[0];
          if (!instance) {
            refreshedProfiles.push(profile);
            continue;
          }
          try {
            refreshedProfiles.push(await bridge.checkModUpdates(profile.id, instance.arch));
          } catch (error) {
            refreshedProfiles.push(profile);
            errors.push(`Could not check updates for ${profile.name}: ${messageFrom(error)}`);
          }
        }

        return {
          settings: loadedSettings,
          games: detectedGames,
          catalog: nextCatalog,
          profiles: refreshedProfiles,
          errors,
          freshSourceMigration:
            loadedSettings.setupComplete && !loadedSettings.freshSourceSetupComplete,
        };
      })().finally(endOperation);
    }

    void startupPromiseRef.current
      .then((result) => {
        if (!current) return;
        initialWorkspacePreparationStarted.current =
          !result.settings.setupComplete || result.freshSourceMigration;
        setSettings(result.settings);
        setFreshSourceMigration(result.freshSourceMigration);
        setGames(result.games);
        setCatalog(result.catalog);
        setProfiles(result.profiles);
        const persisted = result.settings.activeProfile;
        setActiveId(
          persisted && result.profiles.some((profile) => profile.id === persisted)
            ? persisted
            : result.profiles[0].id,
        );
        setLoaded(true);
        if (result.settings.recoveryWarning) notify(result.settings.recoveryWarning, "error");
        if (result.settings.storageWarning) notify(result.settings.storageWarning, "error");
        for (const error of result.errors) notify(error, "error");
      })
      .catch((error) => {
        if (!current) return;
        setStartupError(messageFrom(error));
        setLoaded(true);
      });

    return () => {
      current = false;
    };
  }, []);

  useEffect(() => {
    let current = true;
    void (async () => {
      try {
        const available = await bridge.checkUpdate();
        if (!current || !available || automaticUpdateRef.current) return;
        try {
          await installApplicationUpdate(available);
        } catch (error) {
          if (!current) return;
          setUpdate(available);
          setUpdateDismissed(false);
          notify(`Automatic update failed: ${messageFrom(error)}`, "error");
        }
      } catch (error) {
        // Automatic update discovery is best-effort. An unpublished or temporarily
        // unavailable feed must not interrupt an otherwise healthy app startup.
        if (import.meta.env.DEV) console.warn("Update check failed", error);
      }
    })();
    return () => {
      current = false;
    };
  }, []);

  useEffect(() => {
    if (!loaded) return;
    let current = true;
    let unlisten: (() => void) | undefined;
    let pendingCode: string | null = null;
    let dialogObserver: MutationObserver | null = null;

    const openPendingLink = () => {
      if (!current || !pendingCode || document.querySelector('[aria-modal="true"]')) return;
      const code = pendingCode;
      pendingCode = null;
      dialogObserver?.disconnect();
      dialogObserver = null;
      setLobbyCode(code);
      setLobbyOpen(true);
    };
    const receiveLink = (code: string) => {
      if (!document.querySelector('[aria-modal="true"]')) {
        setLobbyCode(code);
        setLobbyOpen(true);
        return;
      }
      pendingCode = code;
      if (!dialogObserver) {
        dialogObserver = new MutationObserver(openPendingLink);
        dialogObserver.observe(document.body, {
          childList: true,
          subtree: true,
          attributes: true,
          attributeFilter: ["aria-modal"],
        });
      }
    };

    bridge
      .onLobbyLink((code) => {
        if (current) receiveLink(code);
      })
      .then((stop) => {
        if (!current) stop?.();
        else if (typeof stop === "function") unlisten = stop;
      })
      .catch((error) => {
        if (current) notify(`Lobby link failed: ${messageFrom(error)}`, "error");
      });
    return () => {
      current = false;
      dialogObserver?.disconnect();
      unlisten?.();
    };
  }, [loaded]);

  const active = profiles.find((profile) => profile.id === activeId) ?? profiles[0];
  const installedSnapshot = useMemo(
    () => active?.mods.map((mod) => [mod.packageId, mod.version] as [string, string]) ?? [],
    [active?.mods],
  );
  const gameInstances = settings.gameInstances;
  const gameForProfile = (profile: Profile | undefined): GameInstance | null =>
    gameInstances.find((instance) => instance.id === profile?.gameInstanceId) ?? gameInstances[0] ?? null;
  const activeGame = gameForProfile(active);
  const arch: Arch = activeGame?.arch ?? "x86";
  const gameStatus = { store: activeGame?.store ?? "manual", arch, running };
  const busy = operationBusy || running;
  const firstRun = loaded && !settings.setupComplete;

  useEffect(() => {
    if (
      !loaded ||
      firstRun ||
      !active ||
      !activeGame ||
      operationBusy ||
      running ||
      initialWorkspacePreparationStarted.current
    ) {
      return;
    }
    initialWorkspacePreparationStarted.current = true;
    let current = true;
    void trackedExclusive(
      {
        scope: "profile",
        title: "Preparing selected profile",
        message: "Checking the isolated workspace",
      },
      (report) => bridge.syncProfile(activeGame.path, active.id, report),
    )
      .then((warning) => {
        if (current && warning) notify(warning, "error");
      })
      .catch((error) => {
        if (current && error !== OPERATION_BUSY) {
          notify(`Could not prepare ${active.name}: ${messageFrom(error)}`, "error");
        }
      });
    return () => {
      current = false;
    };
  }, [loaded, firstRun, active?.id, activeGame?.path, operationBusy, running]);

  useEffect(() => {
    if (!loaded || !active || !activeGame) {
      setUnmanagedPlugins([]);
      setUnmanagedScanError(null);
      setUnmanagedLoading(false);
      return;
    }
    let current = true;
    setUnmanagedLoading(true);
    setUnmanagedScanError(null);
    void bridge
      .listUnmanagedPlugins(activeGame.path, active.id)
      .then((plugins) => {
        if (current) {
          setUnmanagedPlugins(plugins);
          setUnmanagedScanError(null);
        }
      })
      .catch((error) => {
        if (current) {
          setUnmanagedPlugins([]);
          setUnmanagedScanError(messageFrom(error));
        }
      })
      .finally(() => {
        if (current) setUnmanagedLoading(false);
      });
    return () => {
      current = false;
    };
  }, [loaded, active?.id, active?.gameInstanceId, activeGame?.path]);

  if (!loaded) {
    return (
      <div className="grid h-[100dvh] place-items-center">
        <p className="subtitle text-ink-dim">Loading Perfect Sync…</p>
      </div>
    );
  }

  if (!active) {
    return (
      <div className="grid h-[100dvh] place-items-center px-8 text-center">
        <div>
          <p className="text-[15px] font-semibold text-ink">Perfect Sync couldn't start</p>
          <p className="mt-1 max-w-[420px] text-[13px] text-ink-dim">
            {startupError ?? "Failed to load your profiles."}
          </p>
          <button
            type="button"
            onClick={() => location.reload()}
            className="ring-focus accent-grad mt-4 rounded-xl px-5 py-2.5 text-[14px] font-bold text-[#0d0820]"
          >
            Retry
          </button>
        </div>
      </div>
    );
  }

  const patchProfile = (updated: Profile) => {
    setProfiles((current) =>
      current.some((profile) => profile.id === updated.id)
        ? current.map((profile) => (profile.id === updated.id ? updated : profile))
        : current,
    );
  };

  const requestUnmanagedPluginResolution = async (
    profile: Profile,
    continuation: boolean,
    targetInstance?: GameInstance,
  ): Promise<boolean> => {
    const instance = targetInstance ?? gameForProfile(profile);
    if (!instance) throw new Error("No Among Us source is assigned to this profile.");
    const plugins = await bridge.listUnmanagedPlugins(instance.path, profile.id);
    if (profile.id === active.id) {
      setUnmanagedPlugins(plugins);
      setUnmanagedScanError(null);
    }
    if (plugins.length === 0) return true;
    if (unmanagedPluginResolver.current) return false;

    const { promise, resolve } = Promise.withResolvers<boolean>();
    unmanagedPluginResolver.current = resolve;
    setUnmanagedPrompt({
      profileId: profile.id,
      profileName: profile.name,
      instanceName: instance.name,
      gamePath: instance.path,
      plugins,
      continuation,
    });
    return promise;
  };

  const closeUnmanagedPluginPrompt = (resolved: boolean) => {
    const resolve = unmanagedPluginResolver.current;
    unmanagedPluginResolver.current = null;
    setUnmanagedPrompt(null);
    resolve?.(resolved);
  };

  const resolveUnmanagedPlugins = async (
    action: "quarantine" | "delete" | "import",
    paths: readonly string[],
  ): Promise<boolean> => {
    const prompt = unmanagedPrompt;
    if (!prompt) return true;
    const selectedPaths = [...paths];
    let updated: Profile | null = null;
    if (action === "quarantine") {
      await bridge.quarantineUnmanagedPlugins(prompt.gamePath, prompt.profileId, selectedPaths);
    } else if (action === "delete") {
      await bridge.deleteUnmanagedPlugins(prompt.gamePath, prompt.profileId, selectedPaths);
    } else {
      updated = await bridge.importUnmanagedPlugins(
        prompt.gamePath,
        prompt.profileId,
        selectedPaths,
      );
      patchProfile(updated);
    }
    const remaining = await bridge.listUnmanagedPlugins(prompt.gamePath, prompt.profileId);
    if (prompt.profileId === active.id) setUnmanagedPlugins(remaining);
    setUnmanagedScanError(null);
    const count = selectedPaths.length;
    const resultMessage = action === "quarantine"
      ? `Moved ${count} plugin${count === 1 ? "" : "s"} to the instance quarantine.`
      : action === "delete"
        ? `Permanently deleted ${count} plugin${count === 1 ? "" : "s"} from the instance.`
        : `Added ${count} local plugin${count === 1 ? "" : "s"} to ${updated?.name ?? prompt.profileName}.`;
    if (remaining.length > 0) {
      setUnmanagedPrompt({ ...prompt, plugins: remaining });
      notify(`${resultMessage} ${remaining.length} still need${remaining.length === 1 ? "s" : ""} review.`);
      return false;
    }
    closeUnmanagedPluginPrompt(true);
    notify(resultMessage);
    return true;
  };

  const reviewUnmanagedPlugins = async (): Promise<void> => {
    if (!activeGame || unmanagedPluginResolver.current) return;
    setUnmanagedLoading(true);
    try {
      const plugins = await bridge.listUnmanagedPlugins(activeGame.path, active.id);
      setUnmanagedPlugins(plugins);
      setUnmanagedScanError(null);
      if (plugins.length === 0) {
        notify("No extra plugins were found in this game instance.");
        return;
      }
      setUnmanagedPrompt({
        profileId: active.id,
        profileName: active.name,
        instanceName: activeGame.name,
        gamePath: activeGame.path,
        plugins,
        continuation: false,
      });
    } catch (error) {
      const message = messageFrom(error);
      setUnmanagedScanError(message);
      notify(`Could not inspect the game’s plugin folder: ${message}`, "error");
    } finally {
      setUnmanagedLoading(false);
    }
  };

  const syncAuthoritativeProfile = async (
    profile: Profile,
    targetInstance?: GameInstance,
    onProgress?: (progress: OperationProgress) => void,
  ): Promise<string | null> => {
    if (!await requestUnmanagedPluginResolution(profile, true, targetInstance)) {
      throw UNMANAGED_REVIEW_CANCELLED;
    }
    const instance = targetInstance ?? gameForProfile(profile);
    if (!instance) throw new Error("No Among Us source is assigned to this profile.");
    return bridge.syncProfile(instance.path, profile.id, onProgress);
  };

  const trustOf = (id: string): Trust => {
    const identity = id.toLowerCase();
    if (identity === "bepinex/bepinex") return "trusted";
    return catalog.find(
      (item) => item.id.toLowerCase() === identity || item.repo.toLowerCase() === identity,
    )?.trust ?? "flagged";
  };

  const refreshProfileUpdates = async (profile: Profile): Promise<Profile> => {
    const instance = gameForProfile(profile);
    if (!instance) throw new Error("Assign an Among Us source before checking mod updates.");
    return bridge.checkModUpdates(profile.id, instance.arch);
  };

  const ensureLoaderInternal = async (profile: Profile): Promise<string | null> => {
    const instance = gameForProfile(profile);
    if (!instance) throw new Error("Add an Among Us folder in Settings before installing BepInEx.");
    return bridge.ensureLoader(instance.path, profile.id, instance.arch);
  };

  const selectProfile = async (id: string) => {
    if (id === active.id) return;
    try {
      await trackedExclusive(
        {
          scope: "profile",
          title: "Switching isolated profile",
          message: "Saving the selected profile",
        },
        async (report) => {
          const next = profiles.find((profile) => profile.id === id);
          if (!next) throw new Error("Profile not found.");
          const normalized = await bridge.saveSettings({ ...settings, activeProfile: id });
          setSettings(normalized);
          setActiveId(id);
          const instance = gameForProfile(next);
          if (instance) {
            report({ phase: "preparing", message: "Building the selected profile workspace" });
            const warning = await bridge.syncProfile(instance.path, next.id, report);
            notify(
              warning
                ? `${next.name} is ready. ${warning}`
                : `${next.name} is ready in its isolated workspace`,
              warning ? "error" : "success",
            );
          }
        },
      );
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const toggleMod = async (modId: string) => {
    try {
      const preflightProfile = profiles.find((candidate) => candidate.id === active.id);
      const preflightInstance = gameForProfile(preflightProfile);
      if (!preflightProfile || !preflightInstance) {
        throw new Error("Assign an Among Us source before changing mods.");
      }
      if (!await requestUnmanagedPluginResolution(preflightProfile, true, preflightInstance)) return;
      await trackedExclusive(
        {
          scope: "profile",
          title: "Updating isolated profile",
          message: "Changing the selected mod",
        },
        async (report) => {
          const profile = profiles.find((candidate) => candidate.id === active.id);
          const mod = profile?.mods.find((candidate) => candidate.packageId === modId);
          const instance = gameForProfile(profile);
          if (!profile || !mod || !instance) {
            throw new Error("The selected profile or Among Us source is no longer available.");
          }
          const updated = await bridge.setModEnabled(profile, modId, !mod.enabled);
          patchProfile(updated);
          report({ phase: "finalizing", message: "Rebuilding the isolated workspace" });
          try {
            const warning = await bridge.syncProfile(instance.path, updated.id, report);
            notify(
              warning ?? `${mod.name} is ${mod.enabled ? "disabled" : "enabled"} in the isolated workspace`,
              warning ? "error" : "success",
            );
          } catch (error) {
            notify(
              `${mod.name} was changed in the profile, but its workspace could not be rebuilt: ${messageFrom(error)}`,
              "error",
            );
          }
        },
      );
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const removeMod = async (modId: string): Promise<void> => {
    try {
      const preflightProfile = profiles.find((candidate) => candidate.id === active.id);
      const preflightInstance = gameForProfile(preflightProfile);
      if (!preflightProfile || !preflightInstance) {
        throw new Error("Assign an Among Us source before removing mods.");
      }
      if (!await requestUnmanagedPluginResolution(preflightProfile, true, preflightInstance)) return;
      await trackedExclusive(
        {
          scope: "profile",
          title: "Removing profile mod",
          message: "Updating the selected profile",
        },
        async (report) => {
          const profile = profiles.find((candidate) => candidate.id === active.id);
          const instance = gameForProfile(profile);
          if (!profile || !instance) {
            throw new Error("The selected profile or Among Us source is no longer available.");
          }
          const name = profile.mods.find((mod) => mod.packageId === modId)?.name ?? "mod";
          const updated = await bridge.removeMod(profile, modId);
          patchProfile(updated);
          report({ phase: "finalizing", message: "Rebuilding the isolated workspace" });
          try {
            const warning = await bridge.syncProfile(instance.path, updated.id, report);
            notify(
              warning ? `Removed ${name}. ${warning}` : `Removed ${name} from the isolated workspace`,
              warning ? "error" : "success",
            );
          } catch (error) {
            notify(
              `${name} was removed from the profile, but its workspace could not be rebuilt: ${messageFrom(error)}`,
              "error",
            );
          }
        },
      );
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const newProfile = async () => {
    try {
      await exclusive(async () => {
        const number = profiles.filter((profile) => profile.name.startsWith("New profile")).length + 1;
        const proposed: Profile = {
          id: `new-${Date.now()}`,
          name: `New profile ${number}`,
          crewColor: CREW_CYCLE[profiles.length % CREW_CYCLE.length],
          gameInstanceId: activeGame?.id,
          mods: [],
        };
        const saved = await bridge.saveProfile(proposed);
        const normalized = await bridge.saveSettings({ ...settings, activeProfile: saved.id });
        setSettings(normalized);
        setProfiles((current) => [...current, saved]);
        setActiveId(saved.id);
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const openAddPanel = () => {
    if (operationRef.current || runningRef.current) return;
    setSelectedCatalogIds([]);
    setMapsOpen(false);
    setAddOpen(true);
  };

  const toggleCatalogSelection = (id: string) => {
    setSelectedCatalogIds((current) =>
      current.includes(id) ? current.filter((selected) => selected !== id) : [...current, id],
    );
  };

  const reviewCatalogSelection = () => {
    const selected = new Set(selectedCatalogIds);
    const targets = catalog.filter((item) => selected.has(item.id));
    if (targets.length === 0) return;
    setBatchTargets(targets);
    setAddOpen(false);
  };

  const addUrl = async (url: string): Promise<void> => {
    if (operationRef.current || runningRef.current) return;
    const match = url.match(/github\.com\/([^/]+)\/([^/#?]+)/i);
    const repo = match ? `${match[1]}/${match[2]}` : url;
    const name = match ? match[2] : "Mod";
    if (active.mods.some((mod) => mod.packageId === repo || mod.repo === repo)) {
      notify(`${name} is already in this profile`, "error");
      return;
    }
    setPickerTarget({ repo, name, trust: trustOf(repo), returnToAdd: true });
  };

  const addLocalMod = async (): Promise<void> => {
    try {
      const preflightProfile = profiles.find((candidate) => candidate.id === active.id);
      const preflightInstance = gameForProfile(preflightProfile);
      if (!preflightProfile || !preflightInstance) {
        throw new Error("Assign an Among Us source before adding a local mod.");
      }
      if (!await requestUnmanagedPluginResolution(preflightProfile, true, preflightInstance)) return;
      await trackedExclusive(
        {
          scope: "profile",
          title: "Adding local mod",
          message: "Choose a local plugin",
        },
        async (report) => {
          const profile = profiles.find((candidate) => candidate.id === active.id);
          const instance = gameForProfile(profile);
          if (!profile || !instance) {
            throw new Error("The selected profile or Among Us source is no longer available.");
          }
          const installed = await bridge.installLocalMod(profile);
          if (!installed) return;
          patchProfile(installed);
          report({ phase: "finalizing", message: "Rebuilding the isolated workspace" });
          try {
            const warning = await bridge.syncProfile(instance.path, installed.id, report);
            notify(
              warning ? `Added local DLL. ${warning}` : "Added local DLL to the isolated workspace",
              warning ? "error" : "success",
            );
          } catch (error) {
            notify(
              `The local DLL was added to the profile, but its workspace could not be rebuilt: ${messageFrom(error)}`,
              "error",
            );
          }
        },
      );
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const installSelectedMods = async (selections: ModInstallSelection[]): Promise<void> => {
    const currentProfile = profiles.find((candidate) => candidate.id === active.id);
    const confirmed = await requestMainModConfirmation(
      currentProfile?.mods ?? [],
      selections,
      "Install anyway",
    );
    if (!confirmed) return;
    try {
      await trackedExclusive(
        {
          scope: "mods",
          title: `Installing ${selections.length} mod${selections.length === 1 ? "" : "s"}`,
          message: "Preparing the reviewed versions and files",
        },
        async (report) => {
          const profile = profiles.find((candidate) => candidate.id === active.id);
          const instance = gameForProfile(profile);
          if (!profile || !instance) throw new Error("Assign an Among Us source before installing mods.");
          const installed = await bridge.installAssets(profile, selections, true, report);
          const warnings: string[] = [];
          report({ phase: "finalizing", message: "Checking installed versions for updates" });
          let refreshed = installed;
          try {
            refreshed = await refreshProfileUpdates(installed);
          } catch (error) {
            warnings.push(`Update refresh failed: ${messageFrom(error)}`);
          }
          patchProfile(refreshed);
          report({ phase: "finalizing", message: "Checking the BepInEx loader" });
          const loaderWarning = await ensureLoaderInternal(refreshed);
          if (loaderWarning) warnings.push(loaderWarning);
          report({ phase: "finalizing", message: "Refreshing the trusted catalog" });
          try {
            setCatalog(await bridge.loadCatalog());
          } catch (error) {
            warnings.push(`Catalog reload failed: ${messageFrom(error)}`);
          }
          setBatchTargets([]);
          setSelectedCatalogIds([]);
          const count = selections.length;
          notify(
            warnings.length > 0
              ? `Installed ${count} mod${count === 1 ? "" : "s"}. ${warnings.join(" ")}`
              : `Installed ${count} mod${count === 1 ? "" : "s"}`,
            warnings.length > 0 ? "error" : "success",
          );
        },
      );
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const installSelectedMaps = async (maps: LevelImposterMap[]): Promise<void> => {
    try {
      const preflightProfile = profiles.find((candidate) => candidate.id === active.id);
      const preflightInstance = gameForProfile(preflightProfile);
      if (!preflightProfile || !preflightInstance) {
        throw new Error("Assign an Among Us source before installing maps.");
      }
      if (!await requestUnmanagedPluginResolution(preflightProfile, true, preflightInstance)) return;
      await trackedExclusive(
        {
          scope: "maps",
          title: `Installing ${maps.length} map${maps.length === 1 ? "" : "s"}`,
          message: "Preparing LevelImposter map downloads",
        },
        async (report) => {
          const profile = profiles.find((candidate) => candidate.id === active.id);
          const instance = gameForProfile(profile);
          if (!profile || !instance) throw new Error("Assign an Among Us source before installing maps.");
          const count = maps.length;
          const installed = await bridge.installLevelImposterMaps(
            profile,
            maps.map((map) => map.id),
            (progress) => {
              const currentMap = progress.phase === "downloading"
                ? maps.find((map) => progress.message.includes(map.id))
                : undefined;
              report(currentMap
                ? { ...progress, message: `Downloading ${currentMap.name} by ${currentMap.authorName}` }
                : progress);
            },
          );
          report({ phase: "finalizing", message: "Rebuilding the isolated workspace with the selected maps" });
          patchProfile(installed);
          let syncError: string | null = null;
          let warning: string | null = null;
          try {
            warning = await syncAuthoritativeProfile(installed, instance);
          } catch (error) {
            syncError = messageFrom(error);
          }
          report({ phase: "finalizing", message: "Checking installed versions for updates" });
          let refreshed = installed;
          try {
            refreshed = await refreshProfileUpdates(installed);
          } catch (error) {
            notify(`Maps installed, but update refresh failed: ${messageFrom(error)}`, "error");
          }
          patchProfile(refreshed);
          setMapsOpen(false);
          setMapsReturnToAdd(false);
          setSelectedCatalogIds([]);
          const mapFolder = "BepInEx\\plugins\\LevelImposter";
          notify(
            syncError
              ? `Downloaded ${count} map${count === 1 ? "" : "s"} to the profile, but could not synchronize ${mapFolder}: ${syncError}`
              : warning
                ? `Downloaded ${count} map${count === 1 ? "" : "s"} to ${mapFolder}. ${warning}`
                : `Downloaded ${count} map${count === 1 ? "" : "s"} to ${mapFolder} for ${profile.name}`,
            syncError || warning ? "error" : "success",
          );
        },
      );
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const removeInstalledMaps = async (mapIds: string[]): Promise<void> => {
    try {
      const preflightProfile = profiles.find((candidate) => candidate.id === active.id);
      const preflightInstance = gameForProfile(preflightProfile);
      if (!preflightProfile || !preflightInstance) {
        throw new Error("Assign an Among Us source before removing maps.");
      }
      if (!await requestUnmanagedPluginResolution(preflightProfile, true, preflightInstance)) return;
      await exclusive(async () => {
        const profile = profiles.find((candidate) => candidate.id === active.id);
        const instance = gameForProfile(profile);
        if (!profile || !instance) throw new Error("Assign an Among Us source before removing maps.");
        const removed = await bridge.removeLevelImposterMaps(profile, mapIds);
        patchProfile(removed);
        let syncError: string | null = null;
        let warning: string | null = null;
        try {
          warning = await syncAuthoritativeProfile(removed, instance);
        } catch (error) {
          syncError = messageFrom(error);
        }
        const count = mapIds.length;
        notify(
          syncError
            ? `Removed ${count} map${count === 1 ? "" : "s"} from the profile, but could not synchronize the game: ${syncError}`
            : warning
              ? `Removed ${count} map${count === 1 ? "" : "s"}. ${warning}`
              : `Removed ${count} LevelImposter map${count === 1 ? "" : "s"}`,
          syncError || warning ? "error" : "success",
        );
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const renameProfile = async (name: string) => {
    try {
      await exclusive(async () => {
        const profile = profiles.find((candidate) => candidate.id === active.id);
        if (!profile) return;
        patchProfile(await bridge.saveProfile({ ...profile, name }));
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const selectGameInstance = async (id: string) => {
    try {
      await trackedExclusive(
        {
          scope: "profile",
          title: "Changing profile source",
          message: "Saving the selected source",
        },
        async (report) => {
          const profile = profiles.find((candidate) => candidate.id === active.id);
          const instance = gameInstances.find((candidate) => candidate.id === id);
          if (!profile || !instance || profile.gameInstanceId === id) return;
          const saved = await bridge.saveProfile({ ...profile, gameInstanceId: id });
          patchProfile(saved);
          report({ phase: "preparing", message: "Rebuilding the isolated workspace from the new source" });
          const warning = await bridge.syncProfile(instance.path, saved.id, report);
          notify(
            warning
              ? `Source changed. ${warning}`
              : "Source changed and the isolated workspace is ready.",
            warning ? "error" : "success",
          );
        },
      );
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const deleteActiveProfile = async (): Promise<void> => {
    try {
      await exclusive(async () => {
        const profile = profiles.find((candidate) => candidate.id === active.id);
        if (!profile) throw new Error("Profile no longer exists.");
        const left = profiles.filter((candidate) => candidate.id !== profile.id);
        let replacement: Profile | undefined;
        if (left.length === 0) {
          replacement = await bridge.saveProfile({
            id: profile.id === "my-mods-a" ? "my-mods-b" : "my-mods-a",
            name: "My mods",
            crewColor: CREW.violet,
            gameInstanceId: activeGame?.id,
            mods: [],
          });
        }
        await bridge.deleteProfile(profile.id);
        const nextProfiles = replacement ? [replacement] : left;
        setProfiles(nextProfiles);
        setActiveId(nextProfiles[0].id);
        setSettings((current) => ({ ...current, activeProfile: undefined }));
        notify(`Deleted ${profile.name}`);
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const openPicker = (modId: string) => {
    if (operationRef.current || runningRef.current) return;
    const mod = active.mods.find((candidate) => candidate.packageId === modId);
    if (mod) {
      const repo = mod.repo ?? mod.packageId;
      setPickerTarget({
        repo,
        name: mod.name,
        trust: trustOf(repo),
        currentVersion: mod.version,
        recommendedVersion: mod.update,
      });
    }
  };


  const pickRelease = async (repo: string, tag: string, assetName: string) => {
    const target = pickerTarget;
    if (!target || target.repo !== repo) return;
    const replacing = active.mods.some(
      (mod) => mod.repo === target.repo || mod.packageId === target.repo,
    );
    if (
      !replacing &&
      !await requestMainModConfirmation(active.mods, [{ repo: target.repo }], "Install anyway")
    ) return;
    try {
      await trackedExclusive(
        {
          scope: "release",
          title: replacing ? `Changing ${target.name} to ${tag}` : `Installing ${target.name}`,
          message: `Preparing ${assetName}`,
        },
        async (report) => {
          const profile = profiles.find((candidate) => candidate.id === active.id);
          const instance = gameForProfile(profile);
          if (!profile || !instance) throw new Error("Assign an Among Us source before installing a mod.");
          const installed = await bridge.installAsset(
            profile,
            target.repo,
            tag,
            assetName,
            instance.arch,
            true,
            report,
          );
          const warnings: string[] = [];
          report({ phase: "finalizing", message: "Checking installed versions for updates" });
          let refreshed = installed;
          try {
            refreshed = await refreshProfileUpdates(installed);
          } catch (error) {
            warnings.push(`Update refresh failed: ${messageFrom(error)}`);
          }
          patchProfile(refreshed);
          report({ phase: "finalizing", message: "Checking the BepInEx loader" });
          const loaderWarning = await ensureLoaderInternal(refreshed);
          if (loaderWarning) warnings.push(loaderWarning);
          report({ phase: "finalizing", message: "Refreshing the trusted catalog" });
          try {
            setCatalog(await bridge.loadCatalog());
          } catch (error) {
            warnings.push(`Catalog reload failed: ${messageFrom(error)}`);
          }
          setPickerTarget(null);
          if (target.returnToAdd) setAddOpen(false);
          notify(
            warnings.length > 0 ? `Installed ${assetName}. ${warnings.join(" ")}` : `Installed ${assetName}`,
            warnings.length > 0 ? "error" : "success",
          );
        },
      );
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const removeCatalogItem = async (id: string) => {
    try {
      await exclusive(async () => setCatalog(await bridge.removeCatalogMod(catalog, id)));
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const moveCatalogItem = async (id: string, direction: "up" | "down") => {
    const ids = catalog.map((item) => item.id);
    const index = ids.indexOf(id);
    const destination = direction === "up" ? index - 1 : index + 1;
    if (index < 0 || destination < 0 || destination >= ids.length) return;
    [ids[index], ids[destination]] = [ids[destination], ids[index]];
    try {
      await exclusive(async () => setCatalog(await bridge.reorderCatalog(catalog, ids)));
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const monitorGame = () => {
    const session = ++launchSession.current;
    const startedAt = Date.now();
    let seen = false;
    const poll = async () => {
      if (launchSession.current !== session) return;
      try {
        const alive = await bridge.gameRunning();
        if (launchSession.current !== session) return;
        if (alive) {
          seen = true;
          window.setTimeout(poll, 2000);
        } else if (seen || Date.now() - startedAt > 20000) {
          setRunning(false);
        } else {
          window.setTimeout(poll, 2000);
        }
      } catch (error) {
        if (launchSession.current === session) {
          setRunning(false);
          notify(`Could not read game status: ${messageFrom(error)}`, "error");
        }
      }
    };
    window.setTimeout(poll, 2000);
  };

  const launchInternal = async (profile: Profile, vanilla: boolean) => {
    const instance = gameForProfile(profile);
    if (!instance) throw new Error("No Among Us source is assigned to this profile.");
    if (!vanilla && !await requestUnmanagedPluginResolution(profile, true, instance)) {
      throw UNMANAGED_REVIEW_CANCELLED;
    }
    setRunning(true);
    try {
      if (vanilla) await bridge.launchVanilla(instance.path);
      else await bridge.launchProfile(instance.path, profile.id);
      notify(
        instance.store === "epic"
          ? `Launching ${vanilla ? "vanilla Among Us" : profile.name}. Epic may ask you to sign in the first time, that's normal.`
          : `Launching ${vanilla ? "vanilla Among Us" : profile.name}`,
      );
      monitorGame();
    } catch (error) {
      setRunning(false);
      throw error;
    }
  };

  const doLaunchProfile = async (profile: Profile) => {
    try {
      await exclusive(async () => {
        const current = profiles.find((candidate) => candidate.id === profile.id);
        const instance = gameForProfile(current);
        if (!current || !instance) throw new Error("No Among Us source is assigned. Add one in Settings.");
        if (!settings.skipLaunchWarning) {
          const status = await bridge.loaderStatus(instance.path, current.id);
          if (!status.current || !status.runtimeReady) {
            setLaunchWarn(current);
            return;
          }
        }
        await launchInternal(current, false);
      });
    } catch (error) {
      if (error !== OPERATION_BUSY && error !== UNMANAGED_REVIEW_CANCELLED) notify(messageFrom(error), "error");
    }
  };

  const launchWarnInstall = async () => {
    const profile = launchWarn;
    if (!profile) return;
    try {
      await exclusive(async () => {
        const warning = await ensureLoaderInternal(profile);
        if (warning) throw new Error(warning);
        setLaunchWarn(null);
        await launchInternal(profile, false);
      });
    } catch (error) {
      if (error !== OPERATION_BUSY && error !== UNMANAGED_REVIEW_CANCELLED) notify(messageFrom(error), "error");
    }
  };

  const launchWarnAnyway = async (dontWarnAgain: boolean) => {
    const profile = launchWarn;
    if (!profile) return;
    try {
      await exclusive(async () => {
        if (dontWarnAgain) {
          const normalized = await bridge.saveSettings({ ...settings, skipLaunchWarning: true });
          setSettings(normalized);
        }
        setLaunchWarn(null);
        await launchInternal(profile, true);
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const openLobby = () => {
    setLobbyCode(undefined);
    setLobbyOpen(true);
  };

  const applyLobby = async (
    doLaunch: boolean,
    code: string,
    mods: readonly MainModCandidate[],
  ) => {
    const confirmed = await requestMainModConfirmation(
      [],
      mods,
      doLaunch ? "Apply & launch anyway" : "Apply anyway",
    );
    if (!confirmed) return;
    try {
      if (!activeGame) throw new Error("Choose an Among Us source before applying a lobby.");
      if (!await requestUnmanagedPluginResolution(active, true, activeGame)) return;
      await trackedExclusive(
        {
          scope: "lobby",
          title: doLaunch ? "Setting up and launching lobby" : "Setting up shared lobby",
          message: "Reading the shared profile, exact versions, assets, and maps",
        },
        async (report) => {
          const instance = activeGame;
          if (!instance) throw new Error("Choose an Among Us source before applying a lobby.");
          const warnings: string[] = [];
          let built = await bridge.applyLobbyCode(code, instance.arch, instance.id, report);
          report({ phase: "finalizing", message: "Checking installed versions for updates" });
          try {
            built = await refreshProfileUpdates(built);
          } catch (error) {
            warnings.push(`Update refresh failed: ${messageFrom(error)}`);
          }
          report({ phase: "finalizing", message: "Selecting the new lobby profile" });
          const normalized = await bridge.saveSettings({ ...settings, activeProfile: built.id });
          setSettings(normalized);
          setProfiles((current) => [...current.filter((profile) => profile.id !== built.id), built]);
          setActiveId(built.id);
          report({ phase: "finalizing", message: "Checking the BepInEx loader" });
          const loaderWarning = await ensureLoaderInternal(built);
          if (doLaunch) {
            if (loaderWarning) throw new Error(loaderWarning);
            report({ phase: "finalizing", message: `Starting ${built.name}` });
            await launchInternal(built, false);
            setLobbyOpen(false);
            if (warnings.length > 0) notify(`Lobby launched. ${warnings.join(" ")}`, "error");
          } else {
            report({ phase: "finalizing", message: "Building the lobby profile in its isolated workspace" });
            const warning = (await syncAuthoritativeProfile(built, instance)) ?? loaderWarning;
            setLobbyOpen(false);
            const details = [warning, ...warnings].filter(Boolean).join(" ");
            notify(details ? `Lobby profile ready: ${built.name}. ${details}` : `Lobby profile ready: ${built.name}`, details ? "error" : "success");
          }
        },
      );
    } catch (error) {
      if (error !== OPERATION_BUSY && error !== UNMANAGED_REVIEW_CANCELLED) notify(messageFrom(error), "error");
    }
  };

  const applyReviewedUpdates = async (packageIds: string[]) => {
    try {
      await trackedExclusive(
        {
          scope: "mods",
          title: "Applying reviewed profile updates",
          message: "Resolving the selected releases and dependencies",
        },
        async (report) => {
          const instance = activeGame;
          if (!instance) throw new Error("Choose an Among Us source before updating mods.");
          const updated = await bridge.applyModUpdates(active, packageIds, instance.arch, report);
          patchProfile(updated);
          setUpdateReviewOpen(false);
          const loaderWarning = await ensureLoaderInternal(updated);
          notify(
            loaderWarning
              ? `Applied ${packageIds.length} reviewed update${packageIds.length === 1 ? "" : "s"}. ${loaderWarning}`
              : `Applied ${packageIds.length} reviewed update${packageIds.length === 1 ? "" : "s"}.`,
            loaderWarning ? "error" : "success",
          );
        },
      );
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const saveSettings = async (draft: Settings, tokenAction: GithubTokenAction): Promise<void> => {
    try {
      await exclusive(async () => {
        const normalized = await bridge.saveSettings(draft, tokenAction);
        setSettings(normalized);
        setGames(normalized.gameInstances);
        setSettingsOpen(false);
        notify("Settings saved");
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const moveStorage = async (storagePath?: string): Promise<void> => {
    const normalized = await trackedExclusive(
      {
        scope: "storage",
        title: "Moving Perfect Sync storage",
        message: "Validating the selected storage folder",
      },
      (report) => bridge.moveStorage(storagePath, report),
    );
    setSettings(normalized);
    notify(normalized.storageWarning ?? "Managed storage location updated", normalized.storageWarning ? "error" : "success");
  };
  const saveErrorLog = async (): Promise<void> => {
    const destination = await exclusive(() => bridge.exportErrorLog());
    if (destination) notify("BepInEx error log saved", "success");
  };



  const completeSetup = async (
    gamePath?: string,
    selectedArch?: string,
    store?: string,
    runtime?: Runtime,
    selection?: SetupSelection,
  ): Promise<boolean> => {
    if (
      selection?.kind === "tou" &&
      !await requestMainModConfirmation(
        active.mods,
        [{ repo: "AU-Avengers/TOU-Mira" }],
        "Install anyway",
      )
    ) return false;
    try {
      await trackedExclusive(
        {
          scope: "setup",
          title: selection?.kind === "tou" ? "Installing Town of Us" : "Finishing setup",
          message: "Saving the selected Among Us source",
        },
        async (report) => {
          const instances = [...gameInstances];
          let gameInstanceId = active.gameInstanceId;
          let selectedInstance: GameInstance | undefined;
          if (gamePath) {
            let instance = instances.find(
              (candidate) =>
                candidate.path.replaceAll("\\", "/").toLowerCase() ===
                gamePath.replaceAll("\\", "/").toLowerCase(),
            );
            if (!instance) {
              const detected = games.find((candidate) => candidate.path === gamePath);
              const instanceStore = (store as Store | undefined) ?? detected?.store ?? "manual";
              const storeCount = instances.filter((candidate) => candidate.store === instanceStore).length;
              const baseName = INSTANCE_NAMES[instanceStore];
              instance = {
                id: `game-${Date.now().toString(36)}`,
                name: storeCount === 0 ? baseName : `${baseName} ${storeCount + 1}`,
                path: gamePath,
                arch: (selectedArch as Arch | undefined) ?? detected?.arch ?? "x86",
                store: instanceStore,
                runtime: runtime ?? detected?.runtime ?? "native",
              };
              instances.push(instance);
            }
            selectedInstance = instance;
            gameInstanceId = instance.id;
          }
          if (selection && !selectedInstance) {
            throw new Error("Choose an Among Us source before preparing the isolated workspace.");
          }

          report({ phase: "preparing", message: "Saving the game source and active profile" });
          // Keep the existing completion state while a rerun is in progress. Each
          // successful persistence step is mirrored locally so a failed download
          // cannot leave the frontend trying to delete a newly assigned instance.
          const provisional = await bridge.saveSettings({
            ...settings,
            gameInstances: instances,
          });
          setSettings(provisional);
          setGames(provisional.gameInstances);
          let savedProfile = await bridge.saveProfile({ ...active, gameInstanceId });
          patchProfile(savedProfile);
          if (selection?.kind === "tou" && selectedInstance) {
            report({ phase: "resolving", message: `Resolving Town of Us ${selection.tag}` });
            savedProfile = await bridge.installAsset(
              savedProfile,
              "AU-Avengers/TOU-Mira",
              selection.tag,
              selection.assetName,
              selectedInstance.arch,
              true,
              report,
            );
            patchProfile(savedProfile);
            report({ phase: "finalizing", message: "Building a fresh isolated Town of Us workspace" });
            await syncAuthoritativeProfile(savedProfile, selectedInstance);
          }
          if (selection?.kind === "bepinex" && selectedInstance) {
            report({ phase: "finalizing", message: "Installing BepInEx into the isolated workspace" });
            await bridge.ensureLoader(
              selectedInstance.path,
              savedProfile.id,
              selectedInstance.arch,
              selection.applyDoorstopFix,
              report,
            );
          }
          report({ phase: "finalizing", message: "Marking first-time setup complete" });
          const normalized = await bridge.saveSettings({
            ...provisional,
            setupComplete: true,
            freshSourceSetupComplete: true,
          });
          setSettings(normalized);
          setSetupOpen(false);
          setFreshSourceMigration(false);
          patchProfile(savedProfile);
        },
      );
      return true;
    } catch (error) {
      if (error === UNMANAGED_REVIEW_CANCELLED) return false;
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const dismissSetup = async () => {
    if (freshSourceMigration) {
      setFreshSourceMigration(false);
      return;
    }
    if (setupOpen) {
      setSetupOpen(false);
      return;
    }
    try {
      await exclusive(async () => {
        const normalized = await bridge.saveSettings({ ...settings, setupComplete: true });
        setSettings(normalized);
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const setupMods = async (profile: Profile) => {
    try {
      await exclusive(async () => {
        const current = profiles.find((candidate) => candidate.id === profile.id);
        const instance = gameForProfile(current);
        if (!current || !instance) throw new Error("No Among Us source is assigned. Add one in Settings.");
        notify("Preparing an isolated mod workspace…");
        const warning = await syncAuthoritativeProfile(current, instance);
        notify(
          warning
            ? `The isolated workspace is ready. ${warning}`
            : "Mods are ready in the isolated workspace. The original game source was not modified.",
        );
      });
    } catch (error) {
      if (error !== OPERATION_BUSY && error !== UNMANAGED_REVIEW_CANCELLED) notify(messageFrom(error), "error");
    }
  };

  const openUpdate = async () => {
    if (!update || busy) return;
    try {
      await installApplicationUpdate(update);
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(`Application update failed: ${messageFrom(error)}`, "error");
    }
  };

  const topLevelOverlayOpen =
    addOpen ||
    batchTargets.length > 0 ||
    mapsOpen ||
    lobbyOpen ||
    settingsOpen ||
    pickerTarget !== null ||
    shareOpen ||
    firstRun ||
    setupOpen ||
    updateReviewOpen ||
    launchWarn !== null ||
    mainModWarning !== null ||
    unmanagedPrompt !== null;

  return (
    <div className="flex h-[100dvh] flex-col max-[720px]:h-auto max-[720px]:min-h-[100dvh]">
      <div
        className="flex min-h-0 flex-1 flex-col max-[720px]:overflow-visible"
        inert={topLevelOverlayOpen}
        aria-hidden={topLevelOverlayOpen}
      >
      <TopBar
        onAddMod={openAddPanel}
        onJoinLobby={openLobby}
        onOpenSettings={() => {
          if (!busy) setSettingsOpen(true);
        }}
      />

      {update && !updateDismissed && (
        <div className="mx-3 mt-2 flex items-center gap-3 rounded-xl border border-[rgba(123,150,255,0.35)] bg-[rgba(123,150,255,0.12)] px-4 py-2 text-[13px] text-[#cbd8ff]">
          <span className="flex-1">Automatic update to Perfect Sync {update.version} needs another attempt.</span>
          <button
            type="button"
            onClick={() => void openUpdate()}
            disabled={busy}
            className="ring-focus accent-grad rounded-lg px-3 py-1.5 text-[12.5px] font-semibold text-[#0d0820] disabled:cursor-not-allowed disabled:opacity-45"
          >
            Retry & restart
          </button>
          <button
            type="button"
            onClick={() => setUpdateDismissed(true)}
            aria-label="Dismiss update"
            className="ring-focus rounded-lg px-2 py-1 text-ink-faint hover:text-ink"
          >
            Dismiss
          </button>
        </div>
      )}

      <div className="flex min-h-0 min-w-0 flex-1 p-3 pt-2.5 max-[720px]:p-2">
        <div className="glass flex min-h-0 min-w-0 flex-1 overflow-hidden rounded-3xl max-[720px]:w-full max-[720px]:flex-col max-[720px]:overflow-visible max-[720px]:rounded-2xl">
          <Sidebar
            profiles={profiles}
            activeId={active.id}
            busy={busy}
            onSelect={(id) => void selectProfile(id)}
            onNewProfile={() => void newProfile()}
          />
          <MainPanel
            profile={active}
            game={gameStatus}
            gameInstances={gameInstances}
            busy={busy}
            unmanagedPlugins={unmanagedPlugins}
            unmanagedLoading={unmanagedLoading}
            unmanagedError={unmanagedScanError}
            trustOf={trustOf}
            onToggle={(id) => void toggleMod(id)}
            onRemove={removeMod}
            onPickRelease={openPicker}
            onShare={() => {
              if (!busy) setShareOpen(true);
            }}
            onRename={(name) => void renameProfile(name)}
            onDelete={deleteActiveProfile}
            onLaunch={() => void doLaunchProfile(active)}
            onAddMod={openAddPanel}
            onReviewUpdates={() => {
              if (!busy) setUpdateReviewOpen(true);
            }}
            onBrowseMaps={() => {
              setMapsReturnToAdd(false);
              if (!busy) setMapsOpen(true);
            }}
            onSetup={() => void setupMods(active)}
            onSelectGameInstance={(id) => void selectGameInstance(id)}
            onManageGameInstances={() => {
              if (!busy) setSettingsOpen(true);
            }}
            onReviewUnmanaged={() => void reviewUnmanagedPlugins()}
          />
        </div>
      </div>
      </div>

      <AddModPanel
        open={addOpen}
        profileName={active.name}
        catalog={catalog}
        installedIds={active.mods.flatMap((mod) => [mod.packageId, mod.repo ?? mod.packageId])}
        selectedIds={selectedCatalogIds}
        onClose={() => {
          if (!operationRef.current) {
            setAddOpen(false);
            setSelectedCatalogIds([]);
          }
        }}
        onToggleCatalog={toggleCatalogSelection}
        onReview={reviewCatalogSelection}
        onBrowseMaps={() => {
          if (!operationRef.current) {
            setMapsReturnToAdd(true);
            setAddOpen(false);
            setMapsOpen(true);
          }
        }}
        onAddUrl={addUrl}
        onAddLocal={addLocalMod}
        onRemoveCatalog={removeCatalogItem}
        onMoveCatalog={moveCatalogItem}
      />
      <MapBrowserPanel
        open={mapsOpen}
        profileId={active.id}
        profileName={active.name}
        levelImposterInstalled={active.mods.some(
          (mod) =>
            mod.packageId.toLowerCase() === "digiworm0/levelimposter" ||
            mod.repo?.toLowerCase() === "digiworm0/levelimposter",
        )}
        busy={operationBusy}
        onClose={() => {
          if (!operationRef.current) {
            setMapsOpen(false);
            if (mapsReturnToAdd) setAddOpen(true);
            setMapsReturnToAdd(false);
          }
        }}
        onInstall={installSelectedMaps}
        onRemove={removeInstalledMaps}
      />
      <BatchInstallReview
        open={batchTargets.length > 0}
        profileId={active.id}
        profileName={active.name}
        items={batchTargets}
        catalog={catalog}
        installedMods={active.mods}
        busy={operationBusy}
        onBack={() => {
          if (!operationRef.current) {
            setBatchTargets([]);
            setAddOpen(true);
          }
        }}
        onClose={() => {
          if (!operationRef.current) {
            setBatchTargets([]);
            setSelectedCatalogIds([]);
          }
        }}
        onInstall={installSelectedMods}
      />
      <LobbyCodeModal
        open={lobbyOpen}
        initialCode={lobbyCode}
        installed={installedSnapshot}
        trustOf={trustOf}
        personalMods={settings.personalMods}
        busyReason={running ? "Close Among Us before applying this lobby." : operationBusy ? "Wait for the current operation to finish." : undefined}
        onClose={() => {
          if (!operationRef.current) setLobbyOpen(false);
        }}
        onApply={applyLobby}
      />
      <SettingsModal
        open={settingsOpen}
        settings={settings}
        profileId={active.id}
        profileGameInstanceId={active.gameInstanceId}
        profileUsesTou={active.mods.some(
          (mod) =>
            mod.enabled &&
            (mod.packageId.toLowerCase() === "au-avengers/tou-mira" ||
              mod.repo?.toLowerCase() === "au-avengers/tou-mira"),
        )}
        onClose={() => {
          if (!operationRef.current) setSettingsOpen(false);
        }}
        onSave={saveSettings}
        onMoveStorage={moveStorage}
        onSaveErrorLog={saveErrorLog}
        onRunSetup={() => {
          setSettingsOpen(false);
          setSetupOpen(true);
        }}
        trustOf={trustOf}
      />
      <BatchUpdateReview
        open={updateReviewOpen}
        profile={active}
        busy={operationBusy}
        onClose={() => setUpdateReviewOpen(false)}
        onApply={(packageIds) => void applyReviewedUpdates(packageIds)}
      />
      <ReleasePicker
        open={pickerTarget !== null}
        repo={pickerTarget?.repo ?? ""}
        modName={pickerTarget?.name ?? ""}
        trust={pickerTarget?.trust ?? "flagged"}
        busy={operationBusy}
        profileId={active.id}
        currentVersion={pickerTarget?.currentVersion}
        recommendedVersion={pickerTarget?.recommendedVersion}
        onClose={() => {
          if (!operationRef.current) setPickerTarget(null);
        }}
        onPick={pickRelease}
      />
      <ShareModal
        open={shareOpen}
        profile={active}
        onClose={() => {
          if (!operationRef.current) setShareOpen(false);
        }}
      />
      <SetupModal
        open={firstRun || setupOpen || freshSourceMigration}
        migrationRequired={freshSourceMigration}
        detected={games}
        activeStoragePath={settings.activeStoragePath}
        defaultStoragePath={settings.defaultStoragePath}
        onMoveStorage={moveStorage}
        onFinish={completeSetup}
        onDismiss={dismissSetup}
      />
      <LaunchWarning
        open={launchWarn !== null}
        onInstall={launchWarnInstall}
        onLaunchAnyway={launchWarnAnyway}
        onCancel={() => {
          if (!operationRef.current) setLaunchWarn(null);
        }}
      />
      <UnmanagedPluginsModal
        open={unmanagedPrompt !== null}
        profileName={unmanagedPrompt?.profileName ?? active.name}
        instanceName={unmanagedPrompt?.instanceName ?? activeGame?.name ?? "Selected instance"}
        plugins={unmanagedPrompt?.plugins ?? []}
        continuation={unmanagedPrompt?.continuation ?? false}
        onCancel={() => closeUnmanagedPluginPrompt(false)}
        onQuarantine={(paths) => resolveUnmanagedPlugins("quarantine", paths)}
        onDelete={(paths) => resolveUnmanagedPlugins("delete", paths)}
        onImport={(paths) => resolveUnmanagedPlugins("import", paths)}
      />
      {mainModWarning && (
        <MainModWarning
          key={mainModWarning.mods.map((mod) => mod.id).join("|")}
          mods={mainModWarning.mods}
          actionLabel={mainModWarning.actionLabel}
          onCancel={() => resolveMainModWarning(false)}
          onConfirm={() => resolveMainModWarning(true)}
        />
      )}
      <OperationProgressModal activity={operationActivity} />
      <Toast toast={toast} onDismiss={() => setToast(null)} />
    </div>
  );
}
