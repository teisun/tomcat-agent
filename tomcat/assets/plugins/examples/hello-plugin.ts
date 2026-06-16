/// <reference path="../../types/tomcat-plugin.d.ts" />

function asName(params: TomcatJsonValue): string {
  if (!params || typeof params !== "object" || Array.isArray(params)) {
    return "Tomcat";
  }
  var raw = (params as { name?: unknown }).name;
  return typeof raw === "string" && raw.trim() ? raw.trim() : "Tomcat";
}

export default function (pi: TomcatPluginAPI) {
  pi.registerFunction("helloPlugin", async function (params) {
    var name = asName(params);
    pi.log({ event: "hello_plugin_called", name: name });
    return {
      message: "Hello, " + name + "!",
      model: pi.getModel()
    };
  });
}
