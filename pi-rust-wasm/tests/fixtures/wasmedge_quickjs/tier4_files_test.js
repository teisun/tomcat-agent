// TASK-05d d.6 Tier 4：files.ts 核心路径 fixture
// 验证：registerCommand("files") → invoke → ctx.sessionManager.getBranch() → 遍历 entries → ctx.ui.custom(factory) → SelectList 渲染
// 依赖全局 __pi_host_call；pi_bridge + shims 由宿主注入在脚本之前。

var Container = globalThis.__pi_tui.Container;
var Text = globalThis.__pi_tui.Text;
var SelectList = globalThis.__pi_tui.SelectList;
var Key = globalThis.__pi_tui.Key;
var matchesKey = globalThis.__pi_tui.matchesKey;
var DynamicBorder = globalThis.__pi_coding_agent.DynamicBorder || globalThis.__pi_tui.DynamicBorder;

if (!Container) throw new Error('Container shim missing');
if (!SelectList) throw new Error('SelectList shim missing');

function hostCall(module, method, params) {
  var req = JSON.stringify({ module: module, method: method, params: params || {} });
  var res = __pi_host_call(req);
  return typeof res === 'string' ? JSON.parse(res) : res;
}

var filesRan = false;

pi.registerCommand('files', {
  description: 'Show session files',
  handler: function (ctx) {
    var branch = ctx.sessionManager.getBranch();
    if (!Array.isArray(branch)) {
      throw new Error('getBranch() did not return array, got: ' + typeof branch);
    }

    var filePaths = [];
    for (var i = 0; i < branch.length; i++) {
      var entry = branch[i];
      if (!entry) continue;
      var msg = entry.message || entry.Message || {};
      if (typeof msg === 'object' && msg.content) {
        var content = Array.isArray(msg.content) ? msg.content : [msg.content];
        for (var j = 0; j < content.length; j++) {
          var block = content[j];
          if (block && block.type === 'toolCall' && block.name === 'write') {
            var path = (block.args && block.args.path) || (block.input && block.input.path);
            if (path && filePaths.indexOf(path) === -1) filePaths.push(path);
          }
          if (block && block.type === 'toolResult' && block.name === 'read') {
            var rpath = (block.args && block.args.path) || (block.input && block.input.path);
            if (rpath && filePaths.indexOf(rpath) === -1) filePaths.push(rpath);
          }
        }
      }
    }

    if (filePaths.length === 0) {
      ctx.ui.notify('No files found in session', 'info');
      filesRan = true;
      return;
    }

    ctx.ui.custom(function (tui, theme, _kb, done) {
      var container = new Container();
      container.addChild(new DynamicBorder(function (s) { return s; }));
      container.addChild(new Text(' Session files (' + filePaths.length + ')', 0, 0));

      var items = filePaths.map(function (p) { return { value: p, label: p }; });
      var selectList = new SelectList(items, Math.min(items.length, 15), {});
      selectList.onSelect = function (item) { done(item.value); };
      selectList.onCancel = function () { done(); };
      container.addChild(selectList);

      container.addChild(new Text(' enter select, esc close', 0, 0));
      container.addChild(new DynamicBorder(function (s) { return s; }));

      return {
        render: function (w) { return container.render(w); },
        invalidate: function () { container.invalidate(); },
        handleInput: function (data) {
          selectList.handleInput(data);
          tui.requestRender();
        }
      };
    });

    filesRan = true;
  }
});

var inv = JSON.parse(__pi_invoke_command('files', '{}'));
if (!inv.ok) throw new Error('invoke files failed: ' + (inv.error || JSON.stringify(inv)));
if (!filesRan) throw new Error('files handler did not run');

print('tier4_files_test.js: done');
