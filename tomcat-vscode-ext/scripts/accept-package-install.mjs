#!/usr/bin/env node

// Legacy entry point retained for convenience: GUI shell smoke only.
// This does not open the installation dialog or inject a host confirmation.
import { runGuiRenderSmoke } from "./run-gui-acceptance.mjs";

runGuiRenderSmoke({ scenario: "package-install" }).catch((error) => {
  console.error(`[package-install shell smoke] ${error.stack ?? error.message}`);
  process.exitCode = 1;
});
