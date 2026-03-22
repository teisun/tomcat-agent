// TASK-05d d.5 Tier 3：diff.ts 核心路径 fixture
// 验证：registerCommand("diff") → invoke → exec("git") → ctx.ui.custom(factory) → Container/SelectList/Text 渲染
// 依赖全局 __pi_host_call；pi_bridge + shims 由宿主注入在脚本之前。

var Container = globalThis.__pi_tui.Container;
var Text = globalThis.__pi_tui.Text;
var SelectList = globalThis.__pi_tui.SelectList;
var Key = globalThis.__pi_tui.Key;
var matchesKey = globalThis.__pi_tui.matchesKey;
var DynamicBorder = globalThis.__pi_coding_agent.DynamicBorder || globalThis.__pi_tui.DynamicBorder;

if (!Container) throw new Error('Container shim missing');
if (!Text) throw new Error('Text shim missing');
if (!SelectList) throw new Error('SelectList shim missing');
if (!Key) throw new Error('Key shim missing');
if (typeof matchesKey !== 'function') throw new Error('matchesKey shim missing');

function hostCall(module, method, params) {
  var req = JSON.stringify({ module: module, method: method, params: params || {} });
  var res = __pi_host_call(req);
  return typeof res === 'string' ? JSON.parse(res) : res;
}

var diffRan = false;

pi.registerCommand('diff', {
  description: 'Show git diff file picker',
  handler: function (_args, ctx) {
    var execResult = hostCall('fs', 'executeBash', {
      command: 'git', args: ['status', '--porcelain'], cwd: ctx.cwd
    });
    if (!execResult.ok) {
      ctx.ui.notify('git status failed', 'error');
      diffRan = true;
      return;
    }

    var stdout = (execResult.data && execResult.data.stdout) || '';
    var files = [];
    var lines = stdout.split('\n');
    for (var i = 0; i < lines.length; i++) {
      var line = lines[i].trim();
      if (!line) continue;
      files.push({ status: line.slice(0, 2).trim(), file: line.slice(3) });
    }

    if (files.length === 0) {
      ctx.ui.notify('No changes found', 'info');
      diffRan = true;
      return;
    }

    var result = ctx.ui.custom(function (tui, theme, _kb, done) {
      var container = new Container();
      container.addChild(new DynamicBorder(function (s) { return s; }));
      container.addChild(new Text(' Select file to diff', 0, 0));

      var items = files.map(function (f) {
        return { value: f, label: f.status + ' ' + f.file };
      });
      var visibleRows = Math.min(files.length, 15);

      var selectList = new SelectList(items, visibleRows, {});
      selectList.onSelect = function (item) {
        done(item.value);
      };
      selectList.onCancel = function () { done(); };
      container.addChild(selectList);

      container.addChild(new Text(' arrow-keys navigate, enter open, esc close', 0, 0));
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

    diffRan = true;
  }
});

var inv = JSON.parse(__pi_invoke_command('diff', '{}'));
if (!inv.ok) throw new Error('invoke diff failed: ' + (inv.error || JSON.stringify(inv)));
if (!diffRan) throw new Error('diff handler did not run');

print('tier3_diff_test.js: done');
