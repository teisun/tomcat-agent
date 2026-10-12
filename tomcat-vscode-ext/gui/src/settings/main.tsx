import "../i18n/hostLocale";
import { LocaleProvider } from "../i18n/LocaleProvider";
import ReactDOM from "react-dom/client";

import "@vscode/codicons/dist/codicon.css";
import { acquireVsCodeApiLike } from "../../../src/shared/settingsProtocol";
import "../styles.css";
import { SettingsApp } from "./SettingsApp";

const root = document.getElementById("root");
if (!root) {
  throw new Error("Tomcat settings root element was not found");
}

const requestedRoute = document.documentElement.dataset.settingsRoute ?? new URLSearchParams(window.location.search).get("route");
const initialRoute = requestedRoute === "general" || requestedRoute === "connectors" ? requestedRoute : "models";

ReactDOM.createRoot(root).render(
  <LocaleProvider><SettingsApp
    initialRoute={initialRoute}
    vscodeApi={acquireVsCodeApiLike()}
  /></LocaleProvider>,
);
