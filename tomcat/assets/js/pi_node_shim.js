// pi_node_shim.js — intentionally tiny compatibility aliases for Tier-A globals.
// Sensitive capabilities must go through `pi.*` hostcalls, not ambient Node modules.
(function () {
  'use strict';

  function unsupported(message) {
    return function () {
      throw new Error(message);
    };
  }

  function asyncUnsupported(message) {
    return function () {
      return Promise.reject(new Error(message));
    };
  }

  var fsMessage = 'node:fs is not available in QuickJS plugins; use pi.readFile/pi.writeFile/pi.editFile instead';
  var execMessage = 'node:child_process is not available in QuickJS plugins; use pi.exec instead';
  var osMessage = 'node:os is not available in QuickJS plugins; prefer pi context/session APIs instead';

  var _fs = {
    existsSync: unsupported(fsMessage),
    readFileSync: unsupported(fsMessage),
    writeFileSync: unsupported(fsMessage),
    appendFileSync: unsupported(fsMessage),
    mkdirSync: unsupported(fsMessage),
    readdirSync: unsupported(fsMessage),
    statSync: unsupported(fsMessage),
    unlinkSync: unsupported(fsMessage),
    rmdirSync: unsupported(fsMessage),
    createReadStream: unsupported(fsMessage),
    createWriteStream: unsupported(fsMessage),
    constants: { F_OK: 0, R_OK: 4, W_OK: 2, X_OK: 1 },
    "default": null
  };
  _fs["default"] = _fs;
  globalThis.__node_fs = _fs;

  var _fsp = {
    access: asyncUnsupported(fsMessage),
    readFile: asyncUnsupported(fsMessage),
    writeFile: asyncUnsupported(fsMessage),
    appendFile: asyncUnsupported(fsMessage),
    mkdir: asyncUnsupported(fsMessage),
    mkdtemp: asyncUnsupported(fsMessage),
    readdir: asyncUnsupported(fsMessage),
    stat: asyncUnsupported(fsMessage),
    unlink: asyncUnsupported(fsMessage),
    rm: asyncUnsupported(fsMessage),
    constants: _fs.constants,
    "default": null
  };
  _fsp["default"] = _fsp;
  globalThis.__node_fs_promises = _fsp;

  var _path = globalThis.path || {};
  _path["default"] = _path;
  globalThis.__node_path = _path;

  var _util = globalThis.util || globalThis.__pi_util || {};
  _util["default"] = _util;
  globalThis.__node_util = _util;

  var _events = globalThis.events || { EventEmitter: globalThis.EventEmitter };
  _events["default"] = _events;
  globalThis.__node_events = _events;

  var _buffer = {
    Buffer: globalThis.Buffer,
    "default": null
  };
  _buffer["default"] = _buffer;
  globalThis.__node_buffer = _buffer;

  var _crypto = globalThis.crypto || {};
  _crypto["default"] = _crypto;
  globalThis.__node_crypto = _crypto;

  var _cp = {
    spawn: unsupported(execMessage),
    execSync: unsupported(execMessage),
    exec: unsupported(execMessage),
    "default": null
  };
  _cp["default"] = _cp;
  globalThis.__node_child_process = _cp;

  var _os = {
    tmpdir: unsupported(osMessage),
    homedir: unsupported(osMessage),
    platform: unsupported(osMessage),
    type: unsupported(osMessage),
    arch: unsupported(osMessage),
    cpus: unsupported(osMessage),
    totalmem: unsupported(osMessage),
    freemem: unsupported(osMessage),
    EOL: '\n',
    "default": null
  };
  _os["default"] = _os;
  globalThis.__node_os = _os;

})();
