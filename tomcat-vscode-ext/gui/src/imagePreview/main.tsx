import "../i18n/hostLocale";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@vscode/codicons/dist/codicon.css";

import { PreviewPanel } from "./PreviewPanel";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <LocaleProvider><PreviewPanel /></LocaleProvider>
  </StrictMode>,
);
