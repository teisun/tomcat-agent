import { Component, type ErrorInfo, type ReactNode, useEffect, useState } from "react";
import { useT } from "./i18n/LocaleProvider";

type ReportError = (error: Error) => void;

type BoundaryProps = {
  children: ReactNode;
  reportError: ReportError;
};

type BoundaryState = {
  error: Error | null;
};

function normalizeError(value: unknown): Error {
  if (value instanceof Error) {
    return value;
  }
  if (typeof value === "string" && value.trim()) {
    return new Error(value);
  }
  try {
    const detail = JSON.stringify(value);
    return new Error(detail && detail !== "{}" ? detail : "Unknown webview error");
  } catch {
    return new Error("Unknown webview error");
  }
}

function WebviewErrorFallback({ error }: { error: Error }) {
  const t = useT();
  return (
    <main aria-label={t("webview.error.aria")} className="tc-webview-error" data-testid="webview-error-fallback">
      <h1>{t("webview.error.title")}</h1>
      <p>{t("webview.error.description")}</p>
      <pre>{error.message || "Unknown webview error"}</pre>
      <button onClick={() => window.location.reload()} type="button">
        {t("common.reload")}
      </button>
    </main>
  );
}

class RenderErrorBoundary extends Component<BoundaryProps, BoundaryState> {
  state: BoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): BoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, _info: ErrorInfo): void {
    this.props.reportError(error);
  }

  render() {
    return this.state.error ? (
      <WebviewErrorFallback error={this.state.error} />
    ) : (
      this.props.children
    );
  }
}

/**
 * Keeps a render exception, a synchronous browser error, and a rejected async
 * operation from degrading the whole webview to an unhelpful blank screen.
 */
export function WebviewErrorBoundary({ children, reportError }: BoundaryProps) {
  const [globalError, setGlobalError] = useState<Error | null>(null);

  useEffect(() => {
    const report = (error: unknown) => {
      const normalized = normalizeError(error);
      reportError(normalized);
      setGlobalError(normalized);
    };
    const onError = (event: ErrorEvent) => report(event.error ?? event.message);
    const onUnhandledRejection = (event: PromiseRejectionEvent) => {
      // The report has a durable host-side destination; suppress Chromium's duplicate
      // unhandled-rejection diagnostic after it has been handed over.
      event.preventDefault();
      report(event.reason);
    };

    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onUnhandledRejection);
    return () => {
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onUnhandledRejection);
    };
  }, [reportError]);

  return globalError ? (
    <WebviewErrorFallback error={globalError} />
  ) : (
    <RenderErrorBoundary reportError={reportError}>{children}</RenderErrorBoundary>
  );
}
