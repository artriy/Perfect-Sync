import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { DotsThree, PencilSimple, PlusCircle, ShareNetwork, Stack, TrashSimple } from "@phosphor-icons/react";
import { ModRow } from "./ModRow";
import { LaunchBar } from "./LaunchBar";
import type { GameStatus, Profile, Trust } from "../lib/types";

interface MainPanelProps {
  profile: Profile;
  game: GameStatus;
  busyModId: string | null;
  onToggle: (modId: string) => void;
  onRemove: (modId: string) => void;
  onPickRelease: (modId: string) => void;
  onShare: () => void;
  onRename: (name: string) => void;
  onDelete: () => void;
  onLaunch: () => void;
  onAddMod: () => void;
  onSetup: () => void;
  trustOf: (id: string) => Trust;
}

export function MainPanel(props: MainPanelProps) {
  const { profile, game, busyModId } = props;
  const reduce = useReducedMotion();
  const userMods = profile.mods.filter((m) => !m.managed);
  const updates = userMods.filter((m) => m.update).length;

  const [menuOpen, setMenuOpen] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState(profile.name);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setRenaming(false);
    setMenuOpen(false);
  }, [profile.id]);

  useEffect(() => {
    if (!menuOpen) return;
    const onDoc = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenuOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [menuOpen]);

  const commitRename = () => {
    const name = draft.trim();
    setRenaming(false);
    if (name && name !== profile.name) props.onRename(name);
  };

  return (
    <section className="flex min-w-0 flex-1 flex-col">
      <div className="flex items-end gap-4 px-6 pt-5 pb-3">
        <div className="min-w-0">
          {renaming ? (
            <input
              value={draft}
              autoFocus
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitRename();
                if (e.key === "Escape") setRenaming(false);
              }}
              onBlur={commitRename}
              aria-label="Profile name"
              className="glass w-full rounded-lg px-2 py-1 text-[24px] font-semibold text-ink focus:outline-none"
            />
          ) : (
            <h1 className="truncate text-[26px] leading-tight font-semibold text-ink">{profile.name}</h1>
          )}
          <div className="mt-1 flex items-center gap-2 text-[13px] text-ink-dim">
            <span>
              {userMods.length} mods
              {updates > 0 ? ` · ${updates} update${updates > 1 ? "s" : ""} available` : ""}
            </span>
            {profile.gameBuild && (
              <span className="glass rounded-full px-2 py-0.5 text-[11.5px] text-ink-dim">
                built for Among Us {profile.gameBuild}
              </span>
            )}
          </div>
        </div>

        <div className="flex-1" />

        <button
          type="button"
          onClick={props.onShare}
          className="ring-focus glass flex items-center gap-1.5 rounded-xl px-3 py-2 text-[13px] text-ink-dim transition-colors hover:text-ink"
        >
          <ShareNetwork size={15} /> Share lobby
        </button>
        <div className="relative" ref={menuRef}>
          <button
            type="button"
            aria-label="More profile actions"
            onClick={() => setMenuOpen((o) => !o)}
            className="ring-focus glass grid h-9 w-9 place-items-center rounded-xl text-ink-dim transition-colors hover:text-ink"
          >
            <DotsThree size={18} weight="bold" />
          </button>
          <AnimatePresence>
            {menuOpen && (
              <motion.div
                initial={reduce ? false : { opacity: 0, y: -6, scale: 0.98 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={reduce ? { opacity: 0 } : { opacity: 0, y: -6, scale: 0.98 }}
                transition={{ duration: 0.14, ease: [0.16, 1, 0.3, 1] }}
                className="glass-strong absolute right-0 z-30 mt-2 w-44 origin-top-right rounded-xl p-1.5"
              >
                <button
                  type="button"
                  onClick={() => {
                    setMenuOpen(false);
                    setDraft(profile.name);
                    setRenaming(true);
                  }}
                  className="ring-focus flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-[13px] text-ink-dim hover:bg-white/10 hover:text-ink"
                >
                  <PencilSimple size={15} /> Rename profile
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setMenuOpen(false);
                    props.onDelete();
                  }}
                  className="ring-focus flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-[13px] text-[#ff8a8a] hover:bg-[rgba(226,59,59,0.15)]"
                >
                  <TrashSimple size={15} /> Delete profile
                </button>
              </motion.div>
            )}
          </AnimatePresence>
        </div>
      </div>

      <motion.div
        key={profile.id}
        initial={reduce ? false : { opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.25, ease: [0.16, 1, 0.3, 1] }}
        className="scroll-region flex flex-1 flex-col gap-2.5 overflow-y-auto px-6 pb-4"
      >
        {userMods.length === 0 ? (
          <EmptyState onAddMod={props.onAddMod} />
        ) : (
          profile.mods.map((mod) => (
            <ModRow
              key={mod.packageId}
              mod={mod}
              trust={props.trustOf(mod.packageId)}
              busy={busyModId === mod.packageId}
              onToggle={() => props.onToggle(mod.packageId)}
              onRemove={() => props.onRemove(mod.packageId)}
              onPickRelease={() => props.onPickRelease(mod.packageId)}
            />
          ))
        )}
      </motion.div>

      <LaunchBar
        profileName={profile.name}
        running={game.running}
        busy={busyModId !== null}
        onLaunch={props.onLaunch}
        onSetup={props.onSetup}
      />
    </section>
  );
}

function EmptyState({ onAddMod }: { onAddMod: () => void }) {
  return (
    <div className="grid flex-1 place-items-center py-16 text-center">
      <div className="max-w-sm">
        <div className="glass mx-auto grid h-14 w-14 place-items-center rounded-2xl text-ink-dim">
          <Stack size={26} />
        </div>
        <h2 className="mt-4 text-[18px] font-semibold text-ink">No mods in this profile yet</h2>
        <p className="mt-1.5 text-[13.5px] leading-relaxed text-ink-dim">
          Add a mod from the catalog, paste a GitHub release URL, or apply a friend's lobby code to fill this profile.
        </p>
        <button
          type="button"
          onClick={onAddMod}
          className="ring-focus accent-grad mx-auto mt-5 flex items-center gap-1.5 rounded-xl px-4 py-2.5 text-[13.5px] font-semibold text-[#0d0820] transition-transform active:scale-[0.97]"
        >
          <PlusCircle size={16} weight="bold" /> Add a mod
        </button>
      </div>
    </div>
  );
}
