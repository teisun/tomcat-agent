// 4 原语 host 调用测试：依次调用 readFile、writeFile、editFile、executeBash。
// 约定：宿主通过 env.__pi_host_call 注入，在 QuickJS 中暴露为全局 __pi_host_call(requestJson) 返回 responseJson。
// 若运行时未暴露该 API 则抛错，与 INTEGRATION_TEST_SPEC 5.4「失败即失败」一致；e2e 须断言 4 次 host 调用（Constitution 不得降低断言）。

function hostCall(module, method, params) {
  if (typeof __pi_host_call !== 'function') {
    throw new Error('__pi_host_call not exposed to JS; wasmedge_quickjs must expose env.__pi_host_call to script (see host-call-protocol.md)');
  }
  var req = JSON.stringify({ module: module, method: method, params: params || {} });
  var res = __pi_host_call(req);
  return typeof res === 'string' ? JSON.parse(res) : res;
}

hostCall('fs', 'readFile', { path: '/tmp/pi_primitives_test_read.txt' });
hostCall('fs', 'writeFile', { path: '/tmp/pi_primitives_test_write.txt', content: 'ok' });
hostCall('fs', 'editFile', { path: '/tmp/pi_primitives_test_edit.txt', edits: [{ type: 'replace', old: 'a', new: 'b' }] });
hostCall('fs', 'executeBash', { command: 'echo ok' });

print('primitives_test.js: 4 host calls done');
