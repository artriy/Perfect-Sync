import { motion, useReducedMotion } from "motion/react";

interface ToggleProps {
  on: boolean;
  onChange: () => void;
  disabled?: boolean;
  label: string;
}

/** Accessible switch. The knob travels on a spring (state-transition feedback). */
export function Toggle({ on, onChange, disabled, label }: ToggleProps) {
  const reduce = useReducedMotion();
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      disabled={disabled}
      onClick={onChange}
      className="ring-focus relative h-[22px] w-[40px] shrink-0 rounded-full transition-colors disabled:opacity-50"
      style={{
        background: on
          ? "linear-gradient(90deg,#9b7bff,#5bc0ff)"
          : "rgba(255,255,255,0.16)",
      }}
    >
      <motion.span
        className="absolute top-[3px] left-[3px] h-4 w-4 rounded-full bg-white shadow"
        animate={{ x: on ? 18 : 0 }}
        transition={reduce ? { duration: 0 } : { type: "spring", stiffness: 520, damping: 32 }}
      />
    </button>
  );
}
