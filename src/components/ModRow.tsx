import { useCallback, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { motion, useReducedMotion } from "motion/react";
import {
  ArrowUp,
  CaretDown,
  CircleNotch,
  GearSix,
  MapTrifold,
  PuzzlePiece,
  Sparkle,
  TrashSimple,
  UsersThree,
  type Icon,
} from "@phosphor-icons/react";
import { Pill, primaryTag } from "./Pill";
import { TrustBadge } from "./TrustBadge";
import { Toggle } from "./Toggle";
import type { ModTag, ProfileMod, Trust } from "../lib/types";
import { useModalFocus } from "../lib/useModalFocus";

const ICON: Partial<Record<ModTag, Icon>> = {
  role: UsersThree,
  "all-client": UsersThree,
  "host-only": UsersThree,
  map: MapTrifold,
  cosmetic: Sparkle,
  library: PuzzlePiece,
  loader: GearSix,
};

function iconBg(tag: ModTag | null): string {
  if (tag === "map") return "linear-gradient(135deg,#5be3b0,#28b8d0)";
  if (tag === "library" || tag === "loader") return "rgba(255,255,255,0.12)";
  if (tag === "cosmetic") return "linear-gradient(135deg,#7aa2ff,#5bc0ff)";
  return "linear-gradient(135deg,#9b7bff,#7a5bff)";
}

interface ModRowProps {
  mod: ProfileMod;
  busy?: boolean;
  onToggle: () => void;
  onRemove: () => Promise<void>;
  onPickRelease: () => void;
  trust?: Trust;
}

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  try {
    const value = String(error);
    return value && value !== "[object Object]" ? value : "The mod could not be removed.";
  } catch {
    return "The mod could not be removed.";
  }
}

export function ModRow({ mod, busy = false, trust, onToggle, onRemove, onPickRelease }: ModRowProps) {
  const reduce = useReducedMotion();
  const tag = mod.tags.length ? primaryTag(mod.tags) : null;
  const Glyph = (tag && ICON[tag]) || PuzzlePiece;
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [removeError, setRemoveError] = useState<string | null>(null);
  const removeInFlight = useRef(false);
  const confirmRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const descriptionId = useId();
  const unavailable = busy || removing;

  const closeConfirm = useCallback(() => {
    if (removeInFlight.current) return;
    setConfirmOpen(false);
    setRemoveError(null);
  }, []);
  useModalFocus(confirmOpen, confirmRef, closeConfirm);

  const requestRemoval = () => {
    if (busy || mod.managed || removeInFlight.current) return;
    setRemoveError(null);
    setConfirmOpen(true);
  };

  const confirmRemoval = async () => {
    if (busy || mod.managed || removeInFlight.current) return;
    removeInFlight.current = true;
    setRemoving(true);
    setRemoveError(null);
    try {
      await onRemove();
      setConfirmOpen(false);
    } catch (error) {
      setRemoveError(errorMessage(error));
    } finally {
      removeInFlight.current = false;
      setRemoving(false);
    }
  };

  return (
    <>
      <motion.div
        layout={!reduce}
        initial={reduce ? false : { opacity: 0, y: 10 }}
        animate={{ opacity: mod.managed ? 0.72 : 1, y: 0 }}
        transition={{ duration: 0.28, ease: [0.16, 1, 0.3, 1] }}
        aria-busy={unavailable}
        className="glass flex min-w-0 flex-wrap items-center gap-3.5 rounded-2xl px-3.5 py-3"
      >
        <span
          className="grid h-9 w-9 shrink-0 place-items-center rounded-[11px] text-[#0d0820]"
          style={{ background: iconBg(tag) }}
          aria-hidden="true"
        >
          <Glyph size={18} weight="bold" />
        </span>

        <div className="min-w-[10rem] flex-1">
          <div className="truncate text-[15px] font-semibold text-ink" title={mod.name}>
            {mod.name}
          </div>
          {mod.managed ? (
            <div className="truncate text-[12px] text-ink-faint" title={mod.file || undefined}>
              {mod.tags.includes("loader") ? "Loader" : "Dependency"} · automatically managed
              {mod.file ? ` · ${mod.file}` : ""}
            </div>
          ) : (
            <div className="truncate text-[12px] text-ink-faint" title={mod.file || mod.repo}>
              {mod.file ? <span className="font-mono text-ink-dim">{mod.file}</span> : mod.repo}
            </div>
          )}
        </div>

        <div className="ml-auto flex min-w-0 max-w-full flex-wrap items-center justify-end gap-2">
          {tag && <Pill tag={tag} />}
          {trust && <TrustBadge trust={trust} compact />}

          {mod.update && !mod.managed && (
            <span
              className="flex min-w-0 max-w-56 items-center gap-1 rounded-lg border border-[rgba(255,210,63,0.35)] bg-[rgba(255,210,63,0.16)] px-2 py-1 text-[11.5px] font-medium text-[#ffe49a]"
              aria-label={`Update available for ${mod.name}: ${mod.update}`}
              title={mod.update}
            >
              <ArrowUp size={11} weight="bold" className="shrink-0" aria-hidden="true" />
              <span className="truncate">{mod.update}</span>
            </span>
          )}

          {busy && !mod.managed && (
            <span
              role="status"
              aria-live="polite"
              className="glass-2 flex shrink-0 items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[12px] text-ink-dim"
            >
              <CircleNotch size={13} className="animate-spin" aria-hidden="true" />
              Working
              <span className="sr-only">; actions for {mod.name} are unavailable</span>
            </span>
          )}

          {mod.managed ? (
            <>
              <span
                className="glass-2 max-w-40 truncate rounded-lg px-2.5 py-1.5 font-mono text-[12.5px] text-ink-faint"
                title={mod.version}
                aria-label={`Installed version ${mod.version}`}
              >
                {mod.version}
              </span>
              <span className="sr-only">Enable, version, and remove controls are unavailable because this mod is automatically managed.</span>
            </>
          ) : (
            <>
              <button
                type="button"
                onClick={() => {
                  if (!unavailable) onPickRelease();
                }}
                disabled={unavailable}
                aria-label={`Choose version and file for ${mod.name}; current version ${mod.version}`}
                title={`Choose version / file (current: ${mod.version})`}
                className="ring-focus glass-2 flex min-w-0 max-w-48 items-center gap-1.5 rounded-lg px-2.5 py-1.5 font-mono text-[12.5px] text-ink-dim transition-colors hover:text-ink disabled:cursor-not-allowed disabled:opacity-50"
              >
                <span className="truncate">{mod.version}</span>
                <CaretDown size={12} weight="bold" className="shrink-0 opacity-70" aria-hidden="true" />
              </button>

              <Toggle
                on={mod.enabled}
                onChange={() => {
                  if (!unavailable) onToggle();
                }}
                disabled={unavailable}
                label={`Enable ${mod.name}`}
              />

              <button
                type="button"
                onClick={requestRemoval}
                disabled={unavailable}
                aria-label={`Remove ${mod.name}`}
                className="ring-focus grid h-8 w-8 shrink-0 place-items-center rounded-lg text-ink-faint transition-colors hover:bg-white/10 hover:text-[#ff8a8a] disabled:cursor-not-allowed disabled:opacity-50"
              >
                <TrashSimple size={16} aria-hidden="true" />
              </button>
            </>
          )}
        </div>
      </motion.div>

      {confirmOpen &&
        createPortal(
          <div className="fixed inset-0 z-[70] grid place-items-center p-4">
            <button
              type="button"
              className="absolute inset-0 cursor-default bg-[rgba(6,4,18,0.62)]"
              onClick={closeConfirm}
              disabled={removing}
              aria-label="Cancel mod removal"
              tabIndex={-1}
            />
            <div
              ref={confirmRef}
              role="alertdialog"
              aria-modal="true"
              aria-labelledby={titleId}
              aria-describedby={descriptionId}
              aria-busy={removing}
              tabIndex={-1}
              className="glass-strong relative max-h-[calc(100dvh-2rem)] w-full max-w-md overflow-y-auto rounded-2xl p-6 text-left"
            >
              <h2 id={titleId} className="text-[18px] font-semibold text-ink">
                Remove this mod?
              </h2>
              <p id={descriptionId} className="mt-2 break-words text-[13px] text-ink-dim [overflow-wrap:anywhere]">
                Remove <strong className="font-semibold text-ink">{mod.name}</strong> from this profile? You can add it again later.
              </p>
              {removeError && (
                <p role="alert" className="mt-3 break-words text-[12.5px] text-[#ff9b9b] [overflow-wrap:anywhere]">
                  {removeError}
                </p>
              )}
              <div className="mt-5 flex flex-wrap justify-end gap-2">
                <button
                  type="button"
                  data-autofocus
                  onClick={closeConfirm}
                  disabled={removing}
                  className="ring-focus glass-2 rounded-xl px-4 py-2 text-[13px] font-semibold text-ink disabled:opacity-50"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={() => void confirmRemoval()}
                  disabled={removing || busy}
                  className="ring-focus rounded-xl bg-[rgba(255,100,100,0.2)] px-4 py-2 text-[13px] font-semibold text-[#ffb0b0] hover:bg-[rgba(255,100,100,0.28)] disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {removing ? "Removing…" : "Remove mod"}
                </button>
              </div>
            </div>
          </div>,
          document.body,
        )}
    </>
  );
}
