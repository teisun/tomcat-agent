export const type = "tomcat_local";
export const label = "Tomcat (local)";

export const models = [{ id: "default", label: "Tomcat configured default" }];

export const agentConfigurationDoc = `# Tomcat (local)

Runs Tomcat through the shared ACP engine. The host must prepare an existing absolute workspace path on the same machine/filesystem as the shared Tomcat service.

- command: Tomcat executable (default: tomcat). agentCommand can override the full ACP launch command; the launcher must accept --skills-dir.
- model: default uses Tomcat's configured default; a custom value is a Tomcat model ID. Inspect configured IDs with tomcat model list. Model listing here does not start a conversation or make a paid request.
- env: forwarded to the controller connection and its tools. Shared-service configuration still belongs to Tomcat; credentials/model configuration are not copied into Paperclip.
- permissionMode / nonInteractivePermissions: replies to ACP approval requests. Tomcat's primitive.auto_confirm must be false for it to request approval. Hard-deny rules always apply.
- Skills are copied into a run-private temporary directory, passed with --skills-dir, and cleaned after runtime teardown. No run-specific skills are installed into ~/.tomcat/skills.
- warmHandleIdleMs must be 0. Session handling and cancellation use Paperclip's existing ACP runtime. A stable POSIX exec launcher selects the private directory from the existing run ID, keeping compatible sessions resumable; use macOS, Linux or WSL2 with sh available.
- Usage is partial. Missing price is unpriced, not zero; the terminal log frame is not a billing receipt.
- Environment checks inspect version, ACP support and service status only. They never start/restart the service, create a session or send a model prompt.
`;
