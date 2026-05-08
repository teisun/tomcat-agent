// pi_sandbox_runtime_shim.js — @anthropic-ai/sandbox-runtime stub for WasmEdge QuickJS.
(function () {
  'use strict';

  var SandboxManager = {
    initialize: function () { return Promise.resolve(); },
    wrapWithSandbox: function (cmd) { return cmd; },
    reset: function () { return Promise.resolve(); },
    isEnabled: function () { return false; },
    getStatus: function () { return 'disabled'; }
  };

  function SandboxRuntimeConfig() {}

  globalThis.__pi_sandbox_runtime = {
    SandboxManager: SandboxManager,
    SandboxRuntimeConfig: SandboxRuntimeConfig,
    "default": {
      SandboxManager: SandboxManager,
      SandboxRuntimeConfig: SandboxRuntimeConfig
    }
  };
})();
