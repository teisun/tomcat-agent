import { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";

import { App } from "./App";
import { WebviewErrorBoundary } from "./WebviewErrorBoundary";
import "@vscode/codicons/dist/codicon.css";
import "./styles.css";
import type { VsCodeApiLike } from "./types";

declare global {
  interface Window {
    acquireVsCodeApi?: () => VsCodeApiLike;
  }
}

const vscodeApi: VsCodeApiLike =
  window.acquireVsCodeApi?.() ?? {
    postMessage() {},
    setState() {},
  };

const root = document.getElementById("root");
if (!root) {
  throw new Error("Tomcat webview root element was not found");
}

let webviewErrorReported = false;

function reportWebviewError(error: Error): void {
  webviewErrorReported = true;
  vscodeApi.postMessage({
    data: {
      message: error.message || "Unknown webview error",
      stack: error.stack,
    },
    messageId: `webview-error-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    type: "webviewError",
  });
}

const ERROR_BOUNDARY_CRASH_FIXTURE_TYPE = "__test.webview_error_boundary_crash";
const ERROR_BOUNDARY_CRASH_MESSAGE =
  "E2E fixture intentionally crashed the Tomcat webview";

function isErrorBoundaryCrashFixture(value: unknown): boolean {
  if (!value || typeof value !== "object") {
    return false;
  }
  const frame = value as {
    channel?: unknown;
    content?: { enabled?: unknown; type?: unknown };
  };
  return (
    frame.channel === "event" &&
    frame.content?.type === ERROR_BOUNDARY_CRASH_FIXTURE_TYPE &&
    frame.content.enabled === true
  );
}

function isDomSnapshotRequest(value: unknown): value is { messageId: string } {
  if (!value || typeof value !== "object") {
    return false;
  }
  const frame = value as {
    channel?: unknown;
    content?: { type?: unknown };
    messageId?: unknown;
  };
  return (
    frame.channel === "event" &&
    frame.content?.type === "__test.capture_dom" &&
    typeof frame.messageId === "string"
  );
}

function ErrorFallbackDomSnapshotResponder() {
  useEffect(() => {
    const onMessage = (event: MessageEvent<unknown>) => {
      if (!webviewErrorReported || !isDomSnapshotRequest(event.data)) {
        return;
      }
      vscodeApi.postMessage({
        data: { html: document.documentElement.outerHTML },
        messageId: event.data.messageId,
        type: "__test.dom_fallback_snapshot",
      });
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, []);

  return null;
}

/**
 * This hook only reacts to the extension host's test-only event channel. Production
 * hosts never emit that frame, so the component stays inert outside the packaged E2E.
 */
function ErrorBoundaryCrashFixture() {
  const [enabled, setEnabled] = useState(false);

  useEffect(() => {
    const onMessage = (event: MessageEvent<unknown>) => {
      if (isErrorBoundaryCrashFixture(event.data)) {
        setEnabled(true);
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, []);

  if (enabled) {
    throw new Error(ERROR_BOUNDARY_CRASH_MESSAGE);
  }
  return null;
}

ReactDOM.createRoot(root).render(
  <>
    <ErrorFallbackDomSnapshotResponder />
    <WebviewErrorBoundary reportError={reportWebviewError}>
      <ErrorBoundaryCrashFixture />
      <App vscodeApi={vscodeApi} />
    </WebviewErrorBoundary>
  </>,
);
