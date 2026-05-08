// pi_tui_shim.js — @mariozechner/pi-tui globalThis shim for WasmEdge QuickJS script mode.
// Ported from pi_agent_rust default_virtual_modules(); see task-05d-compat-research.md Part 4.
(function () {
  'use strict';

  function matchesKey(_data, _key) { return false; }

  function truncateToWidth(text, width) {
    var s = String(text == null ? "" : text);
    var w = Number(width || 0);
    if (!w || w <= 0) return "";
    return s.length <= w ? s : s.slice(0, w);
  }

  function visibleWidth(str) { return String(str == null ? "" : str).length; }

  function wrapTextWithAnsi(text) { return String(text == null ? "" : text); }

  function isKeyRelease() { return false; }

  function parseKey(key) { return { key: String(key == null ? "" : key) }; }

  var CURSOR_MARKER = "\u258C";

  // --- Classes ---

  function _stripAnsi(s) {
    return String(s).replace(/\x1b\[[0-9;]*m/g, '');
  }

  function Text(text, padLeft, padRight) {
    this.text = String(text == null ? "" : text);
    this.padLeft = padLeft || 0;
    this.padRight = padRight || 0;
  }
  Text.prototype.render = function (_width) {
    var pad = '';
    for (var i = 0; i < this.padLeft; i++) pad += ' ';
    return [pad + this.text];
  };
  Text.prototype.invalidate = function () {};

  function TruncatedText(text, width, x, y) {
    Text.call(this, text, x, y);
    this.width = Number(width || 80);
  }
  TruncatedText.prototype = Object.create(Text.prototype);
  TruncatedText.prototype.constructor = TruncatedText;
  TruncatedText.prototype.render = function (width) {
    var w = Math.min(this.width, width || 80);
    var stripped = _stripAnsi(this.text);
    var line = stripped.length <= w ? this.text : stripped.slice(0, w);
    return [line];
  };

  function Container() {
    this.children = [];
  }
  Container.prototype.addChild = function (child) {
    this.children.push(child);
    return this;
  };
  Container.prototype.render = function (width) {
    var lines = [];
    for (var i = 0; i < this.children.length; i++) {
      var child = this.children[i];
      if (child && typeof child.render === 'function') {
        var childLines = child.render(width);
        if (Array.isArray(childLines)) {
          for (var j = 0; j < childLines.length; j++) lines.push(childLines[j]);
        }
      }
    }
    return lines;
  };
  Container.prototype.invalidate = function () {};

  function Markdown() {}
  Markdown.prototype.render = function () { return []; };
  Markdown.prototype.invalidate = function () {};

  function Spacer() {}
  Spacer.prototype.render = function () { return ['']; };
  Spacer.prototype.invalidate = function () {};

  function Editor() { this.value = ""; }
  Editor.prototype.render = function () { return [this.value]; };
  Editor.prototype.invalidate = function () {};

  function Box(_padX, _padY, _styleFn) {
    this.children = [];
  }
  Box.prototype.addChild = function (child) {
    this.children.push(child);
    return this;
  };
  Box.prototype.render = function (width) {
    var lines = [];
    for (var i = 0; i < this.children.length; i++) {
      var child = this.children[i];
      if (child && typeof child.render === 'function') {
        var childLines = child.render(width);
        if (Array.isArray(childLines)) {
          for (var j = 0; j < childLines.length; j++) lines.push(childLines[j]);
        }
      }
    }
    return lines;
  };
  Box.prototype.invalidate = function () {};

  function SelectList(items, visibleRows, opts) {
    this.items = Array.isArray(items) ? items : [];
    this.selected = 0;
    this.visibleRows = visibleRows || 10;
    opts = opts || {};
    this.onSelect = opts.onSelect || null;
    this.onCancel = opts.onCancel || null;
    this.onSelectionChange = opts.onSelectionChange || null;
  }
  SelectList.prototype.setItems = function (items) {
    this.items = Array.isArray(items) ? items : [];
  };
  SelectList.prototype.select = function (index) {
    this.selected = Number(index || 0);
  };
  SelectList.prototype.setSelectedIndex = function (index) {
    this.selected = Number(index || 0);
    if (typeof this.onSelectionChange === 'function') {
      this.onSelectionChange(this.selected);
    }
  };
  SelectList.prototype.handleInput = function (data) {
    var key = (typeof data === 'string') ? data : ((data && data.key) || '');
    if (key === 'up' || key === Key.up) {
      this.selected = Math.max(0, this.selected - 1);
      if (typeof this.onSelectionChange === 'function') this.onSelectionChange(this.items[this.selected]);
    } else if (key === 'down' || key === Key.down) {
      this.selected = Math.min(this.items.length - 1, this.selected + 1);
      if (typeof this.onSelectionChange === 'function') this.onSelectionChange(this.items[this.selected]);
    } else if (key === 'enter' || key === Key.enter) {
      if (typeof this.onSelect === 'function' && this.items[this.selected]) this.onSelect(this.items[this.selected]);
    } else if (key === 'escape' || key === Key.escape || key === Key.esc) {
      if (typeof this.onCancel === 'function') this.onCancel();
    }
  };
  SelectList.prototype.render = function (width) {
    var lines = [];
    var start = Math.max(0, this.selected - Math.floor(this.visibleRows / 2));
    var end = Math.min(this.items.length, start + this.visibleRows);
    if (end - start < this.visibleRows) start = Math.max(0, end - this.visibleRows);
    for (var i = start; i < end; i++) {
      var item = this.items[i];
      var label = (item && item.label) ? String(item.label) : String(item);
      var prefix = (i === this.selected) ? '> ' : '  ';
      var line = prefix + label;
      if (width && line.length > width) line = line.slice(0, width);
      lines.push(line);
    }
    return lines;
  };
  SelectList.prototype.invalidate = function () {};

  function Input() { this.value = ""; }
  Input.prototype.render = function () { return [this.value]; };
  Input.prototype.invalidate = function () {};

  function Image(src) {
    this.src = String(src == null ? "" : src);
    this.width = 0;
    this.height = 0;
  }
  Image.prototype.render = function () { return ['[image: ' + this.src + ']']; };
  Image.prototype.invalidate = function () {};

  var Key = {
    escape: "escape", esc: "esc", enter: "enter", tab: "tab",
    space: "space", backspace: "backspace", "delete": "delete",
    home: "home", end: "end", pageUp: "pageUp", pageDown: "pageDown",
    up: "up", down: "down", left: "left", right: "right",
    ctrl: function (k) { return "ctrl+" + k; },
    shift: function (k) { return "shift+" + k; },
    alt: function (k) { return "alt+" + k; },
    ctrlShift: function (k) { return "ctrl+shift+" + k; },
    shiftCtrl: function (k) { return "shift+ctrl+" + k; },
    ctrlAlt: function (k) { return "ctrl+alt+" + k; },
    altCtrl: function (k) { return "alt+ctrl+" + k; },
    shiftAlt: function (k) { return "shift+alt+" + k; },
    altShift: function (k) { return "alt+shift+" + k; },
    ctrlAltShift: function (k) { return "ctrl+alt+shift+" + k; }
  };

  function DynamicBorder(_styleFn) { this.styleFn = _styleFn || null; }
  DynamicBorder.prototype.render = function (width) {
    var w = Number(width || 80);
    var line = '';
    for (var i = 0; i < w; i++) line += '─';
    if (typeof this.styleFn === 'function') {
      try { line = this.styleFn(line); } catch (_) {}
    }
    return [line];
  };
  DynamicBorder.prototype.invalidate = function () {};

  function SettingsList() { this.items = []; }
  SettingsList.prototype.setItems = function (items) {
    this.items = Array.isArray(items) ? items : [];
  };
  SettingsList.prototype.render = function () {
    return this.items.map(function (item) {
      return String((item && item.label) || item);
    });
  };
  SettingsList.prototype.invalidate = function () {};

  function fuzzyMatch(query, text) {
    var q = String(query == null ? "" : query).toLowerCase();
    var t = String(text == null ? "" : text).toLowerCase();
    if (!q) return { match: true, score: 0, positions: [] };
    if (!t) return { match: false, score: 0, positions: [] };
    var positions = [];
    var qi = 0;
    for (var ti = 0; ti < t.length && qi < q.length; ti++) {
      if (t[ti] === q[qi]) { positions.push(ti); qi++; }
    }
    var m = qi === q.length;
    return { match: m, score: m ? (q.length / t.length) * 100 : 0, positions: positions };
  }

  function getEditorKeybindings() {
    return { save: "ctrl+s", quit: "ctrl+q", copy: "ctrl+c", paste: "ctrl+v", undo: "ctrl+z", redo: "ctrl+y", find: "ctrl+f", replace: "ctrl+h" };
  }

  function fuzzyFilter(query, items) {
    var q = String(query == null ? "" : query).toLowerCase();
    if (!q) return items;
    if (!Array.isArray(items)) return [];
    return items.filter(function (item) {
      var text = typeof item === "string" ? item : String((item && (item.label || item.name)) || item);
      return fuzzyMatch(q, text).match;
    });
  }

  function CancellableLoader(message, opts) {
    this.message = String(message == null ? "Loading..." : message);
    this.cancelled = false;
    this.onCancel = (opts && opts.onCancel) || null;
  }
  CancellableLoader.prototype.cancel = function () {
    this.cancelled = true;
    if (typeof this.onCancel === "function") this.onCancel();
  };
  CancellableLoader.prototype.render = function () {
    return this.cancelled ? [] : [this.message];
  };

  globalThis.__pi_tui = {
    matchesKey: matchesKey, truncateToWidth: truncateToWidth,
    visibleWidth: visibleWidth, wrapTextWithAnsi: wrapTextWithAnsi,
    Text: Text, TruncatedText: TruncatedText,
    Container: Container, Markdown: Markdown, Spacer: Spacer,
    Editor: Editor, Box: Box, SelectList: SelectList, Input: Input, Image: Image,
    CURSOR_MARKER: CURSOR_MARKER, isKeyRelease: isKeyRelease, parseKey: parseKey,
    Key: Key, DynamicBorder: DynamicBorder, SettingsList: SettingsList,
    fuzzyMatch: fuzzyMatch, getEditorKeybindings: getEditorKeybindings,
    fuzzyFilter: fuzzyFilter, CancellableLoader: CancellableLoader,
    "default": {
      matchesKey: matchesKey, truncateToWidth: truncateToWidth,
      visibleWidth: visibleWidth, wrapTextWithAnsi: wrapTextWithAnsi,
      Text: Text, TruncatedText: TruncatedText,
      Container: Container, Markdown: Markdown, Spacer: Spacer,
      Editor: Editor, Box: Box, SelectList: SelectList, Input: Input, Image: Image,
      CURSOR_MARKER: CURSOR_MARKER, isKeyRelease: isKeyRelease, parseKey: parseKey,
      Key: Key, DynamicBorder: DynamicBorder, SettingsList: SettingsList,
      fuzzyMatch: fuzzyMatch, getEditorKeybindings: getEditorKeybindings,
      fuzzyFilter: fuzzyFilter, CancellableLoader: CancellableLoader
    }
  };
})();
