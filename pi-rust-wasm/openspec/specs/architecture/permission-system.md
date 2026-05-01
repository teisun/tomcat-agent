# 权限子系统设计

本文描述当前代码中的权限模型，以 `src/core/permission/*`、`src/core/executor/primitives.rs`、`src/api/chat/*` 与 `src/core/system_prompt.rs` 为准。

## 1. 目标与不变量

- **统一入口**：文件读写、编辑、bash 执行、拖拽授权、cwd lazy prompt、Workspace State 渲染都通过同一个 `PermissionGate` 视图判断。
- **deny 优先**：内置与用户配置的 `path_rules deny` 命中后直接拒绝，不能被 `extra_roots`、session grant、拖拽授权或确认弹窗绕过。
- **readonly 降级**：`path_rules readonly` 与 agent 运行态数据目录允许读、拒绝写；写入不会再展示扩大授权选项。
- **默认可写根是 `agent_definition_dir`**：即 `~/.pi_/workspace-main/` 或 `workspace-<agentId>/`。它承载 Agent 设计态文件，如 `AGENTS.md`、`SOUL.md`、`MEMORY.md` 等。
- **启动 cwd 不是默认授权根**：`agent_workspace_dir` 是用户启动 `pi chat` 时 shell 的当前目录，只用于 prompt 中解释“当前目录 / 这个项目 / 相对路径”。访问该目录仍需 `workspace.extra_roots`、会话授权、拖拽授权或 cwd lazy prompt 授权。
- **executor 必须带 gate**：`DefaultPrimitiveExecutor` 构造时强制传入 `Arc<dyn PermissionGate>`，不再存在无 gate 的 legacy whitelist / no-gate 分支。
- **旧 whitelist 配置已删除**：`primitive.path_whitelist`、`primitive.bash_whitelist`、`primitive.auto_confirm_whitelist` 不再是 schema；路径允许根只由 `agent_definition_dir`、`workspace.extra_roots` 与运行时授权表达。

## 2. 目录语义

| 名称 | 来源 | 权限语义 |
| --- | --- | --- |
| `agent_workspace_dir` | `pi chat` 启动时的 `std::env::current_dir()` snapshot | prompt 语义上的“当前目录 / 项目目录”；不自动授权文件访问 |
| `agent_definition_dir` | `[agent].workspace` 或默认 `~/.pi_/workspace-main/` | 默认 read/write 授权根，`GrantSource::AgentWorkspace` 指向它 |
| `agent_trail_dir` | `~/.pi_/agents/<agentId>/` | 运行态数据目录；sessions/logs/audit 等按 readonly 暴露，敏感子路径由 builtin deny 保护 |
| `workspace.extra_roots` | `~/.pi_/pi.config.toml [workspace] extra_roots` | 全局额外 read/write 授权根，所有 agent 共用 |

`agent_workspace_dir` 与 `agent_definition_dir` 的命名容易混淆：前者是用户项目目录概念，后者是 Agent 自身定义目录。权限系统只把后者作为默认 writable root。

## 3. 三层决策模型

`DefaultPermissionGate::check(level, path)` 的当前顺序：

```text
输入: PermissionLevel + path
  |
  v
Layer 0: 规范化路径
  normalize_path + canonicalize_with_existing_ancestor
  |
  v
Layer 1: path_rules / readonly
  Deny 命中      -> PermissionDecision::Deny
  Readonly+Write -> PermissionDecision::Deny
  Readonly+Read  -> PermissionDecision::Allow(PathRuleReadOnly)
  agent_data_dir -> Read Allow(AgentDataDir) / Write Deny
  |
  v
Layer 2: 已授权根
  agent_definition_dir -> Allow(AgentWorkspace)
  workspace.extra_roots -> Allow(ConfigExtraRoot)
  session_grants -> Allow(SessionGrant)
  dragged_paths -> Allow(DraggedPath)
  |
  v
Layer 3: 未授权
  -> NeedConfirm { prompt, suggested_root }
```

`PermissionDecision` 只有三类：

```rust
pub enum PermissionDecision {
    Allow { source: GrantSource },
    NeedConfirm { prompt: String, suggested_root: Option<PathBuf> },
    Deny { reason: String },
}
```

`suggested_root` 用于确认 UI 展示“加入工作区 / 持久化到 `workspace.extra_roots`”之类选项。命中 deny / readonly write 时不会进入确认层。

## 4. 核心类型

### 4.1 `PermissionLevel`

```rust
pub enum PermissionLevel {
    Read,
    Write,
    Bash,
    BashApproval,
    Forbidden,
}
```

- `Read`：读取文件、列目录。
- `Write`：写文件、编辑文件，以及 bash 命令中解析出的路径预检。
- `Bash`：bash 命令通过 forbidden / approval regex 后的普通执行等级。
- `BashApproval`：命中 `bash_approval_required`，需要用户确认。
- `Forbidden`：命中 `bash_forbidden` 或等价禁止策略。

### 4.2 `GrantSource`

| 来源 | 当前含义 |
| --- | --- |
| `AgentWorkspace` | `agent_definition_dir` 默认授权；名称保留是为了审计 schema 兼容 |
| `ConfigExtraRoot` | `workspace.extra_roots` 命中 |
| `SessionGrant` | 用户在确认、cwd lazy prompt 或拖拽菜单中授予的本会话授权 |
| `DraggedPath` | 纯拖拽路径授权来源 |
| `PathRuleReadOnly` | `path_rules readonly` 允许读 |
| `AgentDataDir` | agent 运行态目录只读集合允许读 |
| `BashPolicy` | bash regex 策略命中后的来源 |
| `AutoConfirmFlag` | `primitive.auto_confirm` 使确认阶段自动允许 |

### 4.3 `GateConfig`

```rust
pub struct GateConfig {
    pub agent_definition_dir: PathBuf,
    pub extra_roots: Vec<PathBuf>,
    pub agent_data_readonly_dirs: Vec<PathBuf>,
    pub user_path_rules: Vec<PathRule>,
    pub user_bash_forbidden: Vec<String>,
    pub user_bash_approval: Vec<String>,
    pub auto_confirm: bool,
}
```

`ChatContext::from_config` 构造 `DefaultPermissionGate`，并把同一个 `Arc<dyn PermissionGate>` 注入：

- `DefaultPrimitiveExecutor`
- `CwdLazyPrompt`
- 拖拽路径菜单处理
- system prompt / `WorkspaceStateSection`
- `config_set` / `path_rules` 相关热更新路径

## 5. Effective Roots

`PermissionGate::effective_roots()` 返回当前 prompt 可见的权限快照：

```rust
pub struct EffectiveRoots {
    pub read_write: Vec<PathBuf>,
    pub read_only: Vec<PathBuf>,
}
```

当前组成：

| bucket | 内容 |
| --- | --- |
| `read_write` | `agent_definition_dir` + `workspace.extra_roots` + `session_grants` + `dragged_paths` |
| `read_only` | `agent_data_readonly_dirs` + 命中 readonly path rule 的范围 |

注意：`read_write` 不包含启动 cwd，除非 cwd 已经被用户加入 `workspace.extra_roots` 或本会话授权。

## 6. 确认与运行时授权

### 6.1 `ConfirmDecision`

```rust
pub enum ConfirmDecision {
    AllowOnce,
    AllowAndPersistRoot { root: PathBuf },
    Deny,
}
```

- `AllowOnce`：写入 `SessionGrants`，只在当前进程会话内生效。
- `AllowAndPersistRoot`：由 UI / 装饰器将 root 写入 `workspace.extra_roots`，并确保当前会话也能继续使用授权。
- `Deny`：拒绝操作，返回权限错误。

旧的 `UserConfirmationProvider::confirm(...) -> bool` 仍作为 trait 兼容入口存在，但新代码路径使用 `confirm_decision(...)`，`DefaultPrimitiveExecutor` 的 gate 分支会传入 `suggested_root`。

### 6.2 `auto_confirm`

`primitive.auto_confirm = true` 不改变 `PermissionGate::check` 的三层路径判定：deny / readonly write 仍会拒绝。它只影响 executor 进入确认阶段后的行为，让 `require_user_confirmation` 自动返回允许，并在审计里记录 `AutoConfirmFlag`。

### 6.3 cwd lazy prompt

`CwdLazyPrompt` 是 `UserConfirmationProvider` 装饰器。首次有非 bash 工具调用落到 `agent_workspace_dir` 子树且 cwd 尚未授权时，它会展示范围级选项：

- `[a]` 写入 `workspace.extra_roots`，以后也允许访问。
- `[s]` 仅本会话允许，写入 `SessionGrants`。
- `[n]` 不加入，本会话内不再弹 cwd 范围级提示，回退到逐文件确认。

该机制只在真实工具调用触达 cwd 时触发，不会因为 cwd 出现在 system prompt 中就自动授权。

## 7. Bash 权限流

Bash 执行分两部分：

1. `PermissionGate::check_bash(command)` 运行 forbidden / approval regex：
   - 命中 `bash_forbidden` -> `Deny`
   - 命中 `bash_approval_required` -> `NeedConfirm`
   - 未命中 -> `Allow { source: BashPolicy }`
2. `DefaultPrimitiveExecutor::execute_bash` 调用 `bash_parser::extract_paths(command)` 静态提取候选路径，并对每个路径执行 `gate_check_path(Write, path)`。

因此 bash 命令本身和命令中的显式路径都必须通过权限系统：

```text
execute_bash(cmd)
  |
  +-- gate.check_bash(cmd)
  |     forbidden -> Deny
  |     approval  -> confirm_decision(Bash, suggested_root=None)
  |
  +-- bash_parser::extract_paths(cmd)
        for each path:
          gate.check(Write, path)
```

当前 `bash_parser` 是静态、尽力而为的解析器，能覆盖绝对路径、`~`、`./`、`../`、含 `/` 的相对路径、`--flag=value` 与 `NAME=/path` assignment RHS。它不能保证发现运行时才出现的路径，例如 `eval $X`、复杂 shell 展开、脚本内部访问、命令替换拼接路径等；该限制已作为安全 TODO 记录，后续应结合命令意图分析与提示词注入防御继续收敛。

## 8. 拖拽路径授权

拖拽层只识别“整行纯路径 token”：

- 纯路径行进入授权菜单。
- “路径 + 意图文字”按普通聊天输入处理，不在拖拽层自动授权。
- deny/cancel 后本轮不发送原始拖拽内容给 LLM，只写入 `[drag-cancel]` 合成 user note。

拖拽菜单由 `render_drag_menu(path, gate)` 根据当前 gate 预检裁剪：

- 命中 deny：只允许取消。
- 命中 readonly：允许读相关选项，不允许扩大成写权限。
- 普通路径：可选择本会话允许、写入 `workspace.extra_roots`、设为 readonly、设为 deny 或取消。

## 9. System Prompt 集成

System prompt 分两个相关段落：

### 9.1 Workspace Context

`WorkspaceContextSection` 解释三个目录的概念：

- `agent_workspace_dir` 是当前目录语义来源，但 **NOT automatically authorized for file access**。
- `agent_definition_dir` 是 Agent 设计态目录。
- `agent_trail_dir` 是运行态数据目录。

### 9.2 Workspace State

`WorkspaceStateSection` 来自 `ctx.gate.effective_roots()` 与 `ctx.gate.effective_path_rules()`，按固定顺序渲染：

```text
## Current Working Directory
...

## Workspace State
### Read/Write roots
...
### Read-only roots
...
### Deny rules
...
```

`Current Working Directory` 必须先于 `Workspace State`，用于告诉 LLM cwd 是相对路径的解释基准；`Workspace State` 则列出实际授权范围，二者不能混为一谈。

## 10. 配置入口

当前相关配置：

```toml
[workspace]
extra_roots = ["/path/to/project"]

[primitive]
auto_confirm = false
bash_forbidden = []
bash_approval_required = []

[[primitive.path_rules]]
path = "~/.ssh"
mode = "deny"
```

CLI / 工具入口：

- `pi workspace add/list/remove` 维护 `[workspace] extra_roots`。
- `pi pathrules add/list` 维护或展示 `primitive.path_rules` 与 builtin 规则。
- chat 内 `config_set primitive.path_rules` 可以在同一会话里热更新 deny / readonly 规则。

## 11. 审计字段

`PrimitiveAuditEntry` 记录权限相关字段：

| 字段 | 含义 |
| --- | --- |
| `permission_level` | `Read` / `Write` / `Bash` / `BashApproval` / `Forbidden` |
| `grant_source` | gate 判定来源，如 `AgentWorkspace`、`ConfigExtraRoot`、`SessionGrant`、`BashPolicy` |
| `in_working_dir` | 历史字段名，当前代码用来标记是否来自默认授权根或配置额外根；不要再把它理解成“启动 cwd 内” |

`AgentWorkspace` 也是历史审计枚举名，当前指 `agent_definition_dir`。

## 12. 测试覆盖

关键自动化覆盖：

- `src/core/permission/tests/gate.rs`：默认根、cwd 未授权、path_rules、bash policy。
- `src/core/executor/tests/suite.rs`：executor 强制 gate、读写编辑、bash 路径预检、确认行为。
- `src/api/chat/tests/cwd_lazy_prompt.rs`：cwd 首次触达范围级授权。
- `tests/dragged_path_e2e.rs`：拖拽 deny/cancel 不进 LLM、菜单裁剪。
- `tests/bash_assignment_deny.rs`：`NAME=/path` RHS 进入 bash 路径预检。
- `tests/system_prompt_cwd_priority.rs` 与 `src/core/tests/system_prompt.rs`：cwd prompt 语义与 Workspace State 授权清单分离。

