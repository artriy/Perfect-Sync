import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { ArrowRight, CaretDown, DownloadSimple, FileArrowDown, Warning, X } from "@phosphor-icons/react";
import { listInstallOptions } from "../lib/bridge";
import type { ModInstallOption, Trust } from "../lib/types";
import { useModalFocus } from "../lib/useModalFocus";
import { TrustBadge } from "./TrustBadge";

interface ReleasePickerProps {
  open: boolean;
  repo: string;
  modName: string;
  trust: Trust;
  busy: boolean;
  profileId: string;
  currentVersion?: string;
  recommendedVersion?: string;
  onClose: () => void;
  onPick: (repo: string, tag: string, assetName: string) => void | Promise<void>;
}

interface InstallOptionsResult {
  repo: string;
  profileId: string;
  options: ModInstallOption[];
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

function assetKind(name: string): "DLL" | "ZIP" {
  return name.toLowerCase().endsWith(".zip") ? "ZIP" : "DLL";
}


const trustDescription: Record<Trust, string> = {
  trusted: "Trusted catalog repository",
  community: "Community-listed repository",
  flagged: "Unverified repository. Confirm the exact asset before installing.",
};

export function ReleasePicker({
  open,
  repo,
  modName,
  trust,
  busy,
  profileId,
  currentVersion,
  recommendedVersion,
  onClose,
  onPick,
}: ReleasePickerProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const openRef = useRef(open);
  const currentRepoRef = useRef(repo);
  const sessionRef = useRef(0);
  const requestRef = useRef(0);
  const pickingRef = useRef<string | null>(null);
  const [result, setResult] = useState<InstallOptionsResult | null>(null);
  const [loadingKey, setLoadingKey] = useState<string | null>(null);
  const [error, setError] = useState<{ key: string; message: string } | null>(null);
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
      setLoadingKey(null);
      return;
    }

    const session = sessionRef.current;
    const request = ++requestRef.current;
    const requestedRepo = repo;
    const requestedProfileId = profileId;
    const requestKey = `${requestedRepo}\n${requestedProfileId}`;
    setLoadingKey(requestKey);

    listInstallOptions(requestedRepo, requestedProfileId)
      .then((options) => {
        if (
          !openRef.current ||
          currentRepoRef.current !== requestedRepo ||
          sessionRef.current !== session ||
          requestRef.current !== request
        ) return;
        setResult({ repo: requestedRepo, profileId: requestedProfileId, options });
      })
      .catch((reason: unknown) => {
        if (
          !openRef.current ||
          currentRepoRef.current !== requestedRepo ||
          sessionRef.current !== session ||
          requestRef.current !== request
        ) return;
        setError({
          key: requestKey,
          message: reason instanceof Error ? reason.message : String(reason),
        });
      })
      .finally(() => {
        if (
          !openRef.current ||
          currentRepoRef.current !== requestedRepo ||
          sessionRef.current !== session ||
          requestRef.current !== request
        ) return;
        setLoadingKey(null);
      });
  }, [open, profileId, repo]);

  const resultMatches = result?.repo === repo && result.profileId === profileId;
  const options = resultMatches ? result.options : [];
  const requestKey = `${repo}\n${profileId}`;
  const loading = loadingKey === requestKey;
  const currentError = error?.key === requestKey ? error.message : null;
  const hasOptions = options.length > 0;
  const controlsBusy = busy || picking !== null;
  const updateVersion =
    recommendedVersion && recommendedVersion !== currentVersion
      ? recommendedVersion
      : undefined;
  const recommendedOption = updateVersion
    ? options.find((option) => option.tag === updateVersion)
    : undefined;
  const latestVersion = recommendedVersion ?? options[0]?.tag;
  const optionGroups = options.reduce<Array<{ tag: string; options: ModInstallOption[] }>>(
    (groups, option) => {
      const current = groups.at(-1);
      if (current?.tag === option.tag) current.options.push(option);
      else groups.push({ tag: option.tag, options: [option] });
      return groups;
    },
    [],
  );

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
        setPickError(reason instanceof Error ? reason.message : String(reason));
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

  const chooseRecommended = () => {
    if (!recommendedOption || controlsBusy) return;
    const choice = {
      repo,
      tag: recommendedOption.tag,
      assetName: recommendedOption.assetName,
    };
    choose(choice);
  };

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-50 grid place-items-center p-4 sm:p-6 max-[600px]:p-0"
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
            aria-label={`${recommendedOption ? "Update" : "Choose a release file for"} ${modName}`}
            aria-hidden={confirmChoice !== null}
            inert={confirmChoice !== null}
            tabIndex={-1}
            initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 12 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.97, y: 8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex max-h-[88vh] w-[600px] max-w-full flex-col rounded-3xl p-5 sm:p-6 max-[600px]:h-[100dvh] max-[600px]:max-h-none max-[600px]:w-full max-[600px]:rounded-none max-[600px]:p-4"
          >
            <button
              type="button"
              onClick={closePicker}
              disabled={controlsBusy}
              aria-label="Close release picker"
              className="ring-focus absolute top-4 right-4 grid h-9 w-9 place-items-center rounded-lg text-ink-faint hover:bg-white/10 hover:text-ink disabled:opacity-40"
            >
              <X size={16} weight="bold" />
            </button>

            <h2 className="pr-12 text-[20px] font-semibold text-ink">
              {recommendedOption ? `Update ${modName}` : "Choose a version"}
            </h2>
            <div className="mt-0.5 flex min-w-0 flex-wrap items-center gap-2 text-[13px] text-ink-dim">
              <span className="max-w-full truncate" title={modName} aria-label={`Mod ${modName}`}>
                {modName}
              </span>
              <span aria-hidden="true">·</span>
              <span className="max-w-full truncate font-mono" title={repo} aria-label={`Repository ${repo}`}>
                {repo}
              </span>
              <TrustBadge trust={trust} compact />
            </div>
            <p
              className={`mt-2 rounded-lg px-3 py-2 text-[12.5px] ${
                trust === "flagged"
                  ? "bg-[rgba(255,170,60,0.12)] text-[#ffd9a8]"
                  : "bg-white/[0.05] text-ink-dim"
              }`}
              role="status"
              aria-live="polite"
            >
              {trustDescription[trust]}
            </p>

            {pickError && (
              <p className="mt-3 rounded-xl bg-[rgba(226,59,59,0.12)] px-3.5 py-2.5 text-[13px] break-words text-[#ff8a8a]" role="alert">
                Update failed: {pickError}
              </p>
            )}

            {loading && (
              <p className="py-8 text-center text-[13px] text-ink-faint" role="status">
                Finding compatible versions…
              </p>
            )}
            {currentError && (
              <p className="py-8 text-center text-[13px] break-words text-[#ff8a8a]" role="alert">
                Could not load versions: {currentError}
              </p>
            )}
            {!loading && !currentError && !hasOptions && (
              <p className="py-8 text-center text-[13px] text-ink-faint">
                No compatible mod files were found in this repository&apos;s releases.
              </p>
            )}

            {!loading && !currentError && recommendedOption && (
              <section className="mt-4 rounded-2xl border border-accent/30 bg-accent/10 p-4">
                <span className="text-[12px] font-semibold text-accent-2">Recommended update</span>
                <div className="mt-2 flex min-w-0 items-center gap-3">
                  <span className="truncate font-mono text-[15px] text-ink-dim">{currentVersion}</span>
                  <ArrowRight size={16} className="shrink-0 text-ink-faint" aria-hidden="true" />
                  <span className="truncate font-mono text-[17px] font-semibold text-ink">{recommendedOption.tag}</span>
                </div>
                <p className="mt-1.5 truncate font-mono text-[12.5px] text-ink-faint" title={recommendedOption.assetName}>
                  Uses {recommendedOption.assetName}
                </p>
                <button
                  data-autofocus
                  type="button"
                  disabled={controlsBusy}
                  onClick={chooseRecommended}
                  className="ring-focus accent-grad mt-4 flex w-full items-center justify-center gap-2 rounded-xl px-5 py-3 text-[14px] font-bold text-[#0d0820] disabled:opacity-50"
                >
                  <DownloadSimple size={16} weight="bold" aria-hidden="true" />
                  {picking ? "Updating…" : `Update to ${recommendedOption.tag}`}
                </button>
              </section>
            )}

            {!loading && !currentError && hasOptions && (
              <details className="mt-4 flex min-h-0 flex-col" open={!recommendedOption}>
                <summary className="ring-focus flex cursor-pointer list-none items-center justify-between rounded-xl px-3 py-2.5 text-[13px] font-semibold text-ink-dim hover:bg-white/[0.06] hover:text-ink">
                  <span>{recommendedOption ? "Choose another version or file" : "Available versions and files"}</span>
                  <CaretDown size={14} weight="bold" aria-hidden="true" />
                </summary>
                <div className="scroll-region mt-2 min-h-0 flex-1 overflow-y-auto pr-1">
                  {optionGroups.map((group) => (
                    <div key={`${repo}-${group.tag}`} className="mb-3 min-w-0">
                      <div className="mb-1.5 flex min-w-0 items-center gap-2 px-1">
                        <span className="max-w-[70%] truncate font-mono text-[12.5px] text-ink" title={group.tag}>
                          {group.tag}
                        </span>
                        {group.tag === latestVersion && (
                          <span className="rounded-md bg-accent/15 px-1.5 py-0.5 text-[10.5px] font-semibold text-accent-2">
                            Latest
                          </span>
                        )}
                        {group.tag === currentVersion && (
                          <span className="rounded-md bg-white/[0.07] px-1.5 py-0.5 text-[10.5px] font-semibold text-ink-dim">
                            Current
                          </span>
                        )}
                        <div className="h-px flex-1 bg-white/10" />
                      </div>
                      <div className="flex flex-col gap-1.5">
                        {group.options.map((option) => {
                          const choice = { repo, tag: option.tag, assetName: option.assetName };
                          const key = `${repo}\n${option.tag}\n${option.assetName}`;
                          return (
                            <button
                              key={`${option.tag}-${option.assetName}`}
                              type="button"
                              disabled={controlsBusy}
                              onClick={() => choose(choice)}
                              aria-label={`Install ${option.assetName}, ${assetKind(option.assetName)} file, ${formatSize(option.size)}, from ${repo} release ${option.tag}`}
                              title={`${option.assetName} · ${assetKind(option.assetName)} · ${formatSize(option.size)}`}
                              className="ring-focus surface-row flex min-w-0 items-center gap-2.5 rounded-xl px-3 py-2.5 text-left hover:bg-white/[0.075] disabled:opacity-50"
                            >
                              <FileArrowDown size={16} className="shrink-0 text-ink-dim" aria-hidden="true" />
                              <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] text-ink">
                                {option.assetName}
                              </span>
                              <span className="shrink-0 rounded bg-white/[0.07] px-1.5 py-0.5 text-[10.5px] font-semibold text-ink-dim">
                                {assetKind(option.assetName)}
                              </span>
                              <span className="shrink-0 text-[12px] text-ink-faint">
                                {formatSize(option.size)}
                              </span>
                              {picking === key ? (
                                <span className="shrink-0 text-[12px] text-ink-dim">Installing…</span>
                              ) : (
                                <DownloadSimple size={14} className="shrink-0 text-accent" aria-hidden="true" />
                              )}
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  ))}
                </div>
              </details>
            )}
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
