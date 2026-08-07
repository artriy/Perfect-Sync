import { invoke } from "@tauri-apps/api/core";

export type SupportLogLevel = "debug" | "info" | "warn" | "error";

let enabled = false;

export function formatSupportError(reason: unknown): string {
  if (reason instanceof Error) return `${reason.name}: ${reason.message}${reason.stack ? `\n${reason.stack}` : ""}`;
  if (typeof reason === "string") return reason;
  try {
    return JSON.stringify(reason) ?? String(reason);
  } catch {
    return String(reason);
  }
}

export function recordSupportEvent(level: SupportLogLevel, message: string): void {
  if (!enabled || !("__TAURI_INTERNALS__" in window)) return;
  void invoke<void>("record_support_event", { level, message }).catch((reason: unknown) => {
    console.warn("Could not persist a diagnostic log event:", reason);
  });
}

export function configureSupportLogging(nextEnabled: boolean): void {
  const changed = enabled !== nextEnabled;
  enabled = nextEnabled;
  if (changed && enabled) {
    recordSupportEvent("info", `Webview diagnostic logging attached; userAgent=${navigator.userAgent}`);
  }
}
