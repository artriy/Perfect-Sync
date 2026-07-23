import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { CheckCircle, WarningCircle, X } from "@phosphor-icons/react";

export interface ToastState {
  id: number;
  msg: string;
  kind?: "success" | "error";
}

export function Toast({ toast, onDismiss }: { toast: ToastState | null; onDismiss: () => void }) {
  const reduce = useReducedMotion();

  return (
    <div data-modal-exempt className="pointer-events-none fixed inset-x-0 bottom-4 z-[60] flex justify-center px-4 sm:bottom-6">
      <AnimatePresence>
        {toast && (
          <motion.div
            key={toast.id}
            role={toast.kind === "error" ? "alert" : "status"}
            aria-live={toast.kind === "error" ? "assertive" : "polite"}
            aria-atomic="true"
            initial={reduce ? { opacity: 0 } : { opacity: 0, y: 16 }}
            animate={reduce ? { opacity: 1 } : { opacity: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, y: 10 }}
            transition={{ duration: reduce ? 0.01 : 0.22, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong pointer-events-auto flex max-h-[calc(100dvh-2rem)] w-full max-w-xl min-w-0 items-start gap-2 overflow-y-auto rounded-xl px-4 py-3 text-[13.5px] text-ink"
          >
            {toast.kind === "error" ? (
              <WarningCircle size={17} weight="fill" className="mt-0.5 shrink-0 text-[#ff8a8a]" aria-hidden="true" />
            ) : (
              <CheckCircle size={17} weight="fill" className="mt-0.5 shrink-0 text-[#5be3b0]" aria-hidden="true" />
            )}
            <span className="min-w-0 flex-1 break-words [overflow-wrap:anywhere]">{toast.msg}</span>
            <button
              type="button"
              onClick={onDismiss}
              aria-label={`Dismiss ${toast.kind === "error" ? "error" : "notification"}`}
              className="ring-focus -m-1 grid h-7 w-7 shrink-0 place-items-center rounded-lg text-ink-faint transition-colors hover:bg-white/10 hover:text-ink"
            >
              <X size={15} weight="bold" aria-hidden="true" />
            </button>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
