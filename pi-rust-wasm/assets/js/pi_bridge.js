// pi_bridge.js — pi-mono compatible bridge layer for pi-rust-wasm
// Constructs globalThis.pi object that routes API calls through __pi_host_call.
// Loaded by run_script_file_impl before user plugin scripts.
// See: architecture/plugin-system/js-bridge-layer.md, architecture/plugin-system/host-call-protocol.md,
//      architecture/plugin-system/js-api-alignment.md, architecture/plugin-system/async-hostcall-event-loop.md
(function () {
  'use strict';

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

    // =========================================================================
    // Command Registration (pi-mono ExtensionAPI.registerCommand)
    // =========================================================================
    registerCommand: function (name, options) {
      return hostCall('tools', 'registerCommand', {
        name: name,
        description: options && options.description
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
    // Tool visibility (pi-mono ExtensionAPI.getActiveTools / setActiveTools)
    // =========================================================================
    getActiveTools: function () {
      return hostCall('tools', 'getActiveTools', {});
    },

    setActiveTools: function (toolNames) {
      return hostCall('tools', 'setActiveTools', { toolNames: toolNames });
    }
  };

  // -- Event dispatch entry (called by host via __pi_dispatch_event) ----------
  globalThis.__pi_dispatch_event = function (eventJson) {
    var envelope = JSON.parse(eventJson);
    var eventType = envelope.type;
    var eventData = envelope.data;
    var snapshot = envelope.context || {};

    var ctx = {
      cwd: snapshot.cwd,
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
        }
      },
      sessionManager: {
        getCurrent: function () {
          return hostCall('session', 'getCurrentSession', {});
        }
      }
    };

    var handlers = __pi_hooks[eventType] || [];
    for (var i = 0; i < handlers.length; i++) {
      try {
        handlers[i].fn(eventData, ctx);
      } catch (e) {
        try { pi.log('pi_bridge: handler error for ' + eventType + ': ' + e); } catch (_) {}
      }
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
      var result = handler.execute(call.toolCallId, call.params, undefined, undefined, null);
      return JSON.stringify({ ok: true, data: result });
    } catch (e) {
      return JSON.stringify({ ok: false, error: String(e) });
    }
  };

  // -- Long-lived VM session event loop (Phase 2) -----------------------------
  // Called by host when VM actor receives Init command.
  // Uses setTimeout(loop, 0) to yield control after each event dispatch,
  // allowing QuickJS run_loop_without_io() to drain Promises and tick tasks
  // before blocking again on waitForEvent.
  //
  // Two-layer collaboration:
  //   JS layer: setTimeout(loop, 0) schedules next iteration as a tick task
  //   Rust layer: run_loop_without_io() processes pending Promises + tick tasks
  //   Net effect: all async work resolves between consecutive waitForEvent calls.
  globalThis.__pi_start_event_loop = function () {
    function loop() {
      var raw = __pi_host_call(JSON.stringify({
        module: '__session',
        method: 'waitForEvent'
      }));
      var res = typeof raw === 'string' ? JSON.parse(raw) : raw;

      if (!res.ok || (res.data && res.data.type === '__shutdown')) {
        return;
      }

      try {
        __pi_dispatch_event(JSON.stringify(res.data));
      } catch (e) {
        try { pi.log('event_loop error: ' + e); } catch (_) {}
      }

      setTimeout(loop, 0);
    }
    loop();
  };
})();
