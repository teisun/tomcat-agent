import ReactDOM from "react-dom/client";

import { App } from "./App";
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

ReactDOM.createRoot(root).render(<App vscodeApi={vscodeApi} />);
