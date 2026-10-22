import { buildAdapterEnvConfig } from "@paperclipai/adapter-utils";
import { parseTomcatStdoutLine } from "@paperclipai/adapter-tomcat-local/ui";
import type { UIAdapterModule } from "../types";
import { buildSchemaAdapterConfig, SchemaConfigFields } from "../schema-config-fields";

export const tomcatLocalUIAdapter: UIAdapterModule = {
  type: "tomcat_local",
  label: "Tomcat",
  parseStdoutLine: parseTomcatStdoutLine,
  ConfigFields: SchemaConfigFields,
  buildAdapterConfig(values) {
    const config = buildSchemaAdapterConfig(values);
    const env = buildAdapterEnvConfig(values.envBindings, values.envVars);
    if (Object.keys(env).length > 0) config.env = env;
    if (values.promptTemplate) config.promptTemplate = values.promptTemplate;
    return config;
  },
};
