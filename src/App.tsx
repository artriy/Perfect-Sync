import { useRef, useState } from "react";
import { TopBar } from "./components/TopBar";
import { Sidebar } from "./components/Sidebar";
import { MainPanel } from "./components/MainPanel";
import { LobbyCodeModal } from "./components/LobbyCodeModal";
import { AddModPanel } from "./components/AddModPanel";
import { Toast, type ToastState } from "./components/Toast";
import { CATALOG, GAME, PROFILES, SAMPLE_CODE, SAMPLE_DIFF } from "./data/mock";
import { CREW } from "./lib/palette";
import type { CatalogItem, Profile, ProfileMod } from "./lib/types";

const CREW_CYCLE = Object.values(CREW);

export function App() {
  const [profiles, setProfiles] = useState<Profile[]>(PROFILES);
  const [activeId, setActiveId] = useState(PROFILES[0].id);
  const [running, setRunning] = useState(GAME.running);
  const [busyModId, setBusyModId] = useState<string | null>(null);

  const [addOpen, setAddOpen] = useState(false);
  const [lobbyOpen, setLobbyOpen] = useState(false);
  const [lobbyCode, setLobbyCode] = useState<string | undefined>(undefined);

  const [toast, setToast] = useState<ToastState | null>(null);
  const toastId = useRef(0);
  const notify = (msg: string) => {
    toastId.current += 1;
    const id = toastId.current;
    setToast({ id, msg });
    setTimeout(() => setToast((t) => (t?.id === id ? null : t)), 2600);
  };

  const active = profiles.find((p) => p.id === activeId) ?? profiles[0];

  const patchActive = (fn: (mods: ProfileMod[]) => ProfileMod[]) =>
    setProfiles((ps) => ps.map((p) => (p.id === active.id ? { ...p, mods: fn(p.mods) } : p)));

  const toggleMod = (modId: string) =>
    patchActive((mods) => mods.map((m) => (m.packageId === modId ? { ...m, enabled: !m.enabled } : m)));

  const changeVersion = (modId: string, v: string) => {
    setBusyModId(modId);
    // simulate fetching + swapping the release
    setTimeout(() => {
      patchActive((mods) =>
        mods.map((m) =>
          m.packageId === modId ? { ...m, version: v, update: m.update === v ? undefined : m.update } : m,
        ),
      );
      setBusyModId(null);
      const name = active.mods.find((m) => m.packageId === modId)?.name ?? "mod";
      notify(`${name} set to ${v}`);
    }, 700);
  };

  const removeMod = (modId: string) => {
    const name = active.mods.find((m) => m.packageId === modId)?.name ?? "mod";
    patchActive((mods) => mods.filter((m) => m.packageId !== modId));
    notify(`Removed ${name}`);
  };

  const newProfile = () => {
    const n = profiles.filter((p) => p.name.startsWith("New profile")).length + 1;
    const id = `new-${Date.now()}`;
    const color = CREW_CYCLE[profiles.length % CREW_CYCLE.length];
    setProfiles((ps) => [...ps, { id, name: `New profile ${n}`, crewColor: color, mods: [] }]);
    setActiveId(id);
  };

  const hasRoleMod = (mods: ProfileMod[]) =>
    mods.some((m) => !m.managed && m.tags.includes("role"));

  const addCatalog = (item: CatalogItem) => {
    if (active.mods.some((m) => m.packageId === item.id)) {
      notify(`${item.name} is already in this profile`);
      return;
    }
    if (item.tags.includes("role") && hasRoleMod(active.mods)) {
      notify("Only one role mod per profile. Remove the current one first.");
      return;
    }
    patchActive((mods) => [
      {
        packageId: item.id,
        name: item.name,
        repo: item.repo,
        version: item.latest,
        versions: [item.latest],
        enabled: true,
        source: "catalog",
        tags: item.tags,
      },
      ...mods,
    ]);
    setAddOpen(false);
    notify(`Added ${item.name} to ${active.name}`);
  };

  const addUrl = (url: string) => {
    const m = url.match(/github\.com\/([^/]+)\/([^/#?]+)/i);
    const repo = m ? `${m[1]}/${m[2]}` : url;
    const name = m ? m[2] : "Mod";
    if (active.mods.some((mod) => mod.packageId === repo)) {
      notify(`${name} is already in this profile`);
      return;
    }
    patchActive((mods) => [
      {
        packageId: repo,
        name,
        repo,
        version: "latest",
        versions: ["latest"],
        enabled: true,
        source: "github",
        tags: [],
      },
      ...mods,
    ]);
    setAddOpen(false);
    notify(`Added ${name} from GitHub`);
  };

  const copyCode = () => {
    navigator.clipboard?.writeText(SAMPLE_CODE).catch(() => {});
    notify("Lobby code copied to clipboard");
  };

  const launch = () => {
    if (running) return;
    setRunning(true);
    notify(`Launching ${active.name}`);
    setTimeout(() => setRunning(false), 2800);
  };

  const openLobbyFromSidebar = () => {
    setLobbyCode(undefined);
    setLobbyOpen(true);
  };
  const openLobbyFromCode = (code: string) => {
    setLobbyCode(code);
    setLobbyOpen(true);
  };

  const applyLobby = (doLaunch: boolean) => {
    const built = buildLobbyProfile();
    setProfiles((ps) => {
      const without = ps.filter((p) => p.id !== built.id);
      return [...without, built];
    });
    setActiveId(built.id);
    setLobbyOpen(false);
    if (doLaunch) {
      setRunning(true);
      notify(`Applied lobby and launching ${built.name}`);
      setTimeout(() => setRunning(false), 2800);
    } else {
      notify(`Lobby profile ready: ${built.name}`);
    }
  };

  return (
    <div className="flex h-[100dvh] flex-col">
      <TopBar game={{ ...GAME, running }} onAddMod={() => setAddOpen(true)} onPasteCode={openLobbyFromCode} />

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
            game={{ ...GAME, running }}
            busyModId={busyModId}
            onToggle={toggleMod}
            onVersion={changeVersion}
            onRemove={removeMod}
            onCopyCode={copyCode}
            onLaunch={launch}
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
