import { useEffect, useRef, useState } from "react";
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
import { CATALOG } from "./data/mock";
import { CREW } from "./lib/palette";
import type { Arch, CatalogItem, GameInstall, Profile, ProfileMod, Settings, Store, Trust } from "./lib/types";

const CREW_CYCLE = Object.values(CREW);

export function App() {
  const [loaded, setLoaded] = useState(false);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeId, setActiveId] = useState<string>("");
  const [running, setRunning] = useState(false);
  const [busyModId, setBusyModId] = useState<string | null>(null);

  const [game, setGame] = useState<GameInstall | null>(null);
  const [games, setGames] = useState<GameInstall[]>([]);
  const [settings, setSettings] = useState<Settings>({});
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
    personal?: boolean;
  } | null>(null);

  const [toast, setToast] = useState<ToastState | null>(null);
  const toastId = useRef(0);
  const notify = (msg: string, kind: "success" | "error" = "success") => {
    toastId.current += 1;
    const id = toastId.current;
    setToast({ id, msg, kind });
    setTimeout(() => setToast((t) => (t?.id === id ? null : t)), 2600);
  };

  // Load settings, detect the game, and read persisted profiles on startup.
  useEffect(() => {
    (async () => {
      const [st, detectedGames, profs] = await Promise.all([
        bridge.getSettings(),
        bridge.detectGames(),
        bridge.loadProfiles(),
      ]);
      setSettings(st);
      setGames(detectedGames);
      setGame(detectedGames[0] ?? null);
      let list = profs;
      if (list.length === 0) {
        const starter: Profile = { id: "my-mods", name: "My mods", crewColor: CREW.violet, mods: [] };
        await bridge.saveProfile(starter);
        list = [starter];
      }
      setProfiles(list);
      const persisted = st.activeProfile;
      setActiveId(persisted && list.some((p) => p.id === persisted) ? persisted : list[0].id);
      setLoaded(true);
      // show the cached catalog right away, then refresh from the hosted copy
      bridge.loadCatalog().then(setCatalog).catch(() => {});
      bridge
        .refreshCatalog()
        .catch(() => {})
        .then(() => bridge.loadCatalog())
        .then(setCatalog)
        .catch(() => {});
    })().catch((e) => {
      setStartupError(String(e));
      setLoaded(true);
    });
  }, []);

  // Persist the active profile so the right mod set is restored on restart.
  useEffect(() => {
    if (!loaded || !activeId) return;
    setSettings((prev) => {
      if (prev.activeProfile === activeId) return prev;
      const next = { ...prev, activeProfile: activeId };
      bridge.saveSettings(next).catch(() => {});
      return next;
    });
  }, [activeId, loaded]);

  useEffect(() => {
    bridge.checkUpdate().then(setUpdate).catch(() => {});
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    bridge
      .onLobbyLink((code) => {
        setLobbyCode(code);
        setLobbyOpen(true);
      })
      .then((u) => {
        if (typeof u === "function") unlisten = u;
      })
      .catch(() => {});
    return () => unlisten?.();
  }, []);

  const arch: Arch = game?.arch ?? settings.arch ?? "x86";
  const gameStatus = { store: game?.store ?? "steam", arch, running };
  const firstRun = loaded && !settings.setupComplete;

  const active = profiles.find((p) => p.id === activeId) ?? profiles[0];

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

  const patchProfile = (updated: Profile) =>
    setProfiles((ps) => ps.map((p) => (p.id === updated.id ? updated : p)));

  // a mod's vetting tier, resolved against the (bundled-authoritative) catalog
  const trustOf = (id: string): Trust =>
    catalog.find((c) => c.id === id || c.repo === id)?.trust ?? "flagged";

  // Install/verify the BepInEx loader for a profile, surfacing any failure.
  const ensureLoader = async (profileId: string) => {
    if (!bridge.inTauri) return;
    const gamePath = settings.gamePath ?? game?.path;
    if (!gamePath) {
      notify("Set your Among Us folder in Settings so BepInEx can install.", "error");
      return;
    }
    try {
      await bridge.ensureLoader(gamePath, profileId, arch);
    } catch (e) {
      notify(`BepInEx setup failed: ${e}`, "error");
    }
  };

  const hasRoleMod = (mods: ProfileMod[]) => mods.some((m) => !m.managed && m.tags.includes("role"));

  const toggleMod = async (modId: string) => {
    const mod = active.mods.find((m) => m.packageId === modId);
    if (!mod) return;
    try {
      patchProfile(await bridge.setModEnabled(active, modId, !mod.enabled));
    } catch (e) {
      notify(String(e), "error");
    }
  };

  const removeMod = async (modId: string) => {
    const name = active.mods.find((m) => m.packageId === modId)?.name ?? "mod";
    try {
      patchProfile(await bridge.removeMod(active, modId));
      notify(`Removed ${name}`);
    } catch (e) {
      notify(String(e), "error");
    }
  };

  const newProfile = async () => {
    const n = profiles.filter((p) => p.name.startsWith("New profile")).length + 1;
    const id = `new-${Date.now()}`;
    const profile: Profile = {
      id,
      name: `New profile ${n}`,
      crewColor: CREW_CYCLE[profiles.length % CREW_CYCLE.length],
      mods: [],
    };
    setProfiles((ps) => [...ps, profile]);
    setActiveId(id);
    try {
      await bridge.saveProfile(profile);
    } catch (e) {
      notify(String(e), "error");
    }
  };

  // adding a mod opens the release/file picker so the user chooses the exact dll
  const addCatalog = (item: CatalogItem) => {
    if (active.mods.some((m) => m.packageId === item.id)) {
      notify(`${item.name} is already in this profile`, "error");
      return;
    }
    if (item.tags.includes("role") && hasRoleMod(active.mods)) {
      notify("Only one role mod per profile. Remove the current one first.", "error");
      return;
    }
    setAddOpen(false);
    setPickerTarget({ repo: item.repo, name: item.name });
  };

  const addUrl = (url: string) => {
    const m = url.match(/github\.com\/([^/]+)\/([^/#?]+)/i);
    const repo = m ? `${m[1]}/${m[2]}` : url;
    const name = m ? m[2] : "Mod";
    if (active.mods.some((mod) => mod.packageId === repo)) {
      notify(`${name} is already in this profile`, "error");
      return;
    }
    setAddOpen(false);
    setPickerTarget({ repo, name });
  };

  const renameProfile = async (name: string) => {
    const updated = { ...active, name };
    patchProfile(updated);
    try {
      await bridge.saveProfile(updated);
    } catch (e) {
      notify(String(e), "error");
    }
  };

  const deleteActiveProfile = async () => {
    const id = active.id;
    const name = active.name;
    try {
      await bridge.deleteProfile(id);
    } catch (e) {
      notify(String(e), "error");
    }
    const left = profiles.filter((p) => p.id !== id);
    if (left.length === 0) {
      const starter: Profile = { id: "my-mods", name: "My mods", crewColor: CREW.violet, mods: [] };
      await bridge.saveProfile(starter).catch(() => {});
      setProfiles([starter]);
      setActiveId(starter.id);
    } else {
      setProfiles(left);
      setActiveId(left[0].id);
    }
    notify(`Deleted ${name}`);
  };

  const openPicker = (modId: string) => {
    const m = active.mods.find((x) => x.packageId === modId);
    if (m) setPickerTarget({ repo: m.repo ?? m.packageId, name: m.name });
  };

  const addPersonal = (repo: string, name: string) => {
    setSettingsOpen(false);
    setPickerTarget({ repo, name, personal: true });
  };

  const removePersonal = async (repo: string) => {
    const next: Settings = {
      ...settings,
      personalMods: (settings.personalMods ?? []).filter((p) => p.repo !== repo),
    };
    setSettings(next);
    try {
      await bridge.saveSettings(next);
    } catch (e) {
      notify(String(e), "error");
    }
  };

  const togglePersonal = async (repo: string, enabled: boolean) => {
    const next: Settings = {
      ...settings,
      personalMods: (settings.personalMods ?? []).map((p) =>
        p.repo === repo ? { ...p, enabled } : p,
      ),
    };
    setSettings(next);
    try {
      await bridge.saveSettings(next);
    } catch (e) {
      notify(String(e), "error");
    }
  };

  const pickRelease = async (tag: string, assetName: string) => {
    const target = pickerTarget;
    if (!target) return;
    setPickerTarget(null);

    // personal "always-include" mod: store in settings, don't install to a profile
    if (target.personal) {
      const next: Settings = {
        ...settings,
        personalMods: [
          ...(settings.personalMods ?? []).filter((p) => p.repo !== target.repo),
          { repo: target.repo, tag, asset: assetName, name: target.name, enabled: (settings.personalMods ?? []).find((p) => p.repo === target.repo)?.enabled ?? true },
        ],
      };
      setSettings(next);
      try {
        await bridge.saveSettings(next);
        notify(`${target.name} will be added to every lobby you join`);
      } catch (e) {
        notify(String(e), "error");
      }
      return;
    }

    setBusyModId(target.repo);
    notify(`Installing ${assetName}…`);
    try {
      patchProfile(await bridge.installAsset(active, target.repo, tag, assetName, arch));
      notify(`Installed ${assetName}`);
      await ensureLoader(active.id);
      bridge.loadCatalog().then(setCatalog).catch(() => {});
    } catch (e) {
      notify(String(e), "error");
    } finally {
      setBusyModId(null);
    }
  };

  const removeCatalogItem = async (id: string) => {
    try {
      setCatalog(await bridge.removeCatalogMod(catalog, id));
    } catch (e) {
      notify(String(e), "error");
    }
  };

  const moveCatalogItem = async (id: string, dir: "up" | "down") => {
    const ids = catalog.map((c) => c.id);
    const i = ids.indexOf(id);
    const j = dir === "up" ? i - 1 : i + 1;
    if (i < 0 || j < 0 || j >= ids.length) return;
    [ids[i], ids[j]] = [ids[j], ids[i]];
    try {
      setCatalog(await bridge.reorderCatalog(catalog, ids));
    } catch (e) {
      notify(String(e), "error");
    }
  };

  const runLaunch = async (p: Profile) => {
    const gamePath = settings.gamePath ?? game?.path;
    try {
      setRunning(true);
      await bridge.launchProfile(gamePath ?? "", p.id);
      const isEpic = (game?.store ?? settings.store) === "epic";
      notify(
        isEpic
          ? `Launching ${p.name}. Epic may ask you to sign in the first time, that's normal.`
          : `Launching ${p.name}`,
      );
      if (bridge.inTauri) {
        // unlock when the game process exits, or after a grace if it never appears
        const start = Date.now();
        let seen = false;
        const poll = setInterval(async () => {
          let alive = false;
          try {
            alive = await bridge.gameRunning();
          } catch {
            return;
          }
          if (alive) {
            seen = true;
            return;
          }
          if (seen || Date.now() - start > 20000) {
            clearInterval(poll);
            setRunning(false);
          }
        }, 2000);
      } else {
        setTimeout(() => setRunning(false), 2800);
      }
    } catch (e) {
      setRunning(false);
      notify(String(e), "error");
    }
  };

  const doLaunchProfile = async (p: Profile) => {
    const gamePath = settings.gamePath ?? game?.path;
    if (bridge.inTauri && !gamePath) {
      notify("No Among Us folder set. Open Settings to choose it.", "error");
      return;
    }
    if (bridge.inTauri && gamePath && !settings.skipLaunchWarning) {
      const status = await bridge.loaderStatus(gamePath, p.id).catch(() => null);
      if (!status?.current) {
        setLaunchWarn(p);
        return;
      }
    }
    await runLaunch(p);
  };

  const launchWarnInstall = async () => {
    const p = launchWarn;
    if (!p) return;
    setLaunchWarn(null);
    await ensureLoader(p.id);
    await runLaunch(p);
  };

  const launchWarnAnyway = async (dontWarnAgain: boolean) => {
    const p = launchWarn;
    if (!p) return;
    setLaunchWarn(null);
    if (dontWarnAgain) {
      const next: Settings = { ...settings, skipLaunchWarning: true };
      setSettings(next);
      bridge.saveSettings(next).catch((e) => notify(String(e), "error"));
    }
    await runLaunch(p);
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
    setLobbyOpen(false);
    notify("Setting up lobby…");
    try {
      const built = await bridge.applyLobbyCode(code, arch, buildLobbyProfile());
      setProfiles((ps) => [...ps.filter((p) => p.id !== built.id), built]);
      setActiveId(built.id);
      await ensureLoader(built.id);
      if (doLaunch) {
        await doLaunchProfile(built);
      } else {
        const gamePath = settings.gamePath ?? game?.path;
        if (bridge.inTauri && gamePath) await bridge.syncProfile(gamePath, built.id);
        notify(`Lobby profile ready: ${built.name}`);
      }
    } catch (e) {
      notify(String(e), "error");
    }
  };

  const saveSettings = async (s: Settings) => {
    setSettings(s);
    setSettingsOpen(false);
    try {
      await bridge.saveSettings(s);
      notify("Settings saved");
    } catch (e) {
      notify(String(e), "error");
    }
  };

  const completeSetup = async (gamePath?: string, arch?: string, store?: string) => {
    const next: Settings = {
      ...settings,
      setupComplete: true,
      ...(gamePath ? { gamePath } : {}),
      ...(arch ? { arch: arch as Arch } : {}),
      ...(store ? { store: store as Store } : {}),
    };
    setSettings(next);
    try {
      await bridge.saveSettings(next);
    } catch (e) {
      notify(String(e), "error");
    }
  };

  const setupMods = async (p: Profile) => {
    const gamePath = settings.gamePath ?? game?.path;
    if (bridge.inTauri && !gamePath) {
      notify("No Among Us folder set. Open Settings to choose it.", "error");
      return;
    }
    notify("Setting up mods…");
    setBusyModId("__setup__");
    try {
      await bridge.syncProfile(gamePath ?? "", p.id);
      notify("Mods set up in your Among Us folder. Launch Among Us when ready.");
    } catch (e) {
      notify(String(e), "error");
    } finally {
      setBusyModId(null);
    }
  };

  return (
    <div className="flex h-[100dvh] flex-col">
      <TopBar
        onAddMod={() => setAddOpen(true)}
        onPasteCode={openLobbyFromCode}
        onOpenSettings={() => setSettingsOpen(true)}
      />

      {update && !updateDismissed && (
        <div className="mx-3 mt-2 flex items-center gap-3 rounded-xl border border-[rgba(123,150,255,0.35)] bg-[rgba(123,150,255,0.12)] px-4 py-2 text-[13px] text-[#cbd8ff]">
          <span className="flex-1">Perfect-Sync {update.version} is available.</span>
          <button
            type="button"
            onClick={() => bridge.openUrl(update.url)}
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
            onSelect={setActiveId}
            onNewProfile={newProfile}
            onPasteCode={openLobbyFromSidebar}
          />
          <MainPanel
            profile={active}
            game={gameStatus}
            busyModId={busyModId}
            trustOf={trustOf}
            onToggle={toggleMod}
            onRemove={removeMod}
            onPickRelease={openPicker}
            onShare={() => setShareOpen(true)}
            onRename={renameProfile}
            onDelete={deleteActiveProfile}
            onLaunch={() => doLaunchProfile(active)}
            onAddMod={() => setAddOpen(true)}
            onSetup={() => setupMods(active)}
          />
        </div>
      </div>

      <AddModPanel
        open={addOpen}
        profileName={active.name}
        catalog={catalog}
        onClose={() => setAddOpen(false)}
        onAddCatalog={addCatalog}
        onAddUrl={addUrl}
        onRemoveCatalog={removeCatalogItem}
        onMoveCatalog={moveCatalogItem}
      />
      <LobbyCodeModal
        open={lobbyOpen}
        initialCode={lobbyCode}
        installed={active.mods.map((m) => [m.packageId, m.version] as [string, string])}
        trustOf={trustOf}
        personalMods={settings.personalMods ?? []}
        onClose={() => setLobbyOpen(false)}
        onApply={applyLobby}
      />
      <SettingsModal
        open={settingsOpen}
        settings={settings}
        game={game}
        profileId={active.id}
        onClose={() => setSettingsOpen(false)}
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
        busy={busyModId !== null}
        onClose={() => setPickerTarget(null)}
        onPick={pickRelease}
      />
      <ShareModal
        open={shareOpen}
        profile={active}
        onClose={() => setShareOpen(false)}
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
        onCancel={() => setLaunchWarn(null)}
      />
      <Toast toast={toast} />
    </div>
  );
}

function buildLobbyProfile(): Profile {
  const find = (id: string) => CATALOG.find((c) => c.id === id)!;
  const tou = find("AU-Avengers/TOU-Mira");
  const sub = find("SubmergedAmongUs/Submerged");
  const li = find("Dolfannn/LevelImposter");
  const mk = (c: CatalogItem, version: string): ProfileMod => ({
    packageId: c.id,
    name: c.name,
    repo: c.repo,
    version,
    versions: [version],
    enabled: true,
    source: "github",
    tags: c.tags,
  });
  return {
    id: "lobby-townofus-night",
    name: "Lobby - TownOfUs Night",
    crewColor: CREW.gold,
    gameBuild: "17.0.1",
    mods: [
      mk(tou, "1.6.3"),
      mk(sub, "2025.11.20"),
      mk(li, "0.7.2"),
      {
        packageId: "All-Of-Us-Mods/MiraAPI",
        name: "MiraAPI",
        repo: "All-Of-Us-Mods/MiraAPI",
        version: "0.3.9",
        versions: ["0.3.9"],
        enabled: true,
        source: "catalog",
        tags: ["library"],
        managed: true,
      },
      {
        packageId: "NuclearPowered/Reactor",
        name: "Reactor",
        repo: "NuclearPowered/Reactor",
        version: "2.5.0",
        versions: ["2.5.0"],
        enabled: true,
        source: "catalog",
        tags: ["library"],
        managed: true,
      },
      {
        packageId: "BepInEx/BepInEx",
        name: "BepInEx",
        version: "6.0.0-be.735",
        versions: ["6.0.0-be.735"],
        enabled: true,
        source: "catalog",
        tags: ["loader"],
        managed: true,
      },
    ],
  };
}
