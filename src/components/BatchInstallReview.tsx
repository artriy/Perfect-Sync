import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { ArrowLeft, DownloadSimple, X } from "@phosphor-icons/react";
import { listInstallOptions } from "../lib/bridge";
import type { CatalogItem, ModInstallOption, ModInstallSelection } from "../lib/types";
import { useModalFocus } from "../lib/useModalFocus";
import { TrustBadge } from "./TrustBadge";

interface BatchInstallReviewProps {
  open: boolean;
  profileId: string;
  profileName: string;
  items: CatalogItem[];
  catalog: CatalogItem[];
  busy: boolean;
  onBack: () => void;
  onClose: () => void;
  onInstall: (selections: ModInstallSelection[]) => Promise<void>;
}

interface OptionState {
  loading: boolean;
  options: ModInstallOption[];
  error?: string;
}

interface ReviewRow {
  item: CatalogItem;
  managed: boolean;
  rootName: string;
  depth: number;
}

function buildReviewRows(roots: CatalogItem[], catalog: CatalogItem[]): ReviewRow[] {
  const byId = new Map(catalog.map((item) => [item.id.toLowerCase(), item]));
  const rootIds = new Set(roots.map((item) => item.id.toLowerCase()));
  const addedDependencies = new Set<string>();
  const rows: ReviewRow[] = [];
  for (const root of roots) {
    rows.push({ item: root, managed: false, rootName: root.name, depth: 0 });
    const visit = (dependencyId: string, depth: number) => {
      const folded = dependencyId.toLowerCase();
      if (rootIds.has(folded) || addedDependencies.has(folded)) return;
      const dependency = byId.get(folded);
      if (!dependency) return;
      addedDependencies.add(folded);
      rows.push({ item: dependency, managed: true, rootName: root.name, depth });
      for (const nested of dependency.dependencies ?? []) visit(nested, depth + 1);
    };
    for (const dependency of root.dependencies ?? []) visit(dependency, 1);
  }
  return rows;
}

function formatSize(bytes: number): string {
  if (bytes <= 0) return "size unknown";
  if (bytes < 1048576) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / 1048576).toFixed(1)} MB`;
}

export function BatchInstallReview({
  open,
  profileId,
  profileName,
  items,
  catalog,
  busy,
  onBack,
  onClose,
  onInstall,
}: BatchInstallReviewProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const sessionRef = useRef(0);
  const pendingRef = useRef(false);
  const rows = useMemo(() => buildReviewRows(items, catalog), [catalog, items]);
  const [states, setStates] = useState<Record<string, OptionState>>({});
  const [chosenVersion, setChosenVersion] = useState<Record<string, string>>({});
  const [chosenAsset, setChosenAsset] = useState<Record<string, string>>({});
  const [included, setIncluded] = useState<Record<string, boolean>>({});
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const controlsBusy = busy || pending;
  const close = useCallback(() => {
    if (!pendingRef.current && !busy) onClose();
  }, [busy, onClose]);
  useModalFocus(open, modalRef, close);

  useEffect(() => {
    const session = ++sessionRef.current;
    setError(null);
    setPending(false);
    pendingRef.current = false;
    if (!open) return;

    const loadingStates: Record<string, OptionState> = {};
    const nextIncluded: Record<string, boolean> = {};
    for (const row of rows) {
      loadingStates[row.item.id] = { loading: true, options: [] };
      if (row.managed) nextIncluded[row.item.id] = true;
    }
    setStates(loadingStates);
    setChosenVersion({});
    setChosenAsset({});
    setIncluded(nextIncluded);

    void Promise.all(
      rows.map(async ({ item }) => {
        try {
          const options = await listInstallOptions(item.repo, profileId);
          if (options.length === 0) throw new Error("No direct .dll release asset was found.");
          return { id: item.id, options };
        } catch (reason: unknown) {
          return { id: item.id, options: [], error: String(reason) };
        }
      }),
    ).then((results) => {
      if (sessionRef.current !== session) return;
      const nextStates: Record<string, OptionState> = {};
      const nextVersions: Record<string, string> = {};
      const nextAssets: Record<string, string> = {};
      for (const result of results) {
        nextStates[result.id] = { loading: false, options: result.options, error: result.error };
        if (result.options[0]) {
          nextVersions[result.id] = result.options[0].tag;
          nextAssets[result.id] = result.options[0].assetName;
        }
      }
      setStates(nextStates);
      setChosenVersion(nextVersions);
      setChosenAsset(nextAssets);
    });
  }, [open, profileId, rows]);

  const activeRows = rows.filter((row) => !row.managed || included[row.item.id]);
  const ready =
    items.length > 0 &&
    activeRows.every(({ item }) => {
      const state = states[item.id];
      return !!state && !state.loading && !state.error && state.options.some((option) =>
        option.tag === chosenVersion[item.id] && option.assetName === chosenAsset[item.id]
      );
    });

  const install = async () => {
    if (!ready || controlsBusy || pendingRef.current) return;
    const selections = activeRows.map(({ item, managed }) => {
      const option = states[item.id].options.find((candidate) =>
        candidate.tag === chosenVersion[item.id] && candidate.assetName === chosenAsset[item.id]
      );
      if (!option) throw new Error(`Choose a version and DLL for ${item.name}.`);
      return {
        id: item.id,
        repo: item.repo,
        name: item.name,
        tag: option.tag,
        assetName: option.assetName,
        managed,
      };
    });
    pendingRef.current = true;
    setPending(true);
    setError(null);
    try {
      await onInstall(selections);
    } catch (reason: unknown) {
      if (sessionRef.current) setError(String(reason));
    } finally {
      pendingRef.current = false;
      setPending(false);
    }
  };

  const dependencyCount = activeRows.filter((row) => row.managed).length;
  return (
    <AnimatePresence>
      {open && (
        <motion.div className="fixed inset-0 z-50 grid place-items-center p-4 sm:p-6" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
          <div className="absolute inset-0 bg-[rgba(6,4,18,0.55)]" style={{ backdropFilter: "blur(2px)" }} onClick={!controlsBusy ? close : undefined} />
          <motion.div
            ref={modalRef}
            role="dialog"
            aria-modal="true"
            aria-label={`Review mods for ${profileName}`}
            tabIndex={-1}
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.98, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex max-h-[88vh] w-[720px] max-w-full flex-col rounded-3xl p-5 sm:p-6"
          >
            <button type="button" onClick={close} disabled={controlsBusy} aria-label="Close install review" className="ring-focus absolute top-4 right-4 grid h-8 w-8 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink disabled:opacity-40">
              <X size={16} weight="bold" />
            </button>
            <h2 className="pr-10 text-[20px] font-semibold text-ink">Review your mods</h2>
            <p className="mt-1 text-[13px] text-ink-dim">The latest version and catalog DLL are selected by default. Change either one or exclude automatic dependencies before installing to {profileName}. ZIP assets are never installed.</p>

            {error && <p className="mt-3 rounded-xl bg-[rgba(226,59,59,0.12)] px-3.5 py-2.5 text-[13px] break-words text-[#ff8a8a]" role="alert">Install failed: {error}</p>}

            <div className="scroll-region mt-4 flex-1 space-y-2.5 overflow-y-auto pr-1" aria-busy={rows.some((row) => states[row.item.id]?.loading)}>
              {rows.map((row) => {
                const { item } = row;
                const state = states[item.id] ?? { loading: true, options: [] };
                const versions = Array.from(new Set(state.options.map((candidate) => candidate.tag)));
                const assets = state.options.filter((candidate) => candidate.tag === chosenVersion[item.id]);
                const option = assets.find((candidate) => candidate.assetName === chosenAsset[item.id]);
                const enabled = !row.managed || included[item.id];
                return (
                  <div
                    key={item.id}
                    className={`glass min-w-0 rounded-2xl p-3.5 ${row.managed ? "border-l-2 border-l-accent/35" : ""}`}
                    style={row.managed ? { marginLeft: `${Math.min(row.depth, 2) * 18}px` } : undefined}
                  >
                    <div className="flex min-w-0 items-center gap-2">
                      {row.managed && (
                        <label className="ring-focus flex shrink-0 cursor-pointer items-center gap-2 rounded-lg px-1.5 py-1 text-[12px] text-ink-dim">
                          <input
                            type="checkbox"
                            checked={enabled}
                            disabled={controlsBusy}
                            onChange={(event) => setIncluded((current) => ({ ...current, [item.id]: event.target.checked }))}
                            aria-label={`Auto include ${item.name}`}
                            className="accent-[#9b7bff]"
                          />
                          Auto include
                        </label>
                      )}
                      <span className={`min-w-0 flex-1 truncate text-[14.5px] font-semibold ${enabled ? "text-ink" : "text-ink-faint"}`} title={item.name}>{item.name}</span>
                      <TrustBadge trust={item.trust ?? "flagged"} compact />
                    </div>
                    <p className="mt-1 truncate font-mono text-[11.5px] text-ink-faint" title={item.repo}>{item.repo}</p>
                    {row.managed && <p className="mt-1 text-[11.5px] text-ink-faint">Dependency for {row.rootName}. Excluding it may prevent that mod from loading.</p>}
                    {state.loading ? (
                      <p className="mt-3 text-[12.5px] text-ink-dim" role="status">Finding compatible versions…</p>
                    ) : state.error ? (
                      <p className="mt-3 text-[12.5px] break-words text-[#ff8a8a]" role="alert">{state.error}</p>
                    ) : (
                      <div className="mt-3 grid min-w-0 gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.45fr)]">
                        <label className="min-w-0">
                          <span className="mb-1 block text-[10.5px] tracking-[0.12em] text-ink-faint uppercase">Version</span>
                          <select
                            value={chosenVersion[item.id] ?? ""}
                            onChange={(event) => {
                              const version = event.target.value;
                              const firstAsset = state.options.find((candidate) => candidate.tag === version);
                              setChosenVersion((current) => ({ ...current, [item.id]: version }));
                              setChosenAsset((current) => ({ ...current, [item.id]: firstAsset?.assetName ?? "" }));
                            }}
                            disabled={controlsBusy || !enabled}
                            aria-label={`${item.name} version`}
                            className="ring-focus glass w-full min-w-0 rounded-xl px-3 py-2 text-[12.5px] text-ink disabled:opacity-50"
                          >
                            {versions.map((version, index) => <option key={version} value={version}>{version}{index === 0 ? " (latest)" : ""}</option>)}
                          </select>
                        </label>
                        <label className="min-w-0">
                          <span className="mb-1 block text-[10.5px] tracking-[0.12em] text-ink-faint uppercase">DLL file</span>
                          <select
                            value={chosenAsset[item.id] ?? ""}
                            onChange={(event) => setChosenAsset((current) => ({ ...current, [item.id]: event.target.value }))}
                            disabled={controlsBusy || !enabled}
                            aria-label={`${item.name} DLL file`}
                            className="ring-focus glass w-full min-w-0 rounded-xl px-3 py-2 font-mono text-[11.5px] text-ink disabled:opacity-50"
                          >
                            {assets.map((candidate) => (
                              <option key={candidate.assetName} value={candidate.assetName}>{candidate.assetName} · {formatSize(candidate.size)}</option>
                            ))}
                          </select>
                          {option && <span className="sr-only">Selected DLL size {formatSize(option.size)}</span>}
                        </label>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>

            <div className="mt-5 flex flex-wrap items-center justify-between gap-3 border-t border-white/10 pt-4">
              <button type="button" onClick={onBack} disabled={controlsBusy} className="ring-focus glass flex items-center gap-1.5 rounded-xl px-4 py-2.5 text-[13.5px] text-ink disabled:opacity-50">
                <ArrowLeft size={15} /> Back
              </button>
              <button type="button" onClick={() => void install()} disabled={!ready || controlsBusy} className="ring-focus accent-grad flex items-center gap-2 rounded-xl px-5 py-2.5 text-[13.5px] font-bold text-[#0d0820] disabled:opacity-40">
                <DownloadSimple size={16} weight="bold" /> {pending ? "Installing…" : `Install ${items.length} mod${items.length === 1 ? "" : "s"}${dependencyCount ? ` + ${dependencyCount} dependenc${dependencyCount === 1 ? "y" : "ies"}` : ""}`}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
