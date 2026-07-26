import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { ArrowsClockwise, CaretDown, DotsThree, GameController, PencilSimple, PlusCircle, ShareNetwork, Stack, TrashSimple, Warning } from "@phosphor-icons/react";
import { ModRow } from "./ModRow";
import { LaunchBar } from "./LaunchBar";
import type { GameInstance, GameStatus, Profile, Trust, UnmanagedPlugin } from "../lib/types";
import { useModalFocus } from "../lib/useModalFocus";
import { displayPath } from "../lib/displayPath";

const TOWN_OF_US_BUNDLED_IDS: Record<string, true> = {
  "all-of-us-mods/miraapi": true,
  "nuclearpowered/reactor": true,
  "miniduikboot/mini.regioninstall": true,
};

interface MainPanelProps {
  profile: Profile;
  game: GameStatus;
  gameInstances: GameInstance[];
  busy: boolean;
  unmanagedPlugins: readonly UnmanagedPlugin[];
  unmanagedLoading: boolean;
  unmanagedError: string | null;
  onToggle: (modId: string) => void;
  onRemove: (modId: string) => Promise<void>;
  onPickRelease: (modId: string) => void;
  onShare: () => void;
  onReviewUpdates: () => void;
  onRename: (name: string) => void;
  onDelete: () => Promise<void>;
  onLaunch: () => void;
  onAddMod: () => void;
  onSetup: () => void;
  onSelectGameInstance: (id: string) => void;
  onBrowseMaps: () => void;
  onManageGameInstances: () => void;
  onReviewUnmanaged: () => void;
  trustOf: (id: string) => Trust;
}

export function MainPanel(props: MainPanelProps) {
  const { profile, game, busy } = props;
  const reduce = useReducedMotion();
  const hasTownOfUs = profile.mods.some(
    (mod) =>
      mod.enabled &&
      (mod.packageId.toLowerCase() === "au-avengers/tou-mira" ||
        mod.repo?.toLowerCase() === "au-avengers/tou-mira"),
  );
  const userMods = profile.mods.filter(
    (mod) =>
      !mod.managed &&
      !(
        hasTownOfUs &&
        (TOWN_OF_US_BUNDLED_IDS[mod.packageId.toLowerCase()] === true ||
          (mod.repo ? TOWN_OF_US_BUNDLED_IDS[mod.repo.toLowerCase()] === true : false))
      ),
  );
  const updates = userMods.filter((m) => m.update).length;
  const selectedGame =
    props.gameInstances.find((instance) => instance.id === profile.gameInstanceId) ??
    props.gameInstances[0] ??
    null;

  const [menuOpen, setMenuOpen] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState(profile.name);
  const menuRef = useRef<HTMLDivElement>(null);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const deletePendingRef = useRef(false);
  const deleteDialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setDraft(profile.name.slice(0, 80));
    setRenaming(false);
    setMenuOpen(false);
    setDeleteOpen(false);
    setDeleting(false);
    deletePendingRef.current = false;
  }, [profile.id, profile.name]);

  useEffect(() => {
    if (!menuOpen) return;
    const onDoc = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenuOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [menuOpen]);

  useEffect(() => {
    if (!busy) return;
    setMenuOpen(false);
    setRenaming(false);
  }, [busy]);

  const closeDelete = useCallback(() => {
    if (!deletePendingRef.current) setDeleteOpen(false);
  }, []);

  useModalFocus(deleteOpen, deleteDialogRef, closeDelete);

  const beginRename = () => {
    if (busy) return;
    setMenuOpen(false);
    setDraft(profile.name.slice(0, 80));
    setRenaming(true);
  };

  const commitRename = () => {
    const name = draft.trim().slice(0, 80);
    setRenaming(false);
    if (!busy && name && name !== profile.name) props.onRename(name);
  };

  const confirmDelete = async () => {
    if (busy || deletePendingRef.current) return;
    deletePendingRef.current = true;
    setDeleting(true);
    try {
      await props.onDelete();
      setDeleteOpen(false);
    } catch {
      // The parent reports the backend error; keep the confirmation visible for retry or cancel.
    } finally {
      deletePendingRef.current = false;
      setDeleting(false);
    }
  };

  return (
    <section className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden max-[720px]:overflow-visible">
      <div className="flex items-center gap-3 border-b border-white/[0.06] px-6 py-2.5 max-[720px]:flex-wrap max-[720px]:px-3">
        <span className="text-[12px] font-medium tracking-[0.1em] text-ink-faint uppercase">
          Among Us instance
        </span>
        {selectedGame ? (
          <>
            <label className="glass relative flex max-w-[360px] min-w-0 flex-1 items-center gap-2 rounded-lg px-2.5 py-1.5 max-[720px]:max-w-none">
              <GameController size={14} className="shrink-0 text-accent-2" />
              <select
                value={selectedGame.id}
                disabled={busy}
                onChange={(e) => props.onSelectGameInstance(e.target.value)}
                aria-label="Among Us instance for this profile"
                title={`${selectedGame.name} · ${selectedGame.store} · ${selectedGame.arch}`}
                className="min-w-0 flex-1 appearance-none truncate bg-transparent pr-5 text-[12.5px] font-medium text-ink focus:outline-none disabled:cursor-not-allowed disabled:opacity-60"
              >
                {props.gameInstances.map((instance) => (
                  <option key={instance.id} value={instance.id} className="bg-[#171225] text-ink">
                    {instance.name} · {instance.store} · {instance.arch}
                  </option>
                ))}
              </select>
              <CaretDown size={12} weight="bold" className="pointer-events-none absolute right-2.5 text-ink-faint" />
            </label>
            <span
              className="min-w-0 flex-1 truncate font-mono text-[12px] text-ink-faint max-[720px]:basis-full"
              title={displayPath(selectedGame.path)}
            >
              {displayPath(selectedGame.path)}
            </span>
          </>
        ) : (
          <button
            type="button"
            disabled={busy}
            onClick={props.onManageGameInstances}
            className="ring-focus glass rounded-lg px-2.5 py-1.5 text-[12px] font-semibold text-accent-2 hover:text-ink disabled:cursor-not-allowed disabled:opacity-60"
          >
            Add an Among Us folder in Settings
          </button>
        )}
      </div>
      {(props.unmanagedPlugins.length > 0 || props.unmanagedError) && (
        <button
          type="button"
          disabled={busy || props.unmanagedLoading}
          onClick={props.onReviewUnmanaged}
          className="ring-focus mx-3 mt-3 flex min-w-0 items-center gap-3 rounded-2xl border border-[rgba(255,193,92,0.24)] bg-[rgba(255,193,92,0.075)] px-3.5 py-3 text-left transition-colors hover:bg-[rgba(255,193,92,0.11)] disabled:cursor-not-allowed disabled:opacity-60 sm:mx-6"
        >
          <Warning size={19} weight="fill" className="shrink-0 text-[#ffd9a8]" aria-hidden="true" />
          <span className="min-w-0 flex-1">
            <span className="block text-[13.5px] font-semibold text-[#ffe2b7]">
              {props.unmanagedError
                ? "Could not verify the game’s plugin folder"
                : `${props.unmanagedPlugins.length} extra plugin${props.unmanagedPlugins.length === 1 ? "" : "s"} will load`}
            </span>
            <span
              className="mt-0.5 block truncate text-[12px] text-[#d8c4aa]"
              title={props.unmanagedError ?? props.unmanagedPlugins.map((plugin) => plugin.path).join(", ")}
            >
              {props.unmanagedError ?? (
                <>
                  {props.unmanagedPlugins.slice(0, 3).map((plugin) => plugin.name).join(", ")}
                  {props.unmanagedPlugins.length > 3 ? ` and ${props.unmanagedPlugins.length - 3} more` : ""}
                </>
              )}
            </span>
          </span>
          <span className="shrink-0 text-[12.5px] font-semibold text-[#ffe2b7]">
            {props.unmanagedError ? "Retry" : "Review"}
          </span>
        </button>
      )}
      <div className="flex min-w-0 items-end gap-3 px-6 pt-5 pb-3 max-[720px]:flex-wrap max-[720px]:px-3 max-[720px]:pt-4">
        <div className="min-w-0 flex-1">
          {renaming ? (
            <input
              value={draft}
              maxLength={80}
              disabled={busy}
              autoFocus
              onChange={(e) => setDraft(e.target.value.slice(0, 80))}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitRename();
                if (e.key === "Escape") setRenaming(false);
              }}
              onBlur={commitRename}
              aria-label="Profile name"
              className="glass w-full max-w-[36rem] rounded-lg px-2 py-1 text-[24px] font-semibold text-ink focus:outline-none disabled:opacity-60"
            />
          ) : (
            <button
              type="button"
              disabled={busy}
              onClick={beginRename}
              aria-label={`Rename profile ${profile.name}`}
              title="Click to rename profile"
              className="ring-focus group flex max-w-full items-center gap-2 rounded-lg text-left text-[26px] leading-tight font-semibold text-ink hover:text-white disabled:cursor-not-allowed disabled:opacity-60"
            >
              <span className="truncate">{profile.name}</span>
              <PencilSimple
                size={17}
                aria-hidden="true"
                className="shrink-0 text-ink-faint opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"
              />
            </button>
          )}
          <div className="mt-1 flex items-center gap-2 text-[13px] text-ink-dim">
            <span>
              {userMods.length} mods
              {updates > 0 ? ` · ${updates} update${updates > 1 ? "s" : ""} available` : ""}
            </span>
          </div>
        </div>


        {updates > 0 && (
          <button
            type="button"
            disabled={busy}
            onClick={props.onReviewUpdates}
            className="ring-focus flex shrink-0 items-center gap-1.5 rounded-xl bg-[#9b7bff]/14 px-3 py-2 text-[13px] font-semibold text-[#d4c6ff] transition-colors hover:bg-[#9b7bff]/22 disabled:cursor-not-allowed disabled:opacity-50"
          >
            <ArrowsClockwise size={15} weight="bold" />
            Review {updates} update{updates === 1 ? "" : "s"}
          </button>
        )}

        <button
          type="button"
          disabled={busy}
          onClick={props.onShare}
          className="ring-focus glass flex shrink-0 items-center gap-1.5 rounded-xl px-3 py-2 text-[13px] text-ink-dim transition-colors hover:text-ink disabled:cursor-not-allowed disabled:opacity-50"
        >
          <ShareNetwork size={15} /> Share lobby
        </button>
        <div className="relative" ref={menuRef}>
          <button
            type="button"
            aria-label="More profile actions"
            aria-expanded={menuOpen}
            disabled={busy}
            onClick={() => setMenuOpen((o) => !o)}
            className="ring-focus glass grid h-9 w-9 shrink-0 place-items-center rounded-xl text-ink-dim transition-colors hover:text-ink disabled:cursor-not-allowed disabled:opacity-50"
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
                  disabled={busy}
                  onClick={beginRename}
                  className="ring-focus flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-[13px] text-ink-dim hover:bg-white/10 hover:text-ink disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <PencilSimple size={15} /> Rename profile
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => {
                    setMenuOpen(false);
                    setDeleteOpen(true);
                  }}
                  className="ring-focus flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-[13px] text-[#ff8a8a] hover:bg-[rgba(226,59,59,0.15)] disabled:cursor-not-allowed disabled:opacity-50"
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
        className="scroll-region flex min-h-0 flex-1 flex-col gap-2.5 overflow-y-auto px-6 pb-4 max-[720px]:overflow-visible max-[720px]:px-3"
      >
        {userMods.length === 0 ? (
          <EmptyState onAddMod={props.onAddMod} busy={busy} />
        ) : (
          userMods.map((mod) => (
            <ModRow
              key={mod.packageId}
              mod={mod}
              trust={props.trustOf(mod.packageId)}
              busy={busy}
              onToggle={() => props.onToggle(mod.packageId)}
              onRemove={() => props.onRemove(mod.packageId)}
              onPickRelease={() => props.onPickRelease(mod.packageId)}
              onBrowseMaps={
                mod.packageId.toLowerCase() === "digiworm0/levelimposter"
                  ? props.onBrowseMaps
                  : undefined
              }
            />
          ))
        )}
      </motion.div>

      <LaunchBar
        profileName={profile.name}
        running={game.running}
        busy={busy}
        onLaunch={props.onLaunch}
        onSetup={props.onSetup}
      />

      <AnimatePresence>
        {deleteOpen && (
          <motion.div
            className="fixed inset-0 z-[60] grid place-items-center bg-[rgba(6,4,18,0.68)] p-6"
            style={{ backdropFilter: "blur(2px)" }}
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.16 }}
            onMouseDown={(event) => {
              if (event.target === event.currentTarget) closeDelete();
            }}
          >
            <motion.div
              ref={deleteDialogRef}
              role="alertdialog"
              aria-modal="true"
              aria-labelledby="delete-profile-title"
              aria-describedby="delete-profile-description"
              aria-busy={deleting}
              tabIndex={-1}
              initial={reduce ? { opacity: 0 } : { opacity: 0, y: 10, scale: 0.97 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={reduce ? { opacity: 0 } : { opacity: 0, y: 6, scale: 0.98 }}
              transition={{ duration: 0.18, ease: [0.16, 1, 0.3, 1] }}
              className="glass-strong relative max-h-[calc(100dvh-3rem)] w-[420px] max-w-full overflow-y-auto rounded-3xl p-6"
            >
              <h2 id="delete-profile-title" className="text-[18px] font-semibold text-ink">
                Delete profile?
              </h2>
              <p id="delete-profile-description" className="mt-2 break-words text-[13.5px] leading-relaxed text-ink-dim">
                Delete “<strong className="font-semibold text-ink">{profile.name}</strong>”? This removes the
                profile and its saved mod selection.
              </p>
              <div className="mt-5 flex justify-end gap-2.5">
                <button
                  type="button"
                  data-autofocus
                  disabled={deleting}
                  onClick={closeDelete}
                  className="ring-focus glass rounded-xl px-4 py-2.5 text-[13.5px] text-ink disabled:cursor-not-allowed disabled:opacity-50"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  disabled={deleting || busy}
                  onClick={() => void confirmDelete()}
                  className="ring-focus rounded-xl bg-[#e23b3b] px-4 py-2.5 text-[13.5px] font-semibold text-white hover:bg-[#ef5151] disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {deleting ? "Deleting…" : "Delete profile"}
                </button>
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
    </section>
  );
}

function EmptyState({ onAddMod, busy }: { onAddMod: () => void; busy: boolean }) {
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
          disabled={busy}
          className="ring-focus accent-grad mx-auto mt-5 flex items-center gap-1.5 rounded-xl px-4 py-2.5 text-[13.5px] font-semibold text-[#0d0820] transition-transform active:scale-[0.97] disabled:cursor-not-allowed disabled:opacity-50"
        >
          <PlusCircle size={16} weight="bold" /> Add a mod
        </button>
      </div>
    </div>
  );
}
