import { Component, createRef, type ReactNode } from "react";

interface State {
  hasError: boolean;
  error: unknown;
}

function formatThrownValue(error: unknown): string {
  if (error instanceof Error) {
    return error.message || error.name || "An error was thrown without a message.";
  }
  if (typeof error === "string") return error || "An empty error message was thrown.";
  if (error == null) return `A rendering error was thrown without details (${String(error)}).`;

  try {
    if (typeof error === "object") {
      const serialized = JSON.stringify(error);
      if (serialized && serialized !== "{}") return serialized;
    }
    const value = String(error);
    return value || "An unknown rendering error occurred.";
  } catch {
    return "An unknown rendering error occurred.";
  }
}

/** Catches render errors so a bug shows a message instead of a blank window. */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { hasError: false, error: null };
  private readonly recoveryRef = createRef<HTMLDivElement>();

  static getDerivedStateFromError(error: unknown): State {
    return { hasError: true, error };
  }

  componentDidCatch(error: unknown) {
    console.error("UI crash:", error);
    this.recoveryRef.current?.focus();
  }

  render() {
    if (this.state.hasError) {
      return (
        <main className="grid min-h-[100dvh] min-w-0 place-items-center p-4 text-center sm:p-8">
          <div
            ref={this.recoveryRef}
            role="alert"
            aria-live="assertive"
            aria-labelledby="fatal-error-title"
            aria-describedby="fatal-error-details"
            tabIndex={-1}
            className="glass-strong max-h-[calc(100dvh-2rem)] w-full min-w-0 max-w-lg overflow-y-auto rounded-2xl p-6"
          >
            <h1 id="fatal-error-title" className="text-[18px] font-semibold text-ink">
              Something went wrong
            </h1>
            <p
              id="fatal-error-details"
              className="mt-2 min-w-0 break-words font-mono text-[12.5px] text-ink-dim [overflow-wrap:anywhere]"
            >
              {formatThrownValue(this.state.error)}
            </p>
            <button
              type="button"
              onClick={() => window.location.reload()}
              className="ring-focus accent-grad mt-4 rounded-xl px-4 py-2 text-[13px] font-bold text-[#0d0820]"
            >
              Reload
            </button>
          </div>
        </main>
      );
    }
    return this.props.children;
  }
}
