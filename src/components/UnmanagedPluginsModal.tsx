import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Archive, Check, PlusCircle, Trash, Warning } from "@phosphor-icons/react";
import type { UnmanagedPlugin } from "../lib/types";
import { useModalFocus } from "../lib/useModalFocus";

interface UnmanagedPluginsModalProps {
  open: boolean;
  profileName: string;
  instanceName: string;
  plugins: readonly UnmanagedPlugin[];
  continuation: boolean;
  onCancel: () => void;
  onQuarantine: (paths: readonly string[]) => Promise<boolean>;
  onDelete: (paths: readonly string[]) => Promise<boolean>;
  onImport: (paths: readonly string[]) => Promise<boolean>;
}

type PendingAction = "quarantine" | "delete" | "import" | null;

const number = new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 });

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${number.format(bytes / 1024)} KB`;
  return `${number.format(bytes / (1024 * 1024))} MB`;
}

export function UnmanagedPluginsModal({
  open,
  profileName,
  instanceName,
  plugins,
  continuation,
  onCancel,
  onQuarantine,
  onDelete,
  onImport,
}: UnmanagedPluginsModalProps) {
  const reduce = useReducedMotion();
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef(onCancel);
  const [pending, setPending] = useState<PendingAction>(null);
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(new Set());
  const [deleteArmed, setDeleteArmed] = useState(false);
  const [error, setError] = useState("");
  cancelRef.current = onCancel;

  const close = useCallback(() => {
    if (!pending) cancelRef.current();
  }, [pending]);
  useModalFocus(open, dialogRef, close);

  const pluginKey = plugins.map((plugin) => plugin.path).join("\0");
  useEffect(() => {
    if (!open) return;
    setPending(null);
    setSelectedPaths(new Set(plugins.map((plugin) => plugin.path)));
    setDeleteArmed(false);
    setError("");
  }, [open, pluginKey]);

  const togglePath = (path: string) => {
    setSelectedPaths((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
    setDeleteArmed(false);
    setError("");
  };

  const run = async (action: Exclude<PendingAction, null>) => {
    if (pending || selectedPaths.size === 0) return;
    const paths = [...selectedPaths];
    setPending(action);
    setDeleteArmed(false);
    setError("");
    try {
      const complete = action === "quarantine"
        ? await onQuarantine(paths)
        : action === "delete"
          ? await onDelete(paths)
          : await onImport(paths);
      if (!complete) setPending(null);
    } catch (reason: unknown) {
      setError(reason instanceof Error ? reason.message : String(reason));
      setPending(null);
    }
  };

  const selectedPlugins = plugins.filter((plugin) => selectedPaths.has(plugin.path));
  const selectedCount = selectedPlugins.length;
  const allSelected = selectedCount === plugins.length;
  const canImport = selectedCount > 0 && selectedPlugins.every((plugin) => plugin.importable);

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          data-modal-exempt
          className="fixed inset-0 z-[100] isolate grid place-items-center p-4 sm:p-6"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: reduce ? 0.01 : 0.18 }}
        >
          <div
            className="absolute inset-0 bg-[rgba(6,4,18,0.78)]"
            style={{ backdropFilter: "blur(5px) saturate(110%)" }}
            onMouseDown={(event) => event.target === event.currentTarget && close()}
          />
          <motion.div
            ref={dialogRef}
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="unmanaged-plugins-title"
            aria-describedby="unmanaged-plugins-description unmanaged-plugins-safety"
            aria-busy={pending !== null}
            tabIndex={-1}
            initial={reduce ? { opacity: 0 } : { opacity: 0.7, y: 14, scale: 0.975, filter: "blur(6px)" }}
            animate={{ opacity: 1, y: 0, scale: 1, filter: "blur(0px)" }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, y: 8, scale: 0.985, filter: "blur(4px)" }}
            transition={{ duration: reduce ? 0.01 : 0.24, ease: [0.16, 1, 0.3, 1] }}
            className="glass-strong relative flex max-h-[calc(100dvh-2rem)] w-[560px] max-w-full flex-col overflow-hidden rounded-3xl"
          >
            <header className="flex min-w-0 items-start gap-3.5 px-5 pt-5 sm:px-6 sm:pt-6">
              <span className="grid h-11 w-11 shrink-0 place-items-center rounded-2xl border border-[rgba(255,193,92,0.24)] bg-[rgba(255,193,92,0.12)] text-[#ffd9a8] shadow-[0_9px_26px_rgba(79,48,10,0.2)]">
                <Warning size={23} weight="fill" aria-hidden="true" />
              </span>
              <div className="min-w-0 flex-1">
                <h2 id="unmanaged-plugins-title" className="text-[20px] leading-tight font-semibold text-ink">
                  Extra plugins will load
                </h2>
                <p id="unmanaged-plugins-description" className="mt-1.5 max-w-[68ch] text-[13px] leading-relaxed text-ink-dim">
                  {instanceName} contains DLLs outside “{profileName}”. BepInEx would load them even though they are not shown in this profile.
                </p>
              </div>
            </header>

            <div className="scroll-region mt-4 min-h-0 overflow-y-auto px-5 sm:px-6">
              <div className="overflow-hidden rounded-2xl border border-[rgba(255,193,92,0.2)] bg-[rgba(255,193,92,0.055)]">
                <div className="flex items-center justify-between gap-3 border-b border-white/[0.07] px-3.5 py-2">
                  <span className="text-[11.5px] font-semibold text-[#e7d3b7]">
                    {selectedCount} of {plugins.length} selected
                  </span>
                  <button
                    type="button"
                    role="checkbox"
                    aria-checked={allSelected ? true : selectedCount > 0 ? "mixed" : false}
                    disabled={pending !== null}
                    onClick={() => {
                      setSelectedPaths(allSelected ? new Set() : new Set(plugins.map((plugin) => plugin.path)));
                      setDeleteArmed(false);
                      setError("");
                    }}
                    className="ring-focus flex items-center gap-1.5 rounded-lg px-2 py-1 text-[11.5px] font-semibold text-[#ffe2b7] hover:bg-white/[0.06] disabled:opacity-50"
                  >
                    <span className={`grid h-4 w-4 place-items-center rounded border ${allSelected ? "border-[#ffd166] bg-[#ffd166] text-[#211707]" : selectedCount > 0 ? "border-[#ffd166] bg-[#ffd166]/25 text-[#ffd166]" : "border-white/25 text-transparent"}`}>
                      <Check size={11} weight="bold" aria-hidden="true" />
                    </span>
                    {allSelected ? "Clear all" : "Select all"}
                  </button>
                </div>
                <ul aria-label="Plugins outside the selected profile">
                  {plugins.map((plugin, index) => {
                    const selected = selectedPaths.has(plugin.path);
                    return (
                      <li key={plugin.path} className={index ? "border-t border-white/[0.07]" : ""}>
                        <button
                          type="button"
                          role="checkbox"
                          aria-checked={selected}
                          disabled={pending !== null}
                          onClick={() => togglePath(plugin.path)}
                          className={`ring-focus flex w-full min-w-0 items-center gap-3 px-3.5 py-2.5 text-left transition-colors disabled:opacity-50 ${selected ? "bg-[rgba(255,193,92,0.055)]" : "hover:bg-white/[0.035]"}`}
                        >
                          <span className={`grid h-5 w-5 shrink-0 place-items-center rounded-md border transition-colors ${selected ? "border-[#ffd166] bg-[#ffd166] text-[#211707]" : "border-white/25 bg-white/[0.025] text-transparent"}`}>
                            <Check size={13} weight="bold" aria-hidden="true" />
                          </span>
                          <span className="min-w-0 flex-1">
                            <span className="block truncate text-[13.5px] font-semibold text-ink" title={plugin.name}>{plugin.name}</span>
                            <span className="block truncate font-mono text-[11px] text-ink-faint" title={plugin.path}>{plugin.path}</span>
                          </span>
                          <span className="shrink-0 text-[11.5px] text-ink-faint">{formatBytes(plugin.size)}</span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              </div>

              <div id="unmanaged-plugins-safety" className="mt-3.5 flex items-start gap-2.5 rounded-xl bg-[rgba(107,222,180,0.08)] px-3.5 py-3 text-[12.5px] leading-relaxed text-[#bcebd9]">
                <Archive size={17} weight="bold" className="mt-0.5 shrink-0" aria-hidden="true" />
                <span>Quarantine preserves selected files with their paths and checksums. Delete removes selected files permanently.</span>
              </div>

              {selectedCount > 0 && !canImport && (
                <p className="mt-3 text-[12px] leading-relaxed text-ink-faint">
                  The current selection includes a DLL inside a subfolder. Select only root-level DLLs to keep them as standalone local profile mods.
                </p>
              )}
              {error && (
                <p className="mt-3 rounded-xl bg-[rgba(226,59,59,0.12)] px-3.5 py-2.5 text-[12.5px] leading-relaxed break-words text-[#ff9a9a]" role="alert">
                  {error}
                </p>
              )}
            </div>

            <footer className="mt-5 flex flex-wrap items-center justify-between gap-3 border-t border-white/[0.08] px-5 py-4 sm:px-6">
              <p className="text-[11.5px] text-ink-faint" aria-live="polite">
                {selectedCount === 0 ? "Select at least one plugin" : `${selectedCount} selected`}
              </p>
              <div className="flex flex-wrap items-center justify-end gap-2">
                <button
                  type="button"
                  data-autofocus
                  disabled={pending !== null}
                  onClick={close}
                  className="ring-focus glass rounded-xl px-3.5 py-2.5 text-[13px] text-ink disabled:cursor-not-allowed disabled:opacity-50"
                >
                  Cancel
                </button>
                {deleteArmed ? (
                  <>
                    <span className="max-w-40 text-right text-[11.5px] leading-tight text-[#ffabab]">
                      Permanently delete {selectedCount} plugin{selectedCount === 1 ? "" : "s"}? This cannot be undone.
                    </span>
                    <button
                      type="button"
                      disabled={pending !== null}
                      onClick={() => setDeleteArmed(false)}
                      className="ring-focus glass rounded-xl px-3.5 py-2.5 text-[13px] text-ink disabled:opacity-50"
                    >
                      Keep files
                    </button>
                    <button
                      key="confirm-delete"
                      type="button"
                      disabled={pending !== null || selectedCount === 0}
                      onClick={() => void run("delete")}
                      className="ring-focus flex items-center gap-1.5 rounded-xl bg-[#d94b55] px-3.5 py-2.5 text-[13px] font-bold text-white transition-colors hover:bg-[#ea5b65] disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      <Trash size={15} weight="bold" aria-hidden="true" />
                      {pending === "delete" ? "Deleting…" : "Delete permanently"}
                    </button>
                  </>
                ) : (
                  <>
                    <button
                      type="button"
                      disabled={pending !== null || !canImport}
                      title={!canImport && selectedCount > 0 ? "Only root-level DLLs can be kept as standalone local mods." : undefined}
                      onClick={() => void run("import")}
                      className="ring-focus glass flex items-center gap-1.5 rounded-xl px-3.5 py-2.5 text-[13px] font-semibold text-ink disabled:cursor-not-allowed disabled:opacity-40"
                    >
                      <PlusCircle size={15} weight="bold" aria-hidden="true" />
                      {pending === "import" ? "Keeping…" : "Keep selected"}
                    </button>
                    <button
                      type="button"
                      disabled={pending !== null || selectedCount === 0}
                      onClick={() => setDeleteArmed(true)}
                      className="ring-focus flex items-center gap-1.5 rounded-xl border border-[#ff7e86]/30 bg-[#d94b55]/10 px-3.5 py-2.5 text-[13px] font-semibold text-[#ffb3b8] transition-colors hover:bg-[#d94b55]/18 disabled:cursor-not-allowed disabled:opacity-40"
                    >
                      <Trash size={15} weight="bold" aria-hidden="true" />
                      Delete selected
                    </button>
                    <button
                      key="quarantine"
                      type="button"
                      disabled={pending !== null || selectedCount === 0}
                      onClick={() => void run("quarantine")}
                      className="ring-focus rounded-xl bg-[#ffd166] px-3.5 py-2.5 text-[13px] font-bold text-[#211707] transition-colors hover:bg-[#ffe09a] disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      {pending === "quarantine"
                        ? "Moving files…"
                        : continuation && allSelected
                          ? "Quarantine & continue"
                          : "Quarantine selected"}
                    </button>
                  </>
                )}
              </div>
            </footer>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
