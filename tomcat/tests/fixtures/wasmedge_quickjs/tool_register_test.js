// Tool registration E2E fixture (E2E-WASM-011).
// Registers a dummy tool via pi.registerTool and then calls pi.log to
// produce an additional host_call, verifying the full registration flow.

if (typeof pi === 'undefined' || pi === null) {
  throw new Error('pi global not found; pi_bridge.js must be preloaded');
}

pi.registerTool({
  name: 'e2e_test_tool',
  description: 'E2E tool registration test',
  parameters: {
    type: 'object',
    properties: {
      input: { type: 'string', description: 'test input' }
    },
    required: []
  },
  execute: function (params) {
    return { result: 'ok', input: params.input };
  }
});

pi.log('tool_register_test: registerTool completed');
