// pi_bridge.js — pi-mono compatible bridge layer for tomcat
// Constructs globalThis.pi object that routes API calls through __pi_host_call.
// Loaded by run_script_file_impl before user plugin scripts.
// See: architecture/plugin-system/js-bridge-layer.md, architecture/plugin-system/host-call-protocol.md,
//      architecture/plugin-system/js-api-alignment.md, architecture/plugin-system/async-hostcall-event-loop.md
(function () {
  'use strict';

  globalThis.__pi_last_fatal_error = null;

  function markFatal(err) {
    globalThis.__pi_last_fatal_error = String(err);
    return err;
  }

  // -- Node.js polyfills for extensions that reference process.platform --------
  if (typeof globalThis.process === 'undefined') {
    globalThis.process = { platform: 'linux', env: {}, argv: [], pid: 1, exit: function () {}, cwd: function () { return '/'; }, kill: function () {} };
  } else {
    if (typeof globalThis.process.cwd !== 'function') globalThis.process.cwd = function () { return '/'; };
    if (typeof globalThis.process.kill !== 'function') globalThis.process.kill = function () {};
  }

  // -- Low-level synchronous host call wrapper --------------------------------
  // Used for fast APIs that don't need async: logging, event registration, etc.
  function hostCall(module, method, params) {
    var req = JSON.stringify({ module: module, method: method, params: params || {} });
    var res = __pi_host_call(req);
    return typeof res === 'string' ? JSON.parse(res) : res;
  }

  // -- Async host call wrapper (submit/poll pattern) -------------------------
  // Returns a Promise; drives polling via QuickJS built-in event loop.
  // See: architecture/plugin-system/async-hostcall-event-loop.md §11.4.4
  var __callSeq = 0;
  var POLL_INTERVAL_MS = 1;
  var POLL_MAX_INTERVAL_MS = 50;

  function hostCallAsync(module, method, params) {
    var callId = '__call_' + (++__callSeq) + '_' + Date.now();
    var req = JSON.stringify({
      module: module, method: method,
      params: params || {}, callId: callId
    });
    var submitRes = __pi_host_call(req);
    var parsed = typeof submitRes === 'string' ? JSON.parse(submitRes) : submitRes;

    if (!parsed.ok) {
      return Promise.reject(new Error(parsed.error || 'hostcall submit failed'));
    }
    // Synchronous path: no pending flag means the result is already inline.
    if (!parsed.data || !parsed.data.pending) {
      return Promise.resolve(parsed);
    }

    return new Promise(function (resolve, reject) {
      var interval = POLL_INTERVAL_MS;
      function poll() {
        var pollReq = JSON.stringify({
          module: '__async', method: 'poll',
          params: { callId: callId }
        });
        var pollRes = __pi_host_call(pollReq);
        var pr = typeof pollRes === 'string' ? JSON.parse(pollRes) : pollRes;

        if (!pr.ok) {
          reject(new Error(pr.error || 'async poll error'));
          return;
        }
        if (pr.data && pr.data.ready) {
          resolve({ ok: true, data: pr.data.result });
          return;
        }
        // Exponential backoff polling: 1ms → 2ms → … → 50ms cap.
        interval = Math.min(interval * 2, POLL_MAX_INTERVAL_MS);
        setTimeout(poll, interval);
      }
      setTimeout(poll, POLL_INTERVAL_MS);
    });
  }

  // -- Internal registries ----------------------------------------------------
  var __pi_hooks = {};   // eventName -> [{id, fn}, ...]
  var __pi_tools = {};   // toolName  -> handler
  var __pi_functions = {}; // functionName -> handler
  var __pi_commands = {}; // commandName -> { description, handler }
  var __pi_nextId = 1;

  // -- Build globalThis.pi ---------------------------------------------------
  globalThis.pi = {

    // =========================================================================
    // Event Subscription (pi-mono ExtensionAPI.on / off / emit / once)
    // =========================================================================
    on: function (eventName, handler) {
      if (!__pi_hooks[eventName]) __pi_hooks[eventName] = [];
      var lid = __pi_nextId++;
      __pi_hooks[eventName].push({ id: lid, fn: handler });
      var res = hostCall('events', 'subscribe', { eventName: eventName });
      return (res && res.listenerId != null) ? res.listenerId : lid;
    },

    // Supports both off(event, handler) [pi-mono style] and
    // off(event, listenerId) [numeric id, backward-compat].
    off: function (eventName, handlerOrId) {
      var hooks = __pi_hooks[eventName];
      if (!hooks) return;
      var listenerId = null;
      if (typeof handlerOrId === 'function') {
        for (var i = hooks.length - 1; i >= 0; i--) {
          if (hooks[i].fn === handlerOrId) {
            listenerId = hooks[i].id;
            hooks.splice(i, 1);
            break;
          }
        }
      } else {
        for (var j = hooks.length - 1; j >= 0; j--) {
          if (hooks[j].id === handlerOrId) {
            listenerId = hooks[j].id;
            hooks.splice(j, 1);
            break;
          }
        }
      }
      if (listenerId != null) {
        hostCall('events', 'off', { eventName: eventName, listenerId: listenerId });
      }
    },

    emit: function (eventName, payload) {
      return hostCall('events', 'emit', { eventName: eventName, payload: payload || {} });
    },

    // Single-fire listener: auto-unsubscribes after first invocation.
    once: function (eventName, handler) {
      var self = this;
      var wrapped = function (data, ctx) {
        self.off(eventName, wrapped);
        handler(data, ctx);
      };
      return self.on(eventName, wrapped);
    },

    // =========================================================================
    // 4 Primitives — async, aligned with pi-mono Promise-returning signatures
    // =========================================================================
    exec: function (command, args, options) {
      return hostCallAsync('fs', 'executeBash', {
        command: command,
        args: args,
        cwd: options && options.cwd
      }).then(function (r) {
        if (!r.ok) throw new Error(r.error || 'exec failed');
        return r.data; // { stdout, stderr, exitCode }
      });
    },

    // File ops use Promise.resolve-wrapping of sync call (files are fast; no
    // need for submit/poll). Returns Promise<string> matching pi-mono.
    readFile: function (path) {
      var r = hostCall('fs', 'readFile', { path: path });
      if (!r.ok) return Promise.reject(new Error(r.error || 'readFile failed'));
      return Promise.resolve(r.data);
    },

    writeFile: function (path, content, options) {
      var r = hostCall('fs', 'writeFile', {
        path: path,
        content: content,
        overwrite: options && options.overwrite
      });
      if (!r.ok) return Promise.reject(new Error(r.error || 'writeFile failed'));
      return Promise.resolve(r.data);
    },

    editFile: function (path, edits) {
      var r = hostCall('fs', 'editFile', { path: path, edits: edits });
      if (!r.ok) return Promise.reject(new Error(r.error || 'editFile failed'));
      return Promise.resolve(r.data);
    },

    // =========================================================================
    // Tool Registration (pi-mono ExtensionAPI.registerTool / unregisterTool)
    // =========================================================================
    registerTool: function (toolDef) {
      if (toolDef && toolDef.name) {
        __pi_tools[toolDef.name] = toolDef;
      }
      return hostCall('tools', 'registerTool', {
        name: toolDef.name,
        label: toolDef.label,
        description: toolDef.description,
        parameters: toolDef.parameters
      });
    },

    unregisterTool: function (name) {
      if (name && __pi_tools[name]) delete __pi_tools[name];
      return hostCall('tools', 'unregisterTool', { toolName: name });
    },

    registerFunction: function (name, handler) {
      if (name) {
        __pi_functions[name] = handler;
      }
      return null;
    },

    // =========================================================================
    // Command Registration (pi-mono ExtensionAPI.registerCommand)
    // =========================================================================
    registerCommand: function (name, options) {
      options = options || {};
      __pi_commands[name] = {
        description: options.description || '',
        handler: options.handler || null
      };
      return hostCall('commands', 'registerCommand', {
        name: name,
        description: options.description
      });
    },

    // =========================================================================
    // LLM (pi-mono-style createChatCompletion / complete / setModel / getModel)
    // =========================================================================
    createChatCompletion: function (params) {
      return hostCallAsync('llm', 'createChatCompletion', params)
        .then(function (r) {
          if (!r.ok) throw new Error(r.error || 'createChatCompletion failed');
          return r.data; // { message: {role, content}, usage?: {...} }
        });
    },

    // Simplified single-turn LLM call — wraps createChatCompletion.
    // Returns Promise<string> (the assistant reply text).
    complete: function (prompt, options) {
      var msgs = (options && options.messages)
        ? options.messages
        : [{ role: 'user', content: String(prompt) }];
      return this.createChatCompletion({ messages: msgs })
        .then(function (res) {
          return (res && res.message && res.message.content) ? res.message.content : '';
        });
    },

    // Model selection — MVP: acknowledged by host but model change is best-effort.
    setModel: function (model) {
      return hostCallAsync('llm', 'setModel', { model: model })
        .then(function (r) {
          if (!r.ok) throw new Error(r.error || 'setModel failed');
          return r.data;
        });
    },

    // Returns current model name (sync, matches pi-mono signature).
    getModel: function () {
      var r = hostCall('llm', 'getModel', {});
      return (r && r.data && r.data.model) ? r.data.model : null;
    },

    // =========================================================================
    // Session (pi-mono sessionManager subset)
    // =========================================================================
    session: {
      getCurrent: function () {
        return hostCall('session', 'getCurrentSession', {});
      },
      getMessages: function (cap) {
        return hostCall('session', 'getMessages', { cap: cap });
      },
      sendMessage: function (msg) {
        return hostCall('session', 'sendMessage', { message: msg });
      }
    },

    // =========================================================================
    // Messaging (pi-mono ExtensionAPI.sendMessage / sendUserMessage)
    // =========================================================================
    sendMessage: function (message, options) {
      return hostCall('agent', 'sendMessage', {
        message: message,
        options: options
      });
    },

    sendUserMessage: function (content, options) {
      return hostCall('agent', 'sendUserMessage', {
        content: content,
        options: options
      });
    },

    // =========================================================================
    // Logging
    // =========================================================================
    log: function (msg) {
      hostCall('agent', 'log', { message: typeof msg === 'string' ? msg : JSON.stringify(msg) });
    },

    // =========================================================================
    // Tool visibility (pi-mono ExtensionAPI.getActiveTools / setActiveTools / getAllTools)
    // =========================================================================
    getActiveTools: function () {
      return hostCall('tools', 'getActiveTools', {});
    },

    setActiveTools: function (toolNames) {
      return hostCall('tools', 'setActiveTools', { toolNames: toolNames });
    },

    getAllTools: function () {
      var r = hostCall('tools', 'getToolList', {});
      return (r && r.ok && r.data) ? r.data : [];
    },

    // =========================================================================
    // Flags (pi-mono ExtensionAPI.registerFlag / getFlag)
    // =========================================================================
    registerFlag: function (name, options) {
      return hostCall('tools', 'registerFlag', {
        name: name,
        description: (options && options.description) || '',
        type: (options && options.type) || 'boolean'
      });
    },

    getFlag: function (name) {
      var r = hostCall('tools', 'getFlag', { name: name });
      return (r && r.ok && r.data != null) ? r.data.value : undefined;
    },

    // =========================================================================
    // Shortcuts (pi-mono ExtensionAPI.registerShortcut)
    // =========================================================================
    registerShortcut: function (key, options) {
      return hostCall('tools', 'registerShortcut', {
        key: key,
        description: (options && options.description) || ''
      });
    },

    // =========================================================================
    // Session name (pi-mono ExtensionAPI.getSessionName / setSessionName)
    // =========================================================================
    getSessionName: function () {
      var r = hostCall('session', 'getSessionName', {});
      return (r && r.ok && r.data) ? r.data.name : '';
    },

    setSessionName: function (name) {
      return hostCall('session', 'setSessionName', { name: name });
    },

    // =========================================================================
    // Conversation (pi-mono ExtensionAPI.appendEntry)
    // =========================================================================
    appendEntry: function (entry) {
      return hostCall('session', 'appendEntry', { entry: entry });
    },

    // =========================================================================
    // Model control (pi-mono ExtensionAPI.setThinkingLevel)
    // =========================================================================
    setThinkingLevel: function (level) {
      return hostCall('llm', 'setThinkingLevel', { level: level });
    }
  };

  // -- Shared ctx constructor (used by dispatch_event and invoke_command) ------
  function __pi_build_ctx(snapshot) {
    snapshot = snapshot || {};
    var cwdResolved = snapshot.cwd;
    if (cwdResolved == null || cwdResolved === '') {
      var gc = hostCall('context', 'getCwd', {});
      cwdResolved = (gc && gc.data && gc.data.cwd) ? gc.data.cwd : undefined;
    }
    return {
      cwd: cwdResolved,
      model: snapshot.model,
      hasUI: !!snapshot.hasUI,
      isIdle: function () {
        return hostCall('context', 'isIdle', {}).data.idle;
      },
      abort: function () {
        return hostCall('context', 'abort', {});
      },
      hasPendingMessages: function () {
        return hostCall('context', 'hasPendingMessages', {}).data.pending;
      },
      shutdown: function () {
        return hostCall('context', 'shutdown', {});
      },
      getSystemPrompt: function () {
        return hostCall('context', 'getSystemPrompt', {}).data.prompt;
      },
      getContextUsage: function () {
        return hostCall('context', 'getContextUsage', {}).data;
      },
      compact: function (options) {
        return hostCall('context', 'compact', { options: options || {} });
      },
      ui: {
        notify: function (message, type) {
          return hostCall('context', 'uiNotify', { message: message, type: type });
        },
        select: function (title, options) {
          return hostCall('context', 'uiSelect', { title: title, options: options });
        },
        confirm: function (title, message) {
          return hostCall('context', 'uiConfirm', { title: title, message: message });
        },
        input: function (title, placeholder) {
          return hostCall('context', 'uiInput', { title: title, placeholder: placeholder });
        },
        setStatus: function (message, details) {
          return hostCall('context', 'uiSetStatus', {
            message: message,
            details: details != null ? details : null
          });
        },
        custom: function (factory, _options) {
          var termWidth = 80;
          var result;
          var resolved = false;
          function done(val) { result = val; resolved = true; }
          var tui = { requestRender: function () {} };
          var theme = {
            fg: function (_c, t) { return String(t == null ? '' : t); },
            bg: function (_c, t) { return String(t == null ? '' : t); },
            bold: function (t) { return String(t == null ? '' : t); },
            dim: function (t) { return String(t == null ? '' : t); },
            italic: function (t) { return String(t == null ? '' : t); },
            underline: function (t) { return String(t == null ? '' : t); },
            strikethrough: function (t) { return String(t == null ? '' : t); }
          };
          var kb = {};
          try {
            var component = factory(tui, theme, kb, done);
            if (component && typeof component.render === 'function') {
              var lines = component.render(termWidth);
              if (Array.isArray(lines) && lines.length > 0) {
                hostCall('context', 'uiCustom', { lines: lines });
              }
            }
          } catch (err) {
            hostCall('context', 'uiNotify', { message: 'ctx.ui.custom factory error: ' + String(err), type: 'error' });
          }
          if (!resolved) done(undefined);
          return result;
        },
        setWidget: function (key, content, _options) {
          hostCall('context', 'uiSetWidget', { key: key, content: content });
        },
        setFooter: function (_factory) {
          hostCall('context', 'uiSetFooter', {});
        },
        setHeader: function (_factory) {
          hostCall('context', 'uiSetHeader', {});
        },
        editor: function (title, prefill) {
          var r = hostCall('context', 'uiEditor', { title: title, prefill: prefill || '' });
          return (r && r.ok && r.data && r.data.text != null) ? r.data.text : (prefill || '');
        },
        setTitle: function (_title) {},
        setWorkingMessage: function (_msg) {},
        onTerminalInput: function (_cb) {},
        pasteToEditor: function (_text) {},
        setEditorText: function (_text) {},
        getEditorText: function () { return ''; },
        setEditorComponent: function (_component) {},
        theme: {
          fg: function (_c, t) { return String(t == null ? '' : t); },
          bg: function (_c, t) { return String(t == null ? '' : t); },
          bold: function (t) { return String(t == null ? '' : t); },
          dim: function (t) { return String(t == null ? '' : t); }
        },
        getAllThemes: function () { return []; },
        getTheme: function () { return 'default'; },
        setTheme: function (_name) {},
        getToolsExpanded: function () { return false; },
        setToolsExpanded: function (_expanded) {}
      },
      sessionManager: {
        getCurrent: function () {
          return hostCall('session', 'getCurrentSession', {});
        },
        getBranch: function (fromId) {
          var r = hostCall('session', 'getBranch', { fromId: fromId });
          return (r && r.ok && r.data) ? r.data : [];
        },
        getLeafEntry: function () {
          var r = hostCall('session', 'getLeafEntry', {});
          return (r && r.ok) ? r.data : null;
        },
        getLeafId: function () {
          var r = hostCall('session', 'getLeafId', {});
          return (r && r.ok && r.data) ? r.data.id : null;
        },
        getEntry: function (id) {
          var r = hostCall('session', 'getEntry', { id: id });
          return (r && r.ok) ? r.data : null;
        },
        getHeader: function () {
          var r = hostCall('session', 'getHeader', {});
          return (r && r.ok) ? r.data : null;
        },
        getEntries: function (cap) {
          var r = hostCall('session', 'getEntries', { cap: cap });
          return (r && r.ok && r.data) ? r.data : [];
        },
        getCwd: function () {
          var gc = hostCall('context', 'getCwd', {});
          return (gc && gc.data && gc.data.cwd) ? gc.data.cwd : '';
        },
        getSessionDir: function () { return ''; },
        getSessionId: function () { return ''; },
        getSessionFile: function () { return ''; },
        getTree: function () { return []; },
        getLabel: function () { return null; }
      },
      model: snapshot.model || (function () {
        var r = hostCall('context', 'getModel', {});
        return (r && r.data && r.data.model) ? r.data : undefined;
      })(),
      modelRegistry: {
        getAll: function () {
          var r = hostCall('context', 'listModels', {});
          return (r && r.ok && r.data) ? r.data : [];
        },
        getAvailable: function () {
          var r = hostCall('context', 'listModels', {});
          return (r && r.ok && r.data) ? r.data : [];
        },
        getError: function () { return undefined; },
        find: function (_query) { return null; },
        getApiKeyForProvider: function (_provider) { return ''; }
      }
    };
  }

  // -- Expose internals needed by the async main loop injected in instance_wasmedge.rs --
  globalThis.__pi_build_ctx = __pi_build_ctx;
  globalThis.__pi_hostCall = hostCall;
  globalThis.__pi_functions = __pi_functions;
  globalThis.__pi_commands = __pi_commands;

  // -- Event dispatch entry (called by host via __pi_dispatch_event) ----------
  globalThis.__pi_dispatch_event = function (eventJson) {
    var envelope = JSON.parse(eventJson);
    var eventType = envelope.type;
    var eventData = envelope.data;
    var ctx = __pi_build_ctx(envelope.context);

    var handlers = __pi_hooks[eventType] || [];
    for (var i = 0; i < handlers.length; i++) {
      try {
        handlers[i].fn(eventData, ctx);
      } catch (e) {
        try { pi.log('pi_bridge: handler error for ' + eventType + ': ' + e); } catch (_) {}
        throw markFatal(e);
      }
    }
  };

  // -- Plugin command invoke (tests / host bridge; handler 存于 __pi_commands) ---
  globalThis.__pi_invoke_command = function (name, argsJson) {
    var entry = __pi_commands[name];
    if (!entry || typeof entry.handler !== 'function') {
      return JSON.stringify({ ok: false, error: 'unknown command: ' + name });
    }
    var args = {};
    try {
      if (argsJson && argsJson.length > 0) {
        args = JSON.parse(argsJson);
      }
    } catch (e) {
      return JSON.stringify({ ok: false, error: 'invalid args JSON: ' + e });
    }
    try {
      var ctx = __pi_build_ctx({});
      var r = entry.handler(argsJson || '', ctx);
      if (r && typeof r.then === 'function') {
        return JSON.stringify({
          ok: false,
          error: 'async command handler requires await outside __pi_invoke_command'
        });
      }
      return JSON.stringify({ ok: true, data: r });
    } catch (err) {
      return JSON.stringify({ ok: false, error: String(err) });
    }
  };

  // -- Tool execution entry (called by host) ----------------------------------
  globalThis.__pi_execute_tool = function (toolCallJson) {
    var call = JSON.parse(toolCallJson);
    var handler = __pi_tools[call.toolName];
    if (!handler || !handler.execute) {
      return JSON.stringify({ ok: false, error: 'tool not found: ' + call.toolName });
    }
    try {
      var result = handler.execute(call.toolCallId, call.params, undefined, undefined, call.ctx || null);
      if (result && typeof result.then === 'function') {
        return JSON.stringify({ ok: false, error: 'tool returned Promise; use __pi_execute_tool_async' });
      }
      return JSON.stringify({ ok: true, data: result });
    } catch (e) {
      return JSON.stringify({ ok: false, error: String(e) });
    }
  };

  globalThis.__pi_execute_tool_async = async function (toolCallJson) {
    var call = JSON.parse(toolCallJson);
    var handler = __pi_tools[call.toolName];
    if (!handler || !handler.execute) {
      return { ok: false, error: 'tool not found: ' + call.toolName };
    }
    try {
      var result = handler.execute(call.toolCallId, call.params, undefined, undefined, call.ctx || null);
      if (result && typeof result.then === 'function') {
        result = await result;
      }
      return { ok: true, data: result };
    } catch (e) {
      return { ok: false, error: String(e) };
    }
  };

  globalThis.__pi_execute_function = function (functionCallJson) {
    var call = JSON.parse(functionCallJson);
    var handler = __pi_functions[call.functionName];
    if (typeof handler !== 'function') {
      return JSON.stringify({ ok: false, error: 'function not found: ' + call.functionName });
    }
    try {
      var result = handler(call.params, call.ctx || null);
      if (result && typeof result.then === 'function') {
        return JSON.stringify({ ok: false, error: 'function returned Promise; use __pi_execute_function_async' });
      }
      return JSON.stringify({ ok: true, data: result });
    } catch (e) {
      return JSON.stringify({ ok: false, error: String(e) });
    }
  };

  globalThis.__pi_execute_function_async = async function (functionCallJson) {
    var call = JSON.parse(functionCallJson);
    var handler = __pi_functions[call.functionName];
    if (typeof handler !== 'function') {
      return { ok: false, error: 'function not found: ' + call.functionName };
    }
    try {
      var result = handler(call.params, call.ctx || null);
      if (result && typeof result.then === 'function') {
        result = await result;
      }
      return { ok: true, data: result };
    } catch (e) {
      return { ok: false, error: String(e) };
    }
  };

  // -- Long-lived VM session event loop (rquickjs async mode) -----------------
  // Host exposes `__pi_wait_for_event(timeoutMs)` as a Promise-returning async
  // function. That lets the VM keep timers / Promise chains alive while still
  // blocking on host events between dispatches.
  globalThis.__pi_start_event_loop = async function () {
    for (;;) {
      var raw;
      try {
        raw = await __pi_wait_for_event(50);
      } catch (hostErr) {
        try { pi.log('[event_loop] exiting: hostErr=' + hostErr); } catch (_) {}
        return;
      }
      var res = typeof raw === 'string' ? JSON.parse(raw) : raw;

      if (!res.ok) {
        try { pi.log('[event_loop] exiting: !res.ok'); } catch (_) {}
        return;
      }

      if (res.data && res.data.type === '__shutdown') {
        try { pi.log('[event_loop] exiting: __shutdown received'); } catch (_) {}
        return;
      }

      if (res.data && res.data.type === '__tick') {
        continue;
      }

      // command_invoke: store pending command and exit the event loop so the
      // async main loop can await the handler before waiting for the next event.
      if (res.data && res.data.type === 'command_invoke') {
        globalThis.__pi_pending_command_invoke = res.data;
        return;
      }

      try {
        if (typeof __pi_budget_reset === 'function') {
          __pi_budget_reset();
        }
        __pi_dispatch_event(JSON.stringify(res.data));
      } catch (e) {
        try { pi.log('event_loop error: ' + e); } catch (_) {}
        throw markFatal(e);
      }
    }
  };
})();
