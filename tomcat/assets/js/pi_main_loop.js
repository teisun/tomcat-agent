// pi_main_loop.js — async main loop for long-lived VM sessions
//
// Injected at the tail of the combined script by instance_wasmedge.rs init_vm().
// Wraps __pi_start_event_loop in a for(;;) that handles command_invoke events
// asynchronously. When the event loop exits (command_invoke received), the
// pending command is awaited here so that run_loop_without_io + tokio can
// drive Promises and timers.
//
// Dependencies (must be on globalThis before this script runs):
//   __pi_start_event_loop  — synchronous event loop (pi_bridge.js)
//   __pi_pending_command_invoke — set by event loop on command_invoke
//   __pi_hostCall           — synchronous host call helper (pi_bridge.js)
//   __pi_build_ctx          — context constructor (pi_bridge.js)
//   __pi_commands           — registered command handlers (pi_bridge.js)
//
// See: docs/reports/async-handler-in-long-lived-vm.md

(async function __pi_main_loop() {
  for (;;) {
    globalThis.__pi_pending_command_invoke = null;
    await __pi_start_event_loop();
    var cmd = globalThis.__pi_pending_command_invoke;
    if (!cmd) break;
    try {
      var ctx = __pi_build_ctx(cmd.context || {});
      if (typeof __pi_budget_reset === 'function') {
        __pi_budget_reset();
      }
      if (cmd.data && cmd.data.kind === 'tool') {
        var toolResult = await __pi_execute_tool_async(JSON.stringify({
          toolCallId: cmd.data.callId,
          toolName: cmd.data.toolName,
          params: cmd.data.params,
          ctx: ctx
        }));
        if (!toolResult || !toolResult.ok) {
          try {
            __pi_hostCall('context', 'commandFailed', {
              name: cmd.data.toolName,
              callId: cmd.data.callId,
              error: toolResult && toolResult.error ? String(toolResult.error) : 'tool execution failed'
            });
          } catch (_) {}
          continue;
        }
        try {
          __pi_hostCall('context', 'commandCompleted', {
            name: cmd.data.toolName,
            callId: cmd.data.callId,
            result: toolResult.data
          });
        } catch (_) {}
        continue;
      }

      var cmdName = cmd.data && cmd.data.name;
      var cmdArgs = (cmd.data && cmd.data.args) || '';
      var entry = globalThis.__pi_commands && globalThis.__pi_commands[cmdName];
      if (!entry || typeof entry.handler !== 'function') {
        try { __pi_hostCall('context', 'commandFailed', { name: cmdName, error: 'unknown command' }); } catch(_){}
        continue;
      }
      await entry.handler(cmdArgs, ctx);
      try { __pi_hostCall('context', 'commandCompleted', { name: cmdName, callId: cmd.data && cmd.data.callId }); } catch(_){}
    } catch (err) {
      var failedName = (cmd.data && (cmd.data.toolName || cmd.data.name)) || '';
      try { __pi_hostCall('context', 'commandFailed', { name: failedName, callId: cmd.data && cmd.data.callId, error: String(err) }); } catch(_){}
      try {
        if (typeof __pi_interrupt_reason === 'function' && __pi_interrupt_reason()) {
          globalThis.__pi_last_fatal_error = String(err);
          throw err;
        }
      } catch (interruptErr) {
        throw interruptErr;
      }
    }
  }
})();
