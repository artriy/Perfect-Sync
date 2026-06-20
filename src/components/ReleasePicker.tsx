import { useEffect, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { DownloadSimple, FileArrowDown, X } from "@phosphor-icons/react";
import { listReleases, type GhRelease } from "../lib/bridge";

interface ReleasePickerProps {
  open: boolean;
  repo: string;
  modName: string;
  busy: boolean;
  onClose: () => void;
  onPick: (tag: string, assetName: string) => void;
}

function mb(bytes: number): string {
  return bytes > 0 ? `${(bytes / 1048576).toFixed(1)} MB` : "";
}

export function ReleasePicker({ open, repo, modName, busy, onClose, onPick }: ReleasePickerProps) {
  const reduce = useReducedMotion();
  const [releases, setReleases] = useState<GhRelease[]>([]);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setReleases([]);
    setErr(null);
    setLoading(true);
    listReleases(repo)
      .then((r) => setReleases(r))
      .catch((e) => setErr(String(e)))
      .finally(() => setLoading(false));
  }, [open, repo]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

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
            aria-label={`Pick a release file for ${modName}`}
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex max-h-[80vh] w-[560px] max-w-full flex-col rounded-3xl p-6"
          >
            <button
              type="button"
              onClick={onClose}
              aria-label="Close"
              className="ring-focus absolute top-4 right-4 grid h-8 w-8 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink"
            >
              <X size={16} weight="bold" />
            </button>

            <h2 className="text-[20px] font-semibold text-ink">Pick a file</h2>
            <p className="mt-0.5 text-[13px] text-ink-dim">
              {modName} · <span className="font-mono">{repo}</span>
            </p>

            <div className="scroll-region mt-4 flex-1 overflow-y-auto pr-1">
              {loading && <p className="py-8 text-center text-[13px] text-ink-faint">Loading releases…</p>}
              {err && <p className="py-8 text-center text-[13px] text-[#ff8a8a]">{err}</p>}
              {!loading &&
                !err &&
                !releases.some((r) => r.assets.some((a) => a.name.toLowerCase().endsWith(".dll"))) && (
                  <p className="py-8 text-center text-[13px] text-ink-faint">
                    No .dll files in this repo's releases. Zips and source archives are hidden.
                  </p>
                )}
              {!loading &&
                releases
                  .map((rel) => ({
                    rel,
                    dlls: rel.assets.filter((a) => a.name.toLowerCase().endsWith(".dll")),
                  }))
                  .filter(({ dlls }) => dlls.length > 0)
                  .map(({ rel, dlls }) => (
                    <div key={rel.tag_name} className="mb-3">
                      <div className="mb-1.5 flex items-center gap-2 px-1">
                        <span className="font-mono text-[12.5px] text-ink">{rel.tag_name}</span>
                        <div className="h-px flex-1 bg-white/10" />
                      </div>
                      <div className="flex flex-col gap-1.5">
                        {dlls.map((a) => (
                          <button
                            key={a.name}
                            type="button"
                            disabled={busy}
                            onClick={() => onPick(rel.tag_name, a.name)}
                            className="ring-focus glass flex items-center gap-2.5 rounded-xl px-3 py-2.5 text-left hover:bg-white/10 disabled:opacity-50"
                          >
                            <FileArrowDown size={16} className="shrink-0 text-ink-dim" />
                            <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] text-ink">{a.name}</span>
                            <span className="text-[11.5px] text-ink-faint">{mb(a.size)}</span>
                            <DownloadSimple size={14} className="text-[#9b7bff]" />
                          </button>
                        ))}
                      </div>
                    </div>
                  ))}
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
