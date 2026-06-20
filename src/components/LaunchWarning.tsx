import { useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Warning } from "@phosphor-icons/react";

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

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-[58] grid place-items-center p-6"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18 }}
        >
          <div className="absolute inset-0 bg-[rgba(6,4,18,0.55)]" style={{ backdropFilter: "blur(2px)" }} onClick={onCancel} />

          <motion.div
            role="dialog"
            aria-modal="true"
            aria-label="BepInEx not set up"
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex w-[440px] max-w-full flex-col rounded-3xl p-6"
          >
            <div className="flex items-center gap-2.5">
              <Warning size={20} weight="fill" className="text-[#ffe49a]" />
              <h2 className="text-[18px] font-semibold text-ink">BepInEx isn't set up</h2>
            </div>
            <p className="mt-2 text-[13px] text-ink-dim">
              The mod loader isn't fully installed for this game. Launch now and Among Us starts vanilla
              with no mods loaded.
            </p>

            <label className="mt-4 flex cursor-pointer items-center gap-2 text-[12.5px] text-ink-dim">
              <input
                type="checkbox"
                checked={dontWarn}
                onChange={(e) => setDontWarn(e.target.checked)}
                className="h-4 w-4 accent-[#9b7bff]"
              />
              Don't warn me again
            </label>

            <div className="mt-5 flex justify-end gap-2.5">
              <button
                type="button"
                onClick={onCancel}
                className="ring-focus glass rounded-xl px-4 py-2.5 text-[13.5px] text-ink"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => onLaunchAnyway(dontWarn)}
                className="ring-focus glass rounded-xl px-4 py-2.5 text-[13.5px] text-ink"
              >
                Launch anyway
              </button>
              <button
                type="button"
                onClick={onInstall}
                className="ring-focus accent-grad rounded-xl px-4 py-2.5 text-[13.5px] font-bold text-[#0d0820]"
              >
                Set up BepInEx
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
