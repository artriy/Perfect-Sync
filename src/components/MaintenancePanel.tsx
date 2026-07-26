import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  ArrowsClockwise,
  CheckCircle,
  ClockCounterClockwise,
  Database,
  FileZip,
  FirstAid,
  Warning,
  X,
  XCircle,
} from "@phosphor-icons/react";
import {
  backupSaveData,
  collectDiagnostics,
  exportSupportBundle,
  listSaveBackups,
  restoreSaveData,
  type DiagnosticsReport,
  type SaveBackupInfo,
} from "../lib/bridge";
import { useModalFocus } from "../lib/useModalFocus";

interface MaintenancePanelProps {
  open: boolean;
  profileId: string;
  onClose: () => void;
}

function messageFrom(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}

function formatDate(value: number): string {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
}

export function MaintenancePanel({ open, profileId, onClose }: MaintenancePanelProps) {
  const reduce = useReducedMotion();
  const modalRef = useRef<HTMLDivElement>(null);
  const openRef = useRef(open);
  const requestRef = useRef(0);
  const [report, setReport] = useState<DiagnosticsReport | null>(null);
  const [backups, setBackups] = useState<SaveBackupInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [working, setWorking] = useState<"backup" | "export" | `restore:${string}` | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [restoreConfirm, setRestoreConfirm] = useState<string | null>(null);

  openRef.current = open;
  const requestClose = useCallback(() => {
    if (!working) onClose();
  }, [onClose, working]);
  useModalFocus(open, modalRef, requestClose);

  const refresh = useCallback(async () => {
    const request = ++requestRef.current;
    setLoading(true);
    setError("");
    try {
      const [nextReport, nextBackups] = await Promise.all([
        collectDiagnostics(profileId),
        listSaveBackups(),
      ]);
      if (!openRef.current || request !== requestRef.current) return;
      setReport(nextReport);
      setBackups(nextBackups);
    } catch (reason) {
      if (openRef.current && request === requestRef.current) setError(messageFrom(reason));
    } finally {
      if (openRef.current && request === requestRef.current) setLoading(false);
    }
  }, [profileId]);

  useEffect(() => {
    if (!open) return;
    setReport(null);
    setBackups([]);
    setNotice("");
    setRestoreConfirm(null);
    void refresh();
  }, [open, refresh]);

  const createBackup = async () => {
    if (working) return;
    setWorking("backup");
    setError("");
    setNotice("");
    try {
      const backup = await backupSaveData();
      setNotice(`Save data backed up: ${backup.files} files, ${formatBytes(backup.bytes)}.`);
      setBackups(await listSaveBackups());
    } catch (reason) {
      setError(messageFrom(reason));
    } finally {
      setWorking(null);
    }
  };

  const exportBundle = async () => {
    if (working) return;
    setWorking("export");
    setError("");
    setNotice("");
    try {
      const destination = await exportSupportBundle(profileId);
      if (destination) setNotice("Support bundle exported with paths and credentials redacted.");
    } catch (reason) {
      setError(messageFrom(reason));
    } finally {
      setWorking(null);
    }
  };

  const restoreBackup = async (backup: SaveBackupInfo) => {
    if (working) return;
    if (restoreConfirm !== backup.id) {
      setRestoreConfirm(backup.id);
      setNotice("Restoring replaces current Among Us save data. A safety backup will be created first.");
      return;
    }
    setWorking(`restore:${backup.id}`);
    setError("");
    setNotice("");
    try {
      await restoreSaveData(backup.id);
      setRestoreConfirm(null);
      setNotice(`Save data restored from ${formatDate(backup.createdAt)}. A safety backup was kept.`);
      setBackups(await listSaveBackups());
    } catch (reason) {
      setError(messageFrom(reason));
    } finally {
      setWorking(null);
    }
  };

  const hasAttention = !!report && (report.warnings.length > 0 || report.logErrors.length > 0);

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-[60] flex items-center justify-center bg-[rgba(5,3,12,0.72)] p-4"
          initial={reduce ? false : { opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.16 }}
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) requestClose();
          }}
        >
          <motion.div
            ref={modalRef}
            role="dialog"
            aria-modal="true"
            aria-labelledby="maintenance-title"
            className="glass-strong flex max-h-[92vh] w-[900px] max-w-full flex-col overflow-hidden rounded-3xl"
            initial={reduce ? false : { opacity: 0, y: 14, scale: 0.985 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 8, scale: 0.99 }}
            transition={{ duration: 0.22, ease: [0.16, 1, 0.3, 1] }}
          >
            <header className="flex items-center gap-3 border-b border-white/10 px-5 py-4">
              <div className="grid h-10 w-10 shrink-0 place-items-center rounded-xl bg-[#5bc0ff]/12 text-crew-cyan">
                <FirstAid size={21} weight="fill" />
              </div>
              <div className="min-w-0 flex-1">
                <h2 id="maintenance-title" className="text-[18px] font-bold tracking-tight text-ink">
                  Health & maintenance
                </h2>
                <p className="mt-0.5 text-[12.5px] text-ink-faint">
                  Inspect this profile, protect save data, and export a redacted support snapshot.
                </p>
              </div>
              <button
                type="button"
                onClick={() => void refresh()}
                disabled={loading || !!working}
                aria-label="Refresh diagnostics"
                className="ring-focus glass grid h-10 w-10 shrink-0 place-items-center rounded-xl text-ink-dim hover:text-ink disabled:opacity-50"
              >
                <ArrowsClockwise size={17} className={loading ? "animate-spin" : ""} />
              </button>
              <button
                type="button"
                onClick={requestClose}
                disabled={!!working}
                aria-label="Close health and maintenance"
                className="ring-focus glass grid h-10 w-10 shrink-0 place-items-center rounded-xl text-ink-dim hover:text-ink disabled:opacity-50"
              >
                <X size={17} />
              </button>
            </header>

            <div className="scroll-region grid min-h-0 flex-1 grid-cols-[minmax(0,1.1fr)_minmax(300px,0.9fr)] gap-4 overflow-y-auto p-5 max-[760px]:grid-cols-1">
              <section aria-labelledby="diagnostics-heading" className="min-w-0">
                <div className="mb-3 flex items-center justify-between gap-3">
                  <h3 id="diagnostics-heading" className="text-[13px] font-semibold text-ink">
                    Current diagnostic snapshot
                  </h3>
                  {report && (
                    <span
                      className={`flex items-center gap-1 rounded-full px-2.5 py-1 text-[11.5px] font-semibold ${
                        hasAttention ? "bg-[#ffd23f]/12 text-[#ffe58a]" : "bg-[#5be3b0]/12 text-crew-mint"
                      }`}
                    >
                      {hasAttention ? <Warning size={13} weight="fill" /> : <CheckCircle size={13} weight="fill" />}
                      {hasAttention ? "Review needed" : "Healthy"}
                    </span>
                  )}
                </div>

                {loading && !report ? (
                  <div className="glass rounded-2xl px-4 py-8 text-center text-[12.5px] text-ink-faint">
                    Reading game, loader, profile, and recent log state…
                  </div>
                ) : report ? (
                  <div className="overflow-hidden rounded-2xl border border-white/10 bg-white/[0.025]">
                    <StatusRow
                      label="Game"
                      value={report.game ? `${report.game.name} · ${report.game.store} · ${report.game.arch}` : "Not configured"}
                      ok={!!report.game}
                    />
                    <StatusRow
                      label="Build"
                      value={report.game?.build ?? "Could not detect"}
                      ok={!!report.game?.build}
                    />
                    <StatusRow
                      label="Folder access"
                      value={report.game?.writable ? "Writable" : "Managed copy required"}
                      ok={report.game?.writable === true}
                    />
                    <StatusRow
                      label="BepInEx"
                      value={
                        report.loader?.current
                          ? `Current${report.loader.installedVersion ? ` · ${report.loader.installedVersion}` : ""}`
                          : "Incomplete or unavailable"
                      }
                      ok={report.loader?.current === true}
                    />
                    <StatusRow
                      label="Profile assets"
                      value={`${report.assets.filter((asset) => asset.enabled).length} enabled · ${report.assets.length} installed`}
                      ok
                    />
                    <StatusRow
                      label="Among Us process"
                      value={report.gameRunning == null ? "Could not verify" : report.gameRunning ? "Running" : "Stopped"}
                      ok={report.gameRunning === false}
                      last
                    />
                  </div>
                ) : null}

                {report && (report.warnings.length > 0 || report.logErrors.length > 0) && (
                  <div className="mt-4 space-y-2">
                    {report.warnings.map((warning) => (
                      <div key={warning} className="flex gap-2 rounded-xl bg-[#ffd23f]/8 px-3 py-2.5 text-[12px] text-[#ffe8a3]">
                        <Warning size={15} weight="fill" className="mt-0.5 shrink-0" />
                        <span className="break-words">{warning}</span>
                      </div>
                    ))}
                    {report.logErrors.length > 0 && (
                      <details className="rounded-xl border border-[#ff8a8a]/20 bg-[#ff8a8a]/6 px-3 py-2.5">
                        <summary className="ring-focus cursor-pointer rounded text-[12px] font-semibold text-[#ffb4b4]">
                          {report.logErrors.length} recent BepInEx error {report.logErrors.length === 1 ? "line" : "lines"}
                        </summary>
                        <pre className="mt-2 max-h-36 overflow-auto whitespace-pre-wrap break-words font-mono text-[10.5px] leading-relaxed text-ink-dim">
                          {report.logErrors.join("\n")}
                        </pre>
                      </details>
                    )}
                  </div>
                )}

                <button
                  type="button"
                  onClick={() => void exportBundle()}
                  disabled={!!working || loading}
                  className="ring-focus glass mt-4 flex w-full items-center justify-center gap-2 rounded-xl px-4 py-3 text-[12.5px] font-semibold text-ink hover:bg-white/[0.08] disabled:opacity-50"
                >
                  <FileZip size={16} />
                  {working === "export" ? "Exporting support bundle…" : "Export redacted support bundle"}
                </button>
              </section>

              <section aria-labelledby="save-data-heading" className="min-w-0">
                <div className="mb-3 flex items-center justify-between gap-3">
                  <div>
                    <h3 id="save-data-heading" className="text-[13px] font-semibold text-ink">
                      Save-data protection
                    </h3>
                    <p className="mt-0.5 text-[11.5px] text-ink-faint">Backs up all Innersloth save folders.</p>
                  </div>
                  <button
                    type="button"
                    onClick={() => void createBackup()}
                    disabled={!!working}
                    className="ring-focus accent-grad flex shrink-0 items-center gap-1.5 rounded-lg px-3 py-2 text-[12px] font-bold text-[#0d0820] disabled:opacity-50"
                  >
                    <Database size={14} weight="fill" />
                    {working === "backup" ? "Backing up…" : "Back up now"}
                  </button>
                </div>

                <div className="overflow-hidden rounded-2xl border border-white/10 bg-white/[0.025]">
                  {backups.length === 0 ? (
                    <div className="px-4 py-8 text-center">
                      <ClockCounterClockwise size={24} className="mx-auto text-ink-faint" />
                      <p className="mt-2 text-[12.5px] font-medium text-ink-dim">No save-data backups yet</p>
                      <p className="mt-1 text-[11.5px] text-ink-faint">Create one before changing game versions or mod packs.</p>
                    </div>
                  ) : (
                    backups.map((backup, index) => {
                      const restoring = working === `restore:${backup.id}`;
                      const confirming = restoreConfirm === backup.id;
                      return (
                        <div
                          key={backup.id}
                          className={`flex items-center gap-3 px-3.5 py-3 ${index ? "border-t border-white/8" : ""}`}
                        >
                          <ClockCounterClockwise size={17} className="shrink-0 text-ink-faint" />
                          <div className="min-w-0 flex-1">
                            <p className="text-[12px] font-semibold text-ink">{formatDate(backup.createdAt)}</p>
                            <p className="mt-0.5 font-mono text-[10.5px] text-ink-faint">
                              {backup.files} files · {formatBytes(backup.bytes)}
                            </p>
                          </div>
                          <button
                            type="button"
                            onClick={() => void restoreBackup(backup)}
                            disabled={!!working}
                            className={`ring-focus shrink-0 rounded-lg px-2.5 py-1.5 text-[11.5px] font-semibold disabled:opacity-50 ${
                              confirming ? "bg-[#ffd23f] text-[#241900]" : "bg-white/8 text-ink-dim hover:text-ink"
                            }`}
                          >
                            {restoring ? "Restoring…" : confirming ? "Confirm restore" : "Restore"}
                          </button>
                        </div>
                      );
                    })
                  )}
                </div>

                <div className="mt-4 rounded-xl bg-white/[0.035] px-3.5 py-3 text-[11.5px] leading-relaxed text-ink-faint">
                  Restore is transactional: Perfect-Sync stages the selected backup and creates a safety backup of current saves before replacing them.
                </div>
              </section>
            </div>

            {(error || notice) && (
              <div
                role={error ? "alert" : "status"}
                className={`mx-5 mb-5 flex items-start gap-2 rounded-xl px-3.5 py-3 text-[12px] ${
                  error ? "bg-[#ff8a8a]/10 text-[#ffb4b4]" : "bg-[#5be3b0]/10 text-crew-mint"
                }`}
              >
                {error ? <XCircle size={16} weight="fill" className="shrink-0" /> : <CheckCircle size={16} weight="fill" className="shrink-0" />}
                <span className="min-w-0 flex-1 break-words">{error || notice}</span>
                {notice && restoreConfirm && (
                  <button
                    type="button"
                    onClick={() => {
                      setRestoreConfirm(null);
                      setNotice("");
                    }}
                    className="ring-focus shrink-0 rounded px-1.5 py-0.5 text-ink-dim hover:text-ink"
                  >
                    Cancel
                  </button>
                )}
              </div>
            )}
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

function StatusRow({
  label,
  value,
  ok,
  last = false,
}: {
  label: string;
  value: string;
  ok: boolean;
  last?: boolean;
}) {
  return (
    <div className={`flex items-center gap-3 px-3.5 py-2.5 ${last ? "" : "border-b border-white/8"}`}>
      {ok ? (
        <CheckCircle size={15} weight="fill" className="shrink-0 text-crew-mint" />
      ) : (
        <Warning size={15} weight="fill" className="shrink-0 text-crew-gold" />
      )}
      <span className="w-24 shrink-0 text-[11.5px] text-ink-faint">{label}</span>
      <span className="min-w-0 flex-1 truncate text-right text-[12px] font-medium text-ink">{value}</span>
    </div>
  );
}
