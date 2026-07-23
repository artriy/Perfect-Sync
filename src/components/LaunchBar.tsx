import { motion, useReducedMotion } from "motion/react";
import { Play, Wrench } from "@phosphor-icons/react";

interface LaunchBarProps {
  profileName: string;
  running: boolean;
  busy: boolean;
  onLaunch: () => void;
  onSetup: () => void;
}

export function LaunchBar({ profileName, running, busy, onLaunch, onSetup }: LaunchBarProps) {
  const reduce = useReducedMotion();
  const blocked = busy || running;

  return (
    <div className="glass-2 flex min-w-0 items-center gap-3 px-5 py-3.5" aria-busy={blocked}>
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
            boxShadow: running ? "0 0 8px #ffd23f" : busy ? "0 0 8px #9b7bff" : "0 0 8px #5be3b0",
          }}
        />
        <span className="truncate">
          {running
            ? "Among Us is running · close it to make changes"
            : busy
              ? "Operation in progress · please wait"
              : "Among Us not running · ready"}
        </span>
      </div>

      <button
        type="button"
        onClick={onSetup}
        disabled={blocked}
        aria-label={running ? "Set up mods unavailable while Among Us is running" : busy ? "Setting up unavailable while busy" : "Set up mods"}
        className="ring-focus glass flex shrink-0 items-center gap-2 rounded-xl px-4 py-3 text-[14px] font-semibold text-ink-dim transition-colors hover:text-ink disabled:cursor-not-allowed disabled:opacity-60"
        title="Install BepInEx + this profile's mods into your Among Us folder without launching"
      >
        <Wrench size={16} />
        {running ? "Game running" : busy ? "Please wait…" : "Set up mods"}
      </button>
      <motion.button
        type="button"
        onClick={onLaunch}
        disabled={blocked}
        aria-label={running ? "Among Us is already running" : busy ? "Launch unavailable while busy" : `Launch ${profileName}`}
        whileHover={reduce || blocked ? undefined : { y: -2 }}
        whileTap={reduce || blocked ? undefined : { scale: 0.98 }}
        className="ring-focus accent-grad flex min-w-0 max-w-[50%] shrink-0 items-center gap-2 rounded-xl px-7 py-3 text-[15px] font-bold text-[#0d0820] disabled:cursor-not-allowed disabled:opacity-60"
        style={{ boxShadow: "0 8px 26px rgba(123,150,255,0.5)" }}
      >
        <Play size={17} weight="fill" className="shrink-0" />
        <span className="truncate">
          {running ? "Already running" : busy ? "Please wait…" : `Launch ${profileName}`}
        </span>
      </motion.button>
    </div>
  );
}
