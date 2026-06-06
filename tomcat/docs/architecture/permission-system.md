# 权限子系统设计

本文描述当前代码中的权限模型，以 `src/core/permission/*`、`src/core/tools/primitive/*`、`src/api/chat/*` 与 `src/core/system_prompt.rs` 为准。

## 1. 目标与不变量

- **统一入口**：文件读写、编辑、bash 执行、`/path` 路径授权、cwd lazy prompt、Workspace State 渲染都通过同一个 `PermissionGate` 视图判断。
- **deny 优先**：内置与用户配置的 `path_rules deny` 命中后直接拒绝，不能被 `workspace_roots`、session grant、`/path` 授权或确认弹窗绕过。
- **readonly 降级**：`path_rules readonly` 与 agent 运行态轨迹目录只允许读；写入不会展示扩大授权选项。
- **默认可写根是 `agent_definition_dir`**：即 `~/.tomcat/workspace-main/` 或 `workspace-<agentId>/`，承载 Agent 设计态文件。
- **启动 cwd 不是默认授权根**：`agent_workspace_dir` 只用于 prompt 中解释“当前目录 / 这个项目 / 相对路径”。访问该目录仍需 `workspace.workspace_roots`、会话授权或 cwd lazy prompt 授权。
- **executor 必须带 gate**：`DefaultPrimitiveExecutor` 构造时强制传入 `Arc<dyn PermissionGate>`，不存在 no-gate legacy 分支。
- **Skill 正文不放大权限**：`load_skill` 只是按名定位后再走 `PermissionGate(Read)` 读 `SKILL.md` / 附件；详细链路见 `skill-system.md`。

## 2. 目录语义

| 名称 | 来源 | 权限语义 |
| --- | --- | --- |
| `agent_workspace_dir` | `tomcat chat` 启动时的 `std::env::current_dir()` snapshot | prompt 语义上的“当前目录 / 项目目录”；不自动授权文件访问 |
| `agent_definition_dir` | `[agent].workspace` 或默认 `~/.tomcat/workspace-main/` | 默认 read/write 授权根，`GrantType::AgentDefinitionDir` |
| `agent_trail_dir` | `~/.tomcat/agents/<agentId>/` | sessions/logs/audit/tool-results 等运行态轨迹目录；按 readonly 暴露，敏感子路径由 builtin deny 保护 |
| `workspace.workspace_roots` | `~/.tomcat/tomcat.config.toml [workspace] workspace_roots` | 全局用户工作区 read/write 授权根，所有 agent 共用 |

## 3. 三层决策模型

`DefaultPermissionGate::check(op, path)` 的当前顺序：

```text
输入: PrimitiveOperation + path
  |
  v
Layer 0: normalize_path + canonicalize_with_existing_ancestor
  |
  v
Layer 1: path_rules / readonly
  Deny 命中      -> Deny
  Readonly+Write -> Deny
  Readonly+Read  -> Allow(PathRuleReadOnly, PathRulesConfig)
  |
  v
Layer 2: 已授权根
  agent_definition_dir     -> Allow(AgentDefinitionDir, BuiltinDefault)
  workspace.workspace_roots -> Allow(AgentWorkspaceRoot, WorkspaceRootsConfig)
  session_grants           -> Allow(SessionScope, stored trigger)
  agent_trail_dir read     -> Allow(AgentTrailDir, BuiltinDefault)
  |
  v
Layer 3: 未授权
  auto_confirm -> Allow(SessionScope, AutoConfirmFlag)
  otherwise    -> NeedConfirm { reason, suggested_root }
```

`PermissionDecision`：

```rust
pub enum PermissionDecision {
    Allow { grant: GrantTrace, scope: PermissionScope },
    NeedConfirm { reason: String, suggested_root: Option<PathBuf> },
    Deny { reason: String },
}
```

## 4. GrantTrace

`GrantSource` 已拆成授权类型和触发来源：

```rust
pub struct GrantTrace {
    pub grant_type: GrantType,
    pub trigger: GrantTrigger,
}
```

| `GrantType` | 含义 |
| --- | --- |
| `AgentDefinitionDir` | 默认 agent 设计态目录授权 |
| `AgentWorkspaceRoot` | `[workspace] workspace_roots` 命中 |
| `SessionScope` | 当前会话授权范围 |
| `PathRuleReadOnly` | `path_rules readonly` 允许读 |
| `AgentTrailDir` | agent 运行态轨迹目录只读集合 |
| `BashPolicy` | bash regex 策略 |

| `GrantTrigger` | 含义 |
| --- | --- |
| `BuiltinDefault` | 内置默认规则 |
| `WorkspaceRootsConfig` | `[workspace] workspace_roots` 配置 |
| `PathRulesConfig` | `path_rules` 配置或运行时追加 |
| `BashRegexConfig` | bash forbidden / approval regex 策略 |
| `UserConfirm` | 普通路径确认菜单 |
| `CwdLazyPrompt` | cwd lazy prompt |
| `DraggedPathMenu` | `/path` 路径授权菜单（名称为兼容历史审计保留） |
| `AutoConfirmFlag` | `primitive.auto_confirm` 自动允许 |

## 5. Effective Roots

`PermissionGate::effective_roots()` 返回当前 prompt 可见的权限快照：

```rust
pub struct EffectiveRoots {
    pub read_write: Vec<PathBuf>,
    pub read_only: Vec<PathBuf>,
}
```

| bucket | 内容 |
| --- | --- |
| `read_write` | `agent_definition_dir` + `workspace.workspace_roots` + `session_grants` |
| `read_only` | `agent_trail_readonly_dirs` + 命中 readonly path rule 的范围 |

注意：`read_write` 不包含启动 cwd，除非 cwd 已经被用户加入 `workspace.workspace_roots` 或本会话授权。`/path` 路径授权不再是独立 bucket；`/path` 菜单 `[a]` 会写入 `SessionGrants`，trigger 记录为 `DraggedPathMenu`（兼容历史审计命名）。

## 6. 确认与运行时授权

普通执行期路径 `NeedConfirm` 统一展示三选项：

- `[s]`：本次会话允许当前目标路径本身。文件即单文件，目录即目录；写入 `SessionGrants`，trigger=`UserConfirm`。
- `[w]`：以后也允许访问 `suggested_root`（当前实现通常为目标路径父目录）。确认层写入 `workspace.workspace_roots`，executor 同步写入本会话授权。
- `[c]`：取消 / 拒绝当前操作。

显式 `/path <路径>` 命令展示五选项：

- `[a]` 本次会话允许，落入 `SessionGrants`，trigger=`DraggedPathMenu`。
- `[w]` 持久写入 `workspace.workspace_roots`，并同步当前会话授权。
- `[r]` 持久写入 `path_rules readonly`。
- `[d]` 持久写入 `path_rules deny`。
- `[c]` 取消，不向 LLM 发送本地命令内容。

`CwdLazyPrompt` 只在首次工具调用触达 `agent_workspace_dir` 子树且 cwd 尚未授权时触发，按键同样是 `[s]/[w]/[c]`。选择 `[c]` 会拒绝当前操作并将 cwd lazy 标记为 dismissed；之后同会话内 cwd 子树路径走普通三选项菜单，不再弹 cwd lazy prompt。用户输入未识别选项时会打印 warning 并按取消处理，不会静默吞掉输入；工具失败回执会提示用户下次触达 cwd 时可重新选择 `[s]/[w]/[c]`，或执行 `tomcat workspace add <cwd>` 永久授权。

bash approval 命中时不展示路径持久化选项，只展示命令、命中规则和 `[y/N]`。同意后记录 `GrantType::BashPolicy` + `GrantTrigger::UserConfirm`。

## 7. Bash 权限流

Bash 执行分两部分：

1. `PermissionGate::check_bash(command)` 运行 forbidden / approval regex：
   - 命中 `bash_forbidden` -> `Deny`
   - 命中 `bash_approval_required` -> `NeedConfirm`
   - 未命中 -> `Allow { grant_type: BashPolicy, trigger: BashRegexConfig }`
2. `DefaultPrimitiveExecutor::execute_bash` 仅对真实 `cwd` 做 `gate_check_path(Read, cwd)`，随后直接进入 `bash_ast` / `check_bash(command)` / `spawn`。

不再从 bash 命令字符串里静态猜测路径再做 `gate_check_path(Bash, token)`。原因是 `node:fs/promises`、`@scope/pkg`、jq 过滤式、heredoc、`node -e` 等组合太多，误判成本远高于收益。

## 8. Audit Schema

`PrimitiveAuditEntry` 当前字段：

- `permission_scope: Option<String>`
- `grant_type: Option<String>`
- `grant_trigger: Option<String>`

旧字段 `grant_source` 与 `in_working_dir` 已删除，开发期不保留兼容。

## 9. System Prompt 集成

System prompt 的 Workspace State 来自 `ctx.gate.effective_roots()` 和 `effective_path_rules()`。`read_write` 只渲染 `agent_definition_dir`、`agent_workspace_root` 与 `session_grant`；不会再出现 `[dragged_path]` 标签。
