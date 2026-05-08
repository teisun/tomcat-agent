// TASK-05c Tier 2：registerCommand + __pi_invoke_command、registerTool(schema 包装)、
// ctx.ui 等价 host 调用、executeBash + args（不经 pi.exec Promise，直接 host 请求以适配同步 _start）。
// 与 primitives_test.js 一致：依赖全局 __pi_host_call；pi_bridge 由宿主注入在脚本之前。

function hostCall(module, method, params) {
  if (typeof __pi_host_call !== 'function') {
    throw new Error('__pi_host_call not exposed');
  }
  var req = JSON.stringify({ module: module, method: method, params: params || {} });
  var res = __pi_host_call(req);
  return typeof res === 'string' ? JSON.parse(res) : res;
}

pi.registerCommand('tier2-e2e-cmd', {
  description: 'tier2 e2e',
  handler: function () {
    return 'ran';
  }
});
var inv = JSON.parse(__pi_invoke_command('tier2-e2e-cmd', '{}'));
if (!inv.ok) {
  throw new Error('__pi_invoke_command failed: ' + inv.error);
}
if (inv.data !== 'ran') {
  throw new Error('expected data ran, got ' + inv.data);
}

pi.registerTool({
  name: 'tier2_tool',
  label: 'Tier2',
  description: 'tool with wrapped schema',
  parameters: { schema: { type: 'object', properties: { q: { type: 'string' } } } }
});

var sel = hostCall('context', 'uiSelect', { title: 'pick', options: ['x', 'y'] });
if (!sel.ok) throw new Error('uiSelect');
var conf = hostCall('context', 'uiConfirm', { title: 'c', message: 'ok?' });
if (!conf.ok) throw new Error('uiConfirm');
var inp = hostCall('context', 'uiInput', { title: 'in', placeholder: 'ph' });
if (!inp.ok) throw new Error('uiInput');
var st = hostCall('context', 'uiSetStatus', { message: 'ready', details: null });
if (!st.ok) throw new Error('uiSetStatus');

var bash = hostCall('fs', 'executeBash', { command: 'echo', args: ['tier2', 'argv'] });
if (!bash.ok) throw new Error('executeBash argv');

print('tier2_compat_test.js: done');
