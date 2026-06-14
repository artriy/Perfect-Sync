import { Component, type ReactNode } from "react";

interface State {
  error: Error | null;
}

/** Catches render errors so a bug shows a message instead of a blank window. */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error) {
    console.error("UI crash:", error);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="grid h-[100dvh] place-items-center p-8 text-center">
          <div className="glass-strong max-w-lg rounded-2xl p-6">
            <h2 className="text-[18px] font-semibold text-ink">Something went wrong</h2>
            <p className="mt-2 font-mono text-[12.5px] break-words text-ink-dim">
              {String(this.state.error.message || this.state.error)}
            </p>
            <button
              type="button"
              onClick={() => location.reload()}
              className="ring-focus accent-grad mt-4 rounded-xl px-4 py-2 text-[13px] font-bold text-[#0d0820]"
            >
              Reload
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
