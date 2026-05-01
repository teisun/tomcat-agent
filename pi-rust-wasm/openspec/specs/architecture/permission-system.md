# 权限子系统（PermissionGate）设计

> 本文档为 [Architecture](../Architecture.md) 中「7. 安全设计核心原则」与「最小权限 / 用户知情权」两条原则的展开，是 T2-P0-004 `workspace-permission-tiers` 在 `feature/workspace-permission-tiers` 上的最终落地版。
>
> 实施 plan 与决议记录见 `.cursor/plans/workspace_permission_tiers_design_1ad7681a.plan.md`，看板与变更日志见 [`agents/TASK_BOARD_002.md`](../../../agents/TASK_BOARD_002.md) §6（2026-04-27 两条 T2-P0-004 变更行）。
>
> **Post-merge hotfix**：`feature/workspace-permission-tiers-hotfix` 把 §10 拆成「§10.0 cwd 注入」+「§10.1 Lazy First-Touch 范围级授权」，并在 §8 追加「§8.1 Token 内存在性切分」。补丁 plan 见 `.cursor/plans/cwd-startup-prompt-fix_7af19637.plan.md`。

---

## 1. 目标与不做的事

### 1.1 目标

- **工作目录与 `extra_roots`**：未被 `path_rules`（Deny / Readonly）否决时，读与写均 **默认允许**，无需确认弹窗。
- **工作目录外**：路径原语必须 **显式授权**（`NeedConfirm` prompt，而非直接 403）；确认时支持 **`always` / `单次` 两档**（`AllowAndPersistRoot` / `AllowOnce`），并可追加 `extra_roots` / `path_rules` 持久化，告别一刀切弹窗。
- Bash 走**结构化解析**而非整命令行 regex：解析 token、抽取目标路径，再走与 4 原语相同的 `PermissionGate` 决策。
- 把 `~/.ssh` / `~/.aws` / agent 自身凭据 / 自身审计目录这些「Agent 不该写」的路径用 builtin `path_rules` 兜住。
- Agent 自我感知：通过 system prompt 告诉模型当前 effective roots / path_rules / agent data dir，让 LLM 不要去试错那些必然被拒的路径。
- 提供 `pi pathrules add/list` / `config_get` / `config_set` 三条人/Agent 改配置的入口，自然语言改配置不再必须手动 `pi config edit`。

### 1.2 不做的事（Anti-goals）

- **不做**审计 JSONL 批量迁移脚本：开发期无存量用户，新字段一律 `#[serde(default)]`，老行直接读出 `None` 兜底。
- **不引入** ACL / 多用户 / 角色模型：本系统是单用户单 agent 的本地权限分级。
- **`pi pathrules` 首版不实现** `remove` / `clear-session`；用户改错可走 `pi config edit` 直接编 TOML。
- **不在权限层做 input sanitize**（路径注入、shell 注入）；这部分在原语层 / `bash_parser` 层各自负责。
- **不缓存** `effective_roots()`：当前规模下重新计算开销可忽略，缓存反而引入失效逻辑（session_grants 变更后系统 prompt 不刷新）。

---

## 2. 三层决策模型

```text
┌──────────────────────────────────────────────────────────────┐
│ Layer 1：path_rules deny（强否决，所有人都拦不住）         │
│   builtin（~/.ssh / ~/.aws / ~/.pi_/credentials/**）         │
│   ∪ user（pi.config.toml::primitive.path_rules）            │
│   ∪ session_runtime（当前 pi chat 新增规则）                 │
└──────────────────────────────────────────────────────────────┘
                       │ 命中 → Deny
                       ▼ 未命中
┌──────────────────────────────────────────────────────────────┐
│ Layer 2：Allow（只读 / 只写都看 op）                         │
│   ① 工作目录（AgentWorkspace）                              │
│   ② 配置 extra_roots（ConfigExtraRoot）                      │
│   ③ session_grants（SessionGrant 仅本会话）                  │
│   ④ dragged_paths（DraggedPath 拖拽留档）                    │
│   ⑤ path_rules readonly（PathRuleReadOnly 仅 read）         │
│   ⑥ auto_confirm 短路（AutoConfirmFlag）                    │
└──────────────────────────────────────────────────────────────┘
                       │ 任一命中 → Allow{source}
                       ▼ 全部未命中
┌──────────────────────────────────────────────────────────────┐
│ Layer 3：第三波兜底                                          │
│   - agent_data_readonly_dirs 内 read → AgentDataDir         │
│   - 其它 → NeedConfirm{prompt}                              │
└──────────────────────────────────────────────────────────────┘
```

实现位于 `src/core/permission/gate.rs::DefaultPermissionGate::check()`，单元覆盖见 `src/core/permission/tests/gate.rs`（含 `pr9_*`）。

把上面的三层展开成「一次调用 `gate.check(op, path)` 内部分支」更直观——同一棵树覆盖 read / write 两种 op，差异只在每个 Allow 来源对 op 的过滤上：

```text
                  gate.check(op, path)
                          │
                          ▼
          ┌────────────────────────────────┐
          │ Layer 1: path_rules deny?      │   builtin ∪ user
          │  (强否决，任何 Allow 都救不回) │
          └────────────────┬───────────────┘
                hit ◄──────┴──────► miss
                 │                   │
                 ▼                   ▼
            Deny{reason}     ┌──────────────────────────┐
                             │ Layer 2: any Allow src?  │
                             └────────────┬─────────────┘
                                          │
              ┌──────────────┬────────────┼────────────┬──────────────┐
              ▼              ▼            ▼            ▼              ▼
        AgentWorkspace  ConfigExtraRoot SessionGrant DraggedPath  PathRuleReadOnly
        path ⊂ ws_dir   path ⊂ extra   path ∈ sess  path ∈ drag   op=Read &&
                        _roots         _grants      _paths       path ∈ ro rules
                                       (write flag) (write flag)
                                                │
                              ┌─────────────────┼─────────────────┐
                              ▼                 ▼                 ▼
                         BashWhitelist    AutoConfirmFlag    (none of above)
                         op=Bash &&       cfg.auto_confirm        │
                         cmd ∈ bw         && op != Bash           ▼
                                                          ┌──────────────────────────┐
                                                          │ Layer 3: agent_data RO?  │
                                                          │ (path ⊂ agent_data_ro    │
                                                          │  && op == Read)          │
                                                          └────────────┬─────────────┘
                                                              hit ◄────┴────► miss
                                                               │              │
                                                               ▼              ▼
                                                  Allow{AgentDataDir}  NeedConfirm{prompt}
              │
              ▼
      Allow{<first matching source>, in_working_dir = path ⊂ ws_dir}
```

几条易踩的坑：

- `AutoConfirmFlag` 仅短路 op ∈ {Read, Write}，bash 永远不走它（避免 `auto_confirm=true` 一开把 `rm -rf` 也放过）。
- `SessionGrant` / `DraggedPath` 是「按 path 颗粒度」的，每条记录自带 `write` 布尔；write=false 时，`check(Write, …)` 跳过该条继续向后找，而不是直接 NeedConfirm。
- Layer 3 只对 op=Read 生效；同路径的 write 仍会落到 `NeedConfirm`，避免 agent 自审计目录被悄悄写脏。

### 2.1 为什么把 path_rules deny 放最前面

- 用户 / builtin / 当前会话运行时规则都可能写 deny，**任何 Allow 来源都不能弱化它**。如果放在 Layer 2 之后，agent_data_readonly_dirs、SessionGrants、DraggedPaths 或 extra_roots 兜底会让被拒路径重新变成可读，违反最小权限。
- 这条规则被 `defaults.rs::BUILTIN_DEFAULT_PATH_RULES` 严格覆盖：`~/.ssh` / `~/.aws` / `~/.gnupg` / `~/.config/gh` / `~/.pi_/pi.config.toml` / `~/.pi_/credentials` / `~/.pi_/agents/*/agent/credentials*` / `~/.pi_/agents/*/agent/auth-profiles*.json` 全部 deny。

### 2.2 为什么 dragged_paths 与 session_grants 分两个 source

- `SessionGrant` 是用户在 confirm UI 里按 `[a]` AllowOnce 时的产物，**仅本会话生效**，重启即失效。
- `DraggedPath` 是用户在 chat TUI 里把路径拖进来的产物，可能伴随 `[w]` 写持久化或 `[a]` 仅一次。两者审计来源不同，便于事后查证「这次允许是从哪里来的」。

---

## 3. 核心抽象

```rust
// src/core/permission/types.rs
pub enum PermissionLevel { Read, Write, Bash }
pub enum PermissionDecision {
    Allow { source: GrantSource, in_working_dir: bool },
    NeedConfirm { prompt: String, in_working_dir: bool },
    Deny { reason: String },
}
pub enum GrantSource {
    AgentWorkspace, ConfigExtraRoot, SessionGrant, DraggedPath,
    PathRuleReadOnly, BashWhitelist, AutoConfirmFlag, AgentDataDir,
}

pub trait PermissionGate: Send + Sync {
    fn check(&self, op: PrimitiveOperation, path: &str) -> Result<PermissionDecision, AppError>;
    fn check_bash(&self, command: &str) -> Result<PermissionDecision, AppError>;
    fn effective_roots(&self) -> EffectiveRoots;
    fn effective_path_rules(&self) -> Vec<PathRule>;
}
```

详细字段表与 `EffectiveRoots`、`SessionGrants`、`DraggedPaths` 见 `src/core/permission/{types,session_grants,dragged_paths,path_rule}.rs`。

trait 与默认实现的依赖关系一览（实线 = 持有，虚线 = 调用）：

```text
                   ┌─────────────────────────────────────┐
                   │ trait PermissionGate                │
                   │   check / check_bash                │
                   │   effective_roots / _path_rules     │
                   └──────────────┬──────────────────────┘
                                  │ impl
                                  ▼
                   ┌─────────────────────────────────────┐
                   │ DefaultPermissionGate               │
                   │ (gate.rs)                           │
                   └──┬──────┬──────┬──────┬─────────────┘
                      │      │      │      │
              ┌───────┘      │      │      └───────────────┐
              │              │      │                      │
              ▼              ▼      ▼                      ▼
      ┌──────────────┐ ┌────────┐ ┌──────────────┐ ┌─────────────────┐
      │ GateConfig   │ │Session │ │ DraggedPaths │ │ defaults.rs     │
      │  workspace   │ │Grants  │ │  Arc<Mutex<  │ │  BUILTIN_PATH   │
      │  extra_roots │ │ Arc<Mu │ │   Vec<…>>>   │ │  _RULES /       │
      │  user_path   │ │ tex<.. │ └──────────────┘ │  BUILTIN_BASH   │
      │  _rules      │ │ >>>    │                  │  _FORBIDDEN     │
      │  bash_*      │ └────────┘                  └─────────────────┘
      │  agent_data  │      ▲                              ▲
      │  _ro_dirs    │      │ runtime push                 │ const
      └──────────────┘      │ (confirm UI / drag)          │
                            │
                       ┌────┴───────────────────┐
                       │ CliConfirmation /      │ ┄┄ check 调用 ┄┄►  (回到 gate)
                       │ chat_loop drag handler │
                       └────────────────────────┘
```

- `GateConfig` 是不可变快照（来自 `AppConfig` + 启动期算出的 `agent_data_readonly_dirs`），重启重读。
- `SessionGrants` / `DraggedPaths` / `SessionPathRules` 是会话内可变状态，由 `ChatContext` 持有同一份并注入 gate；UI 侧追加授权或新增 deny/readonly 后，gate 与 system prompt 在下次调用即看到。
- `defaults.rs` 提供两组 `&'static [_]` 常量，gate 在 `effective_path_rules()` 时与 user 规则、session runtime 规则合并。

---

## 4. EffectiveRoots：「当前会话允许什么」的统一视图

| 桶 | 内容 |
|----|------|
| `read_write` | `workspace_dir` ∪ `extra_roots` ∪ `session_grants(write=true)` ∪ `dragged_paths(write=true)` |
| `read_only` | `path_rules.mode=Readonly` ∪ `agent_data_readonly_dirs` ∪ `dragged_paths(write=false)` ∪ `session_grants(write=false)` |

来源「漏斗」式合并示意（左侧是源，右侧是被合并到的桶；同一条路径若同时进了两侧，read_only 会被 dedup 掉）：

```text
              ┌─────────────────────────┐
   workspace_dir ─────►──┐                                ┐
                         │                                │
   extra_roots ─────►────┤                                │
                         │ ── union ──►  read_write    ───┤
   session_grants(w=t) ──┤                                │
                         │                                │  EffectiveRoots
   dragged_paths(w=t) ───┘                                │  (一次性快照)
                                                          │
   path_rules(Readonly) ─►─┐                              │
                           │                              │
   agent_data_ro_dirs ─────┤                              │
                           │ ── union ── dedup ──►        │
   session_grants(w=f) ────┤    read_only                 │
                           │                              │
   dragged_paths(w=f) ─────┘                              ┘
              └─────────────────────────┘
```

`effective_roots()` 不缓存：每次调用都重新合并；规模小（< 100 路径）+ 调用频次低（system prompt 重建 + UI 启动横幅 + 拖拽决策辅助）→ 不构成瓶颈。

---

## 5. PathRule schema 与 builtin 默认值

```rust
// src/core/permission/path_rule.rs
pub struct PathRule { pub path: String, pub mode: PathRuleMode }
pub enum PathRuleMode { Deny, Readonly }
```

匹配规则（`PathRule::matches(target)`）：

- 含 glob 字符（`*` / `?` / `[`）→ 用 `globset` 严格匹配整条路径，`dir/*` 只匹目录里第一层；`dir/**` 才递归。**directory-like 规则需要同时显式覆盖目录本身**（`*/sessions`）**和其内容**（`*/sessions/**`），否则 `sessions/foo.jsonl` 不会命中。
- 不含 glob 字符 → 先 `normalize_path` 展开 `~` / canonicalize，然后用「等于 / 是前缀且后面是路径分隔符」语义。

builtin 列表（`defaults.rs::BUILTIN_DEFAULT_PATH_RULES`）：

- **凭据 deny**：`~/.ssh` · `~/.aws` · `~/.gnupg` · `~/.config/gh` · `~/.pi_/pi.config.toml` · `~/.pi_/credentials` · `~/.pi_/agents/*/agent/auth-profiles*.json` · `~/.pi_/agents/*/agent/credentials` + `~/.pi_/agents/*/agent/credentials/**`
- **agent 自审计 readonly**：`~/.pi_/agents/*/sessions` + `/sessions/**` · `~/.pi_/agents/*/logs` + `/logs/**` · `~/.pi_/agents/*/audit` + `/audit/**`

builtin 与 user `path_rules` 取**并集**且 user 不能弱化 builtin。

---

## 6. ConfirmDecision：用户授权三选项

```rust
// src/core/confirmation.rs
pub enum ConfirmDecision {
    AllowOnce,             // [a] 仅本次
    AllowAndPersistRoot,   // [w] 写入 extra_roots / path_rules
    Deny,                  // [d] 直接拒
}
```

`UserConfirmationProvider::confirm(req) -> Result<ConfirmDecision, AppError>`，CLI 实现 `CliConfirmation` 在 `src/api/chat/confirmation.rs`，TUI 渲染 prompt + 5 选项菜单见 `src/api/chat/dragged_path.rs::MenuOptions`。

`auto_confirm = true` 时短路：直接返回 `AllowOnce`，但仍写审计；legacy `path_whitelist` / `bash_whitelist` / `auto_confirm_whitelist` 已删除，路径允许根只走 `workspace.extra_roots`，bash 只走 `bash_forbidden` / `bash_approval_required`。

一次 `NeedConfirm` 的完整状态流：

```text
   gate.check → NeedConfirm{prompt}
        │
        ▼
   UserConfirmationProvider::confirm(req)
        │
        ├─► auto_confirm? ── yes ──► AllowOnce  (仍写审计)
        │                                │
        │   no                           │
        ▼                                │
   ┌──────────────────────────────┐      │
   │  CLI prompt:                 │      │
   │   [a] allow once             │      │
   │   [w] allow & persist root   │      │
   │   [d] deny                   │      │
   └──────┬───────┬───────┬───────┘      │
          │       │       │              │
       [a]│    [w]│    [d]│              │
          ▼       ▼       ▼              ▼
     AllowOnce  AllowAnd  Deny      AllowOnce
                Persist    │
          │     Root       │
          │       │        │
          │       ├────────┴────────────────────────────►  audit (grant_source)
          │       │
          │       └─► append_extra_root_to_disk(path)  (写盘 + filelock)
          │           或  append_path_rule_to_disk(rule)
          │
          ▼
     SessionGrants.push(path, write=…)   (会话内有效，重启失效)
          │
          ▼
   原语继续执行 / Deny 直接报 AppError::Permission
```

只有 `[w]` 一条会真正落盘；`[a]` 走 `SessionGrants`，下个进程不复存在；`[d]` 不写任何状态、本次就拒，不会累积成 path_rules。

---

## 7. Bash 路径分级

`src/core/permission/bash_parser.rs::parse_bash_paths(cmd) -> Vec<PathArg>`：

- 用 `shell-words` 切 token，跳过 flag 与 stdin/stdout 重定向操作符。
- 已知子命令 / 已知 flag 列表（`cd` / `ls` / `cat` / `cp` / `mv` / `rm` / `find` / `grep` / `git` / …）按 token 位置抽取目标路径。
- 命令本身的 `argv[0]`（`rm` / `git` / …）不当作路径返回；纯 stdin pipeline（如 `echo abc | sort`）返回空。

`DefaultPermissionGate::check_bash(cmd)`：

1. **Layer 0**：`bash_forbidden` regex（builtin + user）命中 → Deny。
2. **Layer 1**：`bash_approval_required` regex 命中 → NeedConfirm。
3. **Layer 2**：`parse_bash_paths` 抽出的每条 path 各跑一次 `check(Write, path)`（命令默认按 write 权限），任一 Deny → Deny；任一 NeedConfirm → NeedConfirm；全部 Allow → Allow{BashWhitelist or 对应 source}。

builtin `bash_forbidden` 默认禁掉 `rm -rf /` / `rm -rf ~` / `chmod 777 /` / `chown -R root` / `dd if=/dev/zero of=/dev/sd*` / curl-pipe-bash 等 6 类高危命令；详见 `defaults.rs::BUILTIN_DEFAULT_BASH_FORBIDDEN`。

四阶段流水线，前一阶段命中即返回，**不再进下一阶段**：

```text
   raw bash command
         │
         ▼
   ┌──────────────────────────┐
   │ Layer 0: forbidden regex │   builtin ∪ user
   └──────────────┬───────────┘
            hit ──┴──► Deny{"forbidden: <regex>"}
            miss
              │
              ▼
   ┌──────────────────────────┐
   │ Layer 1: approval regex  │   builtin ∪ user
   └──────────────┬───────────┘
            hit ──┴──► NeedConfirm{"approval required"}
            miss
              │
              ▼
   ┌──────────────────────────┐
   │ parse_bash_paths(cmd)    │   shell-words → token → known
   │   → Vec<PathArg>         │   subcmd / flag map → PathArg
   └──────────────┬───────────┘
                  │
                  │  for each path:
                  ▼
   ┌──────────────────────────┐        any Deny  ─► Deny{first}
   │ gate.check(Write, path)  │  ────► any NeedC ─► NeedConfirm{first}
   │   for each PathArg       │        all Allow ─► (continue)
   └──────────────┬───────────┘
                  │ 全部 Allow
                  ▼
   ┌──────────────────────────┐
   │ Layer 3: bash policy ok? │   no forbidden / approval hit
   └──────────────┬───────────┘
           hit ──┴──► Allow{BashPolicy}
            miss ─────► Allow{<paths' first source>}
                         (空 path 列表时回退到 NeedConfirm)
```

举例：

```text
   "rm -rf ~/projects/foo"
   ├─ forbidden  : 不命中（`rm -rf /` / `rm -rf ~` 与本路径不同）
   ├─ approval   : 不命中
   ├─ parse paths: ["~/projects/foo"]
   └─ gate.check(Write, "~/projects/foo")
      → 若在 workspace_dir 内 → Allow{AgentWorkspace}
      → 否则                  → NeedConfirm{prompt}

   "rm -rf /"
   └─ Layer 0 直接 Deny，不进 parse
```

---

## 8. 拖拽 UX：仅纯路径进入授权菜单

`src/api/chat/dragged_path.rs::interpret_dragged_paths(line) -> DragOutcome`：

- **纯路径**：整行所有 token 都是 `/...` 或 `~/...` 路径 token，且每个 token 要么整体存在，要么是纯 ASCII 路径形，返回 `PromptMenu`。
- **普通输入**：任何 token 是文字、flag、或「路径 + 意图」混合形态，返回 `None`，原样进入普通对话，不新增 `DraggedPaths` / `SessionGrants`。

纯路径菜单选项：

- `[a]` SessionGrant（仅本会话）
- `[w]` 追加 `workspace.extra_roots`
- `[r]` 追加 readonly `path_rules`
- `[d]` 追加 deny `path_rules`
- `[c]` 取消，不发送给 LLM

如果 `gate.check(Read, path)` 提前发现 deny / readonly → 缩减菜单（`MenuOptions::deny_only` / `readonly_only`），避免给出无效选项。纯路径进入菜单后，命中 deny 或用户 cancel 时只写入 `[drag-cancel]` 合成 user note，并回到 `u>`；原始拖拽行不得发送给 LLM。

回归测试矩阵（`src/api/chat/dragged_path.rs::tests`）：

| case | 输入示例 | 预期 |
|------|----------|------|
| 纯路径 | `/tmp/project` | `PromptMenu` |
| 文字 + 路径 | `帮我看 /tmp/project` | `None` |
| 引号路径 + 中文意图 | `'/tmp/project'看下里面` | `None` |
| 全 ASCII 不存在路径 | `/etc/foo/nonexistent` | `PromptMenu` |
| 非 ASCII 不存在 token | `/abs/path中文` | `None` |

整体决策树（行内意图 vs 纯路径 → 探针 → 菜单 → 状态变更）：

```text
   user types a line containing path(s)
              │
              ▼
   interpret_dragged_paths(line)
              │
       ┌──────┴──────┐
       │             │
       ▼             ▼
  "看一下 /tmp/foo"  "/tmp/foo"
   行内含意图         纯路径
       │                │
       ▼                ▼
   gate.check(Read, path)  gate.check(Read, path)  (探针)
        │                         │
   deny │ allow/need              ├── deny ─────► deny_only [c]
        │                         ├── readonly ─► readonly_only [a][r][d][c]
        ▼                         └── other ────► full [a][r][w][d][c]
   不写 DraggedPaths                         │
        │                                     ▼
        └── allow/need ─► DraggedPaths.add  [a]/[w] 执行前二次 gate check
                                              [r]/[d] 写盘后同步 SessionPathRules
```

探针那一步会查完整有效规则（builtin ∪ user ∪ session runtime）来提前禁掉无效菜单项。拖入 deny 路径时只允许取消，不允许本会话授权或写入 `extra_roots`；readonly 路径可以确认本次读取，但写/改仍由 gate 拒绝。

---

## 9. config_get / config_set LLM 工具

```text
config_get(key)       → 字符串/JSON 值（受 CONFIG_READ_ALLOWLIST 约束）
config_set(key, value) → 二次 confirm + diff + 落盘
```

约束三件套（`src/api/chat/config_tool.rs`）：

| 名称 | 作用 |
|------|------|
| `CONFIG_READ_ALLOWLIST` | 精确匹配；只有命中的 key 才允许读 |
| `CONFIG_HARDCODED_READ_DENY` | 通配前缀；`llm.api_key*` / `llm.proxy` / `security.*` / `storage.*` 永不可读 |
| `CONFIG_WRITE_ALLOWLIST` | 精确匹配；不在表里的一律拒写 |
| `CONFIG_HARDCODED_WRITE_DENY` | 通配前缀；`llm.*` / `security.*` / `storage.*` / `agent.*` / `primitive.auto_confirm` 以及已删除的 legacy whitelist 字段永不可写 |
| `ARRAY_FIELDS` | 数组类 key（`workspace.extra_roots` / `entries` / `primitive.path_rules` / `bash_*` 等）只允许「单元素追加」语义；删除 / 整数组替换返回错误并引导 `pi config edit` |

每次 `config_set` 都强制走 `UserConfirmationProvider::confirm`，prompt 含 unified diff；用户拒绝 → `applied=false`，配置不动。

CLI（`pi config get/set/edit`）是用户特权通道，**不**受这两个 ALLOWLIST 约束，但底层共用 `append_*_to_disk` + `with_config_lock` 文件锁。

两条通道 + 共享落盘层：

```text
       LLM 工具                         用户 CLI
   config_get / config_set         pi config get/set/edit
            │                              │
            ▼                              ▼
   ┌────────────────────┐          ┌────────────────────┐
   │ ALLOWLIST 三件套   │          │  无 ALLOWLIST 约束 │
   │  + ARRAY_FIELDS    │          │  (用户特权通道)    │
   │  单元素追加语义    │          │                    │
   └─────────┬──────────┘          └─────────┬──────────┘
             │                                │
             ▼ key 受控                       │
   ┌────────────────────┐                     │
   │  二次 confirm      │                     │
   │  (含 unified diff) │                     │
   └─────────┬──────────┘                     │
             │ user [a]/[w]                   │
             ▼                                ▼
       ┌──────────────────────────────────────────────┐
       │  共享落盘层                                  │
       │   with_config_lock(cfg_path, |_| {           │
       │     append_extra_root_to_disk(...)           │
       │     append_path_rule_to_disk(...)            │
       │     append_workspace_entry_to_disk(...)      │
       │   })                                         │
       └─────────────────┬────────────────────────────┘
                         ▼
              pi.config.toml  (atomic rename)
```

LLM 通道之所以多了「二次 confirm + 单元素追加 + ALLOWLIST」三件套，是因为它**永远代理 LLM 的意图**——任何 prompt 注入只能走到 `config_set`，不能直接落到 `append_*_to_disk`。CLI 通道则假定用户对自己的命令负责。

---

## 10. WorkspaceStateSection：让 LLM 知道权限边界

`src/core/system_prompt.rs::WorkspaceStateSection`（priority 150，紧跟「角色」「目标」之后）：

- 输入：`WorkspaceState { cwd, read_write, read_only, path_rules, agent_data_dir }`，由 `api/chat::compute_workspace_state(ctx)` 直接读 `ctx.cwd`（启动 snapshot）+ `ctx.gate.effective_roots()` / `ctx.gate.effective_path_rules()` 构造。
- 输出顺序固定为「Current Working Directory 段 → Workspace State 段」（`cwd` 段必须先于 `Workspace State`，详见 §10.0）。
- Workspace State 段输出形如：
  ```
  Workspace permissions
  - Read+Write roots:
    * /repo (agent_workspace, alias=repo)
    * /tmp/foo (extra_root)
  - Read-only:
    * ~/.pi_/agents/main/audit (agent_data_dir)
    * ~/.pi_/agents/main/sessions (path_rule readonly)
  - Path rules:
    * deny  ~/.ssh                    [builtin]
    * deny  ~/.aws                    [builtin]
    * readonly ~/.pi_/agents/*/sessions/**  [builtin]
  - To change permissions: pi pathrules add <path> --mode deny|readonly
                            or call config_set("workspace.extra_roots", "/new/path")
  ```

> **变更（hotfix `feature/workspace-permission-tiers-hotfix`）**：
> 原 `chat_loop::print_startup_banner` 已被移除。同一份 `WorkspaceState`
> 不再在启动时强制打印 stderr 横幅（用户可用 `pi pathrules list` /
> `pi config get` 主动查询）；权限边界改由 system prompt 注入与
> §10.1「Lazy First-Touch」交互流双轨表达。

### 10.0 cwd 注入到 system prompt

#### 设计动机

进入 `pi chat` 后用户三种典型输入：

```
情况 1：「随机读一个文件」（无路径）
情况 2：「读 src/main.rs」（cwd 内相对路径）
情况 3：「读 /etc/hosts」（cwd 外绝对路径）
```

LLM 缺少 cwd 上下文时，情况 1/2 会路径 hallucinate 或直接拒答。注入 cwd 后三种情况自然分流：

```text
用户输入 → LLM (system prompt 已含 cwd) → tool call + 路径
                    │
   ┌────────────────┼─────────────────────────────────────┐
   情况 1                情况 2                  情况 3
   推断 <cwd> 下读        拼 <cwd>/src/main.rs    拼 /etc/hosts
   target_in_cwd=true    target_in_cwd=true     target_in_cwd=false
   走 §10.1 lazy prompt  走 §10.1 lazy prompt   走 §6 标准 per-file confirm
```

#### 启动 snapshot（避免 set_current_dir footgun）

`ChatContext` 在 `from_config` 阶段一次性记录两个字段：

```rust
pub struct ChatContext {
    // ... 现有字段
    pub cwd: PathBuf,       // std::env::current_dir() 启动 snapshot
    pub cfg_path: PathBuf,  // crate::api::cli::config_file_path() 启动 snapshot
}
```

为什么不动态读：避免「未来某段代码 `std::env::set_current_dir(...)` 后权限边界静默漂移」的 footgun；启动锚定 = 进程内单一真相。

#### system prompt 渲染顺序

`WorkspaceStateSection::render()` 在原 `## Workspace State` 段**之前**插入：

```text
## Current Working Directory

`/Users/yan/proj/new-app`

This directory is currently writable for you (see Workspace State below).
```

文案随 `cwd` 是否在 `effective_roots` 中三态切换：

| cwd 是否在 effective_roots | 渲染文案 |
|----------------------------|----------|
| read_write 命中 | `This directory is currently writable for you` |
| read_only 命中 | `This directory is currently read-only for you` |
| 都未命中 | `This directory is NOT yet authorized. ... 第一次 tool call 会触发 §10.1 lazy prompt` |

这段文字暗示 LLM：把「这个文件 / 当前目录 / 随机一个文件」类指代解析到 cwd，必要时让运行时来弹范围授权，而不是去试错被拒的路径。

测试：`src/core/tests/system_prompt.rs` 覆盖渲染顺序与三态文案。

### 10.1 Lazy First-Touch 范围级授权

> 取代原方案「启动时检测 cwd 是否未授权 → 立刻弹 banner / prompt」。新方案
> 只在 LLM 真的去碰 cwd 内文件时才弹一次范围级授权，并把
> 「[a] / [s] / [n]」三选项落到 `extra_roots` / `SessionGrants` / fall-through。

#### 触发链路

```text
   ┌─────────────────────────────────────────────────────────────┐
   │  用户输入 → LLM 推理 (system prompt 含 cwd, §10.0) → tool   │
   │           call (read_file / grep / bash) ── 路径已被 LLM 解析 │
   └─────────────────────────────────────────────────────────────┘
                               │
                               ▼
                   PermissionGate.check(target)
                               │
                  ┌────────────┼────────────┐
                  │            │            │
                Allow       NeedConfirm    Deny
                  │            │            │
                  │            ▼            │
                  │   CwdLazyPrompt.confirm_decision
                  │            │            │
                  │  ┌─────────┴────────┐   │
                  │  │ dismissed? Bash? │   │
                  │  │ 路径不可解析?     │   │
                  │  └────┬─────────────┘   │
                  │       │                 │
                  │   yes(直接转发 inner)   │
                  │       │                 │
                  │       ▼                 │
                  │   inner.confirm_decision│
                  │   ([a]/[w]/[d] 逐文件)  │
                  │                         │
                  │  no                     │
                  │       │                 │
                  │       ▼                 │
                  │  ┌──────────────────┐   │
                  │  │ 目标在 cwd 子树? │   │
                  │  └─────────┬────────┘   │
                  │     no │   │ yes        │
                  │        │   │            │
                  │        │   ▼            │
                  │        │ ┌────────────┐ │
                  │        │ │ cwd 已授权?│ │
                  │        │ └─────┬──────┘ │
                  │        │  yes │ │ no    │
                  │        │      │ │       │
                  │        │      │ ▼       │
                  │        │      │┌──────────────┐
                  │        │      ││ stdin 是 TTY?│
                  │        │      │└──────┬───────┘
                  │        │      │  no │ │ yes
                  │        │      │     │ │
                  │        │      │     │ ▼
                  │        │      │     │ 弹 [a]/[s]/[n]
                  │        │      │     │       │
                  │        │      │     │   ┌───┴────┬─────┐
                  │        │      │     │  [a]      [s]   [n]
                  │        │      │     │   │        │     │
                  │        │      │     │   │        │     ▼
                  │        │      │     │   │        │  dismissed=true
                  │        │      │     │   │        │     │
                  │        │      │     │   ▼        ▼     ▼
                  │        ▼      ▼     ▼  写 toml  仅 SG  fall-through
                  │   inner.confirm_decision
                  │        │
                  └────────┴────────────────────────────────┐
                                                            ▼
                                                Tool 实际执行 / Deny 报错
```

#### 三分支副作用

| 选项 | 写盘 | SessionGrants | dismissed | 返回值 |
|------|------|----------------|-----------|--------|
| `[a]` AddPersistent | `pi.config.toml` `extra_roots` 追加 cwd canonical 路径（with file lock） | `add(cwd)` | `false` | `AllowOnce` |
| `[s]` AllowSessionOnly | 不写 | `add(cwd)` | `false` | `AllowOnce` |
| `[n]` Skip | 不写 | 不写 | `true` | inner.confirm_decision（per-file 3 选项） |

要点：

- **`AllowOnce` 而非 `AllowAndPersistRoot`**：装饰器自身已经把 cwd 写进 toml + SessionGrants。返回 `AllowOnce` 是因为本次操作直接放行；下一次同 cwd 子树访问被 `PermissionGate.check` 通过 SessionGrants 命中 → 返回 `Allow`，根本不再进 confirm 层。
- **`dismissed` 是 `Arc<AtomicBool>`**：与 `SessionGrants` 同生命周期挂在 `ChatContext` 内。一次 `[n]` 整个会话内不再就 cwd 范围弹此提示，退化为原 `CliConfirmation` 逐文件 3 选项 UX。
- **非 TTY / piped stdin**：`stdin().is_terminal() == false` 时设置 `dismissed=true` 并 fall-through，避免 CI 阻塞读取 stdin。
- **Bash op 跳过**：`PrimitiveOperation::Bash` 不走范围级提示（命令通常包含多个路径，由 §7 流水线逐条决策更清晰）。
- **`cwd_already_authorized` 短路**：装饰器在进入交互前检查 `cwd ∈ effective_roots`，已授权则完全不介入，避免重复弹窗。

#### 实现位置

| 文件 | 角色 |
|------|------|
| `src/api/chat/cwd_lazy_prompt.rs` | `CwdLazyPrompt` 装饰器 + `CwdPromptChoice` + 单测 |
| `src/api/chat/mod.rs::ChatContext::from_config` | 用 `CwdLazyPrompt::new(...)` 包裹底层 `CliConfirmation` |
| `src/infra/config::append_extra_root_to_disk` | `[a]` 落盘的统一入口（与 §6 / §9 共享 file lock） |
| `tests/cwd_lazy_prompt_e2e.rs` | 跨模块装配 + 非 TTY fallback + apply_choice 三分支验收 |

#### 人 / Agent 两路输出共用同一份 `WorkspaceState`

```text
                ChatContext.gate (Arc<dyn PermissionGate>)
                ChatContext.cwd  (PathBuf snapshot)
                                │
                ┌───────────────┴──────────────────┐
                │ effective_roots()                │
                │ effective_path_rules()           │
                └────────────────┬─────────────────┘
                                 │
                                 ▼
                  compute_workspace_state(ctx)
                  (api/chat/mod.rs)
                                 │
                  WorkspaceState {
                    cwd:        String,
                    read_write: Vec<…>,
                    read_only:  Vec<…>,
                    path_rules: Vec<…>,
                    agent_data_dir: Option<…>,
                  }
                                 │
                                 ▼
                  build_system_prompt_with_state
                  (system_prompt.rs)
                                 │
                                 ▼
                  WorkspaceStateSection
                  (priority = 150)
                  ## Current Working Directory  ← §10.0
                  ## Workspace State            ← 原内容
                                 │
                                 ▼
                  SystemPromptBuilder
                  (按 priority 拼接所有 section)
                                 │
                                 ▼
                  最终 system prompt → LLM
```

`gate` 与 `agent_workspace_dir` 是同一个 `Arc<dyn PermissionGate>` / `PathBuf`：原语执行、bash 检查、cwd lazy prompt 范围比对、system prompt 渲染、`compute_workspace_state` 全都对着这两个对象。会话期间 `[a]` / `[s]` / 拖拽菜单改动 `SessionGrants` / `DraggedPaths`，下一次 system prompt 重建会立即看到。

---

## 11. 审计字段扩展

`src/infra/audit/mod.rs::PrimitiveAuditEntry` 新增：

| 字段 | 类型 | 含义 |
|------|------|------|
| `permission_level` | `Option<String>` | `"read" / "write" / "bash"` |
| `grant_source` | `Option<String>` | `GrantSource` 序列化值，例如 `"agent_workspace"` / `"session_grant"` / `"path_rule_readonly"` |
| `in_working_dir` | `Option<bool>` | 该路径是否在工作目录内（仅原语 op 有意义；bash 默认 None） |

三字段全部 `#[serde(default, skip_serializing_if = "Option::is_none")]`，老行 deserialize 后字段为 `None`，**不需要迁移脚本**。详细约束见 `agents/TASK_BOARD_002.md` T2-P0-004 验收标准与本文 §1.2。

---

## 12. 演进与后续工作

- **PR-Doc 后置**：本文档为本任务 `post_arch_doc` todo 的产出。
- **`pi pathrules remove` / `clear-session`**：T2-P0-005 工具系统整改窗口接入；目前先用 `pi config edit` 兜底。
- **持久化 `dragged_paths`**：当前重启失效；如果用户多次拖拽相同路径仍然被反复弹 prompt，再考虑落盘。
- **`auto_confirm` 收敛**：长期目标是把 `auto_confirm` 完全替换为更细粒度的 `path_rules` 与 bash forbidden/approval 规则。
- **多 agent**：当出现多 agent 实例时，每个 agent 各自的 `agents/{id}` 仍走 `agent_data_readonly_dirs` 兜底，但 cross-agent 写入应被默认 deny；规则升级在 [multi-agent.md](multi-agent.md) 里同步。

---

**交叉引用**

- [Architecture.md](../Architecture.md) §7 安全设计核心原则
- [security.md](security.md) §7.4 用户知情权 / 最小权限
- [audit-log.md](audit-log.md) §4 数据结构（`PrimitiveAuditEntry` 字段）
- [interrupt-and-cancellation.md](interrupt-and-cancellation.md) §3 取消语义对授权交互流的影响
- [work-dir-and-data-layout.md](work-dir-and-data-layout.md) §2 `~/.pi_/agents/{id}` 子目录布局（与本系统 `agent_data_readonly_dirs` 同源）
