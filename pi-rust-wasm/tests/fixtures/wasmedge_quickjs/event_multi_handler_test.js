// Multiple event handlers E2E fixture (E2E-WASM-023).
// Registers two handlers with pi.on; the host will call dispatch_event once.
// Each handler calls pi.log() which triggers a host_call.
// With correct on semantics, the host should see exactly 2 log calls.

if (typeof pi === 'undefined' || pi === null) {
  throw new Error('pi global not found; pi_bridge.js must be preloaded');
}

pi.on('__e2e_multi_event', function (data) {
  pi.log('event_multi_handler_test: handler_1 fired');
});

pi.on('__e2e_multi_event', function (data) {
  pi.log('event_multi_handler_test: handler_2 fired');
});
