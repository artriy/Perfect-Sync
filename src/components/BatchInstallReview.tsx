import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { ArrowLeft, DownloadSimple, Warning, X } from "@phosphor-icons/react";
import { listInstallOptions } from "../lib/bridge";
import type { CatalogItem, ModInstallOption, ModInstallSelection, ProfileMod } from "../lib/types";
import { useModalFocus } from "../lib/useModalFocus";
import { TrustBadge } from "./TrustBadge";

interface BatchInstallReviewProps {
  open: boolean;
  profileId: string;
  profileName: string;
  items: CatalogItem[];
  catalog: CatalogItem[];
  installedMods: ProfileMod[];
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
  requirements: string[];
}

function parseStableVersion(value: string): [number, number, number] | null {
  const match = /^v?(\d+)\.(\d+)\.(\d+)(?:\+[0-9A-Za-z.-]+)?$/u.exec(value.trim());
  return match ? [Number(match[1]), Number(match[2]), Number(match[3])] : null;
}

function compareVersions(left: [number, number, number], right: [number, number, number]): number {
  for (let index = 0; index < left.length; index += 1) {
    const difference = left[index] - right[index];
    if (difference) return difference;
  }
  return 0;
}

function satisfiesRequirement(version: string, requirement: string): boolean {
  const current = parseStableVersion(version);
  if (!current) return false;
  return requirement.split(",").every((part) => {
    const match = /^(<=|>=|<|>|=)?\s*v?(\d+)\.(\d+)\.(\d+)$/u.exec(part.trim());
    if (!match) return false;
    const comparison = compareVersions(current, [Number(match[2]), Number(match[3]), Number(match[4])]);
    switch (match[1] ?? "=") {
      case "<": return comparison < 0;
      case "<=": return comparison <= 0;
      case ">": return comparison > 0;
      case ">=": return comparison >= 0;
      default: return comparison === 0;
    }
  });
}

function directRequirement(owner: CatalogItem, dependencyId: string): string | undefined {
  return Object.entries(owner.dependencyVersions ?? {}).find(
    ([id]) => id.toLowerCase() === dependencyId.toLowerCase(),
  )?.[1];
}

function buildInstalledIndex(installedMods: readonly ProfileMod[]): Map<string, ProfileMod> {
  const installed = new Map<string, ProfileMod>();
  for (const mod of installedMods) {
    installed.set(mod.packageId.toLowerCase(), mod);
    if (mod.repo) installed.set(mod.repo.toLowerCase(), mod);
  }
  return installed;
}

function buildReviewRows(
  roots: CatalogItem[],
  catalog: CatalogItem[],
  installedMods: readonly ProfileMod[],
): ReviewRow[] {
  const byId = new Map(catalog.map((item) => [item.id.toLowerCase(), item]));
  const byIdentity = new Map<string, CatalogItem>();
  for (const item of catalog) {
    byIdentity.set(item.id.toLowerCase(), item);
    byIdentity.set(item.repo.toLowerCase(), item);
  }
  const provided = new Set<string>();
  for (const provider of roots) {
    for (const dependency of provider.provides ?? []) provided.add(dependency.toLowerCase());
  }
  for (const mod of installedMods) {
    if (!mod.enabled) continue;
    const provider =
      byIdentity.get(mod.packageId.toLowerCase()) ??
      (mod.repo ? byIdentity.get(mod.repo.toLowerCase()) : undefined);
    for (const dependency of provider?.provides ?? []) provided.add(dependency.toLowerCase());
  }
  const rootIds = new Set(roots.map((item) => item.id.toLowerCase()));
  const installed = buildInstalledIndex(installedMods);
  const dependencyRows = new Map<string, ReviewRow>();
  const visited = new Set<string>();
  const rows: ReviewRow[] = [];

  const visit = (owner: CatalogItem, dependencyId: string, depth: number, rootName: string) => {
    const folded = dependencyId.toLowerCase();
    const dependency = byId.get(folded);
    if (!dependency) return;
    const requirement = directRequirement(owner, dependency.id);
    const visitKey = `${owner.id.toLowerCase()}\0${folded}\0${requirement ?? ""}`;
    if (visited.has(visitKey)) return;
    visited.add(visitKey);

    const current = installed.get(folded) ?? installed.get(dependency.repo.toLowerCase());
    const satisfied =
      !!current &&
      current.enabled &&
      (!requirement || satisfiesRequirement(current.version, requirement));
    if (!rootIds.has(folded) && !provided.has(folded) && !satisfied) {
      let row = dependencyRows.get(folded);
      if (!row) {
        row = { item: dependency, managed: true, rootName, depth, requirements: [] };
        dependencyRows.set(folded, row);
        rows.push(row);
      }
      if (requirement && !row.requirements.includes(requirement)) row.requirements.push(requirement);
    }

    for (const nested of dependency.dependencies ?? []) {
      visit(dependency, nested, depth + 1, rootName);
    }
  };

  for (const root of roots) {
    rows.push({ item: root, managed: false, rootName: root.name, depth: 0, requirements: [] });
    for (const dependency of root.dependencies ?? []) visit(root, dependency, 1, root.name);
  }
  return rows;
}

function missingRecommendations(
  roots: readonly CatalogItem[],
  catalog: readonly CatalogItem[],
  installedMods: readonly ProfileMod[],
): Array<{ item: CatalogItem; requiredBy: string[] }> {
  const available = new Set(roots.flatMap((item) => [item.id.toLowerCase(), item.repo.toLowerCase()]));
  for (const mod of installedMods) {
    available.add(mod.packageId.toLowerCase());
    if (mod.repo) available.add(mod.repo.toLowerCase());
  }
  const byId = new Map(catalog.map((item) => [item.id.toLowerCase(), item]));
  const missing = new Map<string, { item: CatalogItem; requiredBy: string[] }>();
  for (const root of roots) {
    for (const id of root.recommendedDependencies ?? []) {
      const folded = id.toLowerCase();
      if (available.has(folded)) continue;
      const item = byId.get(folded);
      if (!item) continue;
      const current = missing.get(folded) ?? { item, requiredBy: [] };
      if (!current.requiredBy.includes(root.name)) current.requiredBy.push(root.name);
      missing.set(folded, current);
    }
  }
  return [...missing.values()];
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
  installedMods,
  busy,
  onBack,
  onClose,
  onInstall,
}: BatchInstallReviewProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const sessionRef = useRef(0);
  const pendingRef = useRef(false);
  const rows = useMemo(
    () => buildReviewRows(items, catalog, installedMods),
    [catalog, installedMods, items],
  );
  const recommendations = useMemo(
    () => missingRecommendations(items, catalog, installedMods),
    [catalog, installedMods, items],
  );
  const [states, setStates] = useState<Record<string, OptionState>>({});
  const [chosenVersion, setChosenVersion] = useState<Record<string, string>>({});
  const [chosenAsset, setChosenAsset] = useState<Record<string, string>>({});
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
    for (const row of rows) {
      loadingStates[row.item.id] = { loading: true, options: [] };
    }
    setStates(loadingStates);
    setChosenVersion({});
    setChosenAsset({});

    void Promise.all(
      rows.map(async ({ item }) => {
        try {
          const options = await listInstallOptions(item.repo, profileId);
          if (options.length === 0) throw new Error("No compatible release asset was found.");
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
      const rowsById = new Map(rows.map((row) => [row.item.id, row]));
      for (const result of results) {
        const row = rowsById.get(result.id);
        const compatible = result.options.find(
          (option) =>
            !row ||
            row.requirements.every((requirement) => satisfiesRequirement(option.tag, requirement)),
        );
        const compatibilityError =
          !result.error && result.options.length > 0 && !compatible && row?.requirements.length
            ? `No release of ${row.item.name} satisfies ${row.requirements.join(" and ")}.`
            : undefined;
        nextStates[result.id] = {
          loading: false,
          options: result.options,
          error: result.error ?? compatibilityError,
        };
        if (compatible) {
          nextVersions[result.id] = compatible.tag;
          nextAssets[result.id] = compatible.assetName;
        }
      }
      setStates(nextStates);
      setChosenVersion(nextVersions);
      setChosenAsset(nextAssets);
    });
  }, [open, profileId, rows]);

  const ready =
    items.length > 0 &&
    rows.every(({ item }) => {
      const state = states[item.id];
      return !!state && !state.loading && !state.error && state.options.some((option) =>
        option.tag === chosenVersion[item.id] && option.assetName === chosenAsset[item.id]
      );
    });

  const install = async () => {
    if (!ready || controlsBusy || pendingRef.current) return;
    const selections = rows.map(({ item, managed }) => {
      const option = states[item.id].options.find((candidate) =>
        candidate.tag === chosenVersion[item.id] && candidate.assetName === chosenAsset[item.id]
      );
      if (!option) throw new Error(`Choose a version and mod file for ${item.name}.`);
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

  const dependencyCount = rows.filter((row) => row.managed).length;
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
            <p className="mt-1 text-[13px] text-ink-dim">The latest compatible release and catalog file are selected by default. Required dependencies are added automatically. Complete bundle packages install their loader, dependencies, configs, and assets from one release ZIP.</p>

            {error && <p className="mt-3 rounded-xl bg-[rgba(226,59,59,0.12)] px-3.5 py-2.5 text-[13px] break-words text-[#ff8a8a]" role="alert">Install failed: {error}</p>}

            {recommendations.length > 0 && (
              <div className="mt-3 flex items-start gap-3 rounded-xl border border-[#ffd23f]/25 bg-[#ffd23f]/8 px-3.5 py-3" role="status">
                <Warning size={18} weight="fill" className="mt-0.5 shrink-0 text-crew-gold" aria-hidden="true" />
                <div className="min-w-0">
                  <p className="text-[12.5px] font-semibold text-ink">Recommended main mod not selected</p>
                  {recommendations.map(({ item, requiredBy }) => (
                    <p key={item.id} className="mt-1 text-[12px] leading-relaxed text-ink-dim">
                      <span className="font-semibold text-ink">{item.name}</span> is recommended for{" "}
                      {requiredBy.join(", ")}, but it is not installed or selected. Go back to add it,
                      or continue without it; Perfect Sync will not add it automatically.
                    </p>
                  ))}
                </div>
              </div>
            )}

            <div className="scroll-region mt-4 flex-1 space-y-2.5 overflow-y-auto pr-1" aria-busy={rows.some((row) => states[row.item.id]?.loading)}>
              {rows.map((row) => {
                const { item } = row;
                const state = states[item.id] ?? { loading: true, options: [] };
                const availableOptions = row.requirements.length
                  ? state.options.filter((candidate) =>
                      row.requirements.every((requirement) =>
                        satisfiesRequirement(candidate.tag, requirement),
                      ),
                    )
                  : state.options;
                const versions = Array.from(new Set(availableOptions.map((candidate) => candidate.tag)));
                const assets = availableOptions.filter(
                  (candidate) => candidate.tag === chosenVersion[item.id],
                );
                const option = assets.find((candidate) => candidate.assetName === chosenAsset[item.id]);
                return (
                  <div
                    key={item.id}
                    className={`surface-row min-w-0 rounded-2xl p-3.5 ${row.managed ? "bg-accent/[0.035]" : ""}`}
                    style={row.managed ? { marginLeft: `${Math.min(row.depth, 2) * 18}px` } : undefined}
                  >
                    <div className="flex min-w-0 items-center gap-2">
                      {row.managed && (
                        <span className="shrink-0 rounded-lg bg-accent/10 px-2 py-1 text-[11.5px] font-semibold text-[#d4c6ff]">
                          Auto-added
                        </span>
                      )}
                      <span className="min-w-0 flex-1 truncate text-[14.5px] font-semibold text-ink" title={item.name}>{item.name}</span>
                      <TrustBadge trust={item.trust ?? "flagged"} compact />
                    </div>
                    <p className="mt-1 truncate font-mono text-[11.5px] text-ink-faint" title={item.repo}>{item.repo}</p>
                    {row.managed && <p className="mt-1 text-[11.5px] text-ink-faint">Required by {row.rootName}. You can remove it from the profile after installation.</p>}
                    {!row.managed && !!item.included?.length && (
                      <p className="mt-1 text-[11.5px] text-[#cbbcff]">
                        This ZIP owns {item.included.join(", ")}; they install and update as one package.
                      </p>
                    )}
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
                            disabled={controlsBusy}
                            aria-label={`${item.name} version`}
                            className="ring-focus glass w-full min-w-0 rounded-xl px-3 py-2 text-[12.5px] text-ink disabled:opacity-50"
                          >
                            {versions.map((version, index) => <option key={version} value={version}>{version}{index === 0 ? " (latest)" : ""}</option>)}
                          </select>
                        </label>
                        <label className="min-w-0">
                          <span className="mb-1 block text-[10.5px] tracking-[0.12em] text-ink-faint uppercase">Mod file</span>
                          <select
                            value={chosenAsset[item.id] ?? ""}
                            onChange={(event) => setChosenAsset((current) => ({ ...current, [item.id]: event.target.value }))}
                            disabled={controlsBusy}
                            aria-label={`${item.name} mod file`}
                            className="ring-focus glass w-full min-w-0 rounded-xl px-3 py-2 font-mono text-[11.5px] text-ink disabled:opacity-50"
                          >
                            {assets.map((candidate) => (
                              <option key={candidate.assetName} value={candidate.assetName}>{candidate.assetName} · {formatSize(candidate.size)}</option>
                            ))}
                          </select>
                          {option && <span className="sr-only">Selected file size {formatSize(option.size)}</span>}
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
