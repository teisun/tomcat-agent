// Long-lived VM E2E test plugin: global counter that persists across events.
// Registers a handler via pi.on() that increments a global counter
// and logs the current value via pi.log(). The host verifies host_call count
// increases with each dispatch_event.

var counter = 0;

pi.on('test_event', function (data, ctx) {
  counter++;
  pi.log('vm_actor_counter: counter=' + counter);
});

// Enter the long-lived event loop (blocks until shutdown).
__pi_start_event_loop();
