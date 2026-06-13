(function () {
  'use strict';

  function utf8Bytes(input) {
    var text = String(input == null ? '' : input);
    var bytes = [];
    for (var i = 0; i < text.length; i++) {
      var code = text.charCodeAt(i);
      if (code < 0x80) {
        bytes.push(code);
      } else if (code < 0x800) {
        bytes.push(0xc0 | (code >> 6));
        bytes.push(0x80 | (code & 0x3f));
      } else if (code >= 0xd800 && code <= 0xdbff && i + 1 < text.length) {
        var next = text.charCodeAt(++i);
        var cp = ((code - 0xd800) << 10) + (next - 0xdc00) + 0x10000;
        bytes.push(0xf0 | (cp >> 18));
        bytes.push(0x80 | ((cp >> 12) & 0x3f));
        bytes.push(0x80 | ((cp >> 6) & 0x3f));
        bytes.push(0x80 | (cp & 0x3f));
      } else {
        bytes.push(0xe0 | (code >> 12));
        bytes.push(0x80 | ((code >> 6) & 0x3f));
        bytes.push(0x80 | (code & 0x3f));
      }
    }
    return new Uint8Array(bytes);
  }

  function utf8Decode(input) {
    if (input == null) return '';
    var bytes;
    if (typeof input === 'string') return input;
    if (input instanceof ArrayBuffer) {
      bytes = new Uint8Array(input);
    } else if (typeof Uint8Array !== 'undefined' && input instanceof Uint8Array) {
      bytes = input;
    } else if (Array.isArray(input)) {
      bytes = new Uint8Array(input);
    } else if (input.buffer instanceof ArrayBuffer) {
      bytes = new Uint8Array(input.buffer, input.byteOffset || 0, input.byteLength || 0);
    } else {
      return String(input);
    }

    var out = '';
    for (var i = 0; i < bytes.length;) {
      var b0 = bytes[i++];
      if (b0 < 0x80) {
        out += String.fromCharCode(b0);
        continue;
      }
      if (b0 < 0xe0) {
        var b1 = bytes[i++] & 0x3f;
        out += String.fromCharCode(((b0 & 0x1f) << 6) | b1);
        continue;
      }
      if (b0 < 0xf0) {
        var b2a = bytes[i++] & 0x3f;
        var b2b = bytes[i++] & 0x3f;
        out += String.fromCharCode(((b0 & 0x0f) << 12) | (b2a << 6) | b2b);
        continue;
      }
      var b3a = bytes[i++] & 0x3f;
      var b3b = bytes[i++] & 0x3f;
      var b3c = bytes[i++] & 0x3f;
      var cp = ((b0 & 0x07) << 18) | (b3a << 12) | (b3b << 6) | b3c;
      cp -= 0x10000;
      out += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
    }
    return out;
  }

  function base64Bytes(base64) {
    var chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
    var clean = String(base64 || '').replace(/[^A-Za-z0-9+/=]/g, '');
    var bytes = [];
    for (var i = 0; i < clean.length; i += 4) {
      var c0 = chars.indexOf(clean.charAt(i));
      var c1 = chars.indexOf(clean.charAt(i + 1));
      var c2 = chars.indexOf(clean.charAt(i + 2));
      var c3 = chars.indexOf(clean.charAt(i + 3));
      var n = (c0 << 18) | (c1 << 12) | ((c2 < 0 ? 0 : c2) << 6) | (c3 < 0 ? 0 : c3);
      bytes.push((n >> 16) & 0xff);
      if (clean.charAt(i + 2) !== '=') bytes.push((n >> 8) & 0xff);
      if (clean.charAt(i + 3) !== '=') bytes.push(n & 0xff);
    }
    return new Uint8Array(bytes);
  }

  if (typeof globalThis.TextEncoder === 'undefined') {
    globalThis.TextEncoder = function TextEncoder() {};
    globalThis.TextEncoder.prototype.encode = function (input) {
      return utf8Bytes(input);
    };
  }

  if (typeof globalThis.TextDecoder === 'undefined') {
    globalThis.TextDecoder = function TextDecoder() {};
    globalThis.TextDecoder.prototype.decode = function (input) {
      return utf8Decode(input);
    };
  }

  if (typeof globalThis.Buffer === 'undefined') {
    globalThis.Buffer = {
      from: function (input, encoding) {
        if (encoding === 'base64') return base64Bytes(input);
        if (typeof input === 'string') return utf8Bytes(input);
        if (input instanceof ArrayBuffer) return new Uint8Array(input);
        if (typeof Uint8Array !== 'undefined' && input instanceof Uint8Array) return input;
        if (Array.isArray(input)) return new Uint8Array(input);
        return utf8Bytes(String(input == null ? '' : input));
      },
      alloc: function (size) {
        return new Uint8Array(size >>> 0);
      },
      byteLength: function (input, encoding) {
        return globalThis.Buffer.from(input, encoding).length;
      },
      isBuffer: function (value) {
        return typeof Uint8Array !== 'undefined' && value instanceof Uint8Array;
      }
    };
  }

  if (typeof globalThis.EventEmitter === 'undefined') {
    globalThis.EventEmitter = function EventEmitter() {
      this._handlers = {};
    };
    globalThis.EventEmitter.prototype.on = function (eventName, fn) {
      if (!this._handlers[eventName]) this._handlers[eventName] = [];
      this._handlers[eventName].push(fn);
      return this;
    };
    globalThis.EventEmitter.prototype.once = function (eventName, fn) {
      var self = this;
      function wrapped() {
        self.off(eventName, wrapped);
        return fn.apply(null, arguments);
      }
      return this.on(eventName, wrapped);
    };
    globalThis.EventEmitter.prototype.off = function (eventName, fn) {
      var list = this._handlers[eventName] || [];
      for (var i = list.length - 1; i >= 0; i--) {
        if (list[i] === fn) list.splice(i, 1);
      }
      return this;
    };
    globalThis.EventEmitter.prototype.removeListener = globalThis.EventEmitter.prototype.off;
    globalThis.EventEmitter.prototype.emit = function (eventName) {
      var list = this._handlers[eventName] || [];
      var args = Array.prototype.slice.call(arguments, 1);
      for (var i = 0; i < list.length; i++) {
        list[i].apply(null, args);
      }
      return list.length > 0;
    };
  }

  globalThis.__pi_util = globalThis.__pi_util || {
    format: function () {
      var args = Array.prototype.slice.call(arguments);
      if (args.length === 0) return '';
      var fmt = String(args.shift());
      var index = 0;
      return fmt.replace(/%[sdj%]/g, function (token) {
        if (token === '%%') return '%';
        if (index >= args.length) return token;
        var value = args[index++];
        if (token === '%j') {
          try {
            return JSON.stringify(value);
          } catch (_) {
            return '[Circular]';
          }
        }
        return String(value);
      }) + (index < args.length ? ' ' + args.slice(index).map(String).join(' ') : '');
    }
  };

  function hexBytes(hex) {
    var clean = String(hex || '').replace(/[^0-9a-fA-F]/g, '');
    if (clean.length % 2 === 1) clean = '0' + clean;
    var out = new Uint8Array(clean.length / 2);
    for (var i = 0; i < clean.length; i += 2) {
      out[i / 2] = parseInt(clean.slice(i, i + 2), 16);
    }
    return out;
  }

  function bytesToHex(input) {
    var bytes = input instanceof Uint8Array ? input : new Uint8Array(input || []);
    var out = '';
    for (var i = 0; i < bytes.length; i++) {
      var hex = bytes[i].toString(16);
      out += hex.length === 1 ? '0' + hex : hex;
    }
    return out;
  }

  function bytesToBase64(input) {
    var chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
    var bytes = input instanceof Uint8Array ? input : new Uint8Array(input || []);
    var out = '';
    for (var i = 0; i < bytes.length; i += 3) {
      var b0 = bytes[i];
      var b1 = i + 1 < bytes.length ? bytes[i + 1] : 0;
      var b2 = i + 2 < bytes.length ? bytes[i + 2] : 0;
      var n = (b0 << 16) | (b1 << 8) | b2;
      out += chars[(n >> 18) & 63];
      out += chars[(n >> 12) & 63];
      out += i + 1 < bytes.length ? chars[(n >> 6) & 63] : '=';
      out += i + 2 < bytes.length ? chars[n & 63] : '=';
    }
    return out;
  }

  function toBytes(input, encoding) {
    if (input == null) return new Uint8Array(0);
    var normalized = String(encoding || 'utf8').toLowerCase();
    if (typeof input === 'string') {
      if (normalized === 'base64') return base64Bytes(input);
      if (normalized === 'hex') return hexBytes(input);
      return utf8Bytes(input);
    }
    if (input instanceof ArrayBuffer) return new Uint8Array(input);
    if (typeof Uint8Array !== 'undefined' && input instanceof Uint8Array) {
      return new Uint8Array(input);
    }
    if (Array.isArray(input)) return new Uint8Array(input);
    if (input.buffer instanceof ArrayBuffer) {
      return new Uint8Array(input.buffer, input.byteOffset || 0, input.byteLength || 0);
    }
    return utf8Bytes(String(input));
  }

  function installBufferClass() {
    function wrapBuffer(bytes) {
      var view = bytes instanceof Uint8Array ? new Uint8Array(bytes) : new Uint8Array(bytes || []);
      if (Object.setPrototypeOf) {
        Object.setPrototypeOf(view, PiBuffer.prototype);
      } else {
        view.__proto__ = PiBuffer.prototype;
      }
      return view;
    }

    function PiBuffer(input, encoding) {
      return PiBuffer.from(input, encoding);
    }

    PiBuffer.prototype = Object.create(Uint8Array.prototype);
    PiBuffer.prototype.constructor = PiBuffer;
    PiBuffer.prototype.toString = function (encoding) {
      var normalized = String(encoding || 'utf8').toLowerCase();
      if (normalized === 'hex') return bytesToHex(this);
      if (normalized === 'base64') return bytesToBase64(this);
      return utf8Decode(this);
    };
    PiBuffer.prototype.slice = function (start, end) {
      return wrapBuffer(this.subarray(start, end));
    };
    PiBuffer.prototype.subarray = function (start, end) {
      return wrapBuffer(Uint8Array.prototype.subarray.call(this, start, end));
    };
    PiBuffer.prototype.copy = function (target, targetStart, start, end) {
      var to = targetStart >>> 0;
      var from = start >>> 0;
      var until = end == null ? this.length : end >>> 0;
      var copied = 0;
      while (from < until && from < this.length && to < target.length) {
        target[to++] = this[from++];
        copied++;
      }
      return copied;
    };

    PiBuffer.from = function (input, encoding) {
      return wrapBuffer(toBytes(input, encoding));
    };
    PiBuffer.alloc = function (size, fill, encoding) {
      var out = new Uint8Array(size >>> 0);
      if (fill != null) {
        var fillBytes = toBytes(fill, encoding);
        for (var i = 0; i < out.length; i++) {
          out[i] = fillBytes.length === 0 ? 0 : fillBytes[i % fillBytes.length];
        }
      }
      return wrapBuffer(out);
    };
    PiBuffer.byteLength = function (input, encoding) {
      return toBytes(input, encoding).length;
    };
    PiBuffer.isBuffer = function (value) {
      return typeof Uint8Array !== 'undefined' && value instanceof Uint8Array;
    };
    PiBuffer.concat = function (list, totalLength) {
      var items = Array.isArray(list) ? list : [];
      var length = typeof totalLength === 'number' ? totalLength >>> 0 : 0;
      if (!length) {
        for (var i = 0; i < items.length; i++) {
          length += items[i] ? items[i].length >>> 0 : 0;
        }
      }
      var out = new Uint8Array(length);
      var offset = 0;
      for (var j = 0; j < items.length && offset < length; j++) {
        var bytes = toBytes(items[j]);
        for (var k = 0; k < bytes.length && offset < length; k++) {
          out[offset++] = bytes[k];
        }
      }
      return wrapBuffer(out);
    };
    PiBuffer.compare = function (a, b) {
      var left = toBytes(a);
      var right = toBytes(b);
      var len = Math.min(left.length, right.length);
      for (var i = 0; i < len; i++) {
        if (left[i] !== right[i]) return left[i] < right[i] ? -1 : 1;
      }
      if (left.length === right.length) return 0;
      return left.length < right.length ? -1 : 1;
    };

    globalThis.Buffer = PiBuffer;
  }

  function installEventShim() {
    if (typeof globalThis.EventEmitter === 'undefined') {
      globalThis.EventEmitter = function EventEmitter() {
        this._handlers = {};
        this._maxListeners = 10;
      };
    }

    if (typeof globalThis.EventEmitter.prototype.setMaxListeners !== 'function') {
      globalThis.EventEmitter.prototype.setMaxListeners = function (count) {
        this._maxListeners = count;
        return this;
      };
    }
    if (typeof globalThis.EventEmitter.prototype.removeAllListeners !== 'function') {
      globalThis.EventEmitter.prototype.removeAllListeners = function (eventName) {
        if (typeof eventName === 'undefined') {
          this._handlers = {};
        } else {
          delete this._handlers[eventName];
        }
        return this;
      };
    }
    if (typeof globalThis.EventEmitter.prototype.listenerCount !== 'function') {
      globalThis.EventEmitter.prototype.listenerCount = function (eventName) {
        var list = this._handlers[eventName] || [];
        return list.length;
      };
    }

    var events = {
      EventEmitter: globalThis.EventEmitter
    };
    events.default = events;
    globalThis.events = globalThis.events || events;
  }

  function installPathShim() {
    function splitPath(path) {
      var raw = String(path || '').replace(/\\/g, '/');
      var absolute = raw.charAt(0) === '/';
      var parts = raw.split('/');
      var out = [];
      for (var i = 0; i < parts.length; i++) {
        var part = parts[i];
        if (!part || part === '.') continue;
        if (part === '..') {
          if (out.length && out[out.length - 1] !== '..') out.pop();
          else if (!absolute) out.push('..');
        } else {
          out.push(part);
        }
      }
      return { absolute: absolute, parts: out };
    }

    function normalize(path) {
      var parsed = splitPath(path);
      var joined = parsed.parts.join('/');
      if (parsed.absolute) return '/' + joined;
      return joined || '.';
    }

    function resolve() {
      var resolved = '';
      for (var i = arguments.length - 1; i >= 0; i--) {
        var part = String(arguments[i] || '');
        if (!part) continue;
        resolved = part + '/' + resolved;
        if (part.charAt(0) === '/') break;
      }
      return normalize(resolved || '/');
    }

    function join() {
      var parts = [];
      for (var i = 0; i < arguments.length; i++) {
        var part = String(arguments[i] || '');
        if (part) parts.push(part);
      }
      return normalize(parts.join('/'));
    }

    function dirname(path) {
      var normalized = normalize(path);
      if (normalized === '/' || normalized === '.') return normalized;
      var idx = normalized.lastIndexOf('/');
      if (idx < 0) return '.';
      if (idx === 0) return '/';
      return normalized.slice(0, idx);
    }

    function basename(path, ext) {
      var normalized = normalize(path);
      var base = normalized.split('/').pop() || '';
      if (ext && base.slice(-ext.length) === ext) base = base.slice(0, -ext.length);
      return base;
    }

    function extname(path) {
      var base = basename(path);
      var idx = base.lastIndexOf('.');
      return idx > 0 ? base.slice(idx) : '';
    }

    function isAbsolute(path) {
      return String(path || '').charAt(0) === '/';
    }

    function relative(from, to) {
      var fromParts = splitPath(resolve(from)).parts;
      var toParts = splitPath(resolve(to)).parts;
      while (fromParts.length && toParts.length && fromParts[0] === toParts[0]) {
        fromParts.shift();
        toParts.shift();
      }
      var out = [];
      while (fromParts.length) {
        fromParts.pop();
        out.push('..');
      }
      return out.concat(toParts).join('/') || '.';
    }

    function parse(path) {
      var normalized = normalize(path);
      var base = basename(normalized);
      var ext = extname(base);
      return {
        root: isAbsolute(normalized) ? '/' : '',
        dir: dirname(normalized),
        base: base,
        ext: ext,
        name: ext ? base.slice(0, -ext.length) : base
      };
    }

    function format(parsed) {
      if (!parsed) return '.';
      var dir = parsed.dir || parsed.root || '';
      var base = parsed.base || ((parsed.name || '') + (parsed.ext || ''));
      if (!dir) return base || '.';
      if (dir === '/') return '/' + base;
      return normalize(dir + '/' + base);
    }

    var path = {
      sep: '/',
      delimiter: ':',
      normalize: normalize,
      resolve: resolve,
      join: join,
      dirname: dirname,
      basename: basename,
      extname: extname,
      isAbsolute: isAbsolute,
      relative: relative,
      parse: parse,
      format: format
    };
    path.posix = path;
    path.default = path;
    globalThis.path = globalThis.path || path;
  }

  installBufferClass();
  installEventShim();
  installPathShim();
  globalThis.util = globalThis.util || { format: globalThis.__pi_util.format };

  if (typeof globalThis.console === 'undefined') {
    function logLike(level, argsLike) {
      var message = Array.prototype.slice.call(argsLike).map(function (item) {
        return typeof item === 'string' ? item : globalThis.__pi_util.format('%j', item);
      }).join(' ');
      try {
        if (globalThis.pi && typeof globalThis.pi.log === 'function') {
          globalThis.pi.log('[' + level + '] ' + message);
          return;
        }
      } catch (_) {}
      try {
        if (typeof globalThis.print === 'function') {
          globalThis.print('[' + level + '] ' + message);
        }
      } catch (_) {}
    }
    globalThis.console = {
      log: function () { logLike('log', arguments); },
      info: function () { logLike('info', arguments); },
      warn: function () { logLike('warn', arguments); },
      error: function () { logLike('error', arguments); }
    };
  }

  if (typeof globalThis.setTimeout === 'undefined' && typeof globalThis.__pi_sleep === 'function') {
    var __timerSeq = 0;
    var __timers = {};

    function installTimer(looping, cb, ms) {
      var id = ++__timerSeq;
      __timers[id] = { cancelled: false, looping: !!looping };

      function tick() {
        var state = __timers[id];
        if (!state || state.cancelled) return;
        globalThis.__pi_sleep(Math.max(0, Number(ms) || 0)).then(function () {
          state = __timers[id];
          if (!state || state.cancelled) return;
          try {
            if (typeof globalThis.__pi_budget_reset === 'function') {
              globalThis.__pi_budget_reset();
            }
            if (typeof cb === 'function') cb();
          } catch (err) {
            try { console.error('timer callback failed', String(err)); } catch (_) {}
          }
          if (state.looping && !state.cancelled) {
            tick();
          } else {
            delete __timers[id];
          }
        });
      }

      tick();
      return id;
    }

    globalThis.setTimeout = function (cb, ms) {
      return installTimer(false, cb, ms);
    };
    globalThis.clearTimeout = function (id) {
      if (__timers[id]) {
        __timers[id].cancelled = true;
        delete __timers[id];
      }
    };
    globalThis.setInterval = function (cb, ms) {
      return installTimer(true, cb, ms);
    };
    globalThis.clearInterval = globalThis.clearTimeout;
  }
})();
