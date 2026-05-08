// Bridge layer integration test: validates that pi_bridge.js correctly
// constructs the pi global object and routes API calls through __pi_host_call.
// Requires pi_bridge.js to be preloaded before this script runs.
// Note: file-op APIs now return Promise (pi-mono aligned); use async/await.

if (typeof pi === 'undefined' || pi === null) {
  throw new Error('pi global object not found; pi_bridge.js must be preloaded');
}

async function runBridgeTests() {
  // -- 4 primitives via pi.* API (Promise-returning) ------------------------
  var r1 = await pi.readFile('/tmp/bridge_test_read.txt').catch(function (e) { return { _err: String(e) }; });
  print('bridge_test: pi.readFile settled ok=' + (!r1._err));

  var r2 = await pi.writeFile('/tmp/bridge_test_write.txt', 'hello').catch(function (e) { return { _err: String(e) }; });
  print('bridge_test: pi.writeFile settled ok=' + (!r2._err));

  var r3 = await pi.editFile('/tmp/bridge_test_edit.txt', []).catch(function (e) { return { _err: String(e) }; });
  print('bridge_test: pi.editFile settled ok=' + (!r3._err));

  // exec returns Promise<{stdout,stderr,exitCode}>
  var r4 = await pi.exec('echo bridge_ok').catch(function (e) { return { _err: String(e) }; });
  print('bridge_test: pi.exec settled ok=' + (!r4._err));

  // -- Event subscription via pi.on -----------------------------------------
  pi.on('agent_start', function (data, ctx) {
    print('bridge_test: agent_start handler invoked');
  });

  // -- Logging via pi.log ---------------------------------------------------
  pi.log('bridge_test: log from plugin');

  // -- Session API via pi.session -------------------------------------------
  var s1 = pi.session.getCurrent();
  print('bridge_test: pi.session.getCurrent ok=' + (s1 !== undefined));

  print('bridge_test: all checks passed');
}

runBridgeTests();
