// Long-lived VM E2E test plugin: multiple handlers staying registered.
// Registers two handlers for the same event, verifies both fire on each dispatch.

pi.on('multi_evt', function (data, ctx) {
  pi.log('handler_1 fired seq=' + (data && data.seq));
});

pi.on('multi_evt', function (data, ctx) {
  pi.log('handler_2 fired seq=' + (data && data.seq));
});

__pi_start_event_loop();
