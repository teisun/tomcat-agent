# Sandbox 与 Approvals

**设计思想**：Sandbox 基于 Docker 提供 Agent 隔离，策略由 tool-policy、sandbox 白名单控制；Approvals 管理 exec 等敏感操作的审批流程，Gateway 提供 exec-approval-manager 与 server-methods。

---

## 一、Sandbox

- **sandbox/**：`openclaw/src/agents/sandbox/`，docker、context、config、browser、tool-policy、runtime-status。
- **sandbox.ts**：`openclaw/src/agents/sandbox.ts`，resolveSandboxContext、创建参数。
- **sandbox-cli**：recreate、explain、prune 等。
- **sandbox-paths**：workspace 与容器路径。

---

## 二、Approvals

- **exec-approval-manager**：`openclaw/src/gateway/exec-approval-manager.js`（或 .ts），管理 exec 审批请求与响应。
- **server-methods/exec-approvals**：Gateway Methods 暴露审批 API。
- **infra/exec-approvals**：底层审批逻辑。
