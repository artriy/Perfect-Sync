import React from "react";
import { createRoot } from "react-dom/client";
import { MotionConfig } from "motion/react";
import "@fontsource-variable/outfit";
import "@fontsource-variable/jetbrains-mono";
import "./index.css";
import { App } from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { formatSupportError, recordSupportEvent } from "./lib/supportLog";

window.addEventListener("error", (event) => {
  recordSupportEvent(
    "error",
    `uncaught webview error; message=${event.message}; source=${event.filename}:${event.lineno}:${event.colno}; error=${formatSupportError(event.error)}`,
  );
});
window.addEventListener("unhandledrejection", (event) => {
  recordSupportEvent("error", `unhandled webview rejection; error=${formatSupportError(event.reason)}`);
});

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <MotionConfig reducedMotion="user">
        <App />
      </MotionConfig>
    </ErrorBoundary>
  </React.StrictMode>,
);
