import { useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Warning } from "@phosphor-icons/react";
import type { MainMod } from "../lib/mainMods";
import { useModalFocus } from "../lib/useModalFocus";

interface MainModWarningProps {
  mods: readonly MainMod[];
  actionLabel: string;
  onCancel: () => void;
  onConfirm: () => void;
}

export function MainModWarning({ mods, actionLabel, onCancel, onConfirm }: MainModWarningProps) {
  const reduce = useReducedMotion();
  const dialogRef = useRef<HTMLDivElement>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  useModalFocus(true, dialogRef, onCancel);

  return (
    <AnimatePresence>
      <motion.div
        className="fixed inset-0 z-[80] grid place-items-center p-4 sm:p-6"
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0 }}
        transition={{ duration: 0.16 }}
      >
        <div
          className="absolute inset-0 bg-[rgba(6,4,18,0.76)]"
          style={{ backdropFilter: "blur(3px)" }}
          onMouseDown={(event) => event.target === event.currentTarget && onCancel()}
        />
        <motion.div
          ref={dialogRef}
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="main-mod-warning-title"
          aria-describedby="main-mod-warning-description"
          tabIndex={-1}
          initial={reduce ? false : { opacity: 0, scale: 0.97, y: 10 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.98, y: 6 }}
          transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
          className="glass-strong relative w-[500px] max-w-full rounded-3xl p-5 sm:p-6"
        >
          <div className="flex items-start gap-3">
            <span className="grid h-10 w-10 shrink-0 place-items-center rounded-xl bg-[rgba(255,193,92,0.14)] text-[#ffd9a8]">
              <Warning size={22} weight="fill" aria-hidden="true" />
            </span>
            <div className="min-w-0">
              <h2 id="main-mod-warning-title" className="text-[19px] font-semibold text-ink">
                These main mods may conflict
              </h2>
              <p id="main-mod-warning-description" className="mt-1 text-[13px] leading-relaxed text-ink-dim">
                Main mods replace many of the same game systems and are not designed to run together. This combination may prevent Among Us from starting or cause broken lobbies.
              </p>
            </div>
          </div>

          <ul className="mt-4 overflow-hidden rounded-2xl border border-[rgba(255,193,92,0.24)] bg-[rgba(255,193,92,0.07)]" aria-label="Conflicting main mods">
            {mods.map((mod, index) => (
              <li key={mod.id} className={`flex min-w-0 items-center gap-2.5 px-3.5 py-2.5 text-[13.5px] font-semibold text-ink ${index ? "border-t border-white/8" : ""}`}>
                <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-[#ffd166]" aria-hidden="true" />
                <span className="min-w-0 break-words">{mod.name}</span>
              </li>
            ))}
          </ul>

          <label className="ring-focus mt-4 flex cursor-pointer items-start gap-3 rounded-xl px-1 py-1 text-[12.5px] leading-relaxed text-ink-dim">
            <input
              data-autofocus
              type="checkbox"
              checked={acknowledged}
              onChange={(event) => setAcknowledged(event.target.checked)}
              className="mt-0.5 h-4 w-4 shrink-0 accent-[#ffd166]"
            />
            <span>I understand these main mods are incompatible and want to continue anyway.</span>
          </label>

          <div className="mt-5 flex flex-wrap justify-end gap-2.5 border-t border-white/10 pt-4">
            <button type="button" onClick={onCancel} className="ring-focus glass rounded-xl px-4 py-2.5 text-[13.5px] text-ink">
              Cancel
            </button>
            <button
              type="button"
              disabled={!acknowledged}
              onClick={onConfirm}
              className="ring-focus rounded-xl bg-[#ffd166] px-4 py-2.5 text-[13.5px] font-bold text-[#211707] transition-colors hover:bg-[#ffe09a] disabled:cursor-not-allowed disabled:opacity-40"
            >
              {actionLabel}
            </button>
          </div>
        </motion.div>
      </motion.div>
    </AnimatePresence>
  );
}
