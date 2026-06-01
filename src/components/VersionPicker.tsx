import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { CaretDown, Check } from "@phosphor-icons/react";

interface VersionPickerProps {
  value: string;
  options: string[];
  onChange: (v: string) => void;
  disabled?: boolean;
}

/** Upgrade/downgrade control. Custom menu so it matches the glass surface. */
export function VersionPicker({ value, options, onChange, disabled }: VersionPickerProps) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const reduce = useReducedMotion();

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className="relative" ref={ref}>
      <button
        type="button"
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={`Version, currently ${value}`}
        className="ring-focus glass-2 flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 font-mono text-[12.5px] text-ink-dim transition-colors hover:text-ink disabled:opacity-50"
      >
        {value}
        <CaretDown size={12} weight="bold" className="opacity-70" />
      </button>

      <AnimatePresence>
        {open && (
          <motion.ul
            role="listbox"
            initial={reduce ? false : { opacity: 0, y: -6, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, y: -6, scale: 0.98 }}
            transition={{ duration: 0.14, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong absolute right-0 z-30 mt-2 min-w-[150px] origin-top-right rounded-xl p-1.5"
          >
            {options.map((opt) => (
              <li key={opt}>
                <button
                  type="button"
                  role="option"
                  aria-selected={opt === value}
                  onClick={() => {
                    onChange(opt);
                    setOpen(false);
                  }}
                  className="ring-focus flex w-full items-center justify-between gap-3 rounded-lg px-2.5 py-1.5 text-left font-mono text-[12.5px] text-ink-dim hover:bg-white/10 hover:text-ink"
                >
                  {opt}
                  {opt === value && <Check size={13} weight="bold" className="text-[#5bc0ff]" />}
                </button>
              </li>
            ))}
          </motion.ul>
        )}
      </AnimatePresence>
    </div>
  );
}
