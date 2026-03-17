// Long-lived VM E2E: setInterval runs during session (E2E-WASM-033).
// Fires pi.log every 200ms; host_call count should be > 1 after sleep.

var tickCount = 0;
setInterval(function () {
  tickCount++;
  pi.log('vm_actor_set_interval: tick=' + tickCount);
}, 200);

__pi_start_event_loop();
