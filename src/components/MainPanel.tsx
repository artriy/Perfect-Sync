import { motion, useReducedMotion } from "motion/react";
import { Copy, DotsThree, PlusCircle, Stack } from "@phosphor-icons/react";
import { ModRow } from "./ModRow";
import { LaunchBar } from "./LaunchBar";
import type { GameStatus, Profile } from "../lib/types";

interface MainPanelProps {
  profile: Profile;
  game: GameStatus;
  busyModId: string | null;
  onToggle: (modId: string) => void;
  onVersion: (modId: string, v: string) => void;
  onRemove: (modId: string) => void;
  onCopyCode: () => void;
  onLaunch: () => void;
  onAddMod: () => void;
}

export function MainPanel(props: MainPanelProps) {
  const { profile, game, busyModId } = props;
  const reduce = useReducedMotion();
  const userMods = profile.mods.filter((m) => !m.managed);
  const updates = userMods.filter((m) => m.update).length;

  return (
    <section className="flex min-w-0 flex-1 flex-col">
      <div className="flex items-end gap-4 px-6 pt-5 pb-3">
        <div className="min-w-0">
          <h1 className="truncate text-[26px] leading-tight font-semibold text-ink">{profile.name}</h1>
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
          onClick={props.onCopyCode}
          className="ring-focus glass flex items-center gap-1.5 rounded-xl px-3 py-2 text-[13px] text-ink-dim transition-colors hover:text-ink"
        >
          <Copy size={15} /> Copy lobby code
        </button>
        <button
          type="button"
          aria-label="More profile actions"
          className="ring-focus glass grid h-9 w-9 place-items-center rounded-xl text-ink-dim transition-colors hover:text-ink"
        >
          <DotsThree size={18} weight="bold" />
        </button>
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
              busy={busyModId === mod.packageId}
              onToggle={() => props.onToggle(mod.packageId)}
              onVersion={(v) => props.onVersion(mod.packageId, v)}
              onRemove={() => props.onRemove(mod.packageId)}
            />
          ))
        )}
      </motion.div>

      <LaunchBar
        profileName={profile.name}
        running={game.running}
        busy={busyModId !== null}
        onLaunch={props.onLaunch}
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
