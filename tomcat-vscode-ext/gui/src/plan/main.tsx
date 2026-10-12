import "../i18n/hostLocale";
import { LocaleProvider } from "../i18n/LocaleProvider";
import ReactDOM from "react-dom/client";
import { WebviewErrorBoundary } from "../WebviewErrorBoundary";

import { acquireVsCodeApiLike } from "../../../src/shared/planPreviewProtocol";
import "@vscode/codicons/dist/codicon.css";
import "../styles.css";
import { PlanPreviewApp } from "./PlanPreviewApp";

const root = document.getElementById("root");
if (!root) {
  throw new Error("Tomcat plan preview root element was not found");
}

function reportPlanPreviewError(error: Error): void {
  // Plan Preview has no recovery path after a render exception, but the shared
  // boundary turns it into an actionable Reload screen instead of an empty tab.
  console.error("Tomcat Plan Preview failed to render", error);
}

ReactDOM.createRoot(root).render(
  <LocaleProvider><WebviewErrorBoundary reportError={reportPlanPreviewError}>
    <PlanPreviewApp vscodeApi={acquireVsCodeApiLike()} />
  </WebviewErrorBoundary></LocaleProvider>,
);
