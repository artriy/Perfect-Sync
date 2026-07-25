import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Check, DownloadSimple, MagnifyingGlass, MapTrifold, TrashSimple, X } from "@phosphor-icons/react";
import { fetchLevelImposterBanner, listLevelImposterMaps, searchLevelImposterMaps } from "../lib/bridge";
import type { LevelImposterMap } from "../lib/types";
import { useModalFocus } from "../lib/useModalFocus";

interface MapBrowserPanelProps {
  open: boolean;
  profileId: string;
  profileName: string;
  levelImposterInstalled: boolean;
  busy: boolean;
  onClose: () => void;
  onInstall: (maps: LevelImposterMap[]) => Promise<void>;
  onRemove: (mapIds: string[]) => Promise<void>;
}

const MAX_SELECTION = 32;

export function MapBrowserPanel({
  open,
  profileId,
  profileName,
  levelImposterInstalled,
  busy,
  onClose,
  onInstall,
  onRemove,
}: MapBrowserPanelProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const sessionRef = useRef(0);
  const pendingRef = useRef(false);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<LevelImposterMap[]>([]);
  const [installedIds, setInstalledIds] = useState<string[]>([]);
  const [selectedMaps, setSelectedMaps] = useState<LevelImposterMap[]>([]);
  const [loading, setLoading] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const controlsBusy = busy || pending;
  const installed = new Set(installedIds);
  const selected = new Set(selectedMaps.map((map) => map.id));
  const close = useCallback(() => {
    if (!pendingRef.current && !busy) onClose();
  }, [busy, onClose]);
  useModalFocus(open, modalRef, close);

  useEffect(() => {
    const session = ++sessionRef.current;
    setQuery("");
    setResults([]);
    setSelectedMaps([]);
    setInstalledIds([]);
    setError(null);
    setPending(false);
    pendingRef.current = false;
    if (!open) return;
    void listLevelImposterMaps(profileId)
      .then((ids) => {
        if (sessionRef.current === session) setInstalledIds(ids);
      })
      .catch((reason: unknown) => {
        if (sessionRef.current === session) setError(`Could not read installed maps: ${String(reason)}`);
      });
  }, [open, profileId]);

  useEffect(() => {
    if (!open) return;
    const session = sessionRef.current;
    setLoading(true);
    setError(null);
    const timeout = window.setTimeout(() => {
      void searchLevelImposterMaps(query)
        .then((maps) => {
          if (sessionRef.current === session) setResults(maps);
        })
        .catch((reason: unknown) => {
          if (sessionRef.current === session) {
            setResults([]);
            setError(`Could not search maps: ${String(reason)}`);
          }
        })
        .finally(() => {
          if (sessionRef.current === session) setLoading(false);
        });
    }, query.trim() ? 300 : 0);
    return () => window.clearTimeout(timeout);
  }, [open, query]);

  const toggle = (map: LevelImposterMap) => {
    if (installed.has(map.id)) return;
    setSelectedMaps((current) => {
      if (current.some((selectedMap) => selectedMap.id === map.id)) {
        return current.filter((selectedMap) => selectedMap.id !== map.id);
      }
      if (current.length >= MAX_SELECTION) return current;
      return [...current, map];
    });
  };
  const toggleVisible = () => {
    const selectable = results.filter((map) => !installed.has(map.id));
    const selectedVisible = selectable.filter((map) => selected.has(map.id));
    const clearVisible =
      selectedVisible.length > 0 &&
      (selectedVisible.length === selectable.length || selectedMaps.length >= MAX_SELECTION);
    if (clearVisible) {
      const visible = new Set(selectable.map((map) => map.id));
      setSelectedMaps((current) => current.filter((map) => !visible.has(map.id)));
      return;
    }
    setSelectedMaps((current) => {
      const next = [...current];
      for (const map of selectable) {
        if (!next.some((selectedMap) => selectedMap.id === map.id) && next.length < MAX_SELECTION) {
          next.push(map);
        }
      }
      return next;
    });
  };

  const install = async () => {
    if (selectedMaps.length === 0 || controlsBusy || pendingRef.current) return;
    pendingRef.current = true;
    setPending(true);
    setError(null);
    try {
      await onInstall(selectedMaps);
    } catch (reason: unknown) {
      setError(`Map install failed: ${String(reason)}`);
    } finally {
      pendingRef.current = false;
      setPending(false);
    }
  };

  const removeInstalled = async (id: string) => {
    if (controlsBusy || pendingRef.current) return;
    pendingRef.current = true;
    setPending(true);
    setError(null);
    try {
      await onRemove([id]);
      setInstalledIds((current) => current.filter((installedId) => installedId !== id));
      setSelectedMaps((current) => current.filter((map) => map.id !== id));
    } catch (reason: unknown) {
      setError(`Map removal failed: ${String(reason)}`);
    } finally {
      pendingRef.current = false;
      setPending(false);
    }
  };

  const selectableResults = results.filter((map) => !installed.has(map.id));
  const selectedVisibleCount = selectableResults.filter((map) => selected.has(map.id)).length;
  const allVisibleSelected =
    selectedVisibleCount > 0 &&
    (selectedVisibleCount === selectableResults.length || selectedMaps.length >= MAX_SELECTION);
  const visibleMaps = new Map(results.map((map) => [map.id, map]));

  return (
    <AnimatePresence>
      {open && (
        <motion.div className="fixed inset-0 z-50 grid place-items-center p-4 sm:p-6" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
          <div className="absolute inset-0 bg-[rgba(6,4,18,0.55)]" style={{ backdropFilter: "blur(2px)" }} onClick={!controlsBusy ? close : undefined} />
          <motion.div
            ref={modalRef}
            role="dialog"
            aria-modal="true"
            aria-label={`Browse LevelImposter maps for ${profileName}`}
            tabIndex={-1}
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.98, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex max-h-[90vh] w-[760px] max-w-full flex-col rounded-3xl p-5 sm:p-6"
          >
            <button type="button" onClick={close} disabled={controlsBusy} aria-label="Close map browser" className="ring-focus absolute top-4 right-4 grid h-8 w-8 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink disabled:opacity-40">
              <X size={16} weight="bold" />
            </button>
            <div className="flex items-center gap-2.5 pr-10">
              <MapTrifold size={22} className="shrink-0 text-[#9b7bff]" />
              <h2 className="text-[20px] font-semibold text-ink">Community maps</h2>
            </div>
            <p className="mt-1 text-[13px] text-ink-dim">Search the complete LevelImposter map index, select up to {MAX_SELECTION}, then download them together.</p>
            {!levelImposterInstalled && (
              <p className="mt-3 rounded-xl bg-[rgba(123,150,255,0.12)] px-3.5 py-2.5 text-[12.5px] text-[#cbd8ff]">LevelImposter v0.21.2-beta or the latest compatible release will be installed automatically with your maps.</p>
            )}

            <div className="mt-4 flex flex-wrap gap-2.5">
              <label className="glass flex min-w-[220px] flex-1 items-center gap-2 rounded-xl px-3 py-2.5 text-ink-dim focus-within:text-ink">
                <MagnifyingGlass size={16} className="shrink-0 opacity-70" />
                <input
                  data-autofocus
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Search any map or creator"
                  aria-label="Search LevelImposter maps"
                  className="ring-focus min-w-0 w-full bg-transparent text-[13px] text-ink placeholder:text-ink-faint focus:outline-none"
                />
              </label>
              <button type="button" onClick={toggleVisible} disabled={controlsBusy || selectableResults.length === 0} className="ring-focus glass rounded-xl px-3.5 py-2.5 text-[12.5px] font-semibold text-ink disabled:opacity-40">
                {allVisibleSelected ? "Clear results" : "Select results"}
              </button>
            </div>

            {installedIds.length > 0 && (
              <section className="mt-4 rounded-2xl border border-white/10 bg-white/[0.035] p-3.5" aria-label="Installed LevelImposter maps">
                <div className="flex items-center justify-between gap-3">
                  <h3 className="text-[12.5px] font-semibold text-ink">Installed maps</h3>
                  <span className="font-mono text-[11px] text-ink-faint">{installedIds.length}</span>
                </div>
                <div className="scroll-region mt-2.5 flex max-h-28 flex-col gap-1.5 overflow-y-auto pr-1">
                  {installedIds.map((id) => {
                    const map = visibleMaps.get(id);
                    const label = map?.name || id;
                    return (
                      <div key={id} className="glass flex min-w-0 items-center gap-2 rounded-xl px-3 py-2">
                        <span className="min-w-0 flex-1 truncate text-[12px] text-ink" title={map ? `${map.name} by ${map.authorName}` : id}>
                          {label}
                          {map?.authorName && <span className="text-ink-faint"> · {map.authorName}</span>}
                        </span>
                        <button
                          type="button"
                          onClick={() => void removeInstalled(id)}
                          disabled={controlsBusy}
                          aria-label={`Remove ${label}`}
                          className="ring-focus flex shrink-0 items-center gap-1 rounded-lg px-2 py-1 text-[11px] font-semibold text-[#ff9b9b] hover:bg-[rgba(226,59,59,0.12)] disabled:opacity-40"
                        >
                          <TrashSimple size={12} /> Remove
                        </button>
                      </div>
                    );
                  })}
                </div>
              </section>
            )}

            {error && <p className="mt-3 rounded-xl bg-[rgba(226,59,59,0.12)] px-3.5 py-2.5 text-[13px] break-words text-[#ff8a8a]" role="alert">{error}</p>}

            <div className="scroll-region mt-4 grid flex-1 auto-rows-min gap-2.5 overflow-y-auto pr-1 sm:grid-cols-2" aria-busy={loading}>
              {loading && <p className="col-span-full py-10 text-center text-[13px] text-ink-faint" role="status">Searching community maps…</p>}
              {!loading && results.map((map) => {
                const isInstalled = installed.has(map.id);
                const isSelected = selected.has(map.id);
                return (
                  <button
                    key={map.id}
                    type="button"
                    onClick={() => toggle(map)}
                    disabled={controlsBusy || isInstalled}
                    aria-pressed={isSelected}
                    aria-label={`${isInstalled ? "Installed" : isSelected ? "Selected" : "Select"} ${map.name} by ${map.authorName}`}
                    className={`ring-focus glass relative min-h-40 min-w-0 overflow-hidden rounded-2xl p-3.5 text-left disabled:opacity-55 ${isSelected ? "outline outline-1 outline-[rgba(155,123,255,0.75)]" : ""}`}
                  >
                    {map.thumbnailUrl && (
                      <img
                        src={map.thumbnailUrl}
                        alt=""
                        loading="lazy"
                        referrerPolicy="no-referrer"
                        onError={(event) => {
                          const image = event.currentTarget;
                          if (image.dataset.proxyAttempted) {
                            image.hidden = true;
                            return;
                          }
                          image.dataset.proxyAttempted = "true";
                          void fetchLevelImposterBanner(map.thumbnailUrl!)
                            .then((dataUrl) => {
                              if (image.isConnected) image.src = dataUrl;
                            })
                            .catch(() => {
                              if (image.isConnected) image.hidden = true;
                            });
                        }}
                        className="pointer-events-none absolute inset-0 h-full w-full object-cover opacity-55"
                      />
                    )}
                    <span className="pointer-events-none absolute inset-0 bg-gradient-to-t from-[#100b20] via-[rgba(16,11,32,0.72)] to-[rgba(16,11,32,0.28)]" />
                    <span className="relative flex h-full flex-col">
                      <span className="flex min-w-0 items-start gap-2.5">
                        <span className={`mt-0.5 grid h-5 w-5 shrink-0 place-items-center rounded-md border ${isSelected || isInstalled ? "border-[#9b7bff] bg-[rgba(36,24,66,0.82)] text-[#ded5ff]" : "border-white/50 bg-[rgba(16,11,32,0.45)] text-transparent"}`}>
                          <Check size={13} weight="bold" />
                        </span>
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-[14px] font-semibold text-white drop-shadow" title={map.name}>{map.name}</span>
                          <span className="mt-0.5 block truncate text-[11.5px] text-white/75 drop-shadow" title={map.authorName}>by {map.authorName || "Unknown creator"}</span>
                        </span>
                        {isInstalled && <span className="shrink-0 rounded-lg bg-[rgba(16,11,32,0.72)] px-2 py-0.5 text-[10.5px] font-semibold text-white/85">Installed</span>}
                      </span>
                      <span className="mt-auto line-clamp-3 pt-4 text-[12px] leading-snug text-white/80 drop-shadow">{map.description || "No description provided."}</span>
                    </span>
                  </button>
                );
              })}
              {!loading && results.length === 0 && !error && <p className="col-span-full py-10 text-center text-[13px] text-ink-faint">No maps matched that search.</p>}
            </div>

            <div className="mt-5 flex flex-wrap items-center justify-between gap-3 border-t border-white/10 pt-4">
              <p className="text-[12.5px] text-ink-dim">{selectedMaps.length} selected{selectedMaps.length >= MAX_SELECTION ? ` · ${MAX_SELECTION} map limit reached` : ""}</p>
              <button type="button" onClick={() => void install()} disabled={selectedMaps.length === 0 || controlsBusy} className="ring-focus accent-grad flex items-center gap-2 rounded-xl px-5 py-2.5 text-[13.5px] font-bold text-[#0d0820] disabled:opacity-40">
                <DownloadSimple size={16} weight="bold" /> {pending ? "Downloading…" : `Download ${selectedMaps.length} map${selectedMaps.length === 1 ? "" : "s"}`}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
