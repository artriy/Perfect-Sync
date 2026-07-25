import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  ArrowUp,
  Check,
  DownloadSimple,
  LinkSimple,
  MapTrifold,
  Play,
  ShieldCheck,
  Warning,
  X,
} from "@phosphor-icons/react";
import { Pill, primaryTag } from "./Pill";
import { TrustBadge } from "./TrustBadge";
import type { DiffItem, PersonalMod, Trust } from "../lib/types";
import { extractLobbyCode, previewCode } from "../lib/bridge";
import { useModalFocus } from "../lib/useModalFocus";

type Mode = "input" | "decoding" | "diff";

interface PreviewedLobby {
  code: string;
  rows: DiffItem[];
  name: string;
  levelImposterMaps: string[];
}

interface Confirmation {
  code: string;
  launch: boolean;
}

interface LobbyCodeModalProps {
  open: boolean;
  initialCode?: string;
  installed: [string, string][];
  trustOf: (id: string) => Trust;
  personalMods: PersonalMod[];
  busyReason?: string;
  onClose: () => void;
  onApply: (launch: boolean, code: string) => void;
}

export function LobbyCodeModal({ open, initialCode, installed, trustOf, personalMods, busyReason, onClose, onApply }: LobbyCodeModalProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const openRef = useRef(open);
  const installedRef = useRef(installed);
  const sessionRef = useRef(0);
  const requestRef = useRef(0);
  const [mode, setMode] = useState<Mode>("input");
  const [code, setCode] = useState("");
  const [previewed, setPreviewed] = useState<PreviewedLobby | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);

  openRef.current = open;
  installedRef.current = installed;

  const closeModal = useCallback(() => {
    sessionRef.current += 1;
    requestRef.current += 1;
    setConfirmation(null);
    onClose();
  }, [onClose]);

  useModalFocus(open && confirmation === null, modalRef, closeModal);

  const runDecode = useCallback((value: string) => {
    const session = sessionRef.current;
    const installedSnapshot = installedRef.current;
    const request = ++requestRef.current;
    setConfirmation(null);
    setPreviewed(null);
    setMode("decoding");
    setError(null);

    previewCode(value, installedSnapshot)
      .then((preview) => {
        if (!openRef.current || installedRef.current !== installedSnapshot || sessionRef.current !== session || requestRef.current !== request) return;
        setPreviewed({
          code: value,
          rows: preview.items,
          name: preview.name,
          levelImposterMaps: preview.levelImposterMaps,
        });
        setMode("diff");
      })
      .catch((reason: unknown) => {
        if (!openRef.current || installedRef.current !== installedSnapshot || sessionRef.current !== session || requestRef.current !== request) return;
        console.error("lobby code decode failed", reason);
        setPreviewed(null);
        setError(String(reason));
        setMode("diff");
      });
  }, []);

  useEffect(() => {
    sessionRef.current += 1;
    requestRef.current += 1;
    setConfirmation(null);
    setPreviewed(null);
    setError(null);

    if (!open) return;
    const nextCode = initialCode ?? "";
    setCode(nextCode);
    if (nextCode) {
      runDecode(extractLobbyCode(nextCode) ?? nextCode);
    } else {
      setMode("input");
    }
  }, [initialCode, open, runDecode]);

  useEffect(() => {
    if (busyReason) setConfirmation(null);
  }, [busyReason]);

  const changeCode = (value: string) => {
    requestRef.current += 1;
    setConfirmation(null);
    setPreviewed(null);
    setError(null);
    setCode(value);
  };

  const decode = () => {
    const extracted = extractLobbyCode(code);
    if (!extracted) return;
    setCode(extracted);
    runDecode(extracted);
  };

  const requestApply = (launch: boolean) => {
    if (!previewed || busyReason) return;
    const flaggedCount = previewed.rows.filter((item) => item.trust === "flagged").length;
    if (flaggedCount > 0) {
      setConfirmation({ code: previewed.code, launch });
      return;
    }
    onApply(launch, previewed.code);
  };

  const confirmApply = () => {
    if (busyReason || !confirmation || !previewed || confirmation.code !== previewed.code) {
      setConfirmation(null);
      return;
    }
    const { launch, code: confirmedCode } = confirmation;
    setConfirmation(null);
    onApply(launch, confirmedCode);
  };

  const rows = previewed?.rows ?? [];
  const flaggedCount = rows.filter((item) => item.trust === "flagged").length;

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-50 grid place-items-center p-4 sm:p-6"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18 }}
        >
          <div
            className="absolute inset-0 bg-[rgba(6,4,18,0.5)]"
            style={{ backdropFilter: "blur(2px)" }}
            onClick={confirmation === null ? closeModal : undefined}
          />

          <motion.div
            ref={modalRef}
            role="dialog"
            aria-modal="true"
            aria-label="Set up this lobby"
            aria-hidden={confirmation !== null}
            inert={confirmation !== null}
            tabIndex={-1}
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className={`glass-strong relative flex max-h-[92vh] max-w-full flex-col rounded-3xl p-5 sm:p-6 ${mode === "input" ? "w-[560px]" : "w-[820px]"}`}
          >
            <button
              data-autofocus
              type="button"
              onClick={closeModal}
              aria-label="Close lobby code dialog"
              className="ring-focus absolute top-4 right-4 grid h-8 w-8 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink"
            >
              <X size={16} weight="bold" />
            </button>

            <h2 className="pr-10 text-[20px] font-semibold text-ink">Set up this lobby</h2>
            <p className="mt-0.5 text-[13px] text-ink-dim">
              {mode === "input"
                ? "Paste a friend's PERFECT- code and we'll show exactly what changes."
                : "Decoded from a shared code. Here is precisely what will change."}
            </p>

            <div className="mt-4 flex min-h-0 flex-1 flex-col">
              {mode === "input" ? (
                <InputStep code={code} setCode={changeCode} onDecode={decode} />
              ) : (
                <ResultStep
                  mode={mode}
                  diff={rows}
                  levelImposterMaps={previewed?.levelImposterMaps ?? []}
                  personalMods={personalMods}
                  trustOf={trustOf}
                  error={error}
                  name={previewed?.name ?? ""}
                  previewedCode={previewed?.code ?? ""}
                  flaggedCount={flaggedCount}
                  canApply={previewed !== null}
                  busyReason={busyReason}
                  onApply={requestApply}
                />
              )}
            </div>
          </motion.div>

          <UnverifiedConfirmation
            open={confirmation !== null}
            flaggedCount={flaggedCount}
            code={confirmation?.code ?? ""}
            onCancel={() => setConfirmation(null)}
            onConfirm={confirmApply}
          />
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
  setCode: (value: string) => void;
  onDecode: () => void;
}) {
  const valid = extractLobbyCode(code) != null;
  return (
    <>
      <textarea
        data-autofocus
        value={code}
        onChange={(event) => setCode(event.target.value)}
        rows={3}
        placeholder="PERFECT-…"
        aria-label="Lobby code"
        className="glass ring-focus w-full resize-none rounded-xl px-3.5 py-3 font-mono text-[13px] text-ink placeholder:text-ink-faint focus:outline-none"
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
  levelImposterMaps,
  name,
  previewedCode,
  personalMods,
  trustOf,
  error,
  flaggedCount,
  busyReason,
  canApply,
  onApply,
}: {
  mode: Mode;
  diff: DiffItem[];
  levelImposterMaps: string[];
  name: string;
  previewedCode: string;
  personalMods: PersonalMod[];
  trustOf: (id: string) => Trust;
  error: string | null;
  flaggedCount: number;
  busyReason?: string;
  canApply: boolean;
  onApply: (launch: boolean) => void;
}) {
  const alwaysAdded = personalMods.filter((item) => item.enabled !== false);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="scroll-region -mr-2 min-h-0 flex-1 overflow-y-auto pr-2">
        <div className="glass mb-4 flex min-w-0 items-center gap-2 rounded-xl px-3 py-2.5 font-mono text-[12.5px] text-[#bfe0ff]">
          <LinkSimple size={14} className="shrink-0" />
          <span className="min-w-0 truncate" title={previewedCode} aria-label={`Previewed lobby code ${previewedCode || "unavailable"}`}>
            {previewedCode || "PERFECT-…"}
          </span>
          <span className={`ml-auto shrink-0 rounded-full px-2 py-0.5 font-sans text-[11px] ${error ? "bg-[rgba(226,59,59,0.2)] text-[#ff8a8a]" : "bg-[rgba(91,227,176,0.2)] text-[#aef3d8]"}`}>
            {error ? "invalid" : mode === "decoding" ? "checking" : "valid"}
          </span>
        </div>

        <span className="mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">New profile</span>
        <div
          className="glass mb-4 max-w-full truncate rounded-xl px-3.5 py-2.5 text-[14px] text-ink"
          title={name || "Imported lobby"}
          aria-label={`New profile name ${name || "Imported lobby"}`}
        >
          {name || "Imported lobby"}
        </div>

        <span className="mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
          Required mods + dependencies
        </span>

        <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
          {mode === "decoding"
            ? [0, 1, 2, 3].map((index) => <SkeletonRow key={index} />)
            : error
              ? <p className="glass rounded-xl px-3.5 py-4 text-[13px] break-words text-[#ff8a8a] sm:col-span-2">This code could not be read: {error}</p>
              : diff.map((item, index) => <DiffRow key={`${item.repo ?? item.name}-${item.to ?? "current"}-${index}`} item={item} />)}
        </div>

        {mode !== "decoding" && !error && levelImposterMaps.length > 0 && (
          <>
            <span className="mt-4 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
              LevelImposter maps
            </span>
            <div
              className="glass flex items-center gap-3 rounded-xl px-3.5 py-3"
              title={levelImposterMaps.join("\n")}
            >
              <span className="grid h-8 w-8 shrink-0 place-items-center rounded-xl bg-[rgba(91,227,176,0.16)] text-[#aef3d8]">
                <MapTrifold size={17} weight="fill" />
              </span>
              <div className="min-w-0">
                <p className="text-[13.5px] font-semibold text-ink">
                  {levelImposterMaps.length} exact map{levelImposterMaps.length === 1 ? "" : "s"}
                </p>
                <p className="text-[11.5px] text-ink-faint">Downloaded from LevelImposter and applied to this lobby profile.</p>
              </div>
            </div>
          </>
        )}

        {mode !== "decoding" && !error && flaggedCount > 0 && (
          <div
            className="mt-3 flex items-start gap-2.5 rounded-xl px-3.5 py-2.5 text-[13px]"
            style={{ background: "rgba(255,170,60,0.12)", border: "1px solid rgba(255,170,60,0.32)", color: "#ffd9a8" }}
          >
            <Warning size={16} weight="fill" className="mt-0.5 shrink-0" />
            <span>
              {flaggedCount} mod{flaggedCount > 1 ? "s are" : " is"} <strong>unverified</strong> (not in the trusted catalog). Only apply a code from someone you trust.
            </span>
          </div>
        )}

        {alwaysAdded.length > 0 && (
          <>
            <span className="mt-4 mb-2 block text-[11px] font-medium tracking-[0.14em] text-ink-faint uppercase">
              Always added (your mods)
            </span>
            <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
              {alwaysAdded.map((personalMod) => {
                const displayName = personalMod.name ?? personalMod.repo;
                return (
                  <div
                    key={personalMod.repo}
                    className="glass flex min-w-0 items-center gap-3 overflow-hidden rounded-xl px-3 py-2.5"
                    aria-label={`${displayName}, repository ${personalMod.repo}, version ${personalMod.tag}`}
                    title={`${displayName} · ${personalMod.repo} · ${personalMod.tag}`}
                  >
                    <span className="grid h-[22px] w-[22px] shrink-0 place-items-center rounded-lg" style={{ color: "#d4c6ff", background: "rgba(155,123,255,0.3)" }}>
                      <DownloadSimple size={13} weight="bold" />
                    </span>
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-[14px] font-semibold text-ink">{displayName}</div>
                      <div className="truncate text-[12px] text-ink-faint">{personalMod.repo}</div>
                    </div>
                    <TrustBadge trust={trustOf(personalMod.repo)} compact />
                    <span className="max-w-24 shrink-0 truncate font-mono text-[12px] text-ink-dim" title={personalMod.tag}>{personalMod.tag}</span>
                  </div>
                );
              })}
            </div>
          </>
        )}

        {!error && mode !== "decoding" && (
          <>
            <div className="mt-4 flex items-start gap-2.5 rounded-xl px-3.5 py-2.5 text-[13px]" style={{ background: "rgba(91,227,176,0.12)", border: "1px solid rgba(91,227,176,0.3)", color: "#aef3d8" }}>
              <ShieldCheck size={16} weight="fill" className="mt-0.5 shrink-0" />
              <span>All all-client mods will match the lobby <strong>exactly</strong>, so the Reactor handshake passes.</span>
            </div>
            <p className="mt-2 px-1 text-[12.5px] text-ink-faint">Built for Among Us 17.0.1 (reference only; the app won't change your game version).</p>
          </>
        )}

        {busyReason && (
          <p id="lobby-apply-busy-reason" role="status" className="mt-3 px-1 text-right text-[12.5px] text-[#ffd9a8]">
            {busyReason}
          </p>
        )}
      </div>

      <div className="mt-4 flex flex-wrap justify-end gap-2.5 border-t border-white/10 pt-4">
        <button type="button" onClick={() => onApply(false)} disabled={!canApply || mode === "decoding" || !!error || !!busyReason} aria-describedby={busyReason ? "lobby-apply-busy-reason" : undefined} className="ring-focus glass rounded-xl px-4 py-2.5 text-[14px] text-ink disabled:opacity-50">
          Apply only
        </button>
        <button type="button" onClick={() => onApply(true)} disabled={!canApply || mode === "decoding" || !!error || !!busyReason} aria-describedby={busyReason ? "lobby-apply-busy-reason" : undefined} className="ring-focus accent-grad flex items-center gap-2 rounded-xl px-5 py-2.5 text-[14px] font-bold text-[#0d0820] disabled:opacity-50" style={{ boxShadow: "0 8px 24px rgba(123,150,255,0.5)" }}>
          <Play size={15} weight="fill" /> Apply &amp; Launch
        </button>
      </div>
    </div>
  );
}

function UnverifiedConfirmation({ open, flaggedCount, code, onCancel, onConfirm }: { open: boolean; flaggedCount: number; code: string; onCancel: () => void; onConfirm: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useModalFocus(open, ref, onCancel);

  if (!open) return null;
  return (
    <div className="absolute inset-0 z-20 grid place-items-center rounded-3xl bg-[rgba(6,4,18,0.72)] p-4 sm:p-6" style={{ backdropFilter: "blur(2px)" }} onMouseDown={(event) => event.target === event.currentTarget && onCancel()}>
      <div ref={ref} role="alertdialog" aria-modal="true" aria-labelledby="lobby-unverified-title" aria-describedby="lobby-unverified-description" tabIndex={-1} className="glass-strong w-[420px] max-w-full rounded-2xl p-5">
        <div className="flex items-center gap-2.5">
          <Warning size={20} weight="fill" className="shrink-0 text-[#ffd9a8]" />
          <h3 id="lobby-unverified-title" className="text-[16px] font-semibold text-ink">Install unverified mods?</h3>
        </div>
        <p id="lobby-unverified-description" className="mt-2 text-[13px] text-ink-dim">
          This preview includes {flaggedCount} mod{flaggedCount > 1 ? "s" : ""} not in the trusted catalog. Only continue if you trust whoever shared this exact code.
        </p>
        <p className="mt-2 truncate font-mono text-[11px] text-ink-faint" title={code} aria-label={`Code awaiting confirmation ${code}`}>{code}</p>
        <div className="mt-4 flex flex-wrap justify-end gap-2.5">
          <button data-autofocus type="button" onClick={onCancel} className="ring-focus glass rounded-xl px-4 py-2.5 text-[13.5px] text-ink">Cancel</button>
          <button type="button" onClick={onConfirm} className="ring-focus accent-grad rounded-xl px-4 py-2.5 text-[13.5px] font-bold text-[#0d0820]">Install this preview anyway</button>
        </div>
      </div>
    </div>
  );
}

function DiffRow({ item }: { item: DiffItem }) {
  const tag = item.tags.length ? primaryTag(item.tags) : null;
  const badge = item.action === "install"
    ? { node: <DownloadSimple size={13} weight="bold" />, fg: "#d4c6ff", bg: "rgba(155,123,255,0.3)" }
    : item.action === "change"
      ? { node: <ArrowUp size={13} weight="bold" />, fg: "#ffe49a", bg: "rgba(255,210,63,0.22)" }
      : { node: <Check size={13} weight="bold" />, fg: "#aef3d8", bg: "rgba(91,227,176,0.24)" };
  const identity = `${item.name}${item.repo ? `, repository ${item.repo}` : ""}, ${item.detail}${item.to ? `, version ${item.to}` : ""}${item.asset ? `, release asset ${item.asset}` : ""}`;

  return (
    <div className="glass flex min-w-0 items-center gap-3 overflow-hidden rounded-xl px-3 py-2.5" aria-label={identity} title={identity}>
      <span className="grid h-[22px] w-[22px] shrink-0 place-items-center rounded-lg" style={{ color: badge.fg, background: badge.bg }}>{badge.node}</span>
      <div className="min-w-0 flex-1">
        <div className="truncate text-[14px] font-semibold text-ink">{item.name}</div>
        <div className="truncate text-[12px] text-ink-faint" title={item.asset ?? item.repo ?? item.detail}>
          {item.asset ? `Asset: ${item.asset}` : (item.repo ?? item.detail)}
        </div>
      </div>
      <div className="flex min-w-0 shrink items-center gap-2">
        {tag && <Pill tag={tag} />}
        {item.trust && <TrustBadge trust={item.trust} compact />}
        {item.to && <span className="max-w-24 truncate font-mono text-[12px] text-ink-dim" title={item.to}>{item.action === "change" ? `→ ${item.to}` : item.action === "install" ? item.to : ""}</span>}
      </div>
    </div>
  );
}

function SkeletonRow() {
  return (
    <div className="glass flex items-center gap-3 rounded-xl px-3 py-2.5" aria-label="Loading mod preview">
      <span className="h-[22px] w-[22px] shrink-0 animate-pulse rounded-lg bg-white/10" />
      <div className="flex-1 space-y-1.5">
        <div className="h-3 w-2/5 animate-pulse rounded bg-white/10" />
        <div className="h-2.5 w-3/5 animate-pulse rounded bg-white/[0.07]" />
      </div>
      <span className="h-4 w-16 animate-pulse rounded-full bg-white/10" />
    </div>
  );
}
