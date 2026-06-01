import { motion, useReducedMotion } from "motion/react";
import { Play } from "@phosphor-icons/react";

interface LaunchBarProps {
  profileName: string;
  running: boolean;
  busy?: boolean;
  onLaunch: () => void;
}

export function LaunchBar({ profileName, running, busy, onLaunch }: LaunchBarProps) {
  const reduce = useReducedMotion();
  return (
    <div className="glass-2 flex items-center gap-4 px-5 py-3.5">
      <div className="flex items-center gap-2 text-[13px] text-ink-dim">
        <span
          className="h-2 w-2 rounded-full"
          style={{
            background: running ? "#ffd23f" : "#5be3b0",
            boxShadow: running ? "0 0 8px #ffd23f" : "0 0 8px #5be3b0",
          }}
        />
        {running ? "Among Us is running · close it to make changes" : "Among Us not running · ready"}
      </div>

      <div className="flex-1" />

      <motion.button
        type="button"
        onClick={onLaunch}
        disabled={busy}
        whileHover={reduce || busy ? undefined : { y: -2 }}
        whileTap={reduce || busy ? undefined : { scale: 0.98 }}
        className="ring-focus accent-grad flex items-center gap-2 rounded-xl px-7 py-3 text-[15px] font-bold text-[#0d0820] disabled:opacity-60"
        style={{ boxShadow: "0 8px 26px rgba(123,150,255,0.5)" }}
      >
        <Play size={17} weight="fill" /> Launch {profileName}
      </motion.button>
    </div>
  );
}
