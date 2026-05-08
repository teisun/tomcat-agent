// Event dispatch integration test plugin.
// Registers a handler via pi.on(), then the host calls dispatch_event()
// which appends __pi_dispatch_event() to trigger the handler.
// The handler verifies ctx properties by calling context module methods.

pi.on('test_event', function (data, ctx) {
  print('event_dispatch_test: handler invoked, data.hello=' + data.hello);

  // Verify static ctx snapshot properties
  print('event_dispatch_test: ctx.cwd=' + ctx.cwd);
  print('event_dispatch_test: ctx.hasUI=' + ctx.hasUI);

  // Verify dynamic ctx methods (each triggers a hostCall)
  var idle = ctx.isIdle();
  print('event_dispatch_test: ctx.isIdle()=' + idle);

  var pending = ctx.hasPendingMessages();
  print('event_dispatch_test: ctx.hasPendingMessages()=' + pending);

  var prompt = ctx.getSystemPrompt();
  print('event_dispatch_test: ctx.getSystemPrompt()=' + JSON.stringify(prompt));

  var usage = ctx.getContextUsage();
  print('event_dispatch_test: ctx.getContextUsage()=' + JSON.stringify(usage));

  ctx.compact();
  print('event_dispatch_test: ctx.compact() ok');

  ctx.ui.notify('hello from handler', 'info');
  print('event_dispatch_test: ctx.ui.notify ok');

  pi.log('event_dispatch_test: handler completed');
});
