import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Check, Copy, DiscordLogo, LinkSimple, WarningCircle, X } from "@phosphor-icons/react";
import type { Profile } from "../lib/types";
import { discordShare, encodeLobbyCode, webLobbyLink } from "../lib/bridge";
import { useModalFocus } from "../lib/useModalFocus";

interface ShareModalProps {
  open: boolean;
  profile: Profile;
  onClose: () => void;
}

type EncodeState =
  | { kind: "loading"; profileId: string }
  | { kind: "ready"; profileId: string; code: string }
  | { kind: "error"; profileId: string; message: string };

interface ShareRow {
  key: "code" | "link" | "discord";
  label: string;
  hint: string;
  value: string;
  icon: ReactNode;
}

export function ShareModal({ open, profile, onClose }: ShareModalProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const openRef = useRef(open);
  const currentProfileRef = useRef(profile);
  const sessionRef = useRef(0);
  const requestRef = useRef(0);
  const [encodeState, setEncodeState] = useState<EncodeState>({ kind: "loading", profileId: profile.id });
  const [copied, setCopied] = useState<ShareRow["key"] | null>(null);
  const [clipboardError, setClipboardError] = useState<{ key: ShareRow["key"]; message: string } | null>(null);

  openRef.current = open;
  currentProfileRef.current = profile;

  const closeShare = useCallback(() => {
    sessionRef.current += 1;
    requestRef.current += 1;
    onClose();
  }, [onClose]);

  useModalFocus(open, modalRef, closeShare);

  const encode = useCallback((targetProfile: Profile) => {
    const session = sessionRef.current;
    const request = ++requestRef.current;
    const profileId = targetProfile.id;
    setEncodeState({ kind: "loading", profileId });
    setCopied(null);
    setClipboardError(null);

    encodeLobbyCode(targetProfile)
      .then((code) => {
        if (!openRef.current || currentProfileRef.current !== targetProfile || sessionRef.current !== session || requestRef.current !== request) return;
        setEncodeState({ kind: "ready", profileId, code });
      })
      .catch((reason: unknown) => {
        if (!openRef.current || currentProfileRef.current !== targetProfile || sessionRef.current !== session || requestRef.current !== request) return;
        setEncodeState({ kind: "error", profileId, message: String(reason) });
      });
  }, []);

  useEffect(() => {
    sessionRef.current += 1;
    requestRef.current += 1;
    setCopied(null);
    setClipboardError(null);
    if (!open) return;
    encode(profile);
  }, [encode, open, profile]);

  const currentState = encodeState.profileId === profile.id ? encodeState : { kind: "loading" as const, profileId: profile.id };
  const rows: ShareRow[] = currentState.kind === "ready"
    ? [
        {
          key: "code",
          label: "Code",
          hint: "Paste into Perfect-Sync's lobby box.",
          value: currentState.code,
          icon: <LinkSimple size={14} />,
        },
        {
          key: "link",
          label: "Link",
          hint: "Clickable anywhere. Opens the app.",
          value: webLobbyLink(profile.name, currentState.code),
          icon: <LinkSimple size={14} />,
        },
        {
          key: "discord",
          label: "Discord",
          hint: "Profile name becomes the clickable link.",
          value: discordShare(profile.name, currentState.code),
          icon: <DiscordLogo size={14} />,
        },
      ]
    : [];

  const copy = async (row: ShareRow) => {
    const session = sessionRef.current;
    const targetProfile = currentProfileRef.current;
    setCopied(null);
    setClipboardError(null);
    try {
      if (!navigator.clipboard || typeof navigator.clipboard.writeText !== "function") {
        throw new Error("Clipboard access is unavailable");
      }
      await navigator.clipboard.writeText(row.value);
      if (!openRef.current || currentProfileRef.current !== targetProfile || sessionRef.current !== session) return;
      setCopied(row.key);
    } catch (reason: unknown) {
      if (!openRef.current || currentProfileRef.current !== targetProfile || sessionRef.current !== session) return;
      setClipboardError({
        key: row.key,
        message: `${String(reason)}. Select the ${row.label.toLowerCase()} value below and copy it manually.`,
      });
    }
  };

  return (
    <AnimatePresence>
      {open && (
        <motion.div className="fixed inset-0 z-50 grid place-items-center p-4 sm:p-6" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.18 }}>
          <div className="absolute inset-0 bg-[rgba(6,4,18,0.5)]" style={{ backdropFilter: "blur(2px)" }} onClick={closeShare} />

          <motion.div
            ref={modalRef}
            role="dialog"
            aria-modal="true"
            aria-label={`Share lobby profile ${profile.name}`}
            tabIndex={-1}
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex max-h-[92vh] w-[600px] max-w-full flex-col rounded-3xl p-5 sm:p-6"
          >
            <button type="button" onClick={closeShare} aria-label="Close share dialog" className="ring-focus absolute top-4 right-4 grid h-8 w-8 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink">
              <X size={16} weight="bold" />
            </button>

            <h2 className="pr-10 text-[20px] font-semibold text-ink">Share this lobby</h2>
            <p className="mt-0.5 text-[13px] text-ink-dim">
              Everyone who opens this gets the exact mods and versions from <strong className="inline-block max-w-full truncate align-bottom" title={profile.name}>{profile.name}</strong>, then just clicks Launch.
            </p>

            <div className="scroll-region mt-4 flex min-h-0 flex-col gap-3 overflow-y-auto pr-1">
              {currentState.kind === "loading" && (
                <div className="glass h-20 animate-pulse rounded-xl" role="status" aria-label="Creating share code" />
              )}

              {currentState.kind === "error" && (
                <div className="rounded-xl bg-[rgba(226,59,59,0.12)] p-4" role="alert">
                  <div className="flex items-start gap-2.5 text-[#ff8a8a]">
                    <WarningCircle size={18} className="mt-0.5 shrink-0" />
                    <div className="min-w-0">
                      <p className="text-[13.5px] font-semibold">Could not create a share code</p>
                      <p className="mt-1 text-[12.5px] break-words">{currentState.message}</p>
                    </div>
                  </div>
                  <button data-autofocus type="button" onClick={() => encode(profile)} className="ring-focus glass mt-3 rounded-lg px-3 py-1.5 text-[12.5px] font-semibold text-ink">Retry creating share code</button>
                </div>
              )}

              {rows.map((row) => {
                const failed = clipboardError?.key === row.key;
                const succeeded = copied === row.key;
                return (
                  <div key={row.key} className="glass min-w-0 rounded-xl p-3">
                    <div className="mb-1.5 flex min-w-0 flex-wrap items-center gap-2">
                      <span className="text-ink-dim">{row.icon}</span>
                      <span className="text-[12px] font-semibold tracking-[0.1em] text-ink uppercase">{row.label}</span>
                      <span className="min-w-0 flex-1 truncate text-[11.5px] text-ink-faint" title={row.hint}>{row.hint}</span>
                      <button
                        type="button"
                        onClick={() => void copy(row)}
                        aria-label={`Copy ${row.label.toLowerCase()} share format`}
                        className="ring-focus ml-auto flex shrink-0 items-center gap-1.5 rounded-lg bg-white/10 px-2.5 py-1 text-[12px] text-ink hover:bg-white/15"
                      >
                        {succeeded ? <Check size={13} weight="bold" /> : <Copy size={13} />}
                        {succeeded ? `${row.label} copied` : `Copy ${row.label}`}
                      </button>
                    </div>
                    <textarea
                      readOnly
                      rows={row.key === "code" ? 2 : 3}
                      value={row.value}
                      onFocus={(event) => event.currentTarget.select()}
                      aria-label={`${row.label} share value; focus to select it for manual copying`}
                      className="ring-focus scroll-region w-full resize-none overflow-auto rounded-lg bg-black/10 px-2.5 py-2 font-mono text-[12px] break-all text-[#bfe0ff]"
                    />
                    <div className={`mt-1.5 min-h-4 text-[11.5px] ${failed ? "text-[#ff8a8a]" : "text-[#aef3d8]"}`} aria-live="polite" role={failed ? "alert" : "status"}>
                      {failed ? clipboardError.message : succeeded ? `${row.label} copied to the clipboard.` : ""}
                    </div>
                  </div>
                );
              })}
            </div>
            {currentState.kind === "ready" && (
              <p className="mt-3 px-1 text-[12px] leading-snug text-ink-faint">
                The <strong>Discord</strong> link puts your profile name in clickable text for bot or webhook posts. In a normal message, use the <strong>Link</strong> format.
              </p>
            )}
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
