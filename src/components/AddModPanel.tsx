import { useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { GithubLogo, MagnifyingGlass, Plus, X } from "@phosphor-icons/react";
import { Pill, primaryTag } from "./Pill";
import type { CatalogItem } from "../lib/types";

interface AddModPanelProps {
  open: boolean;
  profileName: string;
  catalog: CatalogItem[];
  onClose: () => void;
  onAddCatalog: (item: CatalogItem) => void;
  onAddUrl: (url: string) => void;
}

export function AddModPanel({ open, profileName, catalog, onClose, onAddCatalog, onAddUrl }: AddModPanelProps) {
  const reduce = useReducedMotion();
  const [url, setUrl] = useState("");
  const [q, setQ] = useState("");

  const looksLikeRepo = /github\.com\/.+\/.+/i.test(url.trim());
  const results = catalog.filter(
    (c) =>
      c.name.toLowerCase().includes(q.toLowerCase()) ||
      c.summary.toLowerCase().includes(q.toLowerCase()),
  );

  const addUrl = () => {
    if (!looksLikeRepo) return;
    onAddUrl(url.trim());
    setUrl("");
  };

  return (
    <AnimatePresence>
      {open && (
        <motion.div className="fixed inset-0 z-40" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
          <div className="absolute inset-0 bg-[rgba(6,4,18,0.45)]" onClick={onClose} />

          <motion.aside
            initial={reduce ? { opacity: 0 } : { x: 36, opacity: 0 }}
            animate={{ x: 0, opacity: 1 }}
            exit={reduce ? { opacity: 0 } : { x: 36, opacity: 0 }}
            transition={{ duration: 0.24, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong absolute top-0 right-0 flex h-full w-[420px] max-w-full flex-col rounded-l-3xl"
            role="dialog"
            aria-label="Add a mod"
          >
            <div className="flex items-center justify-between px-5 pt-5 pb-3">
              <div>
                <h2 className="text-[19px] font-semibold text-ink">Add a mod</h2>
                <p className="text-[12.5px] text-ink-dim">to {profileName}</p>
              </div>
              <button
                type="button"
                onClick={onClose}
                aria-label="Close"
                className="ring-focus grid h-8 w-8 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink"
              >
                <X size={16} weight="bold" />
              </button>
            </div>

            {/* paste any GitHub URL */}
            <div className="px-5 pb-3">
              <label className="glass flex items-center gap-2 rounded-xl px-3 py-2.5 text-ink-dim focus-within:text-ink">
                <GithubLogo size={16} className="opacity-75" />
                <input
                  value={url}
                  onChange={(e) => setUrl(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && addUrl()}
                  placeholder="Paste any GitHub repo or release URL"
                  aria-label="GitHub URL"
                  className="w-full bg-transparent text-[13px] text-ink placeholder:text-ink-faint focus:outline-none"
                />
                <button
                  type="button"
                  onClick={addUrl}
                  disabled={!looksLikeRepo}
                  className="ring-focus shrink-0 rounded-lg bg-white/10 px-2.5 py-1 text-[12px] font-semibold text-ink disabled:opacity-40"
                >
                  Add
                </button>
              </label>
            </div>

            <div className="flex items-center gap-3 px-5 pb-2">
              <div className="h-px flex-1 bg-white/10" />
              <span className="text-[11px] tracking-[0.14em] text-ink-faint uppercase">Catalog</span>
              <div className="h-px flex-1 bg-white/10" />
            </div>

            <div className="px-5 pb-3">
              <label className="glass flex items-center gap-2 rounded-xl px-3 py-2 text-ink-dim focus-within:text-ink">
                <MagnifyingGlass size={15} className="opacity-70" />
                <input
                  value={q}
                  onChange={(e) => setQ(e.target.value)}
                  placeholder="Search the catalog"
                  aria-label="Search catalog"
                  className="w-full bg-transparent text-[13px] text-ink placeholder:text-ink-faint focus:outline-none"
                />
              </label>
            </div>

            <div className="scroll-region flex flex-1 flex-col gap-2 overflow-y-auto px-5 pb-5">
              {results.map((item) => (
                <div key={item.id} className="glass rounded-2xl p-3.5">
                  <div className="flex items-center gap-2">
                    <span className="text-[14.5px] font-semibold text-ink">{item.name}</span>
                    <Pill tag={primaryTag(item.tags)} />
                    {item.latest && (
                      <span className="ml-auto font-mono text-[12px] text-ink-faint">{item.latest}</span>
                    )}
                  </div>
                  <p className="mt-1.5 text-[12.5px] leading-snug text-ink-dim">{item.summary}</p>
                  <div className="mt-3 flex items-center justify-between">
                    <span className="font-mono text-[11.5px] text-ink-faint">{item.repo}</span>
                    <button
                      type="button"
                      onClick={() => onAddCatalog(item)}
                      className="ring-focus accent-grad flex items-center gap-1 rounded-lg px-3 py-1.5 text-[12.5px] font-semibold text-[#0d0820] transition-transform active:scale-[0.96]"
                    >
                      <Plus size={13} weight="bold" /> Add
                    </button>
                  </div>
                </div>
              ))}
              {results.length === 0 && (
                <p className="px-1 py-6 text-center text-[13px] text-ink-faint">
                  No catalog match. Paste the GitHub URL above to add it anyway.
                </p>
              )}
            </div>
          </motion.aside>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
