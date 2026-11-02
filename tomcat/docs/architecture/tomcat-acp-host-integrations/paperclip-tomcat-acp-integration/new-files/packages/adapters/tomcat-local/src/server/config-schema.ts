import type { AdapterConfigSchema } from "@paperclipai/adapter-utils";

export function getConfigSchema(): AdapterConfigSchema {
  return {
    fields: [
      { key: "command", label: "Tomcat executable", type: "text", default: "tomcat", hint: "Executable name or absolute path. The adapter starts tomcat acp." },
      { key: "agentCommand", label: "ACP command override", type: "text", hint: "Optional complete Tomcat ACP launch command. It must accept --skills-dir. Leave empty to use the executable above." },
      {
        key: "permissionMode", label: "ACP approval replies", type: "select", default: "approve-all",
        options: [{ value: "approve-all", label: "Approve all" }, { value: "approve-reads", label: "Approve reads" }, { value: "deny-all", label: "Deny all" }],
        hint: "Controls replies only when Tomcat asks. Set primitive.auto_confirm=false in Tomcat to require approval; hard-deny rules always apply.",
      },
      {
        key: "nonInteractivePermissions", label: "Non-interactive permission requests", type: "select", default: "deny",
        options: [{ value: "deny", label: "Deny" }, { value: "fail", label: "Fail the run" }],
      },
    ],
  };
}
