let counter = 0;

pi.registerFunction("echoHost", function (params) {
  params = params || {};
  return {
    plugin: "function-echo-plugin",
    point: "test.echo",
    echoed: params.text || null
  };
});

pi.registerFunction("nextCount", function (params) {
  params = params || {};
  counter += 1;
  return {
    plugin: "function-echo-plugin",
    point: "test.counter",
    count: counter,
    label: params.label || null
  };
});
