import { describe, expect, it } from "vitest";
import { defaultCreateValues } from "../../components/agent-config-defaults";
import { getUIAdapter } from "../registry";
import { SchemaConfigFields } from "../schema-config-fields";
import { tomcatLocalUIAdapter } from "./index";

describe("Tomcat UI registration", () => {
  it("uses the existing schema form and keeps model, settings, prompt and environment bindings", () => {
    const adapter = getUIAdapter("tomcat_local");
    expect(adapter).toBe(tomcatLocalUIAdapter);
    expect(adapter.ConfigFields).toBe(SchemaConfigFields);
    const config = adapter.buildAdapterConfig({
      ...defaultCreateValues,
      adapterType: "tomcat_local", model: "my-model", promptTemplate: "Perform {{ task.title }}",
      envBindings: { HOST_VALUE: { type: "plain", value: "present" } },
      adapterSchemaValues: { command: "/opt/tomcat", permissionMode: "deny-all" },
    });
    expect(config).toMatchObject({ model: "my-model", command: "/opt/tomcat", permissionMode: "deny-all", promptTemplate: "Perform {{ task.title }}", env: { HOST_VALUE: { type: "plain", value: "present" } } });
  });
});
