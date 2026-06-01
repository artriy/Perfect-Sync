import { useEffect, useRef, useState } from "react";
import { TopBar } from "./components/TopBar";
import { Sidebar } from "./components/Sidebar";
import { MainPanel } from "./components/MainPanel";
import { LobbyCodeModal } from "./components/LobbyCodeModal";
import { AddModPanel } from "./components/AddModPanel";
import { SettingsModal } from "./components/SettingsModal";
import { Toast, type ToastState } from "./components/Toast";
import * as bridge from "./lib/bridge";
import { CATALOG, SAMPLE_DIFF } from "./data/mock";
import { CREW } from "./lib/palette";
import type { Arch, CatalogItem, GameInstall, Profile, ProfileMod, Settings } from "./lib/types";

const CREW_CYCLE = Object.values(CREW);

export function App() {
  const [loaded, setLoaded] = useState(false);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeId, setActiveId] = useState<string>("");
  const [running, setRunning] = useState(false);
  const [busyModId, setBusyModId] = useState<string | null>(null);

  const [game, setGame] = useState<GameInstall | null>(null);
  const [settings, setSettings] = useState<Settings>({});

  const [addOpen, setAddOpen] = useState(false);
  const [lobbyOpen, setLobbyOpen] = useState(false);
  const [lobbyCode, setLobbyCode] = useState<string | undefined>(undefined);
  const [settingsOpen, setSettingsOpen] = useState(false);

  const [toast, setToast] = useState<ToastState | null>(null);
  const toastId = useRef(0);
  const notify = (msg: string) => {
    toastId.current += 1;
    const id = toastId.current;
    setToast({ id, msg });
    setTimeout(() => setToast((t) => (t?.id === id ? null : t)), 2600);
  };

  // Load settings, detect the game, and read persisted profiles on startup.
  useEffect(() => {
    (async () => {
      const [st, games, profs] = await Promise.all([
        bridge.getSettings(),
        bridge.detectGames(),
        bridge.loadProfiles(),
      ]);
      setSettings(st);
      setGame(games[0] ?? null);
      let list = profs;
      if (list.length === 0) {
        const starter: Profile = { id: "my-mods", name: "My mods", crewColor: CREW.violet, mods: [] };
        await bridge.saveProfile(starter);
        list = [starter];
      }
      setProfiles(list);
      setActiveId(list[0].id);
      setLoaded(true);
    })().catch((e) => {
      notify(String(e));
      setLoaded(true);
    });
  }, []);

  const arch: Arch = game?.arch ?? settings.arch ?? "x86";
  const gameStatus = { store: game?.store ?? "steam", arch, running };
  const firstRun = loaded && !game && !settings.gamePath;

  const active = profiles.find((p) => p.id === activeId) ?? profiles[0];

  if (!loaded || !active) {
    return (
      <div className="grid h-[100dvh] place-items-center">
        <p className="subtitle text-ink-dim">Loading Perfect-Sync…</p>
      </div>
    );
  }

  const patchProfile = (updated: Profile) =>
    setProfiles((ps) => ps.map((p) => (p.id === updated.id ? updated : p)));

  const hasRoleMod = (mods: ProfileMod[]) => mods.some((m) => !m.managed && m.tags.includes("role"));

  const toggleMod = async (modId: string) => {
    const mod = active.mods.find((m) => m.packageId === modId);
    if (!mod) return;
    try {
      patchProfile(await bridge.setModEnabled(active, modId, !mod.enabled));
    } catch (e) {
      notify(String(e));
    }
  };

  const changeVersion = async (modId: string, v: string) => {
    setBusyModId(modId);
    const name = active.mods.find((m) => m.packageId === modId)?.name ?? "mod";
    try {
      patchProfile(await bridge.setModVersion(active, modId, v, arch));
      notify(`${name} set to ${v}`);
    } catch (e) {
      notify(String(e));
    } finally {
      setBusyModId(null);
    }
  };

  const removeMod = async (modId: string) => {
    const name = active.mods.find((m) => m.packageId === modId)?.name ?? "mod";
    try {
      patchProfile(await bridge.removeMod(active, modId));
      notify(`Removed ${name}`);
    } catch (e) {
      notify(String(e));
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
      notify(String(e));
    }
  };

  const addCatalog = async (item: CatalogItem) => {
    if (active.mods.some((m) => m.packageId === item.id)) {
      notify(`${item.name} is already in this profile`);
      return;
    }
    if (item.tags.includes("role") && hasRoleMod(active.mods)) {
      notify("Only one role mod per profile. Remove the current one first.");
      return;
    }
    setAddOpen(false);
    const browserMod: ProfileMod = {
      packageId: item.id,
      name: item.name,
      repo: item.repo,
      version: item.latest,
      versions: [item.latest],
      enabled: true,
      source: "catalog",
      tags: item.tags,
    };
    notify(`Adding ${item.name}…`);
    try {
      patchProfile(await bridge.addMod(active, item.repo, arch, browserMod));
      notify(`Added ${item.name} to ${active.name}`);
    } catch (e) {
      notify(String(e));
    }
  };

  const addUrl = async (url: string) => {
    const m = url.match(/github\.com\/([^/]+)\/([^/#?]+)/i);
    const repo = m ? `${m[1]}/${m[2]}` : url;
    const name = m ? m[2] : "Mod";
    if (active.mods.some((mod) => mod.packageId === repo)) {
      notify(`${name} is already in this profile`);
      return;
    }
    setAddOpen(false);
    const browserMod: ProfileMod = {
      packageId: repo,
      name,
      repo,
      version: "latest",
      versions: ["latest"],
      enabled: true,
      source: "github",
      tags: [],
    };
    notify(`Adding ${name}…`);
    try {
      patchProfile(await bridge.addMod(active, url, arch, browserMod));
      notify(`Added ${name} from GitHub`);
    } catch (e) {
      notify(String(e));
    }
  };

  const copyCode = async () => {
    try {
      const code = await bridge.encodeLobbyCode(active);
      await navigator.clipboard?.writeText(code);
      notify("Lobby code copied to clipboard");
    } catch (e) {
      notify(String(e));
    }
  };

  const doLaunchProfile = async (p: Profile) => {
    const gamePath = game?.path ?? settings.gamePath;
    if (bridge.inTauri && !gamePath) {
      notify("No game detected. Set the game path in Settings.");
      return;
    }
    try {
      setRunning(true);
      await bridge.launchProfile(gamePath ?? "", p.id);
      notify(`Launching ${p.name}`);
      if (!bridge.inTauri) setTimeout(() => setRunning(false), 2800);
    } catch (e) {
      setRunning(false);
      notify(String(e));
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
    setLobbyOpen(false);
    notify("Setting up lobby…");
    try {
      const built = await bridge.applyLobbyCode(code, arch, buildLobbyProfile());
      setProfiles((ps) => [...ps.filter((p) => p.id !== built.id), built]);
      setActiveId(built.id);
      if (doLaunch) await doLaunchProfile(built);
      else notify(`Lobby profile ready: ${built.name}`);
    } catch (e) {
      notify(String(e));
    }
  };

  const saveSettings = async (s: Settings) => {
    setSettings(s);
    setSettingsOpen(false);
    try {
      await bridge.saveSettings(s);
      notify("Settings saved");
    } catch (e) {
      notify(String(e));
    }
  };

  return (
    <div className="flex h-[100dvh] flex-col">
      <TopBar
        game={gameStatus}
        onAddMod={() => setAddOpen(true)}
        onPasteCode={openLobbyFromCode}
        onOpenSettings={() => setSettingsOpen(true)}
      />

      {firstRun && (
        <button
          type="button"
          onClick={() => setSettingsOpen(true)}
          className="ring-focus mx-3 mt-2 rounded-xl border border-[rgba(255,210,63,0.35)] bg-[rgba(255,210,63,0.12)] px-4 py-2 text-left text-[13px] text-[#ffe49a]"
        >
          No Among Us install detected. Click to open Settings and point Perfect-Sync at your game.
        </button>
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
            onToggle={toggleMod}
            onVersion={changeVersion}
            onRemove={removeMod}
            onCopyCode={copyCode}
            onLaunch={() => doLaunchProfile(active)}
            onAddMod={() => setAddOpen(true)}
          />
        </div>
      </div>

      <AddModPanel
        open={addOpen}
        profileName={active.name}
        onClose={() => setAddOpen(false)}
        onAddCatalog={addCatalog}
        onAddUrl={addUrl}
      />
      <LobbyCodeModal
        open={lobbyOpen}
        initialCode={lobbyCode}
        diff={SAMPLE_DIFF}
        onClose={() => setLobbyOpen(false)}
        onApply={applyLobby}
      />
      <SettingsModal
        open={settingsOpen}
        settings={settings}
        game={game}
        onClose={() => setSettingsOpen(false)}
        onSave={saveSettings}
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
