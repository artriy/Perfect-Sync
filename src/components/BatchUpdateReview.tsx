import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { ArrowsClockwise, Check, Warning, X } from "@phosphor-icons/react";
import type { Profile } from "../lib/types";
import { useModalFocus } from "../lib/useModalFocus";

interface BatchUpdateReviewProps {
  open: boolean;
  profile: Profile;
  busy: boolean;
  onClose: () => void;
  onApply: (packageIds: string[]) => void;
}

export function BatchUpdateReview({ open, profile, busy, onClose, onApply }: BatchUpdateReviewProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const updates = profile.mods.filter((mod) => !mod.managed && mod.update);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);

  useEffect(() => {
    if (open) setSelectedIds(updates.map((mod) => mod.packageId));
  }, [open, profile.id]);

  const requestClose = useCallback(() => {
    if (!busy) onClose();
  }, [busy, onClose]);
  useModalFocus(open, modalRef, requestClose);

  const selected = new Set(selectedIds);
  const toggle = (packageId: string) => {
    if (busy) return;
    setSelectedIds((current) =>
      current.includes(packageId)
        ? current.filter((candidate) => candidate !== packageId)
        : [...current, packageId],
    );
  };

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-[55] grid place-items-center bg-[rgba(6,4,18,0.68)] p-4 sm:p-6"
          initial={reduce ? false : { opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) requestClose();
          }}
        >
          <motion.div
            ref={modalRef}
            role="dialog"
            aria-modal="true"
            aria-labelledby="update-review-title"
            className="glass-strong flex max-h-[88vh] w-[620px] max-w-full flex-col overflow-hidden rounded-3xl"
            initial={reduce ? false : { opacity: 0, y: 12, scale: 0.985 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 8, scale: 0.99 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
          >
            <header className="flex items-start gap-3 border-b border-white/10 px-5 py-4">
              <div className="grid h-10 w-10 shrink-0 place-items-center rounded-xl bg-[#9b7bff]/14 text-accent-2">
                <ArrowsClockwise size={20} weight="bold" />
              </div>
              <div className="min-w-0 flex-1">
                <h2 id="update-review-title" className="text-[18px] font-bold tracking-tight text-ink">
                  Review profile updates
                </h2>
                <p className="mt-0.5 text-[12.5px] text-ink-faint">
                  Select every version change you want to apply to {profile.name}. Nothing updates automatically.
                </p>
              </div>
              <button
                type="button"
                onClick={requestClose}
                disabled={busy}
                aria-label="Close update review"
                className="ring-focus grid h-9 w-9 shrink-0 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink disabled:opacity-50"
              >
                <X size={16} />
              </button>
            </header>

            <div className="scroll-region min-h-0 flex-1 overflow-y-auto p-5">
              <div className="overflow-hidden rounded-2xl border border-white/10 bg-white/[0.025]">
                {updates.map((mod, index) => {
                  const checked = selected.has(mod.packageId);
                  return (
                    <button
                      key={mod.packageId}
                      type="button"
                      role="checkbox"
                      aria-checked={checked}
                      disabled={busy}
                      onClick={() => toggle(mod.packageId)}
                      className={`ring-focus flex w-full items-center gap-3 px-3.5 py-3 text-left transition-colors disabled:opacity-50 ${
                        index ? "border-t border-white/8" : ""
                      } ${checked ? "bg-[#9b7bff]/8" : "hover:bg-white/[0.035]"}`}
                    >
                      <span
                        className={`grid h-5 w-5 shrink-0 place-items-center rounded-md border transition-colors ${
                          checked
                            ? "border-accent bg-accent text-[#100923]"
                            : "border-white/20 bg-white/[0.035] text-transparent"
                        }`}
                      >
                        <Check size={13} weight="bold" />
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-[13px] font-semibold text-ink">{mod.name}</span>
                        <span className="mt-0.5 block truncate text-[11.5px] text-ink-faint">
                          {mod.repo ?? mod.packageId}
                        </span>
                      </span>
                      <span className="flex shrink-0 items-center gap-1.5 font-mono text-[11.5px]">
                        <span className="text-ink-faint">{mod.version}</span>
                        <span aria-hidden="true" className="text-ink-faint">→</span>
                        <span className="font-semibold text-[#d4c6ff]">{mod.update}</span>
                      </span>
                    </button>
                  );
                })}
              </div>
              <div className="mt-3 flex items-start gap-2 rounded-xl bg-[#ffd23f]/8 px-3 py-2.5 text-[11.5px] leading-relaxed text-[#ffe8a3]">
                <Warning size={14} weight="fill" className="mt-0.5 shrink-0" />
                <span>The selected updates and any required dependency changes are committed together. A failure leaves the profile unchanged.</span>
              </div>
            </div>

            <footer className="flex flex-wrap items-center justify-between gap-3 border-t border-white/10 px-5 py-4">
              <p className="text-[12px] text-ink-faint">
                {selectedIds.length} of {updates.length} selected
              </p>
              <div className="flex gap-2.5">
                <button
                  type="button"
                  onClick={requestClose}
                  disabled={busy}
                  className="ring-focus glass rounded-xl px-4 py-2.5 text-[13px] text-ink disabled:opacity-50"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={() => onApply(selectedIds)}
                  disabled={busy || selectedIds.length === 0}
                  className="ring-focus accent-grad flex items-center gap-2 rounded-xl px-4 py-2.5 text-[13px] font-bold text-[#0d0820] disabled:opacity-50"
                >
                  <ArrowsClockwise size={15} className={busy ? "animate-spin" : ""} />
                  {busy ? "Applying updates…" : `Apply ${selectedIds.length} update${selectedIds.length === 1 ? "" : "s"}`}
                </button>
              </div>
            </footer>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
