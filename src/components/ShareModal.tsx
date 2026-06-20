import { useEffect, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Check, Copy, DiscordLogo, LinkSimple, X } from "@phosphor-icons/react";
import type { Profile } from "../lib/types";
import { discordShare, encodeLobbyCode, webLobbyLink } from "../lib/bridge";

interface ShareModalProps {
  open: boolean;
  profile: Profile;
  onClose: () => void;
}

export function ShareModal({ open, profile, onClose }: ShareModalProps) {
  const reduce = useReducedMotion();
  const [code, setCode] = useState("");
  const [copied, setCopied] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setCode("");
    setCopied(null);
    encodeLobbyCode(profile).then(setCode).catch(() => setCode(""));
  }, [open, profile]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  const copy = async (label: string, text: string) => {
    try {
      await navigator.clipboard?.writeText(text);
      setCopied(label);
      setTimeout(() => setCopied((c) => (c === label ? null : c)), 1600);
    } catch (e) {
      console.error("clipboard write failed", e);
    }
  };

  const rows = code
    ? [
        {
          label: "Code",
          hint: "Paste into Perfect-Sync's lobby box.",
          value: code,
          icon: <LinkSimple size={14} />,
        },
        {
          label: "Link",
          hint: "Blue, clickable anywhere. Opens the app.",
          value: webLobbyLink(profile.name, code),
          icon: <LinkSimple size={14} />,
        },
        {
          label: "Discord",
          hint: "Profile name becomes the clickable link.",
          value: discordShare(profile.name, code),
          icon: <DiscordLogo size={14} />,
        },
      ]
    : [];

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
            aria-label="Share this lobby"
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex w-[560px] max-w-full flex-col rounded-3xl p-6"
          >
            <button
              type="button"
              onClick={onClose}
              aria-label="Close"
              className="ring-focus absolute top-4 right-4 grid h-8 w-8 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink"
            >
              <X size={16} weight="bold" />
            </button>

            <h2 className="text-[20px] font-semibold text-ink">Share this lobby</h2>
            <p className="mt-0.5 text-[13px] text-ink-dim">
              Everyone who opens this gets your <strong>exact</strong> mods and versions, then just clicks Launch.
            </p>

            <div className="mt-4 flex flex-col gap-3">
              {rows.length === 0 ? (
                <div className="glass h-16 animate-pulse rounded-xl" />
              ) : (
                rows.map((r) => (
                  <div key={r.label} className="glass rounded-xl p-3">
                    <div className="mb-1.5 flex items-center gap-2">
                      <span className="text-ink-dim">{r.icon}</span>
                      <span className="text-[12px] font-semibold tracking-[0.1em] text-ink uppercase">
                        {r.label}
                      </span>
                      <span className="min-w-0 truncate text-[11.5px] text-ink-faint">{r.hint}</span>
                      <button
                        type="button"
                        onClick={() => copy(r.label, r.value)}
                        className="ring-focus ml-auto flex shrink-0 items-center gap-1.5 rounded-lg bg-white/10 px-2.5 py-1 text-[12px] text-ink hover:bg-white/15"
                      >
                        {copied === r.label ? <Check size={13} weight="bold" /> : <Copy size={13} />}
                        {copied === r.label ? "Copied" : "Copy"}
                      </button>
                    </div>
                    <p className="scroll-region overflow-x-auto font-mono text-[12px] whitespace-nowrap text-[#bfe0ff]">
                      {r.value}
                    </p>
                  </div>
                ))
              )}
            </div>
            <p className="mt-3 px-1 text-[12px] leading-snug text-ink-faint">
              The <strong>Discord</strong> link puts your profile name as the clickable text. Discord only
              renders that inside bot or webhook posts; in a normal message, paste the <strong>Link</strong> above
              (it stays blue and opens the app).
            </p>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
