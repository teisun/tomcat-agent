// pi_ai_shim.js — @mariozechner/pi-ai globalThis shim for WasmEdge QuickJS script mode.
// Ported from pi_agent_rust default_virtual_modules(); see task-05d-compat-research.md Part 4.
(function () {
  'use strict';

  function StringEnum(values, opts) {
    var list = Array.isArray(values) ? values.map(function (v) { return String(v); }) : [];
    return Object.assign({ type: "string", "enum": list }, opts || {});
  }

  function calculateCost() {}

  function createAssistantMessageEventStream() {
    return { push: function () {}, end: function () {} };
  }

  function streamSimpleAnthropic() {
    throw new Error("@mariozechner/pi-ai.streamSimpleAnthropic is not available in tomcat");
  }

  function streamSimpleOpenAIResponses() {
    throw new Error("@mariozechner/pi-ai.streamSimpleOpenAIResponses is not available in tomcat");
  }

  function complete(_model, _messages, _opts) {
    return Promise.resolve({ content: "", model: _model || "unknown", usage: { input_tokens: 0, output_tokens: 0 } });
  }

  function completeSimple(_model, _prompt, _opts) {
    return Promise.resolve("");
  }

  function getModel() { return "claude-sonnet-4-5"; }
  function getApiProvider() { return "anthropic"; }
  function getModels() { return ["claude-sonnet-4-5", "claude-haiku-3-5"]; }

  function loginOpenAICodex() {
    return Promise.resolve({ accessToken: "", refreshToken: "", expiresAt: Date.now() + 3600000 });
  }

  function refreshOpenAICodexToken() {
    return Promise.resolve({ accessToken: "", refreshToken: "", expiresAt: Date.now() + 3600000 });
  }

  globalThis.__pi_ai = {
    StringEnum: StringEnum,
    calculateCost: calculateCost,
    createAssistantMessageEventStream: createAssistantMessageEventStream,
    streamSimpleAnthropic: streamSimpleAnthropic,
    streamSimpleOpenAIResponses: streamSimpleOpenAIResponses,
    complete: complete,
    completeSimple: completeSimple,
    getModel: getModel,
    getApiProvider: getApiProvider,
    getModels: getModels,
    loginOpenAICodex: loginOpenAICodex,
    refreshOpenAICodexToken: refreshOpenAICodexToken,
    "default": {
      StringEnum: StringEnum, calculateCost: calculateCost,
      createAssistantMessageEventStream: createAssistantMessageEventStream,
      streamSimpleAnthropic: streamSimpleAnthropic,
      streamSimpleOpenAIResponses: streamSimpleOpenAIResponses,
      complete: complete, completeSimple: completeSimple,
      getModel: getModel, getApiProvider: getApiProvider, getModels: getModels,
      loginOpenAICodex: loginOpenAICodex, refreshOpenAICodexToken: refreshOpenAICodexToken
    }
  };
})();
