// Event once-semantics E2E fixture (E2E-WASM-022).
// Registers a handler with pi.once; the host will call dispatch_event twice.
// The handler calls pi.log() which triggers a host_call each time it fires.
// With correct once semantics, the host should see exactly 1 log call from the handler.

if (typeof pi === 'undefined' || pi === null) {
  throw new Error('pi global not found; pi_bridge.js must be preloaded');
}

pi.once('__e2e_once_event', function (data) {
  pi.log('event_once_test: handler fired data=' + JSON.stringify(data));
});
