import { motion, useReducedMotion } from "motion/react";
import { Play, Stop, Wrench } from "@phosphor-icons/react";

interface LaunchBarProps {
  profileName: string;
  running: boolean;
  busy: boolean;
  onLaunch: () => void;
  onStop: () => void;
  onSetup: () => void;
}

export function LaunchBar({ profileName, running, busy, onLaunch, onStop, onSetup }: LaunchBarProps) {
  const reduce = useReducedMotion();
  const setupBlocked = busy || running;

  return (
    <div className="glass-2 flex min-w-0 items-center gap-3 px-5 py-3.5 max-[720px]:sticky max-[720px]:bottom-0 max-[720px]:z-20 max-[720px]:flex-wrap max-[720px]:px-3" aria-busy={busy}>
      <div
        className="flex min-w-0 flex-1 items-center gap-2 text-[13px] text-ink-dim"
        role="status"
        aria-live="polite"
      >
        <span
          aria-hidden="true"
          className="h-2 w-2 shrink-0 rounded-full"
          style={{
            background: running ? "#ffd23f" : busy ? "#9b7bff" : "#5be3b0",
          }}
        />
        <span className="truncate">
          {running
            ? "Among Us is running · stop it before making changes"
            : busy
              ? "Operation in progress · please wait"
              : "Among Us not running · ready"}
        </span>
      </div>

      <button
        type="button"
        onClick={onSetup}
        disabled={setupBlocked}
        aria-label={running ? "Set up mods unavailable while Among Us is running" : busy ? "Setting up unavailable while busy" : "Set up mods"}
        className="ring-focus glass flex shrink-0 items-center gap-2 rounded-xl px-4 py-3 text-[14px] font-semibold text-ink-dim transition-colors hover:text-ink disabled:cursor-not-allowed disabled:opacity-60"
        title="Prepare BepInEx and this profile's mods in its direct instance without launching"
      >
        <Wrench size={16} />
        {running ? "Game running" : busy ? "Please wait…" : "Set up mods"}
      </button>
      <motion.button
        type="button"
        onClick={running ? onStop : onLaunch}
        disabled={busy}
        aria-label={running ? "Stop Among Us" : busy ? "Launch unavailable while busy" : `Launch ${profileName}`}
        whileHover={reduce || busy ? undefined : { y: -2 }}
        whileTap={reduce || busy ? undefined : { scale: 0.98 }}
        className={`ring-focus flex min-h-11 min-w-0 max-w-[50%] shrink-0 items-center gap-2 rounded-xl px-7 py-3 text-[15px] font-bold disabled:cursor-not-allowed disabled:opacity-60 max-[720px]:max-w-none max-[720px]:flex-1 max-[720px]:justify-center max-[720px]:px-4 ${
          running
            ? "border border-[rgba(255,138,138,0.4)] bg-[rgba(226,59,59,0.16)] text-[#ffb0b0]"
            : "accent-grad text-[#0d0820]"
        }`}
        style={{ boxShadow: running ? "0 8px 22px rgba(226,59,59,0.15)" : "0 8px 22px rgba(91,192,255,0.24)" }}
      >
        {running
          ? <Stop size={17} weight="fill" className="shrink-0" />
          : <Play size={17} weight="fill" className="shrink-0" />}
        <span className="truncate max-[720px]:hidden">
          {running ? "Stop Among Us" : busy ? "Please wait…" : `Launch ${profileName}`}
        </span>
        <span className="hidden max-[720px]:inline">
          {running ? "Stop" : busy ? "Wait…" : "Launch"}
        </span>
      </motion.button>
    </div>
  );
}
