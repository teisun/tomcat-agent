// pi_node_shim.js — Node.js built-in module stubs for WasmEdge QuickJS script mode.
// Plugins that `import ... from "node:fs"` etc. need these globalThis properties
// so SWC-rewritten `var { X } = globalThis.__node_fs` resolves at runtime.
(function () {
  'use strict';

  // --- fs / node:fs ---
  var _fs = {
    existsSync: function () { return false; },
    readFileSync: function (_path, _enc) { return ''; },
    writeFileSync: function () {},
    appendFileSync: function () {},
    mkdirSync: function () {},
    readdirSync: function () { return []; },
    statSync: function () { return { isFile: function () { return false; }, isDirectory: function () { return false; }, size: 0, mtime: new Date() }; },
    unlinkSync: function () {},
    rmdirSync: function () {},
    createReadStream: function () { return { on: function () { return this; }, pipe: function () { return this; } }; },
    createWriteStream: function () { return { write: function () {}, end: function () {}, on: function () { return this; } }; },
    constants: { F_OK: 0, R_OK: 4, W_OK: 2, X_OK: 1 },
    "default": null
  };
  _fs["default"] = _fs;
  globalThis.__node_fs = _fs;

  // --- fs/promises / node:fs/promises ---
  var _fsp = {
    access: function () { return Promise.resolve(); },
    readFile: function () { return Promise.resolve(''); },
    writeFile: function () { return Promise.resolve(); },
    appendFile: function () { return Promise.resolve(); },
    mkdir: function () { return Promise.resolve(); },
    mkdtemp: function (prefix) { return Promise.resolve((prefix || '/tmp/tmp-') + 'xxxxxx'); },
    readdir: function () { return Promise.resolve([]); },
    stat: function () { return Promise.resolve({ isFile: function () { return false; }, isDirectory: function () { return false; }, size: 0 }); },
    unlink: function () { return Promise.resolve(); },
    rm: function () { return Promise.resolve(); },
    constants: _fs.constants,
    "default": null
  };
  _fsp["default"] = _fsp;
  globalThis.__node_fs_promises = _fsp;

  // --- path / node:path ---
  var _path = {
    join: function () {
      var parts = [];
      for (var i = 0; i < arguments.length; i++) {
        var p = String(arguments[i] || '');
        if (p) parts.push(p);
      }
      return parts.join('/').replace(/\/+/g, '/');
    },
    resolve: function () {
      var parts = [];
      for (var i = 0; i < arguments.length; i++) {
        var p = String(arguments[i] || '');
        if (p.charAt(0) === '/') { parts = [p]; } else if (p) { parts.push(p); }
      }
      return parts.join('/').replace(/\/+/g, '/') || '/';
    },
    dirname: function (p) {
      p = String(p || '');
      var idx = p.lastIndexOf('/');
      return idx > 0 ? p.slice(0, idx) : (idx === 0 ? '/' : '.');
    },
    basename: function (p, ext) {
      p = String(p || '');
      var base = p.split('/').pop() || '';
      if (ext && base.endsWith(ext)) base = base.slice(0, -ext.length);
      return base;
    },
    extname: function (p) {
      p = String(p || '');
      var base = p.split('/').pop() || '';
      var dot = base.lastIndexOf('.');
      return dot > 0 ? base.slice(dot) : '';
    },
    sep: '/',
    delimiter: ':',
    "default": null
  };
  _path["default"] = _path;
  globalThis.__node_path = _path;

  // --- child_process / node:child_process ---
  function MockEventEmitter() {
    this._handlers = {};
  }
  MockEventEmitter.prototype.on = function (ev, fn) { if (!this._handlers[ev]) this._handlers[ev] = []; this._handlers[ev].push(fn); return this; };
  MockEventEmitter.prototype.once = function (ev, fn) { return this.on(ev, fn); };
  MockEventEmitter.prototype.emit = function (ev) { var h = this._handlers[ev] || []; var args = [].slice.call(arguments, 1); for (var i = 0; i < h.length; i++) h[i].apply(null, args); };
  MockEventEmitter.prototype.removeListener = function () { return this; };
  MockEventEmitter.prototype.removeAllListeners = function () { this._handlers = {}; return this; };

  var _cp = {
    spawn: function (_cmd, _args, _opts) {
      var proc = new MockEventEmitter();
      proc.stdin = { write: function () {}, end: function () {} };
      proc.stdout = new MockEventEmitter();
      proc.stderr = new MockEventEmitter();
      proc.pid = 0;
      proc.kill = function () {};
      setTimeout(function () {
        proc.stdout.emit('data', '');
        proc.emit('close', 0);
        proc.emit('exit', 0);
      }, 1);
      return proc;
    },
    execSync: function () { return ''; },
    exec: function (_cmd, cb) { if (cb) setTimeout(function () { cb(null, '', ''); }, 1); },
    "default": null
  };
  _cp["default"] = _cp;
  globalThis.__node_child_process = _cp;

  // --- os / node:os ---
  var _os = {
    tmpdir: function () { return '/tmp'; },
    homedir: function () { return '/home/user'; },
    platform: function () { return 'linux'; },
    type: function () { return 'Linux'; },
    arch: function () { return 'x64'; },
    cpus: function () { return [{ model: 'stub', speed: 0, times: {} }]; },
    totalmem: function () { return 8 * 1024 * 1024 * 1024; },
    freemem: function () { return 4 * 1024 * 1024 * 1024; },
    EOL: '\n',
    "default": null
  };
  _os["default"] = _os;
  globalThis.__node_os = _os;

  // --- crypto / node:crypto ---
  var _crypto = {
    randomUUID: function () {
      var hex = '0123456789abcdef';
      var s = '';
      for (var i = 0; i < 36; i++) {
        if (i === 8 || i === 13 || i === 18 || i === 23) s += '-';
        else if (i === 14) s += '4';
        else s += hex[Math.floor(Math.random() * 16)];
      }
      return s;
    },
    randomBytes: function (n) { var b = new Uint8Array(n); for (var i = 0; i < n; i++) b[i] = Math.floor(Math.random() * 256); return b; },
    createHash: function () { return { update: function () { return this; }, digest: function () { return '0'.repeat(64); } }; },
    "default": null
  };
  _crypto["default"] = _crypto;
  globalThis.__node_crypto = _crypto;

  // --- subagent local ./agents.js stub ---
  globalThis.__pi_subagent_agents = {
    discoverAgents: function () { return []; },
    "default": { discoverAgents: function () { return []; } }
  };
})();
