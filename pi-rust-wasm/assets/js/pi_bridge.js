// pi_bridge.js — pi-mono compatible bridge layer for pi-rust-wasm
// Constructs globalThis.pi object that routes API calls through __pi_host_call.
// Loaded by run_script_file_impl before user plugin scripts.
// See: architecture/js-bridge-layer.md, host-call-protocol.md
(function () {
  'use strict';

  // -- Low-level host call wrapper ----------------------------------------
  function hostCall(module, method, params) {
    var req = JSON.stringify({ module: module, method: method, params: params || {} });
    var res = __pi_host_call(req);
    return typeof res === 'string' ? JSON.parse(res) : res;
  }

  // -- Internal registries ------------------------------------------------
  var __pi_hooks = {};   // eventName -> [{id, fn}, ...]
  var __pi_tools = {};   // toolName  -> handler
  var __pi_nextId = 1;

  // -- Build globalThis.pi ------------------------------------------------
  globalThis.pi = {

    // =====================================================================
    // Event Subscription (pi-mono ExtensionAPI.on)
    // =====================================================================
    on: function (eventName, handler) {
      if (!__pi_hooks[eventName]) __pi_hooks[eventName] = [];
      __pi_hooks[eventName].push({ id: __pi_nextId++, fn: handler });
      hostCall('events', 'subscribe', { eventName: eventName });
    },

    // =====================================================================
    // 4 Primitives (aligned with pi-mono exec/readFile/writeFile/editFile)
    // =====================================================================
    exec: function (command, args, options) {
      return hostCall('fs', 'executeBash', {
        command: command,
        args: args,
        cwd: options && options.cwd
      });
    },

    readFile: function (path) {
      return hostCall('fs', 'readFile', { path: path });
    },

    writeFile: function (path, content, options) {
      return hostCall('fs', 'writeFile', {
        path: path,
        content: content,
        overwrite: options && options.overwrite
      });
    },

    editFile: function (path, edits) {
      return hostCall('fs', 'editFile', { path: path, edits: edits });
    },

    // =====================================================================
    // Tool Registration (pi-mono ExtensionAPI.registerTool)
    // =====================================================================
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

    // =====================================================================
    // Command Registration (pi-mono ExtensionAPI.registerCommand)
    // =====================================================================
    registerCommand: function (name, options) {
      return hostCall('tools', 'registerCommand', {
        name: name,
        description: options && options.description
      });
    },

    // =====================================================================
    // LLM (pi-mono-style createChatCompletion)
    // =====================================================================
    createChatCompletion: function (params) {
      return hostCall('llm', 'createChatCompletion', params);
    },

    // =====================================================================
    // Session (pi-mono sessionManager subset)
    // =====================================================================
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

    // =====================================================================
    // Messaging (pi-mono ExtensionAPI.sendMessage / sendUserMessage)
    // =====================================================================
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

    // =====================================================================
    // Logging
    // =====================================================================
    log: function (msg) {
      hostCall('agent', 'log', { message: typeof msg === 'string' ? msg : JSON.stringify(msg) });
    },

    // =====================================================================
    // Model
    // =====================================================================
    getActiveTools: function () {
      return hostCall('tools', 'getActiveTools', {});
    },

    setActiveTools: function (toolNames) {
      return hostCall('tools', 'setActiveTools', { toolNames: toolNames });
    }
  };

  // -- Event dispatch entry (called by host via __pi_dispatch_event) ------
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

  // -- Tool execution entry (called by host) ------------------------------
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
})();
