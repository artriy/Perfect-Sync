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
const MOD_UPDATE_REFRESH_MS = 30_000;

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
  supportLogging: false,
  hasGithubToken: false,
  freshSourceSetupComplete: false,
  activeStoragePath: "",
  defaultStoragePath: "",
};
interface StartupResult {
  settings: Settings;
  catalog: CatalogItem[];
  profiles: Profile[];
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
interface ProfileSelectionIntent {
  request: number;
  id: string;
}

interface GameInstanceSelectionIntent {
  request: number;
  profileId: string;
  desiredId: string;
  persistedId?: string;
}

interface ModToggleIntent {
  request: number;
  profileId: string;
  packageId: string;
  desired: boolean;
  persisted: boolean;
}


function messageFrom(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (/^HTTP status 403$/i.test(message.trim())) {
    return "HTTP 403: GitHub temporarily refused this web request. Normal catalog installs do not use REST API quota; retry shortly and verify that github.com is reachable.";
  }
  if (/(source|original).*(unavailable|not found|does not exist|cannot be reached)|cannot access.*(source|original)/i.test(message)) {
    return `The recorded original Among Us source is unavailable. Existing valid direct instances can still launch. Reconnect its drive or choose the exact original folder again in Settings before building or repairing. ${message}`;
  }
  if (/(source.*(fingerprint|build).*(changed|differ|mismatch)|source changed)/i.test(message)) {
    return `The original Among Us source changed since its source record was saved. Check the original source in Settings and save its updated fingerprint and build before rebuilding this direct instance. ${message}`;
  }
  if (/(storage.*(inside|overlap|contain).*(source|among us)|(source|among us).*(inside|overlap|contain).*storage|storage.*(source|among us).*cannot contain|cannot contain one another|unsafe storage)/i.test(message)) {
    return `Perfect Sync storage overlaps the original Among Us source. Move storage to a non-overlapping location in Settings before building a direct instance. ${message}`;
  }
  if (/(invalid.*(source|among us)|(source|among us).*(invalid|not (?:a )?valid)|mod-loader artifacts|among us executable|non-link directory|regular.*directory)/i.test(message)) {
    return `The selected original Among Us source is invalid. Verify or repair the game in its store, then check the original folder again. ${message}`;
  }
  return message;
}

export function App() {
  const [loaded, setLoaded] = useState(false);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeId, setActiveId] = useState("");
  const [runningStatus, setRunningStatus] = useState({ profileId: "", running: false, known: false });
  const [operationBusy, setOperationBusy] = useState(false);
  const [operationActivity, setOperationActivity] = useState<OperationActivity | null>(null);
  const [profileMutationCount, setProfileMutationCount] = useState(0);
  const [pendingGameInstanceProfiles, setPendingGameInstanceProfiles] = useState<Set<string>>(new Set());
  const [pendingModToggles, setPendingModToggles] = useState<Set<string>>(new Set());
  const [workspacePreparationProfileId, setWorkspacePreparationProfileId] = useState("");

  const operationRef = useRef(false);
  const startupPromiseRef = useRef<Promise<StartupResult> | null>(null);
  const operationActivityId = useRef(0);
  const initialWorkspacePreparationStarted = useRef(false);
  const workspacePreparationPromise = useRef<Promise<void> | null>(null);
  const automaticUpdateRef = useRef(false);
  const mainModWarningResolver = useRef<((confirmed: boolean) => void) | null>(null);
  const unmanagedPluginResolver = useRef<((resolved: boolean) => void) | null>(null);
  const profileSelectionRequest = useRef(0);
  const profileSelectionIntent = useRef<ProfileSelectionIntent | null>(null);
  const profileSelectionWorker = useRef(false);
  const persistedActiveProfile = useRef("");
  const gameInstanceSelectionRequest = useRef(0);
  const gameInstanceSelectionIntents = useRef(new Map<string, GameInstanceSelectionIntent>());
  const gameInstanceSelectionWorkers = useRef(new Set<string>());
  const modToggleRequest = useRef(0);
  const modToggleIntents = useRef(new Map<string, ModToggleIntent>());
  const modToggleWorkers = useRef(new Set<string>());
  const profileMutationQueue = useRef<Promise<void>>(Promise.resolve());
  const profileMutationCountRef = useRef(0);
  const profileUpdateRefreshRef = useRef<Promise<void> | null>(null);
  const profileUpdateWarningShownRef = useRef(false);
  const profilesRef = useRef<Profile[]>([]);
  const activeIdRef = useRef("");
  const settingsRef = useRef<Settings>(EMPTY_SETTINGS);

  const [games, setGames] = useState<GameInstall[]>([]);
  const [settings, setSettings] = useState<Settings>(EMPTY_SETTINGS);
  const [catalog, setCatalog] = useState<CatalogItem[]>([]);
  profilesRef.current = profiles;
  settingsRef.current = settings;
  activeIdRef.current = activeId;
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
  const preservePendingProfileState = (updated: Profile): Profile => {
    const pendingInstance = gameInstanceSelectionIntents.current.get(updated.id);
    let preserved = pendingInstance
      ? { ...updated, gameInstanceId: pendingInstance.desiredId }
      : updated;
    if (modToggleIntents.current.size > 0) {
      preserved = {
        ...preserved,
        mods: preserved.mods.map((mod) => {
          const pending = modToggleIntents.current.get(`${updated.id}\0${mod.packageId}`);
          return pending ? { ...mod, enabled: pending.desired } : mod;
        }),
      };
    }
    return preserved;
  };


  const refreshModUpdates = (): Promise<void> => {
    const pending = profileUpdateRefreshRef.current;
    if (pending) return pending;

    const instances = new Map(
      settingsRef.current.gameInstances.map((instance) => [instance.id, instance]),
    );
    const targets = profilesRef.current.flatMap((profile) => {
      const instance = instances.get(profile.gameInstanceId ?? "");
      return instance ? [{ profile, arch: instance.arch }] : [];
    });
    if (targets.length === 0) {
      profileUpdateWarningShownRef.current = false;
      return Promise.resolve();
    }

    const refresh = Promise.allSettled(
      targets.map(({ profile, arch }) => bridge.checkModUpdates(profile.id, arch)),
    )
      .then((results) => {
        const updated = results.flatMap((result) =>
          result.status === "fulfilled" ? [result.value] : [],
        );
        if (updated.length > 0) {
          const byId = new Map(updated.map((profile) => [profile.id, profile]));
          setProfiles((current) => {
            const next = current.map((profile) => {
              const refreshed = byId.get(profile.id);
              if (!refreshed) return profile;
              const refreshedMods = new Map(
                refreshed.mods.map((mod) => [mod.packageId.toLowerCase(), mod]),
              );
              let changed = false;
              const mods = profile.mods.map((mod) => {
                const update = refreshedMods.get(mod.packageId.toLowerCase())?.update;
                if (update === mod.update) return mod;
                changed = true;
                return { ...mod, update };
              });
              return changed ? { ...profile, mods } : profile;
            });
            profilesRef.current = next;
            return next;
          });
        }

        const failed = results.find(
          (result): result is PromiseRejectedResult => result.status === "rejected",
        );
        if (failed) {
          if (!profileUpdateWarningShownRef.current) {
            profileUpdateWarningShownRef.current = true;
            notify(`Could not refresh mod updates: ${messageFrom(failed.reason)}`, "error");
          }
        } else {
          profileUpdateWarningShownRef.current = false;
        }
      })
      .finally(() => {
        profileUpdateRefreshRef.current = null;
      });
    profileUpdateRefreshRef.current = refresh;
    return refresh;
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


  const beginProfileMutation = () => {
    profileMutationCountRef.current += 1;
    setProfileMutationCount(profileMutationCountRef.current);
  };

  const endProfileMutation = () => {
    profileMutationCountRef.current = Math.max(0, profileMutationCountRef.current - 1);
    setProfileMutationCount(profileMutationCountRef.current);
  };

  const enqueueProfileMutation = (action: () => Promise<void>) => {
    const queued = async () => {
      await workspacePreparationPromise.current?.catch(() => undefined);
      await action();
    };
    profileMutationQueue.current = profileMutationQueue.current.then(queued, queued);
  };

  const beginOperation = (): boolean => {
    if (operationRef.current || profileMutationCountRef.current > 0) return false;
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
      await workspacePreparationPromise.current?.catch(() => undefined);
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
    await workspacePreparationPromise.current?.catch(() => undefined);
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

  // StrictMode replays effects. The startup promise contains local persistence
  // work only; remote discovery starts after the cached UI is visible.
  useEffect(() => {
    let current = true;
    if (!startupPromiseRef.current) {
      if (!beginOperation()) return;
      startupPromiseRef.current = (async (): Promise<StartupResult> => {
        const [loadedSettings, loadedProfiles, loadedCatalog] = await Promise.all([
          bridge.getSettings(),
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
        }

        return {
          settings: loadedSettings,
          catalog: loadedCatalog,
          profiles: nextProfiles,
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
        const persisted = result.settings.activeProfile;
        const selectedProfileId =
          persisted && result.profiles.some((profile) => profile.id === persisted)
            ? persisted
            : result.profiles[0].id;
        persistedActiveProfile.current = selectedProfileId;
        setSettings({ ...result.settings, activeProfile: selectedProfileId });
        setFreshSourceMigration(result.freshSourceMigration);
        setCatalog(result.catalog);
        setProfiles(result.profiles);
        setActiveId(selectedProfileId);
        setLoaded(true);
        if (result.settings.recoveryWarning) notify(messageFrom(result.settings.recoveryWarning), "error");
        if (result.settings.storageWarning) notify(messageFrom(result.settings.storageWarning), "error");

        void bridge.detectGames()
          .then((detected) => {
            if (current) setGames(detected);
          })
          .catch((error) => {
            if (current) notify(`Game detection failed: ${messageFrom(error)}`, "error");
          });

        void bridge.refreshCatalog()
          .then(() => bridge.loadCatalog())
          .then((refreshed) => {
            if (current) setCatalog(refreshed);
          })
          .catch((error) => {
            if (current) notify(`Catalog refresh failed: ${messageFrom(error)}`, "error");
          });

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
    if (!loaded) return;
    const refreshWhenVisible = () => {
      if (
        document.visibilityState === "visible" &&
        !document.querySelector('[aria-modal="true"]')
      ) {
        void refreshModUpdates();
      }
    };
    refreshWhenVisible();
    const timer = window.setInterval(refreshWhenVisible, MOD_UPDATE_REFRESH_MS);
    window.addEventListener("focus", refreshWhenVisible);
    document.addEventListener("visibilitychange", refreshWhenVisible);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", refreshWhenVisible);
      document.removeEventListener("visibilitychange", refreshWhenVisible);
    };
  }, [loaded]);

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
  const runningKnown = runningStatus.profileId === active?.id && runningStatus.known;
  const running = runningStatus.profileId === active?.id && runningStatus.running;
  const installedSnapshot = useMemo(
    () => active?.mods.map((mod) => [mod.packageId, mod.version] as [string, string]) ?? [],
    [active?.mods],
  );
  const gameInstances = settings.gameInstances;
  const gameForProfile = (profile: Profile | undefined): GameInstance | null =>
    gameInstances.find((instance) => instance.id === profile?.gameInstanceId) ?? null;
  const activeGame = gameForProfile(active);
  const arch: Arch = activeGame?.arch ?? "x86";
  const gameStatus = { store: activeGame?.store ?? "manual", arch, running };
  const busy = operationBusy || profileMutationCount > 0;
  const activePendingModIds = useMemo(() => {
    if (!active) return new Set<string>();
    const prefix = `${active.id}\0`;
    return new Set(
      [...pendingModToggles]
        .filter((key) => key.startsWith(prefix))
        .map((key) => key.slice(prefix.length)),
    );
  }, [active?.id, pendingModToggles]);
  const firstRun = loaded && !settings.setupComplete;

  useEffect(() => {
    if (!loaded || !active) {
      setRunningStatus({ profileId: "", running: false, known: false });
      return;
    }
    let current = true;
    let timer: number | undefined;
    let warned = false;
    const profileId = active.id;
    const updateRunningStatus = (isRunning: boolean, known: boolean) => {
      setRunningStatus((previous) =>
        previous.profileId === profileId &&
        previous.running === isRunning &&
        previous.known === known
          ? previous
          : { profileId, running: isRunning, known },
      );
    };
    const poll = async () => {
      try {
        const isRunning = await bridge.gameRunning(profileId);
        if (!current) return;
        warned = false;
        updateRunningStatus(isRunning, true);
      } catch (error) {
        if (!current) return;
        updateRunningStatus(false, false);
        if (!warned) {
          warned = true;
          notify(`Could not read ${active.name}'s game status: ${messageFrom(error)}`, "error");
        }
      }
      if (current) timer = window.setTimeout(poll, 2000);
    };
    void poll();
    return () => {
      current = false;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [loaded, active?.id]);

  useEffect(() => {
    if (
      !loaded ||
      firstRun ||
      !active ||
      !activeGame ||
      busy ||
      !runningKnown ||
      running ||
      initialWorkspacePreparationStarted.current
    ) {
      return;
    }
    initialWorkspacePreparationStarted.current = true;
    const profileId = active.id;
    setWorkspacePreparationProfileId(profileId);
    const preparation = bridge
      .syncProfile(activeGame.path, profileId)
      .then((warning) => {
        if (activeIdRef.current === profileId && warning) notify(warning, "error");
      })
      .catch((error) => {
        if (activeIdRef.current === profileId) {
          notify(`Could not prepare ${active.name}: ${messageFrom(error)}`, "error");
        }
      })
      .finally(() => {
        workspacePreparationPromise.current = null;
        setWorkspacePreparationProfileId((preparingId) => preparingId === profileId ? "" : preparingId);
      });
    workspacePreparationPromise.current = preparation;
  }, [loaded, firstRun, active?.id, activeGame?.path, busy, runningKnown, running]);

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
    const preserved = preservePendingProfileState(updated);
    setProfiles((current) => {
      if (!current.some((profile) => profile.id === updated.id)) return current;
      const next = current.map((profile) => profile.id === updated.id ? preserved : profile);
      profilesRef.current = next;
      return next;
    });
  };

  const requestUnmanagedPluginResolution = async (
    profile: Profile,
    continuation: boolean,
    targetInstance?: GameInstance,
  ): Promise<boolean> => {
    const instance = targetInstance ?? gameForProfile(profile);
    if (!instance) throw new Error("No Among Us source is assigned to this profile.");
    const plugins = await bridge.listUnmanagedPlugins(instance.path, profile.id);
    if (profile.id === activeIdRef.current) {
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
      ? `Moved ${count} plugin${count === 1 ? "" : "s"} to the direct-instance quarantine.`
      : action === "delete"
        ? `Permanently deleted ${count} plugin${count === 1 ? "" : "s"} from the direct instance.`
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
        notify("No extra plugins were found in this direct profile instance.");
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

  const refreshProfileUpdatesInBackground = (profile: Profile, context: string): void => {
    void refreshProfileUpdates(profile)
      .then(patchProfile)
      .catch((error) => notify(`${context}: ${messageFrom(error)}`, "error"));
  };

  const ensureLoaderInternal = async (
    profile: Profile,
    onProgress?: (progress: OperationProgress) => void,
  ): Promise<string | null> => {
    const instance = gameForProfile(profile);
    if (!instance) throw new Error("Add an Among Us folder in Settings before installing BepInEx.");
    return bridge.ensureLoader(instance.path, profile.id, instance.arch, false, onProgress);
  };

  const selectProfile = (id: string): void => {
    if (id === activeIdRef.current) return;
    if (!profilesRef.current.some((profile) => profile.id === id)) {
      notify("Profile not found.", "error");
      return;
    }
    const intent = { request: ++profileSelectionRequest.current, id };
    profileSelectionIntent.current = intent;
    activeIdRef.current = id;
    setActiveId(id);
    setSettings((current) => ({ ...current, activeProfile: id }));
    if (profileSelectionWorker.current) return;

    profileSelectionWorker.current = true;
    void (async () => {
      try {
        while (profileSelectionIntent.current) {
          const current = profileSelectionIntent.current;
          try {
            await bridge.selectActiveProfile(current.id);
            persistedActiveProfile.current = current.id;
            if (profileSelectionIntent.current?.request === current.request) {
              profileSelectionIntent.current = null;
            }
          } catch (error) {
            if (profileSelectionIntent.current?.request !== current.request) continue;
            profileSelectionIntent.current = null;
            const rollbackId = persistedActiveProfile.current;
            activeIdRef.current = rollbackId;
            setActiveId(rollbackId);
            setSettings((settingsState) => ({ ...settingsState, activeProfile: rollbackId }));
            notify(`Could not select profile: ${messageFrom(error)}`, "error");
          }
        }
      } finally {
        profileSelectionWorker.current = false;
      }
    })();
  };

  const toggleMod = (modId: string) => {
    if (operationRef.current) return;
    const profile = profilesRef.current.find((candidate) => candidate.id === activeIdRef.current);
    const mod = profile?.mods.find((candidate) => candidate.packageId === modId);
    const instance = settingsRef.current.gameInstances.find(
      (candidate) => candidate.id === profile?.gameInstanceId,
    );
    if (!profile || !mod || !instance) {
      notify("Assign an Among Us source before changing mods.", "error");
      return;
    }

    const key = `${profile.id}\0${modId}`;
    const previous = modToggleIntents.current.get(key);
    const intent: ModToggleIntent = {
      request: ++modToggleRequest.current,
      profileId: profile.id,
      packageId: modId,
      desired: !(previous?.desired ?? mod.enabled),
      persisted: previous?.persisted ?? mod.enabled,
    };
    modToggleIntents.current.set(key, intent);
    patchProfile({
      ...profile,
      mods: profile.mods.map((candidate) =>
        candidate.packageId === modId ? { ...candidate, enabled: intent.desired } : candidate
      ),
    });
    if (modToggleWorkers.current.has(key)) return;

    modToggleWorkers.current.add(key);
    beginProfileMutation();
    setPendingModToggles((current) => new Set(current).add(key));
    enqueueProfileMutation(async () => {
      try {
        while (modToggleIntents.current.has(key)) {
          const current = modToggleIntents.current.get(key)!;
          const currentProfile = profilesRef.current.find(
            (candidate) => candidate.id === current.profileId,
          );
          const currentMod = currentProfile?.mods.find(
            (candidate) => candidate.packageId === current.packageId,
          );
          const currentInstance = settingsRef.current.gameInstances.find(
            (candidate) => candidate.id === currentProfile?.gameInstanceId,
          );
          if (!currentProfile || !currentMod || !currentInstance) {
            if (modToggleIntents.current.get(key)?.request === current.request) {
              modToggleIntents.current.delete(key);
            }
            notify("The selected profile, mod, or Among Us source is no longer available.", "error");
            continue;
          }

          let saved: Profile;
          try {
            if (!await requestUnmanagedPluginResolution(currentProfile, true, currentInstance)) {
              throw UNMANAGED_REVIEW_CANCELLED;
            }
            const pendingInstance = gameInstanceSelectionIntents.current.get(current.profileId);
            saved = await bridge.setModEnabled(
              {
                ...currentProfile,
                gameInstanceId: pendingInstance?.persistedId ?? currentProfile.gameInstanceId,
                mods: currentProfile.mods.map((candidate) => {
                  const pending = modToggleIntents.current.get(
                    `${current.profileId}\0${candidate.packageId}`,
                  );
                  const enabled =
                    candidate.packageId === current.packageId
                      ? current.desired
                      : pending?.persisted ?? candidate.enabled;
                  return enabled === candidate.enabled ? candidate : { ...candidate, enabled };
                }),
              },
              current.packageId,
              current.desired,
            );
          } catch (error) {
            const latest = modToggleIntents.current.get(key);
            if (latest?.request === current.request) {
              modToggleIntents.current.delete(key);
              const rollbackProfile = profilesRef.current.find(
                (candidate) => candidate.id === current.profileId,
              );
              if (rollbackProfile) {
                patchProfile({
                  ...rollbackProfile,
                  mods: rollbackProfile.mods.map((candidate) =>
                    candidate.packageId === current.packageId
                      ? { ...candidate, enabled: current.persisted }
                      : candidate
                  ),
                });
              }
              if (error !== UNMANAGED_REVIEW_CANCELLED) {
                notify(`Could not change ${currentMod.name}: ${messageFrom(error)}`, "error");
              }
            }
            continue;
          }

          const latest = modToggleIntents.current.get(key);
          if (latest) {
            modToggleIntents.current.set(key, { ...latest, persisted: current.desired });
          }
          patchProfile(saved);

          let workspaceError: string | null = null;
          let warning: string | null = null;
          try {
            warning = await bridge.syncProfile(currentInstance.path, saved.id);
          } catch (error) {
            workspaceError = messageFrom(error);
          }

          const completed = modToggleIntents.current.get(key);
          if (completed?.request !== current.request) continue;
          modToggleIntents.current.delete(key);
          if (workspaceError) {
            notify(
              `${currentMod.name} was changed in the profile, but its direct instance could not be rebuilt: ${workspaceError}`,
              "error",
            );
          } else {
            notify(
              warning ?? `${currentMod.name} is ${current.desired ? "enabled" : "disabled"} in the direct profile instance`,
              warning ? "error" : "success",
            );
          }
        }
      } finally {
        modToggleWorkers.current.delete(key);
        setPendingModToggles((current) => {
          const next = new Set(current);
          next.delete(key);
          return next;
        });
        endProfileMutation();
      }
    });
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
          report({ phase: "finalizing", message: "Rebuilding the direct profile instance" });
          try {
            const warning = await bridge.syncProfile(instance.path, updated.id, report);
            notify(
              warning ? `Removed ${name}. ${warning}` : `Removed ${name} from the direct profile instance`,
              warning ? "error" : "success",
            );
          } catch (error) {
            notify(
              `${name} was removed from the profile, but its direct instance could not be rebuilt: ${messageFrom(error)}`,
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
        persistedActiveProfile.current = saved.id;
        activeIdRef.current = saved.id;
        setActiveId(saved.id);
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const openAddPanel = () => {
    if (operationRef.current) return;
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
    if (operationRef.current) return;
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
          report({ phase: "finalizing", message: "Rebuilding the direct profile instance" });
          try {
            const warning = await bridge.syncProfile(instance.path, installed.id, report);
            notify(
              warning ? `Added local DLL. ${warning}` : "Added local DLL to the direct profile instance",
              warning ? "error" : "success",
            );
          } catch (error) {
            notify(
              `The local DLL was added to the profile, but its direct instance could not be rebuilt: ${messageFrom(error)}`,
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
          patchProfile(installed);
          report({ phase: "finalizing", message: "Checking the BepInEx loader" });
          const loaderWarning = await ensureLoaderInternal(installed);
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
          refreshProfileUpdatesInBackground(installed, "Background update refresh failed");
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
          report({ phase: "finalizing", message: "Rebuilding the direct profile instance with the selected maps" });
          patchProfile(installed);
          let syncError: string | null = null;
          let warning: string | null = null;
          try {
            warning = await syncAuthoritativeProfile(installed, instance);
          } catch (error) {
            syncError = messageFrom(error);
          }
          refreshProfileUpdatesInBackground(installed, "Background map update refresh failed");
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

  const selectGameInstance = (id: string) => {
    if (operationRef.current) return;
    const profile = profilesRef.current.find((candidate) => candidate.id === activeIdRef.current);
    const instance = settingsRef.current.gameInstances.find((candidate) => candidate.id === id);
    if (!profile || !instance) {
      notify("The selected profile or Among Us source is no longer available.", "error");
      return;
    }

    const previous = gameInstanceSelectionIntents.current.get(profile.id);
    if ((previous?.desiredId ?? profile.gameInstanceId) === id) return;
    const intent: GameInstanceSelectionIntent = {
      request: ++gameInstanceSelectionRequest.current,
      profileId: profile.id,
      desiredId: id,
      persistedId: previous?.persistedId ?? profile.gameInstanceId,
    };
    gameInstanceSelectionIntents.current.set(profile.id, intent);
    patchProfile({ ...profile, gameInstanceId: id });
    if (gameInstanceSelectionWorkers.current.has(profile.id)) return;

    const profileId = profile.id;
    gameInstanceSelectionWorkers.current.add(profileId);
    beginProfileMutation();
    setPendingGameInstanceProfiles((current) => new Set(current).add(profileId));
    enqueueProfileMutation(async () => {
      try {
        while (gameInstanceSelectionIntents.current.has(profileId)) {
          const current = gameInstanceSelectionIntents.current.get(profileId)!;
          const currentProfile = profilesRef.current.find(
            (candidate) => candidate.id === current.profileId,
          );
          const currentInstance = settingsRef.current.gameInstances.find(
            (candidate) => candidate.id === current.desiredId,
          );
          if (!currentProfile || !currentInstance) {
            if (gameInstanceSelectionIntents.current.get(profileId)?.request === current.request) {
              gameInstanceSelectionIntents.current.delete(profileId);
            }
            notify("The selected profile or Among Us source is no longer available.", "error");
            continue;
          }

          let saved: Profile;
          try {
            if (!await requestUnmanagedPluginResolution(currentProfile, true, currentInstance)) {
              throw UNMANAGED_REVIEW_CANCELLED;
            }
            saved = await bridge.saveProfile({
              ...currentProfile,
              gameInstanceId: current.desiredId,
              mods: currentProfile.mods.map((candidate) => {
                const pending = modToggleIntents.current.get(
                  `${current.profileId}\0${candidate.packageId}`,
                );
                return pending && pending.desired !== pending.persisted
                  ? { ...candidate, enabled: pending.persisted }
                  : candidate;
              }),
            });
          } catch (error) {
            const latest = gameInstanceSelectionIntents.current.get(profileId);
            if (latest?.request === current.request) {
              gameInstanceSelectionIntents.current.delete(profileId);
              const rollbackProfile = profilesRef.current.find(
                (candidate) => candidate.id === current.profileId,
              );
              if (rollbackProfile) {
                patchProfile({ ...rollbackProfile, gameInstanceId: current.persistedId });
              }
              if (error !== UNMANAGED_REVIEW_CANCELLED) {
                notify(`Could not change source: ${messageFrom(error)}`, "error");
              }
            }
            continue;
          }

          const latest = gameInstanceSelectionIntents.current.get(profileId);
          if (latest) {
            gameInstanceSelectionIntents.current.set(profileId, {
              ...latest,
              persistedId: current.desiredId,
            });
          }
          patchProfile(saved);

          let workspaceError: string | null = null;
          let warning: string | null = null;
          try {
            warning = await bridge.syncProfile(currentInstance.path, saved.id);
          } catch (error) {
            workspaceError = messageFrom(error);
          }

          const completed = gameInstanceSelectionIntents.current.get(profileId);
          if (completed?.request !== current.request) continue;
          gameInstanceSelectionIntents.current.delete(profileId);
          if (workspaceError) {
            notify(
              `Source changed, but its direct instance could not be rebuilt: ${workspaceError}`,
              "error",
            );
          } else {
            notify(
              warning
                ? `Source changed. ${warning}`
                : "Source changed and the direct profile instance is ready.",
              warning ? "error" : "success",
            );
          }
        }
      } finally {
        gameInstanceSelectionWorkers.current.delete(profileId);
        setPendingGameInstanceProfiles((current) => {
          const next = new Set(current);
          next.delete(profileId);
          return next;
        });
        endProfileMutation();
      }
    });
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
        persistedActiveProfile.current = nextProfiles[0].id;
        activeIdRef.current = nextProfiles[0].id;
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
    if (operationRef.current) return;
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
          patchProfile(installed);
          report({ phase: "finalizing", message: "Checking the BepInEx loader" });
          const loaderWarning = await ensureLoaderInternal(installed);
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
          refreshProfileUpdatesInBackground(installed, "Background update refresh failed");
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


  const launchInternal = async (
    profile: Profile,
    vanilla: boolean,
    onProgress?: (progress: OperationProgress) => void,
  ) => {
    const instance = gameForProfile(profile);
    if (!instance) throw new Error("No Among Us source is assigned to this profile.");
    setRunningStatus({ profileId: profile.id, running: true, known: true });
    try {
      const warning = vanilla
        ? (await bridge.launchVanilla(instance.path, profile.id, onProgress), null)
        : await bridge.launchProfile(instance.path, profile.id, onProgress);
      if (warning) {
        setRunningStatus({ profileId: profile.id, running: false, known: true });
        return warning;
      }
      notify(
        instance.store === "epic"
          ? `Launching ${vanilla ? "vanilla Among Us" : profile.name}. Epic may ask you to sign in the first time, that's normal.`
          : `Launching ${vanilla ? "vanilla Among Us" : profile.name}`,
      );
      return null;
    } catch (error) {
      try {
        const running = await bridge.gameRunning(profile.id);
        setRunningStatus({ profileId: profile.id, running, known: true });
      } catch {
        setRunningStatus({ profileId: profile.id, running: false, known: false });
      }
      throw error;
    }
  };

  const doLaunchProfile = async (profile: Profile) => {
    try {
      const preflightProfile = profiles.find((candidate) => candidate.id === profile.id);
      const preflightInstance = gameForProfile(preflightProfile);
      if (!preflightProfile || !preflightInstance) {
        throw new Error("No Among Us source is assigned. Add one in Settings.");
      }
      if (!await requestUnmanagedPluginResolution(preflightProfile, true, preflightInstance)) return;
      await trackedExclusive(
        {
          scope: "launch",
          title: `Launching ${preflightProfile.name}`,
          message: "Checking the direct profile instance and BepInEx loader",
        },
        async (report) => {
          const current = profiles.find((candidate) => candidate.id === profile.id);
          const instance = gameForProfile(current);
          if (!current || !instance) {
            throw new Error("No Among Us source is assigned. Add one in Settings.");
          }
          const warning = await launchInternal(current, false, report);
          if (warning) setLaunchWarn(current);
        },
      );
    } catch (error) {
      if (error !== OPERATION_BUSY && error !== UNMANAGED_REVIEW_CANCELLED) notify(messageFrom(error), "error");
    }
  };

  const doStopProfile = async (profile: Profile) => {
    try {
      const stopped = await exclusive(() => bridge.stopGame(profile.id));
      setRunningStatus({ profileId: profile.id, running: false, known: true });
      notify(stopped ? "Among Us stopped" : "Among Us is no longer running");
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(`Could not stop Among Us: ${messageFrom(error)}`, "error");
    }
  };

  const launchWarnInstall = async () => {
    const profile = launchWarn;
    if (!profile) return;
    try {
      const instance = gameForProfile(profile);
      if (!instance) throw new Error("No Among Us source is assigned. Add one in Settings.");
      if (!await requestUnmanagedPluginResolution(profile, true, instance)) return;
      await trackedExclusive(
        {
          scope: "launch",
          title: `Preparing ${profile.name}`,
          message: "Installing the BepInEx loader",
        },
        async (report) => {
          const warning = await ensureLoaderInternal(profile, report);
          if (warning) throw new Error(warning);
          setLaunchWarn(null);
          const launchWarning = await launchInternal(profile, false, report);
          if (launchWarning) throw new Error(launchWarning);
        },
      );
    } catch (error) {
      if (error !== OPERATION_BUSY && error !== UNMANAGED_REVIEW_CANCELLED) notify(messageFrom(error), "error");
    }
  };

  const launchWarnAnyway = async (dontWarnAgain: boolean) => {
    const profile = launchWarn;
    if (!profile) return;
    try {
      await trackedExclusive(
        {
          scope: "launch",
          title: "Launching vanilla Among Us",
          message: "Preparing the direct vanilla instance",
        },
        async (report) => {
          if (dontWarnAgain) {
            const normalized = await bridge.saveSettings({ ...settings, skipLaunchWarning: true });
            setSettings(normalized);
          }
          setLaunchWarn(null);
          await launchInternal(profile, true, report);
        },
      );
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
          const built = await bridge.applyLobbyCode(code, instance.arch, instance.id, report);
          report({ phase: "finalizing", message: "Selecting the new lobby profile" });
          const normalized = await bridge.saveSettings({ ...settings, activeProfile: built.id });
          setSettings(normalized);
          setProfiles((current) => [...current.filter((profile) => profile.id !== built.id), built]);
          persistedActiveProfile.current = built.id;
          activeIdRef.current = built.id;
          setActiveId(built.id);
          report({ phase: "finalizing", message: "Checking the BepInEx loader" });
          const loaderWarning = await ensureLoaderInternal(built);
          if (doLaunch) {
            if (loaderWarning) throw new Error(loaderWarning);
            report({ phase: "finalizing", message: `Starting ${built.name}` });
            const launchWarning = await launchInternal(built, false, report);
            if (launchWarning) throw new Error(launchWarning);
            setLobbyOpen(false);
            if (warnings.length > 0) notify(`Lobby launched. ${warnings.join(" ")}`, "error");
          } else {
            report({ phase: "finalizing", message: "Building the lobby profile's direct instance" });
            const warning = (await syncAuthoritativeProfile(built, instance)) ?? loaderWarning;
            setLobbyOpen(false);
            const details = [warning, ...warnings].filter(Boolean).join(" ");
            notify(details ? `Lobby profile ready: ${built.name}. ${details}` : `Lobby profile ready: ${built.name}`, details ? "error" : "success");
          }
          refreshProfileUpdatesInBackground(built, "Background lobby update refresh failed");
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
        const normalized = await bridge.saveSettings(
          { ...draft, activeProfile: settingsRef.current.activeProfile },
          tokenAction,
        );
        settingsRef.current = normalized;
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
    settingsRef.current = normalized;
    setSettings(normalized);
    notify(
      normalized.storageWarning
        ? messageFrom(normalized.storageWarning)
        : "Direct-instance storage location updated",
      normalized.storageWarning ? "error" : "success",
    );
  };
  const saveErrorLog = async (): Promise<void> => {
    const destination = await exclusive(() => bridge.exportErrorLog(active.id));
    if (destination) notify("BepInEx error log saved", "success");
  };
  const openSupportLogs = async (): Promise<void> => {
    const directory = await exclusive(() => bridge.openSupportLogs(active.id));
    if (directory) notify("Opened diagnostic logs folder", "success");
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
          const currentSettings = settingsRef.current;
          const instances = [...currentSettings.gameInstances];
          const currentProfile =
            profilesRef.current.find((profile) => profile.id === activeIdRef.current) ?? active;
          let gameInstanceId = currentProfile.gameInstanceId;
          let selectedInstance: GameInstance | undefined;
          if (gamePath) {
            const inspected = await bridge.inspectGame(gamePath);
            const normalizedPath = inspected.path
              .replaceAll("\\", "/")
              .replace(/\/+$/u, "")
              .toLocaleLowerCase();
            let instance = instances.find(
              (candidate) =>
                candidate.path
                  .replaceAll("\\", "/")
                  .replace(/\/+$/u, "")
                  .toLocaleLowerCase() === normalizedPath,
            );
            const instanceStore = (store as Store | undefined) ?? inspected.store;
            if (instance) {
              instance = {
                ...instance,
                ...inspected,
                id: instance.id,
                name: instance.name,
                store: instanceStore,
                arch: (selectedArch as Arch | undefined) ?? inspected.arch,
                runtime: runtime ?? inspected.runtime ?? "native",
              };
              instances[instances.findIndex((candidate) => candidate.id === instance!.id)] = instance;
            } else {
              const storeCount = instances.filter((candidate) => candidate.store === instanceStore).length;
              const name = INSTANCE_NAMES[instanceStore];
              instance = {
                ...inspected,
                id: `game-${Date.now().toString(36)}`,
                name: storeCount === 0 ? name : `${name} ${storeCount + 1}`,
                store: instanceStore,
                arch: (selectedArch as Arch | undefined) ?? inspected.arch,
                runtime: runtime ?? inspected.runtime ?? "native",
              };
              instances.push(instance);
            }
            selectedInstance = instance;
            gameInstanceId = instance.id;
          }
          if (selection && !selectedInstance) {
            throw new Error("Choose an original Among Us source before preparing the direct profile instance.");
          }

          report({ phase: "preparing", message: "Saving the original source record and active profile" });
          const provisional = await bridge.saveSettings({
            ...currentSettings,
            gameInstances: instances,
          });
          settingsRef.current = provisional;
          setSettings(provisional);
          setGames(provisional.gameInstances);
          let savedProfile = await bridge.saveProfile({ ...currentProfile, gameInstanceId });
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
            report({ phase: "finalizing", message: "Building a direct Town of Us profile instance" });
            await syncAuthoritativeProfile(savedProfile, selectedInstance);
          }
          if (selection?.kind === "bepinex" && selectedInstance) {
            report({ phase: "finalizing", message: "Installing BepInEx into the direct profile instance" });
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
          settingsRef.current = normalized;
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
        notify("Preparing the direct profile instance…");
        const warning = await syncAuthoritativeProfile(current, instance);
        notify(
          warning
            ? `The direct profile instance is ready. ${warning}`
            : "Mods are ready in the direct profile instance. The original game source was not modified.",
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
            quickActionsBlocked={operationBusy}
            gameInstancePending={pendingGameInstanceProfiles.has(active.id)}
            pendingModIds={activePendingModIds}
            launchBusy={workspacePreparationProfileId !== ""}
            unmanagedPlugins={unmanagedPlugins}
            unmanagedLoading={unmanagedLoading}
            unmanagedError={unmanagedScanError}
            trustOf={trustOf}
            onToggle={toggleMod}
            onRemove={removeMod}
            onPickRelease={openPicker}
            onShare={() => {
              if (!busy) setShareOpen(true);
            }}
            onRename={(name) => void renameProfile(name)}
            onDelete={deleteActiveProfile}
            onLaunch={() => void doLaunchProfile(active)}
          onStop={() => void doStopProfile(active)}
            onAddMod={openAddPanel}
            onReviewUpdates={() => {
              if (!busy) setUpdateReviewOpen(true);
            }}
            onBrowseMaps={() => {
              setMapsReturnToAdd(false);
              if (!busy) setMapsOpen(true);
            }}
            onSetup={() => void setupMods(active)}
            onSelectGameInstance={selectGameInstance}
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
        busyReason={operationBusy ? "Wait for the current operation to finish." : undefined}
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
        onOpenSupportLogs={openSupportLogs}
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
