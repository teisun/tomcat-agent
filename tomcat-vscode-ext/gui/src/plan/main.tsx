import ReactDOM from "react-dom/client";

import { acquireVsCodeApiLike } from "../../../src/shared/planPreviewProtocol";
import "../styles.css";
import { PlanPreviewApp } from "./PlanPreviewApp";

const root = document.getElementById("root");
if (!root) {
  throw new Error("Tomcat plan preview root element was not found");
}

ReactDOM.createRoot(root).render(<PlanPreviewApp vscodeApi={acquireVsCodeApiLike()} />);
