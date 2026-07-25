import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { DownloadSimple, FileArrowDown, Warning, X } from "@phosphor-icons/react";
import { listReleases, type GhRelease } from "../lib/bridge";
import type { Trust } from "../lib/types";
import { useModalFocus } from "../lib/useModalFocus";
import { TrustBadge } from "./TrustBadge";

interface ReleasePickerProps {
  open: boolean;
  repo: string;
  modName: string;
  trust: Trust;
  busy: boolean;
  onClose: () => void;
  onPick: (repo: string, tag: string, assetName: string) => void | Promise<void>;
}

interface ReleaseResult {
  repo: string;
  releases: GhRelease[];
}

interface AssetChoice {
  repo: string;
  tag: string;
  assetName: string;
}

function formatSize(bytes: number): string {
  if (bytes <= 0) return "size unknown";
  if (bytes < 1048576) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / 1048576).toFixed(1)} MB`;
}


const trustDescription: Record<Trust, string> = {
  trusted: "Trusted catalog repository",
  community: "Community-listed repository",
  flagged: "Unverified repository. Confirm the exact asset before installing.",
};

export function ReleasePicker({ open, repo, modName, trust, busy, onClose, onPick }: ReleasePickerProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const openRef = useRef(open);
  const currentRepoRef = useRef(repo);
  const sessionRef = useRef(0);
  const requestRef = useRef(0);
  const pickingRef = useRef<string | null>(null);
  const [result, setResult] = useState<ReleaseResult | null>(null);
  const [loadingRepo, setLoadingRepo] = useState<string | null>(null);
  const [error, setError] = useState<{ repo: string; message: string } | null>(null);
  const [confirmation, setConfirmation] = useState<AssetChoice | null>(null);
  const [picking, setPicking] = useState<string | null>(null);
  const [pickError, setPickError] = useState<string | null>(null);
  const confirmChoice = confirmation?.repo === repo ? confirmation : null;

  openRef.current = open;
  currentRepoRef.current = repo;

  const closePicker = useCallback(() => {
    if (pickingRef.current !== null) return;
    sessionRef.current += 1;
    requestRef.current += 1;
    setConfirmation(null);
    onClose();
  }, [onClose]);

  useModalFocus(open && confirmChoice === null, modalRef, closePicker);

  useEffect(() => {
    sessionRef.current += 1;
    requestRef.current += 1;
    setConfirmation(null);
    setPickError(null);
    setResult(null);
    setError(null);

    pickingRef.current = null;
    setPicking(null);
    if (!open) {
      setLoadingRepo(null);
      return;
    }

    const session = sessionRef.current;
    const request = ++requestRef.current;
    const requestedRepo = repo;
    setLoadingRepo(requestedRepo);

    listReleases(requestedRepo)
      .then((releases) => {
        if (!openRef.current || currentRepoRef.current !== requestedRepo || sessionRef.current !== session || requestRef.current !== request) return;
        setResult({ repo: requestedRepo, releases });
      })
      .catch((reason: unknown) => {
        if (!openRef.current || currentRepoRef.current !== requestedRepo || sessionRef.current !== session || requestRef.current !== request) return;
        setError({ repo: requestedRepo, message: String(reason) });
      })
      .finally(() => {
        if (!openRef.current || currentRepoRef.current !== requestedRepo || sessionRef.current !== session || requestRef.current !== request) return;
        setLoadingRepo(null);
      });
  }, [open, repo]);

  const releases = result?.repo === repo ? result.releases : [];
  const loading = loadingRepo === repo;
  const currentError = error?.repo === repo ? error.message : null;
  const hasEligibleAssets = releases.some((release) =>
    release.assets.some((asset) => /\.dll$/i.test(asset.name)),
  );
  const controlsBusy = busy || picking !== null;

  const install = async (choice: AssetChoice) => {
    if (choice.repo !== currentRepoRef.current || pickingRef.current !== null || busy) return;
    const key = `${choice.repo}\n${choice.tag}\n${choice.assetName}`;
    const session = sessionRef.current;
    pickingRef.current = key;
    setPicking(key);
    setPickError(null);
    setConfirmation(null);
    try {
      await onPick(choice.repo, choice.tag, choice.assetName);
    } catch (reason: unknown) {
      if (openRef.current && currentRepoRef.current === choice.repo && sessionRef.current === session) {
        setPickError(String(reason));
      }
    } finally {
      if (pickingRef.current === key) pickingRef.current = null;
      if (openRef.current && sessionRef.current === session) setPicking(null);
    }
  };

  const choose = (choice: AssetChoice) => {
    if (controlsBusy || choice.repo !== repo) return;
    setConfirmation(choice);
  };


  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-50 grid place-items-center p-4 sm:p-6"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18 }}
          onClick={(event) => {
            if (event.target === event.currentTarget && confirmChoice === null && !controlsBusy) closePicker();
          }}
        >
          <div
            aria-hidden="true"
            className="pointer-events-none absolute inset-0 bg-[rgba(6,4,18,0.5)]"
            style={{ backdropFilter: "blur(2px)" }}
          />
          <motion.div
            ref={modalRef}
            role="dialog"
            aria-modal="true"
            aria-label={`Pick a release file for ${modName}`}
            aria-hidden={confirmChoice !== null}
            inert={confirmChoice !== null}
            tabIndex={-1}
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex max-h-[88vh] w-[600px] max-w-full flex-col rounded-3xl p-5 sm:p-6"
          >
            <button type="button" onClick={closePicker} disabled={controlsBusy} aria-label="Close release picker" className="ring-focus absolute top-4 right-4 grid h-8 w-8 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink disabled:opacity-40">
              <X size={16} weight="bold" />
            </button>

            <h2 className="pr-10 text-[20px] font-semibold text-ink">Pick a file</h2>
            <div className="mt-0.5 flex min-w-0 flex-wrap items-center gap-2 text-[13px] text-ink-dim">
              <span className="max-w-full truncate" title={modName} aria-label={`Mod ${modName}`}>{modName}</span>
              <span aria-hidden="true">·</span>
              <span className="max-w-full truncate font-mono" title={repo} aria-label={`Repository ${repo}`}>{repo}</span>
              <TrustBadge trust={trust} compact />
            </div>
            <p className={`mt-2 rounded-lg px-3 py-2 text-[12.5px] ${trust === "flagged" ? "bg-[rgba(255,170,60,0.12)] text-[#ffd9a8]" : "bg-white/[0.05] text-ink-dim"}`} role="status" aria-live="polite">
              {trustDescription[trust]}
            </p>

            {pickError && <p className="mt-3 rounded-xl bg-[rgba(226,59,59,0.12)] px-3.5 py-2.5 text-[13px] break-words text-[#ff8a8a]" role="alert">Install failed: {pickError}</p>}

            <div className="scroll-region mt-4 flex-1 overflow-y-auto pr-1">
              {loading && <p className="py-8 text-center text-[13px] text-ink-faint" role="status">Loading releases…</p>}
              {currentError && <p className="py-8 text-center text-[13px] break-words text-[#ff8a8a]" role="alert">Could not load releases: {currentError}</p>}
              {!loading && !currentError && !hasEligibleAssets && (
                <p className="py-8 text-center text-[13px] text-ink-faint">No .dll files were found in this repository&apos;s releases.</p>
              )}
              {!loading && !currentError && releases.map((release) => {
                const assets = release.assets.filter((asset) => /\.dll$/i.test(asset.name));
                if (assets.length === 0) return null;
                return (
                  <div key={`${repo}-${release.tag_name}`} className="mb-3 min-w-0">
                    <div className="mb-1.5 flex min-w-0 items-center gap-2 px-1">
                      <span className="max-w-[70%] truncate font-mono text-[12.5px] text-ink" title={release.tag_name} aria-label={`Release ${release.tag_name}`}>{release.tag_name}</span>
                      <div className="h-px flex-1 bg-white/10" />
                    </div>
                    <div className="flex flex-col gap-1.5">
                      {assets.map((asset) => {
                        const choice = { repo, tag: release.tag_name, assetName: asset.name };
                        const key = `${repo}\n${release.tag_name}\n${asset.name}`;
                        const type = "DLL";
                        const size = formatSize(asset.size);
                        return (
                          <button
                            key={`${release.tag_name}-${asset.name}`}
                            type="button"
                            disabled={controlsBusy}
                            onClick={() => choose(choice)}
                            aria-label={`Install ${asset.name}, ${type} file, ${size}, from ${repo} release ${release.tag_name}`}
                            title={`${asset.name} · ${type} · ${size}`}
                            className="ring-focus glass flex min-w-0 items-center gap-2.5 rounded-xl px-3 py-2.5 text-left hover:bg-white/10 disabled:opacity-50"
                          >
                            <FileArrowDown size={16} className="shrink-0 text-ink-dim" />
                            <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] text-ink">{asset.name}</span>
                            <span className="shrink-0 rounded bg-white/[0.07] px-1.5 py-0.5 text-[10.5px] font-semibold text-ink-dim">{type}</span>
                            <span className="shrink-0 text-[11.5px] text-ink-faint">{size}</span>
                            {picking === key ? <span className="shrink-0 text-[11.5px] text-ink-dim">Installing…</span> : <DownloadSimple size={14} className="shrink-0 text-[#9b7bff]" />}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                );
              })}
            </div>
          </motion.div>

          <AssetConfirmation
            choice={confirmChoice}
            trust={trust}
            pending={controlsBusy}
            onCancel={() => setConfirmation(null)}
            onConfirm={() => confirmChoice && void install(confirmChoice)}
          />
        </motion.div>
      )}
    </AnimatePresence>
  );
}

function AssetConfirmation({
  choice,
  trust,
  pending,
  onCancel,
  onConfirm,
}: {
  choice: AssetChoice | null;
  trust: Trust;
  pending: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useModalFocus(choice !== null, ref, onCancel);

  if (!choice) return null;
  const title = trust === "flagged"
    ? "Install this unverified asset?"
    : trust === "community"
      ? "Install this community-listed asset?"
      : "Install this catalog-selected asset?";
  const description = trust === "flagged"
    ? "This repository is not in the trusted catalog. Confirm the exact repository, release, and file before installing."
    : trust === "community"
      ? "This repository's metadata is community-listed, not publisher-authenticated. Confirm the exact repository, release, and file before installing."
      : "This repository's metadata is in the trusted catalog, but publisher code is not authenticated. Confirm the exact repository, release, and file before installing.";
  return (
    <div className="absolute inset-0 z-20 grid place-items-center bg-[rgba(6,4,18,0.74)] p-4 sm:p-6" style={{ backdropFilter: "blur(2px)" }} onMouseDown={(event) => event.target === event.currentTarget && !pending && onCancel()}>
      <div ref={ref} role="alertdialog" aria-modal="true" aria-labelledby="asset-confirm-title" aria-describedby="asset-confirm-description" tabIndex={-1} className="glass-strong w-[440px] max-w-full rounded-2xl p-5">
        <div className="flex items-center gap-2.5">
          <Warning size={20} weight="fill" className="shrink-0 text-[#ffd9a8]" />
          <h3 id="asset-confirm-title" className="text-[16px] font-semibold text-ink">{title}</h3>
        </div>
        <p id="asset-confirm-description" className="mt-2 text-[13px] text-ink-dim">{description}</p>
        <dl className="mt-3 grid min-w-0 grid-cols-[auto_1fr] gap-x-3 gap-y-1 rounded-xl bg-white/[0.05] p-3 text-[12px]">
          <dt className="text-ink-faint">Repository</dt><dd className="min-w-0 truncate font-mono text-ink" title={choice.repo}>{choice.repo}</dd>
          <dt className="text-ink-faint">Release</dt><dd className="min-w-0 truncate font-mono text-ink" title={choice.tag}>{choice.tag}</dd>
          <dt className="text-ink-faint">Asset</dt><dd className="min-w-0 truncate font-mono text-ink" title={choice.assetName}>{choice.assetName}</dd>
        </dl>
        <div className="mt-4 flex flex-wrap justify-end gap-2.5">
          <button data-autofocus type="button" disabled={pending} onClick={onCancel} className="ring-focus glass rounded-xl px-4 py-2.5 text-[13.5px] text-ink disabled:opacity-50">Cancel</button>
          <button type="button" disabled={pending} onClick={onConfirm} className="ring-focus rounded-xl bg-[#ffb45e] px-4 py-2.5 text-[13.5px] font-bold text-[#201006] disabled:opacity-50">Install this exact asset</button>
        </div>
      </div>
    </div>
  );
}
