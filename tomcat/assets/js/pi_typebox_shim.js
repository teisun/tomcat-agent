// pi_typebox_shim.js — @sinclair/typebox globalThis shim for WasmEdge QuickJS script mode.
// Ported from pi_agent_rust default_virtual_modules(); see task-05d-compat-research.md Part 4.
(function () {
  'use strict';
  var Type = {
    String: function (opts) { return Object.assign({ type: "string" }, opts || {}); },
    Number: function (opts) { return Object.assign({ type: "number" }, opts || {}); },
    Boolean: function (opts) { return Object.assign({ type: "boolean" }, opts || {}); },
    Array: function (items, opts) { return Object.assign({ type: "array", items: items }, opts || {}); },
    Object: function (props, opts) {
      props = props || {};
      opts = opts || {};
      var required = [];
      var properties = {};
      var keys = Object.keys(props);
      for (var i = 0; i < keys.length; i++) {
        var k = keys[i];
        var v = props[k];
        if (v && typeof v === "object" && v.__pi_optional) {
          properties[k] = v.schema;
        } else {
          properties[k] = v;
          required.push(k);
        }
      }
      var out = Object.assign({ type: "object", properties: properties }, opts);
      if (required.length) out.required = required;
      return out;
    },
    Optional: function (schema) { return { __pi_optional: true, schema: schema }; },
    Literal: function (value, opts) { return Object.assign({ "const": value }, opts || {}); },
    Any: function (opts) { return Object.assign({}, opts || {}); },
    Union: function (schemas, opts) { return Object.assign({ anyOf: schemas }, opts || {}); },
    Enum: function (values, opts) { return Object.assign({ "enum": values }, opts || {}); },
    Integer: function (opts) { return Object.assign({ type: "integer" }, opts || {}); },
    Null: function (opts) { return Object.assign({ type: "null" }, opts || {}); },
    Unknown: function (opts) { return Object.assign({}, opts || {}); },
    Tuple: function (items, opts) { return Object.assign({ type: "array", items: items, minItems: items.length, maxItems: items.length }, opts || {}); },
    Record: function (_keySchema, valueSchema, opts) { return Object.assign({ type: "object", additionalProperties: valueSchema }, opts || {}); },
    Ref: function (ref, opts) { return Object.assign({ $ref: ref }, opts || {}); },
    Intersect: function (schemas, opts) { return Object.assign({ allOf: schemas }, opts || {}); }
  };
  globalThis.__pi_typebox = { Type: Type, "default": { Type: Type } };
})();
