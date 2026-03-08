// Bridge layer integration test: validates that pi_bridge.js correctly
// constructs the pi global object and routes API calls through __pi_host_call.
// Requires pi_bridge.js to be preloaded before this script runs.

if (typeof pi === 'undefined' || pi === null) {
  throw new Error('pi global object not found; pi_bridge.js must be preloaded');
}

// -- 4 primitives via pi.* API -------------------------------------------
var r1 = pi.readFile('/tmp/bridge_test_read.txt');
print('bridge_test: pi.readFile ok=' + (r1 && r1.ok));

var r2 = pi.writeFile('/tmp/bridge_test_write.txt', 'hello');
print('bridge_test: pi.writeFile ok=' + (r2 && r2.ok));

var r3 = pi.editFile('/tmp/bridge_test_edit.txt', []);
print('bridge_test: pi.editFile ok=' + (r3 && r3.ok));

var r4 = pi.exec('echo bridge_ok');
print('bridge_test: pi.exec ok=' + (r4 && r4.ok));

// -- Event subscription via pi.on ----------------------------------------
pi.on('agent_start', function(data, ctx) {
  print('bridge_test: agent_start handler invoked');
});

// -- Logging via pi.log --------------------------------------------------
pi.log('bridge_test: log from plugin');

// -- Session API via pi.session ------------------------------------------
var s1 = pi.session.getCurrent();
print('bridge_test: pi.session.getCurrent ok=' + (s1 !== undefined));

print('bridge_test: all checks passed');
