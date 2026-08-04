import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Warning } from "@phosphor-icons/react";
import { useModalFocus } from "../lib/useModalFocus";

interface LaunchWarningProps {
  open: boolean;
  onInstall: () => void;
  onLaunchAnyway: (dontWarnAgain: boolean) => void;
  onCancel: () => void;
}

/** Shown before launch when BepInEx isn't fully installed. */
export function LaunchWarning({ open, onInstall, onLaunchAnyway, onCancel }: LaunchWarningProps) {
  const reduce = useReducedMotion();
  const [dontWarn, setDontWarn] = useState(false);
  const dialogRef = useRef<HTMLDivElement>(null);
  const wasOpenRef = useRef(false);
  const cancelRef = useRef(onCancel);
  cancelRef.current = onCancel;

  const close = useCallback(() => cancelRef.current(), []);
  useModalFocus(open, dialogRef, close);

  useEffect(() => {
    if (open && !wasOpenRef.current) setDontWarn(false);
    wasOpenRef.current = open;
  }, [open]);
  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-[58] isolate grid place-items-center bg-[rgba(6,4,18,0.68)] p-6"
          style={{ backdropFilter: "blur(2px)" }}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18 }}
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) close();
          }}
        >

          <motion.div
            ref={dialogRef}
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="launch-warning-title"
            aria-describedby="launch-warning-description"
            tabIndex={-1}
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex w-[440px] max-w-full flex-col rounded-3xl p-6"
          >
            <div className="flex items-center gap-2.5">
              <Warning size={20} weight="fill" className="text-[#ffe49a]" />
              <h2 id="launch-warning-title" className="text-[18px] font-semibold text-ink">
                Direct profile instance isn't ready
              </h2>
            </div>
            <p id="launch-warning-description" className="mt-2 text-[13px] leading-relaxed text-ink-dim">
              BepInEx and this profile's mods have not been verified in its direct instance. You can prepare it,
              or launch a direct vanilla instance without changing the original game source.
            </p>

            <label className="mt-4 flex cursor-pointer items-center gap-2 text-[12.5px] text-ink-dim">
              <input
                type="checkbox"
                checked={dontWarn}
                onChange={(e) => setDontWarn(e.target.checked)}
                className="h-4 w-4 accent-[#9b7bff]"
              />
              Don't warn before launching vanilla again
            </label>

            <div className="mt-5 flex justify-end gap-2.5">
              <button
                type="button"
                data-autofocus
                onClick={close}
                className="ring-focus glass rounded-xl px-4 py-2.5 text-[13.5px] text-ink"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => onLaunchAnyway(dontWarn)}
                className="ring-focus glass rounded-xl px-4 py-2.5 text-[13.5px] text-ink"
              >
                Launch vanilla
              </button>
              <button
                type="button"
                onClick={onInstall}
                className="ring-focus accent-grad rounded-xl px-4 py-2.5 text-[13.5px] font-bold text-[#0d0820]"
              >
                Prepare instance
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
