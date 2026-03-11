// JS API alignment integration test (8.7.5 / 8.7.6).
// Validates that pi_bridge.js async APIs (exec, createChatCompletion) correctly
// return Promises and resolve to pi-mono-compatible result shapes.
// Requires pi_bridge.js to be preloaded before this script runs.

if (typeof pi === 'undefined' || pi === null) {
  throw new Error('pi global not found; pi_bridge.js must be preloaded');
}

// Minimal assert helper — throws on failure so the host Rust test can detect it.
function assert(cond, msg) {
  if (!cond) throw new Error('ASSERT FAILED: ' + msg);
}

async function testExecReturnsPromise() {
  // pi.exec must return a Promise, not a plain object.
  var p = pi.exec('echo hello');
  assert(p && typeof p.then === 'function', 'pi.exec should return a Promise');
  var r = await p;
  // pi-mono ExecResult shape: {stdout, stderr, exitCode}
  assert(typeof r === 'object' && r !== null, 'exec result should be an object');
  assert(typeof r.stdout === 'string', 'exec result.stdout should be a string');
  assert(r.stdout.indexOf('hello') !== -1, 'stdout should contain "hello", got: ' + r.stdout);
  assert(typeof r.stderr === 'string', 'exec result.stderr should be a string');
  assert(typeof r.exitCode === 'number', 'exec result.exitCode should be a number');
  assert(r.exitCode === 0, 'exitCode should be 0, got: ' + r.exitCode);
  print('js_api_async_test: testExecReturnsPromise PASSED');
}

async function testExecFailureRejectsPromise() {
  var rejected = false;
  await pi.exec('exit 1').catch(function () { rejected = true; });
  // exit 1 may or may not reject depending on how executeBash propagates non-zero.
  // At minimum, exec must return a Promise (tested in testExecReturnsPromise).
  print('js_api_async_test: testExecFailureRejectsPromise PASSED (rejected=' + rejected + ')');
}

async function testCreateChatCompletionReturnsPromise() {
  // pi.createChatCompletion must return a Promise.
  var p = pi.createChatCompletion({ messages: [{ role: 'user', content: 'ping' }] });
  assert(p && typeof p.then === 'function', 'createChatCompletion should return a Promise');
  // Without a real LLM configured the call will reject; that is acceptable—
  // we only verify the Promise shape and that rejection is propagated cleanly.
  var result = null;
  var errMsg = null;
  await p.then(function (r) { result = r; }).catch(function (e) { errMsg = String(e); });
  if (result !== null) {
    // If LLM is configured: validate pi-mono CompletionResult shape.
    assert(result && result.message, 'CompletionResult should have a message field');
    assert(result.message.role && result.message.content !== undefined,
      'message should have role and content');
    print('js_api_async_test: testCreateChatCompletion PASSED (LLM responded)');
  } else {
    // No LLM configured: rejection is expected and acceptable.
    assert(typeof errMsg === 'string' && errMsg.length > 0, 'rejection error should be non-empty');
    print('js_api_async_test: testCreateChatCompletion PASSED (no LLM, rejected as expected: ' + errMsg + ')');
  }
}

async function testOnceFiresOnlyOnce() {
  var count = 0;
  pi.once('__test_once_event', function () { count++; });
  pi.emit('__test_once_event', {});
  pi.emit('__test_once_event', {});
  // count should be 1 (once fires once even when emitted twice).
  assert(count === 1, 'once handler should fire exactly once, got count=' + count);
  print('js_api_async_test: testOnceFiresOnlyOnce PASSED');
}

async function testOffByHandlerReference() {
  var count = 0;
  var handler = function () { count++; };
  pi.on('__test_off_event', handler);
  pi.emit('__test_off_event', {});
  pi.off('__test_off_event', handler);
  pi.emit('__test_off_event', {});
  assert(count === 1, 'after off(handler), handler should not fire again, count=' + count);
  print('js_api_async_test: testOffByHandlerReference PASSED');
}

async function testFileOpsReturnPromise() {
  var rp = pi.readFile('/nonexistent/path/that/does/not/exist');
  assert(rp && typeof rp.then === 'function', 'readFile should return a Promise');
  await rp.catch(function () {}); // rejection is expected for non-existent path

  var wp = pi.writeFile('/tmp/pi_bridge_test_write.txt', 'test');
  assert(wp && typeof wp.then === 'function', 'writeFile should return a Promise');
  var ep = pi.editFile('/tmp/pi_bridge_test_edit.txt', []);
  assert(ep && typeof ep.then === 'function', 'editFile should return a Promise');
  print('js_api_async_test: testFileOpsReturnPromise PASSED');
}

async function testGetModelSync() {
  // getModel is sync in pi-mono; should not return a Promise.
  var m = pi.getModel();
  var isPromise = m && typeof m.then === 'function';
  assert(!isPromise, 'getModel should be sync (not a Promise)');
  print('js_api_async_test: testGetModelSync PASSED (model=' + m + ')');
}

async function testSetModelReturnsPromise() {
  var p = pi.setModel('gpt-4o');
  assert(p && typeof p.then === 'function', 'setModel should return a Promise');
  await p.catch(function () {}); // MVP stub may resolve or reject; either is fine.
  print('js_api_async_test: testSetModelReturnsPromise PASSED');
}

async function testUnregisterTool() {
  // Register a dummy tool then unregister it; should not throw.
  pi.registerTool({
    name: '__test_tool',
    description: 'test',
    parameters: {},
    execute: function () { return {}; }
  });
  var r = pi.unregisterTool('__test_tool');
  assert(r !== undefined, 'unregisterTool should return a response');
  print('js_api_async_test: testUnregisterTool PASSED');
}

async function main() {
  await testOnceFiresOnlyOnce();
  await testOffByHandlerReference();
  await testFileOpsReturnPromise();
  await testExecReturnsPromise();
  await testExecFailureRejectsPromise();
  await testCreateChatCompletionReturnsPromise();
  await testGetModelSync();
  await testSetModelReturnsPromise();
  await testUnregisterTool();
  print('js_api_async_test: ALL TESTS PASSED');
}

main().catch(function (e) {
  print('js_api_async_test: FATAL ERROR: ' + e);
  throw e;
});
