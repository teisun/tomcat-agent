#!/usr/bin/env node

// This is deliberately browser-only evidence. The native and serve integration
// suites prove the confirmation reaches disk and audit storage; the simulated host
// reply below makes that boundary explicit in the saved evidence.

import { runGuiAcceptance } from "./run-gui-acceptance.mjs";

runGuiAcceptance({
  scenario: "package-install",
  simulatedReplies: [
    {
      type: "control_request",
      subtype: "confirmation",
      operation: "Write",
    },
    {
      type: "control_response",
      payload: { decision: "allow_once" },
    },
  ],
}).catch((error) => {
  console.error(`[accept-package-install] ${error.stack ?? error.message}`);
  process.exitCode = 1;
});
