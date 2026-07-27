import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Check, CircleNotch, Clock, DownloadSimple, ShieldCheck } from "@phosphor-icons/react";
import type { OperationProgress } from "../lib/types";
import { useModalFocus } from "../lib/useModalFocus";

export type OperationScope = "lobby" | "mods" | "release" | "maps" | "setup";

export interface OperationActivity extends OperationProgress {
  id: number;
  scope: OperationScope;
  title: string;
  startedAt: number;
}

interface OperationProgressModalProps {
  activity: OperationActivity | null;
}

const PHASES: OperationProgress["phase"][] = ["preparing", "resolving", "downloading", "finalizing"];
const PHASE_LABELS: Record<OperationProgress["phase"], string> = {
  preparing: "Prepare",
  resolving: "Resolve",
  downloading: "Download",
  finalizing: "Finish",
};

function formatBytes(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.max(0, Math.round(bytes / 1024))} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatElapsed(milliseconds: number): string {
  const seconds = Math.max(0, Math.floor(milliseconds / 1000));
  const minutes = Math.floor(seconds / 60);
  return minutes > 0 ? `${minutes}:${String(seconds % 60).padStart(2, "0")}` : `${seconds}s`;
}

export function OperationProgressModal({ activity }: OperationProgressModalProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLElement>(null);
  const [now, setNow] = useState(Date.now());
  const keepOpen = useCallback(() => {}, []);
  useModalFocus(activity !== null, modalRef, keepOpen);

  useEffect(() => {
    if (!activity) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [activity?.id]);

  const phaseIndex = activity ? PHASES.indexOf(activity.phase) : 0;
  const bytesReceived = activity?.bytesReceived ?? 0;
  const bytesTotal = activity?.bytesTotal ?? 0;
  const hasByteTotal = bytesTotal > 0;
  const percentage = hasByteTotal
    ? bytesReceived >= bytesTotal
      ? 100
      : Math.max(0, Math.floor((bytesReceived / bytesTotal) * 100))
    : null;

  return (
    <AnimatePresence>
      {activity && (
        <motion.div
          data-modal-exempt
          className="fixed inset-0 z-[90] grid place-items-center p-4 sm:p-6"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: reduce ? 0.01 : 0.2 }}
        >
          <div
            aria-hidden="true"
            className="absolute inset-0 bg-[rgba(6,4,18,0.64)]"
            style={{ backdropFilter: "blur(5px) saturate(115%)" }}
          />
          <motion.section
            ref={modalRef}
            role="dialog"
            aria-modal="true"
            aria-labelledby="operation-progress-title"
            aria-describedby="operation-progress-message operation-progress-safety"
            tabIndex={-1}
            initial={reduce ? { opacity: 0 } : { opacity: 0.65, y: 18, scale: 0.97, filter: "blur(8px)" }}
            animate={{ opacity: 1, y: 0, scale: 1, filter: "blur(0px)" }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, y: 10, scale: 0.985, filter: "blur(5px)" }}
            transition={{ duration: reduce ? 0.01 : 0.34, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative w-full max-w-[520px] overflow-hidden rounded-3xl px-5 py-5 sm:px-7 sm:py-6"
          >
            <div aria-hidden="true" className="accent-grad absolute inset-x-0 top-0 h-px opacity-90" />

            <header className="flex min-w-0 items-start gap-4">
              <div className="relative grid h-12 w-12 shrink-0 place-items-center rounded-2xl border border-[rgba(139,191,255,0.34)] bg-[rgba(90,137,255,0.14)] text-[#a9d8ff] shadow-[0_10px_30px_rgba(48,102,212,0.2)]">
                {activity.phase === "downloading" ? (
                  <DownloadSimple size={23} weight="bold" aria-hidden="true" />
                ) : (
                  <CircleNotch size={23} weight="bold" className={reduce ? "" : "animate-spin"} aria-hidden="true" />
                )}
                <span className={`absolute -top-1 -right-1 h-2.5 w-2.5 rounded-full bg-[#5be3b0] ring-4 ring-[#17102d] ${reduce ? "" : "animate-pulse"}`} aria-hidden="true" />
              </div>

              <div className="min-w-0 flex-1">
                <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-1">
                  <span className="flex items-center gap-1.5 text-[11px] font-bold tracking-[0.12em] text-[#83efc7] uppercase">
                    <span className="h-1.5 w-1.5 rounded-full bg-current" aria-hidden="true" /> Live progress
                  </span>
                  <span className="flex shrink-0 items-center gap-1.5 font-mono text-[11.5px] text-ink-faint" aria-label={`${formatElapsed(now - activity.startedAt)} elapsed`}>
                    <Clock size={13} aria-hidden="true" /> {formatElapsed(now - activity.startedAt)}
                  </span>
                </div>
                <h2 id="operation-progress-title" className="mt-1.5 text-[22px] leading-tight font-semibold tracking-[-0.015em] text-ink sm:text-[24px]">
                  {activity.title}
                </h2>
              </div>
            </header>

            <div className="mt-6 min-w-0">
              <p className="text-[12px] font-semibold text-[#b9dfff]">{PHASE_LABELS[activity.phase]} in progress</p>
              <p id="operation-progress-message" aria-live="polite" aria-atomic="true" className="mt-1 min-h-6 break-words text-[15px] leading-6 text-ink-dim [overflow-wrap:anywhere]">
                {activity.message}
              </p>

              <div
                className="relative mt-4 h-2 overflow-hidden rounded-full border border-white/10 bg-[rgba(5,8,28,0.58)]"
                role="progressbar"
                aria-label={activity.message}
                aria-valuemin={0}
                aria-valuemax={hasByteTotal ? 100 : undefined}
                aria-valuenow={percentage ?? undefined}
                aria-valuetext={percentage === null ? `${PHASE_LABELS[activity.phase]} in progress` : `${percentage}% downloaded`}
              >
                {percentage === null ? (
                  <motion.div
                    aria-hidden="true"
                    className="accent-grad absolute inset-y-0 w-[42%] rounded-full shadow-[0_3px_16px_rgba(91,192,255,0.38)]"
                    initial={{ x: "-110%" }}
                    animate={reduce ? { x: "70%" } : { x: ["-110%", "245%"] }}
                    transition={reduce ? { duration: 0.01 } : { duration: 1.35, repeat: Infinity, ease: [0.65, 0, 0.35, 1] }}
                  />
                ) : (
                  <div
                    aria-hidden="true"
                    className="accent-grad h-full rounded-full shadow-[0_3px_16px_rgba(91,192,255,0.38)]"
                    style={{ width: `${percentage}%` }}
                  />
                )}
              </div>

              <div className="mt-2 flex min-h-5 items-center justify-between gap-4 font-mono text-[11.5px] text-ink-faint">
                {activity.bytesReceived !== undefined ? (
                  <span>
                    {formatBytes(activity.bytesReceived)}{activity.bytesTotal ? ` / ${formatBytes(activity.bytesTotal)}` : " received"}
                  </span>
                ) : (
                  <span>Working transactionally</span>
                )}
                <span>{percentage === null ? "Still working" : `${percentage}%`}</span>
              </div>
            </div>

            <ol className="mt-6 grid grid-cols-4" aria-label="Installation stages">
              {PHASES.map((phase, index) => {
                const complete = index < phaseIndex;
                const current = index === phaseIndex;
                return (
                  <li key={phase} className="relative min-w-0 text-center">
                    {index > 0 && (
                      <span aria-hidden="true" className={`absolute top-[6px] right-1/2 h-px w-full ${index <= phaseIndex ? "bg-[#7aa2ff]" : "bg-white/14"}`} />
                    )}
                    <span
                      aria-hidden="true"
                      className={`relative mx-auto grid h-3.5 w-3.5 place-items-center rounded-full border ${complete ? "border-[#5be3b0] bg-[#5be3b0] text-[#061c16]" : current ? "border-[#8fcaff] bg-[#233d78] shadow-[0_3px_12px_rgba(91,192,255,0.35)]" : "border-white/20 bg-[#17112d]"}`}
                    >
                      {complete && <Check size={9} weight="bold" />}
                    </span>
                    <span className={`mt-2 block truncate px-1 text-[10.5px] ${current ? "font-semibold text-ink" : complete ? "text-ink-dim" : "text-ink-faint"}`}>
                      {PHASE_LABELS[phase]}
                    </span>
                  </li>
                );
              })}
            </ol>

            <p id="operation-progress-safety" className="mt-6 flex items-start gap-2 border-t border-white/10 pt-4 text-[12.5px] leading-5 text-ink-faint">
              <ShieldCheck size={16} weight="fill" className="mt-0.5 shrink-0 text-[#79e6bd]" aria-hidden="true" />
              Changes stay staged until every download is verified. Perfect Sync replaces live files only when the whole operation is ready.
            </p>
          </motion.section>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
