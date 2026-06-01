import { AnimatePresence, motion } from "motion/react";
import { CheckCircle } from "@phosphor-icons/react";

export interface ToastState {
  id: number;
  msg: string;
}

export function Toast({ toast }: { toast: ToastState | null }) {
  return (
    <AnimatePresence>
      {toast && (
        <motion.div
          key={toast.id}
          initial={{ opacity: 0, y: 16 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: 10 }}
          transition={{ duration: 0.22, ease: [0.16, 1, 0.3, 1] }}
          className="glass-strong fixed bottom-6 left-1/2 z-[60] flex -translate-x-1/2 items-center gap-2 rounded-xl px-4 py-3 text-[13.5px] text-ink"
        >
          <CheckCircle size={17} weight="fill" className="text-[#5be3b0]" />
          {toast.msg}
        </motion.div>
      )}
    </AnimatePresence>
  );
}
