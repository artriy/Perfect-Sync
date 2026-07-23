import { useEffect, useMemo, useRef, useState } from "react";
import { TopBar } from "./components/TopBar";
import { Sidebar } from "./components/Sidebar";
import { MainPanel } from "./components/MainPanel";
import { LobbyCodeModal } from "./components/LobbyCodeModal";
import { AddModPanel } from "./components/AddModPanel";
import { SettingsModal } from "./components/SettingsModal";
import { ReleasePicker } from "./components/ReleasePicker";
import { ShareModal } from "./components/ShareModal";
import { SetupModal } from "./components/SetupModal";
import { LaunchWarning } from "./components/LaunchWarning";
import { Toast, type ToastState } from "./components/Toast";
import * as bridge from "./lib/bridge";
import { CREW } from "./lib/palette";
import type {
  Arch,
  CatalogItem,
  GameInstall,
  GameInstance,
  GithubTokenAction,
  Profile,
  Runtime,
  Settings,
  Store,
  Trust,
} from "./lib/types";

const CREW_CYCLE = Object.values(CREW);
const OPERATION_BUSY = new Error("Another operation is already in progress.");

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
};
interface StartupResult {
  settings: Settings;
  games: GameInstall[];
  catalog: CatalogItem[];
  profiles: Profile[];
  errors: string[];
}

function messageFrom(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function App() {
  const [loaded, setLoaded] = useState(false);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeId, setActiveId] = useState("");
  const [running, setRunningState] = useState(false);
  const [operationBusy, setOperationBusy] = useState(false);

  const operationRef = useRef(false);
  const runningRef = useRef(false);
  const launchSession = useRef(0);
  const startupPromiseRef = useRef<Promise<StartupResult> | null>(null);

  const [games, setGames] = useState<GameInstall[]>([]);
  const [settings, setSettings] = useState<Settings>(EMPTY_SETTINGS);
  const [catalog, setCatalog] = useState<CatalogItem[]>([]);
  const [update, setUpdate] = useState<bridge.UpdateInfo | null>(null);
  const [updateDismissed, setUpdateDismissed] = useState(false);
  const [startupError, setStartupError] = useState<string | null>(null);

  const [addOpen, setAddOpen] = useState(false);
  const [lobbyOpen, setLobbyOpen] = useState(false);
  const [lobbyCode, setLobbyCode] = useState<string | undefined>(undefined);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [shareOpen, setShareOpen] = useState(false);
  const [launchWarn, setLaunchWarn] = useState<Profile | null>(null);
  const [pickerTarget, setPickerTarget] = useState<{
    repo: string;
    name: string;
    trust: Trust;
    personal?: boolean;
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
        };
      })().finally(endOperation);
    }

    void startupPromiseRef.current
      .then((result) => {
        if (!current) return;
        setSettings(result.settings);
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
    bridge
      .checkUpdate()
      .then((available) => {
        if (current) setUpdate(available);
      })
      .catch((error) => {
        if (current) notify(`Update check failed: ${messageFrom(error)}`, "error");
      });
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

  if (!loaded) {
    return (
      <div className="grid h-[100dvh] place-items-center">
        <p className="subtitle text-ink-dim">Loading Perfect-Sync…</p>
      </div>
    );
  }

  if (!active) {
    return (
      <div className="grid h-[100dvh] place-items-center px-8 text-center">
        <div>
          <p className="text-[15px] font-semibold text-ink">Perfect-Sync couldn't start</p>
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

  const trustOf = (id: string): Trust => {
    const identity = id.toLowerCase();
    if (identity === "bepinex/bepinex") return "trusted";
    return catalog.find(
      (item) => item.id.toLowerCase() === identity || item.repo.toLowerCase() === identity,
    )?.trust ?? "flagged";
  };

  const refreshProfileUpdates = async (profile: Profile): Promise<Profile> => {
    const instance = gameForProfile(profile);
    if (!instance) throw new Error("Assign an Among Us instance before checking mod updates.");
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
      await exclusive(async () => {
        const normalized = await bridge.saveSettings({ ...settings, activeProfile: id });
        setSettings(normalized);
        setActiveId(id);
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const toggleMod = async (modId: string) => {
    try {
      await exclusive(async () => {
        const profile = profiles.find((candidate) => candidate.id === active.id);
        const mod = profile?.mods.find((candidate) => candidate.packageId === modId);
        if (!profile || !mod) return;
        patchProfile(await bridge.setModEnabled(profile, modId, !mod.enabled));
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const removeMod = async (modId: string): Promise<void> => {
    try {
      await exclusive(async () => {
        const profile = profiles.find((candidate) => candidate.id === active.id);
        if (!profile) return;
        const name = profile.mods.find((mod) => mod.packageId === modId)?.name ?? "mod";
        patchProfile(await bridge.removeMod(profile, modId));
        notify(`Removed ${name}`);
      });
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

  const addCatalog = async (item: CatalogItem): Promise<void> => {
    if (operationRef.current || runningRef.current) return;
    if (active.mods.some((mod) => mod.packageId === item.id)) {
      notify(`${item.name} is already in this profile`, "error");
      return;
    }
    setPickerTarget({ repo: item.repo, name: item.name, trust: item.trust ?? "flagged", returnToAdd: true });
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
      await exclusive(async () => {
        const profile = profiles.find((candidate) => candidate.id === active.id);
        if (!profile || profile.gameInstanceId === id) return;
        const saved = await bridge.saveProfile({ ...profile, gameInstanceId: id });
        patchProfile(saved);
      });
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
      setPickerTarget({ repo, name: mod.name, trust: trustOf(repo) });
    }
  };

  const addPersonal = async (repo: string, name: string): Promise<void> => {
    if (operationRef.current || runningRef.current) throw OPERATION_BUSY;
    setPickerTarget({ repo, name, trust: trustOf(repo), personal: true });
  };

  const removePersonal = async (repo: string): Promise<void> => {
    try {
      await exclusive(async () => {
        const normalized = await bridge.saveSettings({
          ...settings,
          personalMods: settings.personalMods.filter((personal) => personal.repo !== repo),
        });
        setSettings(normalized);
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const togglePersonal = async (repo: string, enabled: boolean): Promise<void> => {
    try {
      await exclusive(async () => {
        const normalized = await bridge.saveSettings({
          ...settings,
          personalMods: settings.personalMods.map((personal) =>
            personal.repo === repo ? { ...personal, enabled } : personal,
          ),
        });
        setSettings(normalized);
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const pickRelease = async (repo: string, tag: string, assetName: string) => {
    const target = pickerTarget;
    if (!target || target.repo !== repo) return;
    try {
      await exclusive(async () => {
        if (target.personal) {
          const previous = settings.personalMods.find((personal) => personal.repo === target.repo);
          const normalized = await bridge.saveSettings({
            ...settings,
            personalMods: [
              ...settings.personalMods.filter((personal) => personal.repo !== target.repo),
              {
                repo: target.repo,
                tag,
                asset: assetName,
                name: target.name,
                enabled: previous?.enabled ?? true,
              },
            ],
          });
          setSettings(normalized);
          setPickerTarget(null);
          notify(`${target.name} will be added to every lobby you join`);
          return;
        }

        const profile = profiles.find((candidate) => candidate.id === active.id);
        const instance = gameForProfile(profile);
        if (!profile || !instance) throw new Error("Assign an Among Us instance before installing a mod.");
        notify(`Installing ${assetName}…`);
        const installed = await bridge.installAsset(profile, target.repo, tag, assetName, instance.arch, true);
        const warnings: string[] = [];
        let refreshed = installed;
        try {
          refreshed = await refreshProfileUpdates(installed);
        } catch (error) {
          warnings.push(`Update refresh failed: ${messageFrom(error)}`);
        }
        patchProfile(refreshed);
        setPickerTarget(null);
        if (target.returnToAdd) setAddOpen(false);
        const loaderWarning = await ensureLoaderInternal(refreshed);
        if (loaderWarning) warnings.push(loaderWarning);
        try {
          setCatalog(await bridge.loadCatalog());
        } catch (error) {
          warnings.push(`Catalog reload failed: ${messageFrom(error)}`);
        }
        notify(
          warnings.length > 0 ? `Installed ${assetName}. ${warnings.join(" ")}` : `Installed ${assetName}`,
          warnings.length > 0 ? "error" : "success",
        );
      });
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
    if (!instance) throw new Error("No Among Us instance is assigned to this profile.");
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
        if (!current || !instance) throw new Error("No Among Us instance is assigned. Add one in Settings.");
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
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
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
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
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

  const openLobbyFromSidebar = () => {
    setLobbyCode(undefined);
    setLobbyOpen(true);
  };

  const openLobbyFromCode = (code: string) => {
    setLobbyCode(code);
    setLobbyOpen(true);
  };

  const applyLobby = async (doLaunch: boolean, code: string) => {
    try {
      await exclusive(async () => {
        const instance = activeGame;
        if (!instance) throw new Error("Choose a concrete Among Us instance before applying a lobby.");
        notify("Setting up lobby…");
        let built = await bridge.applyLobbyCode(code, instance.arch, instance.id);
        try {
          built = await refreshProfileUpdates(built);
        } catch (error) {
          notify(`Lobby installed, but update refresh failed: ${messageFrom(error)}`, "error");
        }
        const normalized = await bridge.saveSettings({ ...settings, activeProfile: built.id });
        setSettings(normalized);
        setProfiles((current) => [...current.filter((profile) => profile.id !== built.id), built]);
        setActiveId(built.id);
        setLobbyOpen(false);
        const loaderWarning = await ensureLoaderInternal(built);
        if (doLaunch) {
          if (loaderWarning) throw new Error(loaderWarning);
          await launchInternal(built, false);
        } else {
          const warning = (await bridge.syncProfile(instance.path, built.id)) ?? loaderWarning;
          notify(warning ? `Lobby profile ready: ${built.name}. ${warning}` : `Lobby profile ready: ${built.name}`);
        }
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const saveSettings = async (draft: Settings, tokenAction: GithubTokenAction): Promise<void> => {
    try {
      await exclusive(async () => {
        const normalized = await bridge.saveSettings(draft, tokenAction);
        setSettings(normalized);
        setSettingsOpen(false);
        notify("Settings saved");
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
      throw error;
    }
  };

  const completeSetup = async (gamePath?: string, selectedArch?: string, store?: string, runtime?: Runtime) => {
    try {
      await exclusive(async () => {
        const instances = [...gameInstances];
        let gameInstanceId = active.gameInstanceId;
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
          gameInstanceId = instance.id;
        }
        // Persist the instance list first, but publish setupComplete only after the
        // profile assignment succeeds. A failure or crash therefore reopens setup.
        const provisional = await bridge.saveSettings({
          ...settings,
          setupComplete: false,
          gameInstances: instances,
        });
        const savedProfile = await bridge.saveProfile({ ...active, gameInstanceId });
        const normalized = await bridge.saveSettings({ ...provisional, setupComplete: true });
        setSettings(normalized);
        patchProfile(savedProfile);
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const setupMods = async (profile: Profile) => {
    try {
      await exclusive(async () => {
        const current = profiles.find((candidate) => candidate.id === profile.id);
        const instance = gameForProfile(current);
        if (!current || !instance) throw new Error("No Among Us instance is assigned. Add one in Settings.");
        notify("Setting up mods…");
        const warning = await bridge.syncProfile(instance.path, current.id);
        notify(
          warning
            ? `Mods are synchronized in the Among Us folder. ${warning}`
            : "Mods set up in your Among Us folder. Launch Among Us when ready.",
        );
      });
    } catch (error) {
      if (error !== OPERATION_BUSY) notify(messageFrom(error), "error");
    }
  };

  const openUpdate = async () => {
    if (!update) return;
    try {
      await bridge.openUrl(update.url);
    } catch (error) {
      notify(`Could not open the download page: ${messageFrom(error)}`, "error");
    }
  };

  const topLevelOverlayOpen =
    addOpen ||
    lobbyOpen ||
    settingsOpen ||
    pickerTarget !== null ||
    shareOpen ||
    firstRun ||
    launchWarn !== null;

  return (
    <div className="flex h-[100dvh] flex-col">
      <div
        className="flex min-h-0 flex-1 flex-col"
        inert={topLevelOverlayOpen}
        aria-hidden={topLevelOverlayOpen}
      >
      <TopBar
        onAddMod={() => {
          if (!busy) setAddOpen(true);
        }}
        onPasteCode={openLobbyFromCode}
        onOpenSettings={() => {
          if (!busy) setSettingsOpen(true);
        }}
      />

      {update && !updateDismissed && (
        <div className="mx-3 mt-2 flex items-center gap-3 rounded-xl border border-[rgba(123,150,255,0.35)] bg-[rgba(123,150,255,0.12)] px-4 py-2 text-[13px] text-[#cbd8ff]">
          <span className="flex-1">Perfect-Sync {update.version} is available.</span>
          <button
            type="button"
            onClick={() => void openUpdate()}
            className="ring-focus accent-grad rounded-lg px-3 py-1.5 text-[12.5px] font-semibold text-[#0d0820]"
          >
            Download
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

      <div className="flex min-h-0 flex-1 p-3 pt-2.5">
        <div className="glass flex min-h-0 flex-1 overflow-hidden rounded-3xl">
          <Sidebar
            profiles={profiles}
            activeId={active.id}
            busy={busy}
            onSelect={(id) => void selectProfile(id)}
            onNewProfile={() => void newProfile()}
            onPasteCode={openLobbyFromSidebar}
          />
          <MainPanel
            profile={active}
            game={gameStatus}
            gameInstances={gameInstances}
            busy={busy}
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
            onAddMod={() => {
              if (!busy) setAddOpen(true);
            }}
            onSetup={() => void setupMods(active)}
            onSelectGameInstance={(id) => void selectGameInstance(id)}
            onManageGameInstances={() => {
              if (!busy) setSettingsOpen(true);
            }}
          />
        </div>
      </div>
      </div>

      <AddModPanel
        open={addOpen}
        profileName={active.name}
        catalog={catalog}
        onClose={() => {
          if (!operationRef.current) setAddOpen(false);
        }}
        onAddCatalog={addCatalog}
        onAddUrl={addUrl}
        onRemoveCatalog={removeCatalogItem}
        onMoveCatalog={moveCatalogItem}
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
        onClose={() => {
          if (!operationRef.current) setSettingsOpen(false);
        }}
        onSave={saveSettings}
        onAddPersonal={addPersonal}
        onRemovePersonal={removePersonal}
        onTogglePersonal={togglePersonal}
        trustOf={trustOf}
      />
      <ReleasePicker
        open={pickerTarget !== null}
        repo={pickerTarget?.repo ?? ""}
        modName={pickerTarget?.name ?? ""}
        trust={pickerTarget?.trust ?? "flagged"}
        busy={operationBusy}
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
        open={firstRun}
        detected={games}
        profileId={active.id}
        onFinish={completeSetup}
      />
      <LaunchWarning
        open={launchWarn !== null}
        onInstall={launchWarnInstall}
        onLaunchAnyway={launchWarnAnyway}
        onCancel={() => {
          if (!operationRef.current) setLaunchWarn(null);
        }}
      />
      <Toast toast={toast} onDismiss={() => setToast(null)} />
    </div>
  );
}
