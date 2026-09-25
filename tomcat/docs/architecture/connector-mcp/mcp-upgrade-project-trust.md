# MCP configuration upgrade: project trust and runtime settings

```text
mcp.json (Global / Workspace) ── server name, command/URL, auth, toolFilter
                    │
                    ├─ Global: available without project approval
                    └─ Workspace: wait for one project approval ──► all project MCP servers

tomcat.config.toml [connector.mcp] ── one runtime policy for every MCP source
{work_dir}/project-trust.json ── remembered project roots (default work_dir: ~/.tomcat)
```

This is a **compatibility-preserving format migration**. Upgrade the CLI and VS Code extension together. Tomcat no longer *uses* per-server `startupTimeoutMs`, `callTimeoutMs`, `maxConcurrentCalls`, `trusted`, or `integrity` from either Global or Workspace `mcp.json`; if it finds one, it logs a warning naming the file, server, key, and replacement, then continues loading the remaining configuration. Move runtime policy to `[connector.mcp]`; `trusted` and `integrity` have no per-server replacement because project approval is now decided by the one-time project-trust record. Loading never rewrites the file. **Important preservation limit:** the next successful action that saves `mcp.json` (for example Add, Remove, or a tool toggle) serializes only fields Tomcat recognizes, so it removes any unrecognized or obsolete JSON keys. Keep durable comments, metadata, or settings owned by another tool outside `mcp.json`. Do not put obsolete keys in `toolFilter` or `env` as a workaround: those nested objects remain strictly validated.

## Move runtime settings

Keep only MCP server details in `mcp.json`. In `~/.tomcat/tomcat.config.toml`, optionally set:

```toml
[connector.mcp]
startup_timeout_ms = 30000
call_timeout_ms = 120000
max_concurrent_calls = 16
```

These are the defaults, so omit the whole section if they suit you. The startup and call values must be positive representable milliseconds; the concurrent-call limit is 1–64. Environment overrides follow the normal naming convention: `TOMCAT__CONNECTOR__MCP__STARTUP_TIMEOUT_MS`, `TOMCAT__CONNECTOR__MCP__CALL_TIMEOUT_MS`, and `TOMCAT__CONNECTOR__MCP__MAX_CONCURRENT_CALLS`. The policy applies to **all** MCP servers in this process. The 16-call quota is still **per source** (not a single shared 16-call pool); the call timeout covers admission and each renewable idle window. A previous per-server 60-second Playwright startup is no longer implicit: set `startup_timeout_ms = 60000` if your environment needs it, knowing that other MCP sources in this process also receive 60 seconds. Restart Tomcat after editing the main config.

## Trust a project once

When a project is first loaded, the VS Code extension asks `Trust this project?` with the project root, `Not now`, and `Trust project`. CLI asks before the first interactive input; non-TTY sessions do not prompt. `Not now` does not write a denial, does not exit chat, and leaves Global MCP available. Unapproved Workspace connectors appear together under **AWAITING PROJECT TRUST**, with one **Trust project** button on the group. You can also use `/connector trust-project` in CLI. Adding a Workspace connector from Settings uses **Add and Trust** (one request) when the project is untrusted; Global or already-trusted projects use **Add**.

Approval is for the **project root**, not for each server or each command fingerprint. Earlier `connector-trust.json` approvals do **not** upgrade to project approval. That old file is no longer used; you may remove it manually after verifying the new behavior. There is no per-server Deny: use Remove or the existing `[connector] disabled` names to avoid running an individual server. The independent file-access and tool-permission rules still apply.

Before granting trust, inspect the project's MCP config and startup commands. Changing a command in an already trusted project will **not** ask for another MCP-specific approval. Keep project files under your own review; for an untrusted project choose `Not now`.

## Troubleshooting

- **Legacy-key warning**: `startupTimeoutMs`, `callTimeoutMs`, `maxConcurrentCalls`, `trusted`, and `integrity` are ignored after Tomcat logs a warning; move only runtime policy to `[connector.mcp]`. They do not stop MCP loading. The next successful save of `mcp.json` removes them, as it does every unrecognized key, so keep information Tomcat does not own outside that file.
- **Malformed MCP configuration**: invalid JSON or an invalid strict nested object such as `toolFilter` still prevents that MCP configuration from loading; correct the named file and restart Tomcat.
- **Workspace tools unavailable**: check for **AWAITING PROJECT TRUST** in Settings. Global sources can still connect. An unreadable/corrupt project-trust record never grants approval.
- **No prompt in VS Code**: ensure a workspace folder is open and the CLI/extension support `get_project_trust` and `trust_project`. Look at Tomcat output for lookup errors; older Serve versions must be upgraded, not routed back to per-server trust.
- **No prompt in a pipe**: intentional. Use interactive CLI, Settings, or `/connector trust-project` to approve the project.
