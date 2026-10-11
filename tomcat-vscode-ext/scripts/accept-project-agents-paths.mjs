#!/usr/bin/env node

// Legacy entry point retained for convenience: GUI shell smoke only.
// This does not open project settings, inject host replies or migrate paths.
import { runGuiRenderSmoke } from "./run-gui-acceptance.mjs";

runGuiRenderSmoke({ scenario: "project-agents-paths" }).catch((error) => {
  console.error(`[project-agents-paths shell smoke] ${error.stack ?? error.message}`);
  process.exitCode = 1;
});
