import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { CaretDown, CaretUp, Check, FileArrowUp, GithubLogo, MagnifyingGlass, MapTrifold, Plus, TrashSimple, X } from "@phosphor-icons/react";
import { Pill, primaryTag } from "./Pill";
import { TrustBadge } from "./TrustBadge";
import type { CatalogItem } from "../lib/types";
import { useModalFocus } from "../lib/useModalFocus";

interface AddModPanelProps {
  open: boolean;
  profileName: string;
  catalog: CatalogItem[];
  onClose: () => void;
  installedIds: string[];
  selectedIds: string[];
  onToggleCatalog: (id: string) => void;
  onReview: () => void;
  onBrowseMaps: () => void;
  onAddUrl: (url: string) => Promise<void>;
  onAddLocal: () => Promise<void>;
  onRemoveCatalog: (id: string) => Promise<void>;
  onMoveCatalog: (id: string, dir: "up" | "down") => Promise<void>;
}

export function AddModPanel({
  open,
  profileName,
  catalog,
  installedIds,
  selectedIds,
  onClose,
  onToggleCatalog,
  onReview,
  onBrowseMaps,
  onAddUrl,
  onAddLocal,
  onRemoveCatalog,
  onMoveCatalog,
}: AddModPanelProps) {
  const reduce = useReducedMotion();
  const panelRef = useRef<HTMLElement>(null);
  const openRef = useRef(open);
  const sessionRef = useRef(0);
  const pendingRef = useRef<string | null>(null);
  const [url, setUrl] = useState("");
  const [query, setQuery] = useState("");
  const [manage, setManage] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  openRef.current = open;

  const closePanel = useCallback(() => {
    sessionRef.current += 1;
    onClose();
  }, [onClose]);

  useModalFocus(open, panelRef, closePanel);

  useEffect(() => {
    sessionRef.current += 1;
    setActionError(null);
    if (!open) return;
    setUrl("");
    setQuery("");
    setManage(false);
  }, [open, profileName]);

  const looksLikeRepo = /github\.com\/.+\/.+/i.test(url.trim());
  const normalizedQuery = query.toLowerCase();
  const installed = new Set(installedIds.map((id) => id.toLowerCase()));
  const selected = new Set(selectedIds);
  const results = catalog.filter(
    (item) => item.name.toLowerCase().includes(normalizedQuery) || item.summary.toLowerCase().includes(normalizedQuery),
  );

  const runAction = async (key: string, action: () => Promise<void>, afterSuccess?: () => void) => {
    if (pendingRef.current !== null) return;
    const session = sessionRef.current;
    pendingRef.current = key;
    setPending(key);
    setActionError(null);
    try {
      await action();
      if (openRef.current && sessionRef.current === session) afterSuccess?.();
    } catch (reason: unknown) {
      if (openRef.current && sessionRef.current === session) setActionError(String(reason));
    } finally {
      if (pendingRef.current === key) pendingRef.current = null;
      setPending((current) => current === key ? null : current);
    }
  };

  const addUrl = () => {
    const targetUrl = url.trim();
    if (!looksLikeRepo || pendingRef.current !== null) return;
    void runAction(`url:${targetUrl}`, () => onAddUrl(targetUrl), () => {
      setUrl((current) => current.trim() === targetUrl ? "" : current);
    });
  };

  const actionsDisabled = pending !== null;

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-40"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          onClick={(event) => {
            if (event.target === event.currentTarget) closePanel();
          }}
        >
          <div aria-hidden="true" className="pointer-events-none absolute inset-0 bg-[rgba(6,4,18,0.45)]" />

          <motion.aside
            ref={panelRef}
            initial={reduce ? { opacity: 0 } : { x: 36, opacity: 0 }}
            animate={{ x: 0, opacity: 1 }}
            exit={reduce ? { opacity: 0 } : { x: 36, opacity: 0 }}
            transition={{ duration: 0.24, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong absolute top-0 right-0 flex h-full w-[420px] max-w-full flex-col rounded-l-3xl"
            role="dialog"
            aria-modal="true"
            aria-label={`Add a mod to ${profileName}`}
            tabIndex={-1}
          >
            <div className="flex min-w-0 items-center justify-between gap-3 px-5 pt-5 pb-3">
              <div className="min-w-0">
                <h2 className="text-[19px] font-semibold text-ink">Add a mod</h2>
                <p className="truncate text-[12.5px] text-ink-dim" title={profileName} aria-label={`Profile ${profileName}`}>to {profileName}</p>
              </div>
              <button type="button" onClick={closePanel} aria-label="Close add mod panel" className="ring-focus grid h-8 w-8 shrink-0 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink">
                <X size={16} weight="bold" />
              </button>
            </div>

            <div className="px-5 pb-3">
              <label className="glass flex items-center gap-2 rounded-xl px-3 py-2.5 text-ink-dim focus-within:text-ink">
                <GithubLogo size={16} className="shrink-0 opacity-75" />
                <input
                  data-autofocus
                  value={url}
                  onChange={(event) => setUrl(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      addUrl();
                    }
                  }}
                  placeholder="Paste any GitHub repo or release URL"
                  aria-label="GitHub repository or release URL"
                  className="ring-focus min-w-0 w-full bg-transparent text-[13px] text-ink placeholder:text-ink-faint focus:outline-none"
                />
                <button
                  type="button"
                  onClick={addUrl}
                  disabled={!looksLikeRepo || actionsDisabled}
                  aria-label="Add mod from GitHub URL"
                  className="ring-focus shrink-0 rounded-lg bg-white/10 px-2.5 py-1 text-[12px] font-semibold text-ink disabled:opacity-40"
                >
                  {pending?.startsWith("url:") ? "Adding…" : "Add"}
                </button>
              </label>
            </div>
            <div className="px-5 pb-3">
              <button
                type="button"
                onClick={() => void runAction("local", onAddLocal)}
                disabled={actionsDisabled}
                aria-label="Add DLL from this computer"
                className="ring-focus glass flex w-full items-center justify-center gap-2 rounded-xl px-3 py-2.5 text-[13px] font-semibold text-ink disabled:opacity-40"
              >
                <FileArrowUp size={16} />
                {pending === "local" ? "Adding local DLL…" : "Choose a .dll from this computer"}
              </button>
              <p className="mt-1.5 px-1 text-[12.5px] leading-snug text-ink-faint">Local DLLs stay on this profile and cannot be included in lobby codes.</p>
            </div>

            {actionError && <p className="mx-5 mb-3 rounded-xl bg-[rgba(226,59,59,0.12)] px-3 py-2 text-[12.5px] break-words text-[#ff8a8a]" role="alert">Could not add mod: {actionError}</p>}

            <div className="flex items-center gap-3 px-5 pb-2">
              <div className="h-px flex-1 bg-white/10" />
              <span className="text-[11px] tracking-[0.14em] text-ink-faint uppercase">Catalog</span>
              <div className="h-px flex-1 bg-white/10" />
              <button
                type="button"
                onClick={() => setManage((current) => !current)}
                disabled={actionsDisabled}
                aria-label={manage ? "Finish managing catalog" : "Manage catalog order and entries"}
                className="ring-focus shrink-0 rounded-lg px-2 py-0.5 text-[11px] font-semibold text-ink-dim hover:bg-white/10 hover:text-ink disabled:opacity-40"
              >
                {manage ? "Done" : "Manage"}
              </button>
            </div>

            <div className="px-5 pb-3">
              <label className="glass flex items-center gap-2 rounded-xl px-3 py-2 text-ink-dim focus-within:text-ink">
                <MagnifyingGlass size={15} className="shrink-0 opacity-70" />
                <input
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Search the catalog"
                  aria-label="Search catalog"
                  className="ring-focus min-w-0 w-full bg-transparent text-[13px] text-ink placeholder:text-ink-faint focus:outline-none"
                />
              </label>
            </div>

            <div className="scroll-region flex flex-1 flex-col gap-2 overflow-y-auto px-5 pb-5" aria-busy={actionsDisabled}>
              {(manage ? catalog : results).map((item, index, items) => {
                const identity = `${item.name}, repository ${item.repo}${item.latest ? `, latest version ${item.latest}` : ""}`;
                const moveUpKey = `move:${item.id}:up`;
                const moveDownKey = `move:${item.id}:down`;
                const removeKey = `remove:${item.id}`;
                const isInstalled = installed.has(item.id.toLowerCase()) || installed.has(item.repo.toLowerCase());
                const isSelected = selected.has(item.id);
                const supportsMaps = item.id.toLowerCase() === "digiworm0/levelimposter";
                return (
                  <div key={item.id} className="glass min-w-0 shrink-0 overflow-hidden rounded-2xl p-3.5" aria-label={identity}>
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="min-w-0 flex-1 truncate text-[14.5px] font-semibold text-ink" title={item.name}>{item.name}</span>
                      {item.tags.length > 0 && <Pill tag={primaryTag(item.tags)} />}
                      <TrustBadge trust={item.trust ?? "flagged"} />
                      {item.latest && <span className="max-w-24 shrink-0 truncate font-mono text-[12px] text-ink-faint" title={item.latest} aria-label={`Latest version ${item.latest}`}>{item.latest}</span>}
                    </div>
                    <p className="mt-1.5 line-clamp-3 text-[12.5px] leading-snug break-words text-ink-dim" title={item.summary || item.repo}>{item.summary || item.repo}</p>
                    {!!item.included?.length && (
                      <p className="mt-2 text-[11.5px] leading-snug text-[#cbbcff]">
                        Complete ZIP includes {item.included.join(", ")}. These components install and update together.
                      </p>
                    )}
                    <div className="mt-3 flex min-w-0 items-center justify-between gap-2">
                      <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] text-ink-faint" title={item.repo} aria-label={`Repository ${item.repo}`}>{item.repo}</span>
                      {manage ? (
                        <div className="flex shrink-0 items-center gap-1">
                          <button
                            type="button"
                            onClick={() => void runAction(moveUpKey, () => onMoveCatalog(item.id, "up"))}
                            disabled={actionsDisabled || index === 0}
                            aria-label={`Move ${item.name} up`}
                            className="ring-focus grid h-7 w-7 place-items-center rounded-lg text-ink-dim hover:bg-white/10 hover:text-ink disabled:opacity-30"
                          >
                            <CaretUp size={14} weight="bold" />
                          </button>
                          <button
                            type="button"
                            onClick={() => void runAction(moveDownKey, () => onMoveCatalog(item.id, "down"))}
                            disabled={actionsDisabled || index === items.length - 1}
                            aria-label={`Move ${item.name} down`}
                            className="ring-focus grid h-7 w-7 place-items-center rounded-lg text-ink-dim hover:bg-white/10 hover:text-ink disabled:opacity-30"
                          >
                            <CaretDown size={14} weight="bold" />
                          </button>
                          <button
                            type="button"
                            onClick={() => void runAction(removeKey, () => onRemoveCatalog(item.id))}
                            disabled={actionsDisabled}
                            aria-label={`Remove ${item.name} from catalog`}
                            className="ring-focus grid h-7 w-7 place-items-center rounded-lg text-[#ff8a8a] hover:bg-[rgba(226,59,59,0.15)] disabled:opacity-30"
                          >
                            <TrashSimple size={14} />
                          </button>
                        </div>
                      ) : (
                        <div className="flex shrink-0 items-center gap-1.5">
                          {supportsMaps && (
                            <button
                              type="button"
                              onClick={onBrowseMaps}
                              disabled={actionsDisabled}
                              aria-label="Browse LevelImposter maps"
                              className="ring-focus glass flex items-center gap-1 rounded-lg px-2.5 py-1.5 text-[12.5px] font-semibold text-ink disabled:opacity-50"
                            >
                              <MapTrifold size={13} /> Maps
                            </button>
                          )}
                          <button
                            type="button"
                            onClick={() => onToggleCatalog(item.id)}
                            disabled={actionsDisabled || isInstalled}
                            aria-pressed={isSelected}
                            aria-label={
                              isInstalled
                                ? `${item.name} is already installed`
                                : isSelected
                                  ? `Remove ${item.name} from selection`
                                  : `Select ${item.name} for ${profileName}`
                            }
                            className={`ring-focus flex min-w-[78px] items-center justify-center gap-1 rounded-lg px-3 py-1.5 text-[12.5px] font-semibold transition-transform active:scale-[0.96] disabled:opacity-50 ${
                              isSelected ? "glass text-ink" : "accent-grad text-[#0d0820]"
                            }`}
                          >
                            {isInstalled ? (
                              "Installed"
                            ) : isSelected ? (
                              <><Check size={13} weight="bold" /> Selected</>
                            ) : (
                              <><Plus size={13} weight="bold" /> Select</>
                            )}
                          </button>
                        </div>
                      )}
                    </div>
                  </div>
                );
              })}
              {!manage && results.length === 0 && <p className="px-1 py-6 text-center text-[13px] text-ink-faint">No catalog match. Paste the GitHub URL above to add it anyway.</p>}
              {manage && catalog.length === 0 && <p className="px-1 py-6 text-center text-[13px] text-ink-faint">Your catalog is empty. Paste a GitHub URL above to add a mod.</p>}
            </div>
            {!manage && (
              <div className="border-t border-white/10 px-5 py-4">
                <div className="flex items-center justify-between gap-3">
                  <p className="text-[12.5px] text-ink-dim">
                    {selectedIds.length === 0
                      ? "Select one or more mods to continue"
                      : `${selectedIds.length} mod${selectedIds.length === 1 ? "" : "s"} selected`}
                  </p>
                  <button
                    type="button"
                    onClick={onReview}
                    disabled={actionsDisabled || selectedIds.length === 0}
                    className="ring-focus accent-grad rounded-xl px-4 py-2.5 text-[13.5px] font-bold text-[#0d0820] disabled:opacity-40"
                  >
                    Review selection
                  </button>
                </div>
              </div>
            )}
          </motion.aside>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
