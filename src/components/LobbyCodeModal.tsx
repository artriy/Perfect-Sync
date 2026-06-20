import { useEffect, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  ArrowUp,
  Check,
  DownloadSimple,
  LinkSimple,
  Play,
  ShieldCheck,
  X,
} from "@phosphor-icons/react";
import { Pill, primaryTag } from "./Pill";
import type { DiffItem, PersonalMod } from "../lib/types";
import { extractLobbyCode, previewCode } from "../lib/bridge";

type Mode = "input" | "decoding" | "diff";

interface LobbyCodeModalProps {
  open: boolean;
  initialCode?: string;
  diff: DiffItem[];
  personalMods: PersonalMod[];
  onClose: () => void;
  onApply: (launch: boolean, code: string) => void;
}

export function LobbyCodeModal({ open, initialCode, diff, personalMods, onClose, onApply }: LobbyCodeModalProps) {
  const reduce = useReducedMotion();
  const [mode, setMode] = useState<Mode>("input");
  const [code, setCode] = useState("");
  const [rows, setRows] = useState<DiffItem[]>(diff);
  const [name, setName] = useState("");

  const runDecode = (value: string) => {
    setMode("decoding");
    // TODO(phase 2): pass the active profile's real installed [id, version] pairs.
    previewCode(value, [
      ["AU-Avengers/TOU-Mira", "1.6.2"],
      ["Dolfannn/LevelImposter", "0.7.2"],
    ])
      .then((p) => {
        setRows(p.items);
        setName(p.name);
        setMode("diff");
      })
      .catch((err) => {
        console.error("lobby code decode failed", err);
        setMode("diff");
      });
  };

  // Reset + auto-decode whenever the modal is (re)opened.
  useEffect(() => {
    if (!open) return;
    setCode(initialCode ?? "");
    if (initialCode) {
      runDecode(initialCode);
      return;
    }
    setMode("input");
  }, [open, initialCode]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  const decode = () => {
    const c = extractLobbyCode(code);
    if (!c) return;
    setCode(c);
    runDecode(c);
  };

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-50 grid place-items-center p-6"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18 }}
        >
          <div
            className="absolute inset-0 bg-[rgba(6,4,18,0.5)]"
            style={{ backdropFilter: "blur(2px)" }}
            onClick={onClose}
          />

          <motion.div
            role="dialog"
            aria-modal="true"
            aria-label="Set up this lobby"
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex max-h-[90vh] w-[560px] max-w-full flex-col rounded-3xl p-6"
          >
            <button
              type="button"
              onClick={onClose}
              aria-label="Close"
              className="ring-focus absolute top-4 right-4 grid h-8 w-8 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink"
            >
              <X size={16} weight="bold" />
            </button>

            <h2 className="text-[20px] font-semibold text-ink">Set up this lobby</h2>
            <p className="mt-0.5 text-[13px] text-ink-dim">
              {mode === "input"
                ? "Paste a friend's PERFECT- code and we'll show exactly what changes."
                : "Decoded from a shared code. Here is precisely what will change."}
            </p>

            <div className="mt-4 flex min-h-0 flex-1 flex-col">
              {mode === "input" ? (
                <InputStep code={code} setCode={setCode} onDecode={decode} />
              ) : (
                <ResultStep
                  mode={mode}
                  diff={rows}
                  personalMods={personalMods}
                  name={name}
                  code={code || initialCode || ""}
                  onApply={(launch) => onApply(launch, code || initialCode || "")}
                />
              )}
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

function InputStep({
  code,
  setCode,
  onDecode,
}: {
  code: string;
  setCode: (v: string) => void;
  onDecode: () => void;
}) {
  const valid = extractLobbyCode(code) != null;
  return (
    <>
      <textarea
        value={code}
        onChange={(e) => setCode(e.target.value)}
        rows={3}
        placeholder="PERFECT-…"
        aria-label="Lobby code"
        className="glass w-full resize-none rounded-xl px-3.5 py-3 font-mono text-[13px] text-ink placeholder:text-ink-faint focus:outline-none"
      />
      <div className="mt-4 flex justify-end">
        <button
          type="button"
          disabled={!valid}
          onClick={onDecode}
          className="ring-focus accent-grad flex items-center gap-2 rounded-xl px-5 py-2.5 text-[14px] font-bold text-[#0d0820] disabled:opacity-50"
        >
          <LinkSimple size={16} weight="bold" /> Decode
        </button>
      </div>
    </>
  );
}

function ResultStep({
  mode,
  diff,
  name,
  code,
  personalMods,
  onApply,
}: {
  mode: Mode;
  diff: DiffItem[];
  name: string;
  code: string;
  personalMods: PersonalMod[];
  onApply: (launch: boolean) => void;
}) {
  const alwaysAdded = personalMods.filter((p) => p.enabled !== false);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="scroll-region -mr-2 min-h-0 flex-1 overflow-y-auto pr-2">
      <div className="glass mb-4 flex items-center gap-2 rounded-xl px-3 py-2.5 font-mono text-[12.5px] text-[#bfe0ff]">
        <LinkSimple size={14} />
        <span className="truncate">{code || "PERFECT-…"}</span>
        <span className="ml-auto shrink-0 rounded-full bg-[rgba(91,227,176,0.2)] px-2 py-0.5 font-sans text-[11px] text-[#aef3d8]">
          valid
        </span>
      </div>

      <div className="mb-2 flex items-center justify-between">
        <span className="text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
          New profile
        </span>
      </div>
      <div className="glass mb-4 rounded-xl px-3.5 py-2.5 text-[14px] text-ink">
        {name || "Imported lobby"}
      </div>

      <span className="mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
        Required mods + dependencies
      </span>

      <div className="flex flex-col gap-2">
        {mode === "decoding"
          ? [0, 1, 2, 3].map((i) => <SkeletonRow key={i} />)
          : diff.map((d) => <DiffRow key={d.name} item={d} />)}
      </div>

      {alwaysAdded.length > 0 && (
        <>
          <span className="mt-4 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
            Always added (your mods)
          </span>
          <div className="flex flex-col gap-2">
            {alwaysAdded.map((pm) => (
              <div key={pm.repo} className="glass flex items-center gap-3 rounded-xl px-3 py-2.5">
                <span
                  className="grid h-[22px] w-[22px] shrink-0 place-items-center rounded-lg"
                  style={{ color: "#d4c6ff", background: "rgba(155,123,255,0.3)" }}
                >
                  <DownloadSimple size={13} weight="bold" />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[14px] font-semibold text-ink">{pm.name ?? pm.repo}</div>
                  <div className="truncate text-[12px] text-ink-faint">Always added to your lobbies</div>
                </div>
                <span className="font-mono text-[12px] text-ink-dim">{pm.tag}</span>
              </div>
            ))}
          </div>
        </>
      )}

      <div
        className="mt-4 flex items-center gap-2.5 rounded-xl px-3.5 py-2.5 text-[13px]"
        style={{ background: "rgba(91,227,176,0.12)", border: "1px solid rgba(91,227,176,0.3)", color: "#aef3d8" }}
      >
        <ShieldCheck size={16} weight="fill" />
        <span>
          All all-client mods will match the lobby <strong>exactly</strong>, so the Reactor handshake passes.
        </span>
      </div>
      <p className="mt-2 px-1 text-[12.5px] text-ink-faint">
        Built for Among Us 17.0.1 (reference only; the app won't change your game version).
      </p>
      </div>

      <div className="mt-4 flex justify-end gap-2.5 border-t border-white/10 pt-4">
        <button
          type="button"
          onClick={() => onApply(false)}
          disabled={mode === "decoding"}
          className="ring-focus glass rounded-xl px-4 py-2.5 text-[14px] text-ink disabled:opacity-50"
        >
          Apply only
        </button>
        <button
          type="button"
          onClick={() => onApply(true)}
          disabled={mode === "decoding"}
          className="ring-focus accent-grad flex items-center gap-2 rounded-xl px-5 py-2.5 text-[14px] font-bold text-[#0d0820] disabled:opacity-50"
          style={{ boxShadow: "0 8px 24px rgba(123,150,255,0.5)" }}
        >
          <Play size={15} weight="fill" /> Apply &amp; Launch
        </button>
      </div>
    </div>
  );
}

function DiffRow({ item }: { item: DiffItem }) {
  const tag = item.tags.length ? primaryTag(item.tags) : null;
  const badge =
    item.action === "install"
      ? { node: <DownloadSimple size={13} weight="bold" />, fg: "#d4c6ff", bg: "rgba(155,123,255,0.3)" }
      : item.action === "change"
        ? { node: <ArrowUp size={13} weight="bold" />, fg: "#ffe49a", bg: "rgba(255,210,63,0.22)" }
        : { node: <Check size={13} weight="bold" />, fg: "#aef3d8", bg: "rgba(91,227,176,0.24)" };

  return (
    <div className="glass flex items-center gap-3 rounded-xl px-3 py-2.5">
      <span
        className="grid h-[22px] w-[22px] shrink-0 place-items-center rounded-lg"
        style={{ color: badge.fg, background: badge.bg }}
      >
        {badge.node}
      </span>
      <div className="min-w-0">
        <div className="truncate text-[14px] font-semibold text-ink">{item.name}</div>
        <div className="truncate text-[12px] text-ink-faint">{item.detail}</div>
      </div>
      <div className="ml-auto flex items-center gap-2">
        {tag && <Pill tag={tag} />}
        <span className="font-mono text-[12px] text-ink-dim">
          {item.action === "change" ? `→ ${item.to}` : item.action === "install" ? item.to : ""}
        </span>
      </div>
    </div>
  );
}

function SkeletonRow() {
  return (
    <div className="glass flex items-center gap-3 rounded-xl px-3 py-2.5">
      <span className="h-[22px] w-[22px] shrink-0 animate-pulse rounded-lg bg-white/10" />
      <div className="flex-1 space-y-1.5">
        <div className="h-3 w-40 animate-pulse rounded bg-white/10" />
        <div className="h-2.5 w-56 animate-pulse rounded bg-white/[0.07]" />
      </div>
      <span className="h-4 w-16 animate-pulse rounded-full bg-white/10" />
    </div>
  );
}
