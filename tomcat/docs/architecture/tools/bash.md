# `execute_bash` / `bash` 工具：权限、审计、超时与输出治理

本文档是内置 **shell 执行工具**的技术方案（OpenSpec **B 类**：`docs/architecture/tools/`），承接总计划 [`strengthen-four-core-tools_b51c9eae.plan.md`](../../../../../.cursor/plans/strengthen-four-core-tools_b51c9eae.plan.md)。**落地顺序（本方案）**：**PR-A（命名）最先** → **PR-E（T1）** → **PR-I（T2）** → **PR-L（T3）**；与调研报告 [`agent-tools-comparison.md`](../../reports/agent-tools-comparison.md) 对齐选型结论。

**文首声明（避免与 `read.md`「全篇已闭环」口吻混淆）**：

- **§3–§6、§8–§9 前半**：描述**当前仓库**已落地的行为与代码锚点；与实现不一致处以 **`src/` 代码为准**。
- **§1 观察指标表、§2.3–§2.4、§9 后半、§10 中 PENDING 行**：描述**契约草案与路线图**（与 strengthen 计划一致）；合入后以 PR 更新本文状态列。

写作约定见 [`ARCHITECTURE_SPEC.md`](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)（B 类：术语 → 调研 → 目标 → **§4.1/§4.2** 已定稿选型与实施、One-Glance、测试、风险）。

---

## 目录

- [1. 目标与设计原则](#1-目标与设计原则)
- [2. 竞品 / 选型对比](#2-竞品--选型对比)
  - [2.1 Shell 工具的典型关切](#21-shell-工具的典型关切)
  - [2.2 常见实现横向对比](#22-常见实现横向对比)
  - [2.3 落地选型决策表](#23-落地选型决策表)
  - [2.4 实施点（现状与路线图）](#24-实施点现状与路线图)
  - [2.4.1 PR-A：对外短名 `bash`（优先）](#241-pr-a对外短名-bash优先)
  - [2.4.2 PR-现状：catalog、gate 与单次 spawn](#242-pr-现状cataloggate-与单次-spawn)
  - [2.4.3 PR-E（T1）：超时与输出有界](#243-pr-et1超时与输出有界)
  - [2.4.4 PR-I（T2）：后台与 task 三件套](#244-pr-it2后台与-task-三件套)
  - [2.4.5 PR-L（T3）：AST、Sandbox 与 PersistentShell 骨架](#245-pr-lt3astsandbox-与-persistentshell-骨架)
- [3. 术语统一](#3-术语统一)
- [4. 协议（入参 / 出参 / Schema）](#4-协议入参--出参--schema)
- [5. One-Glance Map（文件职责总览）](#5-one-glance-map文件职责总览)
- [6. 调度时序（运行时图）](#6-调度时序运行时图)
- [7. 状态机（当前与规划）](#7-状态机当前与规划)
- [8. 配置与环境变量](#8-配置与环境变量)
- [9. 错误模型 / 截断 / 超时](#9-错误模型--截断--超时)
- [10. 测试矩阵（验收）](#10-测试矩阵验收)
- [11. 风险与应对](#11-风险与应对)
- [12. 历史决策（已被本方案取代或待定）](#12-历史决策已被本方案取代或待定)
- [13. 关联文档](#13-关联文档)
- [附录：与 strengthen 计划章节对照](#附录与-strengthen-计划章节对照)

---

## 1. 目标与设计原则

**一句话**：让模型跑 shell 时 **可审计、路径可控、输出有界、长任务可收尾**；终态工具名与 **pi-mono** 一致为 **`bash`**，工程深度对齐 **cc-fork-01**（截断/权限 AST）与 **pi_agent_rust**（双通道 IO 思路，见路线图）。

### 1.1 观察指标表（路线图对齐；现状见 §10 状态列）

| 目标 | 观察指标（落地后可核对） | 说人话 |
|------|--------------------------|--------|
| G1 命名终态 | catalog / `tool_exec` / system_prompt 仅匹配 **`bash`**；transcript 中 legacy `execute_bash` **不重定向**——与 [read.md](read.md) PR-RA 一致：`tracing::warn` + **未知工具**类错误（或等价 UX） | 对外就叫 `bash`，老 transcript 里长名**不悄悄执行**，只 warn。 |
| G2 墙钟超时 | `tokio::time::timeout` 包裹执行；超时后子进程被 kill，返回归一化字段（如 `timed_out=true`） | 命令不能挂到天荒地老。 |
| G3 输出有界 | 合并 stdout+stderr 超上限 → 头尾保留 + `persisted_output_path` 落盘 | 别把几 GB 日志灌进上下文。 |
| G4 后台可拉流 | `run_in_background` + `task_output` / `task_stop` / `task_list` 可验收 | 编译一小时也能分段看输出。 |
| G5 纵深安全 | 保留 `gate_check_bash` + `extract_paths`；叠加 AST 分段与配置 allow/deny（T3） | 正则拦不住的地方用 AST 补。 |
| G6 审计完整 | `PrimitiveAuditEntry` 含 `permission_scope` / `grant_type` / `grant_trigger`；后台任务带 `task_id` | 事后能追责、能区分用户点过确认。 |

### 1.2 非目标

| 非目标 | 推给 | 说人话 |
|--------|------|--------|
| 跨 session 复用同一 PTY shell | PR-L 后续 / 003 迭代 | 本期不保证「下轮 chat 还是同一个 bash 会话」。 |
| 完整 macOS Seatbelt / Landlock 实现 | `SandboxBackend` 占位后的宿主 PR | 先留接口，不假装已沙箱到内核级。 |
| 用 bash 替代 `search_files` / `grep` | system_prompt + §13 | 搜索走专用工具，别用 shell 跑 rg。 |

---

## 2. 竞品 / 选型对比

### 2.1 Shell 工具的典型关切

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  本地 bash 类工具通常要同时解决的四类问题                                    │
├────────────────────┬─────────────────────────────────────────────────────┤
│  安全与合规        │  任意命令 = 数据外泄 / 删盘 → gate、AST、沙箱后端      │
│  资源与可预期      │  超时、输出上限、后台任务 → 不占满模型上下文与进程表   │
│  IO 语义           │  stdout/stderr 合并或保序 → 模型可读、可测            │
│  可移植性          │  Unix sh vs Windows cmd；Wasm 宿主注入 env            │
└────────────────────┴─────────────────────────────────────────────────────┘
```

### 2.2 常见实现横向对比

| 来源 / 形态 | 超时与输出 | 后台 / 长运行 | 安全深度 | 备注 |
|-------------|------------|---------------|----------|------|
| **cc-fork-01** | `EndTruncatingAccumulator`、持久化路径、字符上限 | `run_in_background` + Task* | tree-sitter-bash AST + bashPermissions | 工程完整度锚点 |
| **pi_agent_rust** | 默认 120s、可关闭；行/字节上限 + 落盘提示 | 弱 | forbidden 等策略 | 双线程泵 stdout/stderr |
| **pi-mono** | spawn + 超时杀进程树；滚动缓冲 + tmp | 无 | 极简；哲学上审批在宿主 | 契约名 **`bash`** |
| **openclaw** | exec/process 拆分、`yieldMs` | `background` + process 工具 | host / Docker 策略 | 与容器强绑定 |
| **hermes-agent** | 前台上限 + rolling buffer | `terminal` bg + process_registry | 多 environment 后端 | Python 栈 |

**结论（写入路线图）**：**契约与短名**对齐 **pi-mono**；**超时 + 输出截断 + 落盘**对齐 **cc-fork-01**；**有序 IO** 借鉴 **pi_agent_rust**；**多后端沙箱**仅留 trait，不照搬 hermes/openclaw 运行时。

### 2.3 落地选型决策表（维度取舍）

**代码落点、交付物、阶段**见 **[§2.4](#24-实施点现状与路线图)**，与 [`ARCHITECTURE_SPEC.md`](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§4.1 / §4.2** 分工一致。**`决策`** 列钉本行裁决结论（**SHOULD**）。

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| **对外工具名** | legacy transcript 是否还能静默执行 | **采用** 仅 `bash` match；legacy warn + UnknownTool，**拒绝** 运行时重定向。 | pi-mono 契约名 + [`read.md`](read.md) PR-RA | 与 `read`/`write`/`edit` **单名对外**；审计与 prompt 不双轨 | × `execute_bash`→`bash` 运行时重定向 + 双轨审计 | 只注册一个名；老 transcript 别静默改写成还能跑。 |
| **默认墙钟超时** | 长命令如何有界 | **采用** 120 s 默认、600 s 可配上限（PR-E）；**拒绝** 无限等。 | strengthen 计划 + 本仓库现状 | **120 s** 默认、**600 s** 可配上限；兼顾编译与防挂死 | × 无限等默认 | 两分钟不够再加，别默认卡死大构建。 |
| **输出策略** | 大 stdout/stderr 如何不进爆上下文 | **采用** 头尾截断 + `persisted_output_path`（PR-E）。 | cc-fork-01 | 头尾截断 + **`persisted_output_path`**（PR-E） | × 仅截断、模型无处读全文 | 中间砍掉，头尾留下，全文去文件里看。 |
| **后台模型** | 长任务是否与 loop 解耦 | **采用** task 三件套（PR-I）+ event_bus。 | openclaw / strengthen PR-I | **task 三件套**（PR-I）与 event_bus 衔接 | × 仅前台依赖用户 Ctrl+C | 大任务别堵在同一轮 tool 里。 |
| **安全栈** | gate 与 AST 是否二选一 | **采用** 保留 gate + 路径预检，T3 叠加 AST。 | 本仓库 gate + strengthen T3 | **保留** gate + 路径预检；T3 **叠加** AST allowlist | × 弃 gate 纯 AST | 老的别拆，新的叠上去。 |
| **`argv` 模式** | 是否强制 `sh -c` 字符串 | **采用** 不经 shell 的 argv 拼接。 | pi-mono | **保留**不经 shell 的 argv 拼接（已实现） | × 仅 `sh -c` 宽注入面 | 能不用字符串 shell 就不用。 |

### 2.4 实施点（现状与路线图）

**实施顺序（本方案，与 strengthen 总计划协调）**：**① PR-A**（`execute_bash` → **`bash`**，测试与 prompt 扫尾）→ **② PR-E**（T1 超时 + 输出有界）→ **③ PR-I**（T2 后台 + task 三件套）→ **④ PR-L**（T3 AST + Sandbox 骨架）。**先改名**可避免后续 PR 在 **`execute_bash` / `bash`** 双套字面量上反复改断言与文档。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
| --- | --- | --- | --- | --- |
| **PR-A（优先）** | **交付物**：短名 `bash`；schema 含 `args`；legacy **warn**。**落地点**：catalog / `tool_exec` / tests / system_prompt | catalog、`tool_exec`、tests、system_prompt、回放 warn | strengthen 计划 §1（**命名**采纳；**fallback 句**以本文为准）+ `catalog_test` | **先改名**，后面 T1/T2/T3 只盯一个工具名。 |
| **PR-E（T1）** | **交付物**：墙钟超时；`timeout_ms`；输出累积 + `persisted_output_path`。**落地点**：`bash.rs`、`output_accum`、config | `bash.rs`；新 `output_accum.rs`；config | `bash_wallclock_timeout_kills_process` 等（**PENDING**） | 超时真杀进程，输出别撑爆内存。 |
| **PR-I（T2）** | **交付物**：`run_in_background`；task 三件套 API。**落地点**：`bash_task.rs`、`tool_exec`、event_bus | 新 `bash_task.rs`；`tool_exec`；event_bus | 计划 §6 E2E 行（**PENDING**） | 后台跑、分段取日志。 |
| **PR-L（T3）** | **交付物**：AST allowlist；`SandboxBackend` trait；PersistentShell 骨架。**落地点**：新模块 + config | 新模块 + config allow/deny | 计划 §4.3（**PENDING**） | AST 与沙箱接口先立住，再填实现。 |
| **PR-现状（基线）** | **交付物**：描述当前 `execute_bash` 行为链。**落地点**：catalog `execute_bash_*`；primitive `bash`；gate | [`catalog.rs`](../../../src/core/tools/contract/catalog.rs) `execute_bash_parameters`；[`bash.rs`](../../../src/core/tools/primitive/executor/bash.rs)；[`tool_exec.rs`](../../../src/core/agent_loop/tool_exec.rs)；[`gate.rs`](../../../src/core/tools/primitive/executor/gate.rs) | `suite_test::execute_bash_success`、`execute_bash_forbidden`；`gate_suite_test::*`；`tests/primitives_tools_tests.rs`；`tests/bash_assignment_deny.rs`；`dispatch_with_extension_test::*` | 描述**今天**代码；PR-A 合入后本行由 **`bash`** 替代，行为链不变。 |

集成测试登记见 **§10**；门禁脚本若扩展 bash 专组，在 PR 合入后于此处补一行路径。

下文按 **实施顺序** 展开技术要点（**2.4.1 = PR-A** 优先，**2.4.2 = PR-现状** 基线，再 **PR-E / I / L**）；**交付边界与代码落点仍以表为准**。写法对齐 [`ARCHITECTURE_SPEC.md`](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§4.2** 硬约束 1（表后拆小节 + ASCII）。

#### 2.4.1 PR-A：对外短名 `bash`（优先）

- **交付**：`catalog` / `tool_exec` `match` / `system_prompt` / 全仓测试断言统一为 **`bash`**；与 [read.md](read.md) **PR-RA** 同口径：**不重定向** legacy 工具名；旧 transcript / 拼写 `execute_bash` → **`tracing::warn`** + **未知工具**（或等价错误路径），避免双轨审计。（strengthen 总计划 §1 曾写「单迭代 fallback」——**以本文 + read 为准**：不采纳运行时重定向。）
- **验收**：`catalog_test` 查找名 **`bash`**；`system_prompt` 字面量以 **`bash`** 为主；存在针对 legacy 名的单测（与 `read_file`→`read` 同形）。
- **与后续 PR 的衔接**：PR-E / PR-I / PR-L 新增字段、测试名与文档样例一律以 **`bash`** 为工具名，避免合并顺序导致的二次全局替换。

```text
  transcript / 旧客户端
        │
        ▼
┌───────────────────┐     注册名仅 "bash"
│  catalog.rs       │──────────────────────────────┐
└───────────────────┘                              │
        │                                            ▼
        ▼                               ┌────────────────────┐
  tool_exec match "bash"                 │ "execute_bash"     │
        │                                │ → warn + UnknownTool│  （PR-A 落地后）
        ▼                                │   （不重定向执行）   │
   正常 bash 路径                         └────────────────────┘
```

**说人话**：**先把对外名字改成 `bash`**；老 transcript 里的长名**不偷偷还能跑**，只打 warn、当未知工具，和 **read** 一套规矩。

#### 2.4.2 PR-现状：catalog、gate 与单次 spawn

- **交付（PR-A 前）**：`BUILTIN_TOOL_CATALOG` 注册名 **`execute_bash`**；`execute_bash_parameters()` 仅 **`command` + `cwd`**；`tool_exec` 额外解析 **`args`** → `PrimitiveExecutor::execute_bash(..., argv)`。
- **执行链**：`cwd`（缺省为 `.`）经 **`gate_check_path(Read, …)`** 得 `cwd_path`；拼 **`audit_cmd`**（argv 时为 `command` + 空格拼接各参数）；**`gate_check_bash`** → **`extract_paths`** 循环 **`gate_check_path(Bash, …)`**；最后 **`Command::output().await`**（**无**外层 `tokio::time::timeout`）；**`BASH_TIMEOUT_SECS`** 仅 **`#[allow(dead_code)]`**，未接线。
- **审计**：成功/失败路径 **`record_primitive`**；成功时带 `exit_code`、stdout/stderr 长度、`permission_scope` / `grant_type` / `grant_trigger`。
- **Wire**：`tool_exec` 将 stdout / `STDERR:` 前缀的 stderr / `(exit code: N)` 拼成**单条** tool 返回字符串。

```text
  LLM tool_call("execute_bash", { command, cwd?, args? })   ← PR-A 后为 "bash"
        │
        ▼
┌───────────────────┐
│  tool_exec.rs     │  parse args → argv_ref
└─────────┬─────────┘
          │
          ▼
┌───────────────────┐     cwd: Some → gate_check_path(Read)
│  bash.rs          │──── cwd: None → PathBuf::from(".")
│  execute_bash_impl│
└─────────┬─────────┘
          │ audit_cmd
          ▼
┌───────────────────┐     Deny / NeedConfirm→confirm
│  gate_check_bash  │──── Allow → (bash_scope, bash_grant)
└─────────┬─────────┘
          │
          ▼
   for raw in extract_paths(audit_cmd)
          │
          ▼
┌───────────────────┐
│ gate_check_path   │  Bash op，任一失败 → Err + 审计失败条
│ (Bash, raw)       │
└─────────┬─────────┘
          │
          ▼
   argv? ──yes──▶ Command::new(command).args(args).current_dir(cwd_path).output()
          │
          no ──▶ sh -c "[ -f env ] && . env; {command}"   (Unix；Windows cmd /C)
          │
          ▼
   BashResult { stdout, stderr, exit_code }  →  tool_exec  stringify → LLM
```

**说人话**：先进 **tool_exec** 拆参数，再进 **bash.rs** 一圈 gate，最后才 **起子进程**；**搜路径**那步是「尽力抠命令里的路径」，抠漏了还有 forbidden 正则兜底。

#### 2.4.3 PR-E（T1）：超时与输出有界

- **交付**：**MUST** 从 **`Command::output()`** 改为 **`spawn` + `wait`（或异步 `wait`）**——否则超时分支拿不到 **`Child`** 句柄 **`kill`**（`wait_with_output` 会消费 `Child`）。用 **`tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait())`**（或等价）包裹等待；**Elapsed** 分支对 **`Child` 调用 `kill`（如 `tokio::process::Child::kill`）** 再 **`wait` 收口**；stdout/stderr 由 **并行 reader 任务**或管道读端拼入累积器（实现细节以 PR 为准）。catalog / config **`timeout_ms`**（默认 **120_000**，上限 **600_000**）；**`EndTruncatingAccumulator`** 风格合并流，超字符上限写 **`persisted_output_path`**，返回体带 **`truncated` / `timed_out`** 等字段（与 strengthen §2.4 一致，字段名以 PR 为准）。
- **对照**：同文件 [`gate.rs`](../../../src/core/tools/primitive/executor/gate.rs) 中 **`run_search_command`** 已对子进程使用 **`timeout`** — bash 路径与之对齐（search 仍可用 `output()` 因其短；bash PR-E 需 **spawn** 才能杀）。
- **可选**：将 **`args`** 写入正式 JSON Schema（**优先在 PR-A 完成**，见 §2.4 表注），消除 §4「实现有、schema 无」双轨。

- **说人话（把上条里的术语拆开）**：
  - **`Child`**：就是 **`spawn` 之后操作系统里那条真实子进程**在 Rust 里的**句柄**（`tokio::process::Child`）。**只有手里还握着这个句柄**，超时分支才能调用 **`kill`** 把进程掐掉；若走 `wait_with_output` 一类 API 把句柄「吃掉了」，后面想杀也杀不了。
  - **并行 reader**：stdout、stderr **各有一条（或轮换的）异步读任务**从管道里**边读边喂**下面的累积器；避免单线程死等一端、另一端缓冲区被写满导致**子进程卡死**（背压问题）。
  - **累积器**：内存里**持续拼接子进程输出**、并**累计字符数**的那层逻辑；到上限就**停止再往一个巨大 `String` 里塞**，改走截断 / 落盘策略，避免 **OOM** 和单次 tool 消息把上下文撑爆。
  - **`EndTruncatingAccumulator` 风格**：沿 strengthen / cc-fork 叙事里的叫法，指**「有头有尾」截断**——超长时返回正文里**只保留开头一段 + 结尾一段**（中间用省略提示），**不是**把整段 stdout/stderr 原样塞进模型。
  - **超字符上限写 `persisted_output_path`**：当合并后的输出**超过配置里的字符上限**（规划键 **`[tools.bash].max_output_chars`**，见 §8「规划」表，**默认/上限度数以 PR-E 钉死为准，当前文档仍为 PENDING**）时，把**完整原文**写到磁盘（`~/.tomcat/…/tool-results/…`），并在返回 JSON 里给出 **`persisted_output_path`**，模型可按路径自己去读尾部。**另一套独立限制是墙钟 `timeout_ms`**：默认 **120_000 ms**、配置上限 **600_000 ms**——管「跑多久」，不管「输出多长」；两条线同时生效。

```text
  Command::spawn()  →  Child
        │
        ▼
┌─────────────────────────────┐
│ tokio::time::timeout(T,     │
│   child.wait())             │   ← 不可写成 timeout(..., wait_with_output())：
└───────────┬─────────────────┘     wait_with_output 消费 Child，超时无法再 kill
            │
     ┌──────┴──────┐
     ▼             ▼
   Ok(status)    Elapsed（超时）
     │             │
     ▼             ▼
  读 stdout/stderr  child.kill() → wait 收口
  → 累积器截断      → BashResult{ timed_out=true, ... }
  head+tail
     │
     ▼
  超限 → 写磁盘 → persisted_output_path 回填
```

**说人话（上图）**：和 **search** 一样用 **tokio 墙钟**包一层，但 bash 必须 **先 spawn 再 wait**，超时才能 **真杀进程**；输出太长就**砍头留尾**，全文扔**日志路径**里。术语细节见上条 **「说人话（把上条里的术语拆开）」**。

#### 2.4.4 PR-I（T2）：后台与 task 三件套

- **交付**：catalog 增加 **`run_in_background`**；为 **`true`** 时 **立即**返回 **`task_id`** + 日志路径，子进程由 **`tokio::spawn`** 守护泵写到 `~/.tomcat/agents/<id>/tool-results/bash-<task>.log`；新增 **`task_output` / `task_stop` / `task_list`**（strengthen 称「三件套」；最终工具名以 PR 为准）与 **`BashTaskRegistry`**（`Arc<RwLock<HashMap<…>>>`）。**本文补充的后台监控方案**是在 PR-I 基线上继续补 `task_output(block=true, timeout_ms=...)`、completion auto-feed 与 session 级后台事件。
- **验收**：集成测试覆盖「起后台 → 拉一段输出 → stop → list」；详见 PR-I 合入后更新 **§10**。

```text
  （以下为「多轮 agent」视角：LLM 每一轮可发 0～N 个 tool 调用；
   后台子进程与 LLM 推理并发，靠 task_id 串起来。）

  ┌─────────────────────────────────────────────────────────────────┐
  │ 轮次 A：起票（仍走 bash 工具；与 T1 同步路径共用 tool_exec）      │
  └─────────────────────────────────────────────────────────────────┘
        LLM 输出 tool_call: bash
        payload 含 run_in_background=true（及 command/args/cwd 等）
                    │
                    ▼
            ┌───────────────┐
            │  tool_exec    │  不阻塞等子进程结束
            └───────┬───────┘
                    │
        ┌───────────┴───────────┐
        ▼                       ▼
  tool 结果写回会话历史         BashTaskRegistry.register(task_id)
  给 LLM 立即看到：              + spawn Child（真实 shell 进程）
  { task_id,                     │
   output_path }                 └── tokio::spawn：泵 stdout/stderr
                                    ──▶ ~/.tomcat/.../bash-<task>.log

  ┌─────────────────────────────────────────────────────────────────┐
  │ 轮次 B、C…：三件套（独立 catalog 工具；模型像调普通 tool 一样调）   │
  └─────────────────────────────────────────────────────────────────┘

        LLM 输出 tool_call: task_output
        { task_id, since? }   （since = 字节偏移或游标，PR 钉死）
                    │
                    ▼
            ┌───────────────┐
            │  tool_exec    │──▶ Registry：从 output_path 读增量
            └───────┬───────┘
                    ▼
        tool 结果 ──▶ LLM（可多次调用，像 tail -f 分段接上）

        LLM 输出 tool_call: task_list
        { } 或 { filter? }
                    │
                    ▼
            ┌───────────────┐
            │  tool_exec    │──▶ Registry：枚举活跃/已完成 task
            └───────┬───────┘
                    ▼
        tool 结果 ──▶ LLM（决定接下来 output 哪个 id、是否 stop）

        LLM 输出 tool_call: task_stop
        { task_id }
                    │
                    ▼
            ┌───────────────┐
            │  tool_exec    │──▶ Registry：Child.kill → wait 收口
            └───────────────┘
                    ▼
        tool 结果 ──▶ LLM（会话里该 task 视为已结束）

  （现状）BackgroundTaskFinished ──▶ event_bus / UI
           不等同于 LLM 已读；LLM 仍可用 task_output 自行拉尾。
  （P1）  BackgroundTaskFinished ──▶ runtime follow_up_queue
           生成 synthetic notification message，让 agent 自动知道
           “后来 shell 跑完了”。
```

**说人话**：长命令**别卡一轮 tool**——第一轮 **`bash` + `run_in_background`** 先拿 **票号 + 日志路径**；后面几轮模型自己决定何时 **`task_output` 拉一段**、何时 **`task_list` 扫一眼**、何时 **`task_stop` 掐掉**；泵日志在后台跑，**和模型想下一步可以并行**。

##### 当前已落地与当前缺口

- **已落地（**PR-I MVP 与 P1 后台监控补齐**）**：
  - [`bash_task.rs`](../../../src/core/tools/primitive/bash_task.rs)：`spawn()` 起后台 child、pump stdout/stderr 到 `bash-<task_id>.log`、`wait()` 结束后翻 `Finished { exit_code }`；**P1 新增**：每 task 一份 `tokio::sync::Notify` + registry 级 `tokio::sync::broadcast::Sender<BackgroundTaskLifecycleEvent>`、`wait_for_change(task_id, since) -> WakeReason`（按"当前文件长度 vs since"判定，先 `notified()` 再读长度的标准 race-free 顺序）、`subscribe_lifecycle()`、`tail_log(task_id, max_bytes)`；`stop()` 与 wait 任务的终态翻转都受 `lifecycle_emitted` guard 保护，broadcast **每个 task 一生只发一次**。
  - [`tool_exec.rs`](../../../src/core/agent_loop/tool_exec.rs)：`bash run_in_background`、`task_output`、`task_stop`、`task_list` 已全部接线；**P1 新增** `task_output(block=true, timeout_ms=...)` 特判分支：`tokio::select!` 在 `wait_for_change` / `sleep_until(deadline)` / `cancel.cancelled()` / countdown tick 之间多路复用，每 500ms 发一次 `ToolExecutionUpdate(partial_result.phase="waiting_for_output", remainingMs, timeoutMs, taskId, since)`；返回 JSON 在 `block=true` 路径**额外**写出 `wakeReason ∈ {"new_output","finished","timeout"}`；`block=false` 路径**不**写出该字段（向后兼容）。
  - [`accessors.rs`](../../../src/core/agent_loop/accessors.rs)：`follow_up(String)` 行为不变；**P1 新增** `with_shared_follow_up_queue(...)`、`with_completion_routes(...)`、`follow_up_message(ChatMessage)` 三个 builder/typed 入口。
  - [`api/chat/mod.rs`](../../../src/api/chat/mod.rs)：**P1 新增** `ChatContext.{follow_up_queue, completion_routes, follow_up_signal, delivered_completion, completion_subscriber_handle}`；`spawn_completion_subscriber()` 在 `chat_loop` 启动时 spawn，按 claim-on-entry 状态机决定是否推 synthetic；主循环改造：`run_chat_turn` 返回后若 `follow_up_queue` 非空且 auto-turn 预算（`AUTO_TURN_BUDGET=K=8`）未耗尽则**跳过 readline**直接以 `input=""` 触发下一轮 turn，否则回 readline。`run_chat_turn` 在装配完 messages 后 drain 一次 session 级 `follow_up_queue`，让后台完成事件能在**首轮 reasoning** 之前就被注入。
  - [`cli_turn_renderer.rs`](../../../src/api/chat/cli_turn_renderer.rs)：**P1 新增**监听 `WIRE_TOOL_EXECUTION_UPDATE`，把 `task_output(block=true)` 的倒计时渲染成一行 dim 灰行 `[tool] task_output … waiting_for_output  task=<id> remaining=<r>/<t>ms`。
  - [`catalog.rs`](../../../src/core/tools/contract/catalog.rs)：**P1 新增** `task_output` 参数 `block: boolean`、`timeout_ms: integer (默认 5_000、上限 30_000、0 等价 block=false)`；description 教三种使用模式 + `<background-task-finished>` tag 识别契约。
  - [`system_prompt.rs`](../../../src/core/llm/system_prompt.rs)：**P1 新增** `BackgroundShellMonitorSection`（priority 30），把三种使用模式 + tag 识别 + 不鼓励的行为写进 system prompt。
- **结论**：经过 P1 后，Tomcat 已经具备 Cursor `Shell + AwaitShell + terminal file` 同等量级的 wait/wake 体验（工具级 block=true wait slice + 完成自动回灌）；P2 进一步把"运行中新输出也提前唤醒 / 宿主级空闲 park-wake / 多任务 running 摘要"补齐，详见 [`bash-background-monitor-p2_54f23d9c.plan.md`](../../../../../.cursor/plans/bash-background-monitor-p2_54f23d9c.plan.md)。

##### 参考实现补充对照（聚焦 completion auto-feed）

- **先说结论**：不是所有参考实现都叫 `follow_up_queue`，但只要支持“后台完成后自动继续”，共同点都是 **队列/事件通道 + synthetic 通知载荷 + 再驱动 agent**。
- **`cc-fork-01`**：后台 shell 完成后走 `commandQueue` + `<task-notification>`；agent 空闲时由 queue processor 开新 query，忙碌时还能把通知内联到当前 query。
- **`hermes-agent`**：有显式 `completion_queue`；完成后会生成 `[IMPORTANT: Background process ...]` 这类 synthetic 文本，再由 CLI `_pending_input` 或 gateway watcher 驱动下一轮。这是和 Tomcat 目标形态最接近的一类。
- **`openclaw`**：走 `system event + requestHeartbeat + heartbeat prompt`；completion 也是 synthetic 通知，但注入点是 session/heartbeat，而不是 `follow_up_queue`。
- **`codex`**：主要是 `ExecCommandOutputDelta` / `ExecCommandEnd` 事件流，加上 `needs_follow_up` / `pending_input` / `maybe_start_turn_for_pending_work()`；子 agent 完成会发 `<subagent_notification>`，同属 synthetic notification，但不是 Tomcat 同名注入点。
- **`pi-mono` / `pi_agent_rust` / `GenericAgent`**：有 follow-up、next-turn 或 task queue 之类基础设施，但 bash/command 自身并没有“后台 shell 完成自动回灌”的现成闭环。
- **对 Tomcat 的启示**：不必照抄某一家 API 名；更合理的是复用本仓已有 `follow_up_queue`，吸收 `cc-fork-01` / `hermes-agent` / `openclaw` 的“完成通知再驱动”语义。

##### P1 / P2 路线图

> **状态（2026-05）**：P1 已**全部落地**。下面保留语义说明作为契约文档；具体代码锚点已在上面 "已落地" 块写明。

- **P1（已交付 ✅）**：
  - `task_output(block, timeout_ms)`：等待到"新输出 / 任务结束 / 超时"再返回；超时**非终态**，调用方可继续调 `task_output(block=true)` 再等。
  - 返回 JSON 新增 `wakeReason ∈ {"new_output","finished","timeout"}`（仅 `block=true` 路径写出）。
  - `timeout_ms` 默认 `5_000`、上限 `30_000`（超过即 cap）、`0` 等价 `block=false`。
  - 后台 shell 自然结束（或 stop）时，runtime 在 `chat_loop` 里 spawn 的 lifecycle subscriber 守护 task 监听 `subscribe_lifecycle()`，按 [§ claim-on-entry 状态机](#claim-on-entry-状态机) 决定是否推 synthetic notification 到 session 级 `follow_up_queue`，由 between-turns drain（或 turn 内的 conv loop drain）自动喂回 agent。
  - synthetic notification 格式：`<background-task-finished task_id="..." exit_code="..." log_path="..." command="...">tail of last ≤4 KiB</background-task-finished>`，类型为 `ChatMessage::user`。
  - CLI：`tool_execution_update` 渲染成 dim 灰行做倒计时；后台完成事件通过 `eprintln!` 一行 `[bg] task <id> finished (exit=<c>); queued for next turn.`，**不**打断 `rustyline.readline()`（idle-aware 唤醒在 P2）。
  - auto-turn 风暴防护：每个真实用户输入之间最多连续 K=8 次 auto-turn，超过强制回 readline。
- **P2（待办）**：
  - runtime 在“当前无别的工作可做”时进入 idle-aware 等待；
  - 运行中只要出现**非完成态的新输出**，也能通过 `BackgroundTaskOutputReady` / `BackgroundTasksUpdate` 之类事件提前唤醒；
  - CLI 侧显示倒计时、running 数量、任务摘要，而不是只在 `tool_execution_end` 时一次性吐出结果。
- **为什么不是新开 `task_wait`**：
  - 复用现有 `task_id + since + next_offset` 契约；
  - 和 `cc-fork-01` 已落地的 `task_output(block, timeout)` 更接近；
  - 不把后台任务 API 从三件套继续膨胀成四件套/五件套。
- **为什么是 `follow_up_queue + synthetic notification message`**：
  - `follow_up_queue` 是**传输机制**；
  - synthetic notification message 是**载荷**；
  - 两者不是二选一，更不该默认伪造 `ChatMessage::tool`，因为这里不是“模型刚又主动调了一次工具”，而是“宿主在补充一个后来发生的新事实”。
- **Prompt 也要同步教模型**
  - `catalog.rs` 的 `task_output` description 已写清三种使用模式 + `<background-task-finished>` tag 识别；
  - `system_prompt.rs` 的 `BackgroundShellMonitorSection` 同步补一份全局指导；
  - 监控长任务时，若 `wakeReason=timeout && finished=false`，应继续用 `task_output(block=true)` 等下一个 slice，**不要**误判成失败或终态。

##### claim-on-entry 状态机

为彻底消除"`task_output(block=true)` 拿到 `wakeReason=finished` 同时 lifecycle subscriber 也推 synthetic"的 TOCTOU 双回灌竞态，session 级共享一份 `completion_routes: Arc<Mutex<HashMap<BashTaskId, CompletionRoute>>>`，状态机：

```text
enum CompletionRoute {
    ToolWillDeliver,   // dispatcher 已 claim, 由 tool result 交付
    Delivered,         // 已交付完成 (任意一路)；终态
}
```

- **dispatcher（block=true 路径）**
  - **entry**：进入 `block=true` 分支后第一件事 `routes.lock()`：
    - 已 `Delivered` → 直接 `read_output(since)` 返回 `wakeReason=finished`，**不**进 wait（lifecycle 已抢先 case）；
    - 否则 `insert(task_id, ToolWillDeliver)`。
  - **wake (Finished)** → `insert(task_id, Delivered)`。
  - **wake (NewOutput, finished=false)** → 保持 `ToolWillDeliver` 不变。
  - **wake (Timeout / Cancel, finished=false)** → `remove(task_id)` 让出 claim，让后续 lifecycle 兜底。
- **lifecycle subscriber（独立 tokio task；`chat_loop` 启动时 spawn，drop 时 abort）**
  - 收到 `BackgroundTaskLifecycleEvent` finished：
    1. 先看 host 内部 `delivered_completion: HashSet<BashTaskId>` 去重；
    2. 再看 `routes.lock()`：已 `ToolWillDeliver` / `Delivered` → 丢弃；否则 `insert(Delivered)` → 取 `tail_log(task_id, 4096)` → push `ChatMessage::user("<background-task-finished ...>tail</background-task-finished>")` 到 `follow_up_queue` → `notify_one()` 唤醒主循环 → `eprintln!` 提示一行。

**正确性**：所有写操作走同一把 `routes` 锁，串行化 dispatcher entry / dispatcher exit / lifecycle subscriber 三方；map 中**至多一个** `task_id` 条目；`Delivered` 是终态；shell 在任何时点完成都被恰好交付一次。

##### race-free `wait_for_change` 协议

`BashTaskRegistry::wait_for_change(task_id, since) -> WakeReason`：

```text
loop {
    let notified = notify.notified();   // ① 先注册等待者
    tokio::pin!(notified);
    if status != Running { return Finished; }  // ② 终态优先
    if file_len(log) > since { return NewOutput; }  // ③ 文件长度判定（不依赖事件计数）
    notified.await;                     // ④ 没新事就睡，等下一次 pump flush / 终态翻转
}
```

`pump` 每次 flush 后 `notify_waiters()`；wait 任务终态翻转后也 `notify_waiters()`。"先 notified() 再读 status / 长度" 的顺序是 race-free 必备：反过来会丢 wakeup，read_output(since=X) 后立即 wait_for_change(since=X) 之间到达的字节也不会丢。

```text
bash(run_in_background=true)
        │
        ├─▶ 立即返回 { task_id, log_path }
        │
        └─▶ BashTaskRegistry 持续 pump / wait
                  │
                  ├─ LLM 调 task_output(block=true, timeout_ms)
                  │    └─▶ 等到“新输出 / finished / timeout”后返回 chunk
                  │
                  └─ 任务自然结束
                        └─▶ BackgroundTaskFinished
                              ├─▶ CLI completion 通知
                              └─▶ host/chat_loop 构造 synthetic notification
                                    └─▶ session 级 follow_up queue
                                          └─▶ 新 turn 的 AgentLoop drain 后 continue
```

#### 2.4.5 PR-L（T3）：AST、Sandbox 与 PersistentShell 骨架 ✅(2026-05-07)

- **范围冻结**：[bash-pr-l-scope.md](bash-pr-l-scope.md)（在开 PR 前出 1 页，关闭 plan §六风险表「Phase-L AST/Sandbox 范围未定义」）。
- **已交付（PR-L 实际落地）**：
  - **`BashAstChecker`**（[`src/core/permission/bash_ast.rs`](../../../src/core/permission/bash_ast.rs)）：**手写切段**（识别顶层 `;` `&` `\n` `&&` `||` `|` 操作符；引号 / 反引号 / `$(...)` / `(...)` 一律按字面量处理，**不**触发外层切段）+ **allowlist/denylist** 命中判定。MVP 拒 heredoc `<<` 与 `for/while/until/if/case/function/select/{` 流程控制，返回 `AstReject::Unsupported`。**未启用** `tree-sitter-bash`：避免新 C 依赖、保持 WASM 兼容；**降级**说明已写入 scope spec §3。
  - **判定语义**：每段 `BashSegment` → `AstSegmentVerdict::AllowedSkipApproval`（命中 allowlist）/ `Defer`（无命中 → 由调用方走旧 `gate_check_bash` 三层）；命中 denylist → 直接 `AstReject::AstDeny` 早退；本 PR 内 `AllowedSkipApproval` **未真正跳过 approval**（只切段判定 + deny 早退；跳 approval 接线动用 grant trace + 审计字段，独立 PR 处理更安全）。
  - **接线**：`DefaultPrimitiveExecutor` 新增 `bash_ast: BashAstChecker` 字段 + `with_bash_ast(...)` builder；[`executor/bash.rs`](../../../src/core/tools/primitive/executor/bash.rs) `execute_bash_impl` 在 `gate_check_bash` **之前**先调 `executor.bash_ast.check(&audit_cmd)` —— 任何 `AstDeny` / `AstUnsupported` **早退 + 审计 success=false**，**不**进入 gate / 不 spawn。
  - **`SandboxBackend` trait** + **`NoopSandboxBackend`**：占位接口；`Noop` 直接 `cmd.spawn()`，与 PR-E.2 行为字节级等价。后续接 macOS Seatbelt / Linux Landlock 仅替换 `Arc<dyn SandboxBackend>` 注入。
  - **`PersistentShell` trait** 占位：真 PTY 循环按 scope spec §3 显式不在 PR-L 内。
  - **`ToolsBashAstConfig { enabled, allowlist, denylist, sandbox_backend }`**：默认 `enabled=true` + 空 list → 与今日 `gate_check_bash` 路径**字节级等价**（scope spec §4 兼容契约硬约束）。`[tools.bash.ast]` TOML 反序列化与 `api/chat` 装配注入留给后续 PR；本 PR 仅暴露 builder。
- **验收**：bash_ast 模块 14 个 `#[cfg(test)]`（disabled / 拆 ; && || | / deny 短路 / allow 跳 approval / 赋值前缀 / 子 shell 字面量 / 子 shell 内分隔符不切段 / 子 shell 未配对 / 引号未配对 / 流程控制 / heredoc / glob 前缀模式 / NoopSandboxBackend spawn echo）；`suite_test` 3 个端到端 `#[tokio::test]`：`bash_ast_allowlist_denies_compound_command_short_circuit`、`bash_ast_default_empty_lists_keeps_legacy_behavior`、`bash_ast_heredoc_returns_unsupported_error`。`cargo test --lib` 765 全绿；`fmt`/`clippy -D warnings` 全绿；`agent_loop_tests` / `bash_assignment_deny` / `cli_tests` / `primitives_tools_tests` / `tool_catalog_doc` 集成回归通过。

```text
  audit_cmd 字符串
        │
        ▼
┌───────────────────┐
│ parse (AST)       │  复合语句 → 子命令列表 [c1, c2, ...]
└─────────┬─────────┘
          │
          ▼
   for each segment ci
          │
          ▼
┌───────────────────┐     命中 deny → Err（理由可执行）
│ allow/deny 规则    │──── 全过 → 进入现有 gate_check_bash
└─────────┬─────────┘
          │
          ▼
┌───────────────────┐     （可选）SandboxBackend::spawn
│ PersistentShell   │     同会话 cwd/env — 骨架期可 no-op
│ （骨架）           │
└───────────────────┘
```

**说人话**：**AST** 把一条复杂命令**拆成多小段**分别过规则；**沙箱**和 **持久 shell** 先把**接口立住**，别一口气做到产品级。

---

## 3. 术语统一

| 术语 | 语义（大白话） | 数据载体 | 行为约束 | 说人话 |
|------|----------------|----------|----------|--------|
| **`audit_cmd`** | 写入审计与 gate 的命令快照字符串 | `String`（`bash.rs` 拼装） | argv 模式为 `command` + 空格拼接各 `args` | 日志里看到底跑了啥。 |
| **argv 模式** | 不经 shell，`Command::new(command).args(args)` | `tool_exec` 解析 `args` 数组 → `Option<&[String]>` | 与 catalog JSON **未声明 `args`** 并存见 §4 | 像 execve 一样传参。 |
| **`cwd_path`** | 实际 `current_dir` | `PathBuf` | `cwd: None` 时用 `.`（相对进程 CWD）；`Some` 时先 `gate_check_path(Read, cwd)` | 工作目录也要过读权限。 |
| **`timed_out`（规划）** | 墙钟超时发生 | JSON / `BashResult` 扩展字段 | 与 `exit_code == -1` 等约定在 PR-E 钉死 | 到点没跑完就算超时类结局。 |
| **`task_id`（规划）** | 后台任务句柄 | UUID 或字符串 | PR-I 与审计 `task_id` 列对齐 strengthen §7 | 跟进程票号一样用来查杀。 |

**「LLM 收到 tool 结果后」**：指 **`tool_exec` 已把 stdout/stderr/exit 格式化成 tool 消息文本**、写入会话历史、**即将进入下一轮模型推理之前**。

---

## 4. 协议（入参 / 出参 / Schema）

**单一事实源（现状）**：

- JSON Schema（模型可见）：[`catalog.rs`](../../../src/core/tools/contract/catalog.rs) `execute_bash_parameters()` → [`docs/tool-catalog.md`](../../tool-catalog.md)（若生成）。
- 运行时扩展：`tool_exec` 对 **`args`** 的解析（**实现有、schema 未列** — 见下表）。
- 原语返回：`BashResult` — [`primitive/types.rs`](../../../src/core/tools/primitive/types.rs)。

### 4.1 入参（工具 arguments）

| 字段 | JSON 类型 | 必填 | 默认 | 说明 | 说人话 |
|------|-----------|------|------|------|--------|
| `command` | string | **是**（catalog） | — | shell 一行的主体，或 argv 模式的可执行文件路径 | 命令本体。 |
| `cwd` | string | 否 | 行为上等价 **`.`**（见代码） | 工作目录；应优先传项目绝对路径（system_prompt 已引导） | 在哪跑。 |
| `args` | string[] | 否 | — | **仅 `tool_exec` 解析**：存在则走 argv 模式，**不**走 `sh -c` | 拆成 argv 就不经过 shell 解析字符串。 |

**Schema 缺口（现状 MUST 写明）**：[`execute_bash_parameters()`](../../../src/core/tools/contract/catalog.rs) 仅声明 `command` 与 `cwd`；**`args` 不在 schema 中**，但 dispatcher / 集成测试会传。文档建议：**随 PR-A（优先）**将 `args` 纳入正式 schema（与改名同 PR 或小步紧跟）；若未做则 PR-E 前须补，避免 T1 合入后仍双轨。

### 4.2 出参（Rust：`BashResult`）

| 字段 | 类型 | 说明 |
|------|------|------|
| `stdout` | `String` | UTF-8 lossy 自字节流 |
| `stderr` | `String` | 同上 |
| `exit_code` | `i32`（serde 名 `code`） | `status.code()`，signal 时 **`-1`** |

**Wire 到模型（`tool_exec`）**：非空 stdout 原样；stderr 前加 **`STDERR: `** 前缀；末尾追加 **`(exit code: N)`** 行。

### 4.3 调用样例（jsonc）

**Shell 模式**：

```jsonc
{
  "command": "cargo test -p tomcat --lib",
  "cwd": "/abs/path/to/tomcat"
}
```

**Argv 模式（实现支持；schema 待补）**：

```jsonc
{
  "command": "echo",
  "cwd": ".",
  "args": ["hello", "world"]
}
```

---

## 5. One-Glance Map（文件职责总览）

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/llm/system_prompt.rs                                             │
│  • 引导优先 search_files；execute_bash.cwd 使用项目路径等                   │
└────────────────────────────────────────────────────────────────────────────┘
        │
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/tools/contract/catalog.rs                                        │
│  • BUILTIN_TOOL_CATALOG：name = "execute_bash"（→ 规划改为 "bash"）         │
│  • execute_bash_parameters()：command, cwd                                 │
└────────────────────────────────────────────────────────────────────────────┘
        │
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/agent_loop/tool_exec.rs                                          │
│  • match "execute_bash"：解析 command / cwd / args → primitive.execute_bash │
│  • 合并 stdout/stderr + exit code 格式化为单字符串 tool 结果                 │
└───────────────────────────────┬────────────────────────────────────────────┘
                                │
                                ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/tools/primitive/executor/bash.rs                                   │
│  • execute_bash_impl：cwd gate → audit_cmd → bash_ast → gate_check_bash      │
│  • Command::output()【当前无 tokio::timeout 包裹】                           │
│  • BASH_TIMEOUT_SECS：#[allow(dead_code)]，未接线                           │
└───────────────────────────────┬────────────────────────────────────────────┘
              ▼
┌──────────────────────────────┐
│  gate.rs::gate_check_bash     │
│  + PermissionGate::check_bash │
└──────────────────────────────┘
              │
              ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  infra/audit + PrimitiveAuditEntry                                         │
│  • Bash 成功/失败均 record_primitive；scope / grant 字段见 gate 返回值       │
└────────────────────────────────────────────────────────────────────────────┘
```

**阅读顺序（说人话）**：模型先看到 **catalog 里的名字与参数**；真正执行时 **tool_exec** 把 JSON 解出来交给 **`execute_bash_impl`**；后者先问 **cwd 能不能读**、再跑 **bash_ast** 和 **整条命令的 bash 策略**；不再从命令字符串里猜路径；最后才 **spawn**；**审计**记一条，不管成功失败。

**配套测试**：[`primitive/tests/suite_test.rs`](../../../src/core/tools/primitive/tests/suite_test.rs)、[`primitive/tests/gate_suite_test.rs`](../../../src/core/tools/primitive/tests/gate_suite_test.rs)。

---

## 6. 调度时序（运行时图）

### 6.1 当前：前台单次执行

```text
LLM          tool_exec              bash::execute_bash_impl        gate / spawn
 │               │                         │                          │
 │ execute_bash  │  parse command/cwd/args │                          │
 │──────────────>│────────────────────────>│ gate_check_path(cwd)     │
 │               │                         │ gate_check_bash        │
 │               │                         │ for path in extract_paths │
 │               │                         │   gate_check_path(Bash) │
 │               │                         │ Command::output()       │
 │               │<────────────────────────│ BashResult               │
 │<──────────────│ 拼接 stdout/stderr/exit  │                          │
```

### 6.2 规划：超时包裹（PR-E）

```text
  let mut child = Command::spawn(...)?;   // PR-E：从 output() 迁出
       │
       ▼
  tokio::time::timeout(T, child.wait())
       │
       ├─ Ok(Ok(status)) ──▶ 取 stdout/stderr → BashResult + 截断累积器
       │
       ├─ Ok(Err(wait_err)) ──▶ Err(Primitive(...))
       │
       └─ Err(_elapsed) ──▶ child.kill().await; let _ = child.wait().await;
                          → BashResult { timed_out=true, exit_code: -1, ... }
```

**前提**：必须在 **`spawn` 仍持有 `Child`** 的前提下 `timeout` + `kill`；**禁止**伪代码写成 `timeout(..., child.wait_with_output())`（`wait_with_output` 会拿走 `Child`，超时分支无法 `kill`）。

**对照**：[`gate.rs::run_search_command`](../../../src/core/tools/primitive/executor/gate.rs) 已对 search 子进程使用 **`tokio::time::timeout`**；bash 路径尚未对齐；PR-E 实施时 bash 侧需 **显式 spawn**（见上）。

---

## 7. 状态机（当前与规划）

### 7.1 当前

```text
           ┌─────────────┐
  调用开始 │   Running   │  （单次 await output）
           └──────┬──────┘
                  │ output 返回
                  ▼
           ┌─────────────┐
           │   Done      │  （成功或非零 exit，均为 Ok(BashResult)）
           └─────────────┘
```

**说明**：子进程 **`kill_on_drop(true)`**；未显式超时，长时间阻塞依赖宿主取消策略（见 [`tool_dispatcher.rs`](../../../src/core/agent_loop/tool_dispatcher.rs) 注释）。

### 7.2 规划（PR-I）

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| — | `run_in_background=true` | BackgroundRunning | 立即返回 `task_id` + log path | 先拿票，再慢慢跑。 |
| BackgroundRunning | `task_output(block=false)` | BackgroundRunning | 立即返回当前增量 | 现在已经有的轮询版。 |
| BackgroundRunning | `task_output(block=true)` | BackgroundRunning | 等到新输出 / 结束 / 超时后再返回 | P1：更像 AwaitShell。 |
| BackgroundRunning | 进程自然退出 | Done | 写入 `exit_code`；P1 额外发 `BackgroundTaskFinished` + synthetic follow-up | 跑完后 agent 不用靠手工 poll 才知道。 |
| BackgroundRunning | 运行中新输出 | BackgroundRunning | P2 可发 `BackgroundTaskOutputReady` / `BackgroundTasksUpdate` | 运行中也能被宿主提前叫醒。 |
| BackgroundRunning | `task_stop` | Killed | `killpg` + wait 收口 | 主动掐掉。 |

---

## 8. 配置与环境变量

**总则**：`env > config > 默认`（与全仓一致）。

| 来源 | 键 | 含义 | 备注 | 说人话 |
|------|-----|------|------|--------|
| `tomcat.config.toml` / `PrimitiveConfig` | `wasmedge_env_path` | Unix 下 `sh -c` 前可选 `source` 的脚本 | 默认 `$HOME/.wasmedge/env`；见 [`infra/config/types.rs`](../../../src/infra/config/types.rs) 注释 | WasmEdge 环境注入用。 |
| **规划** `[tools.bash]` | `timeout_ms` / `max_output_chars` 等 | PR-E 墙钟与输出上限 | **PENDING** | 以后配置文件也能拧超时。 |

---

## 9. 错误模型 / 截断 / 超时

### 9.1 当前归一化结局

```text
                    execute_bash 请求
                            │
        ┌───────────────────┼───────────────────┐
        ▼                   ▼                   ▼
   gate 拒绝           spawn/IO 错误          子进程结束
   Err(Permission/…)    Err(Primitive(..))     Ok(BashResult)
        │                   │                   │
        │                   │                   ├─ exit_code==0
        │                   │                   └─ exit_code!=0 仍为 Ok
        └───────────────────┴───────────────────┘
```

**`tool_exec` 视角**：`PrimitiveExecutor::execute_bash` 的 **`Err`** → 工具失败字符串；**`Ok`** 时非零 exit 也返回**成功工具消息**（内容里含 `exit code`）。

### 9.2 规划（PR-E）

- **前提**：墙钟超时 + **超时杀进程** 要求 **`Command::spawn` + `wait`**（或异步等价）收集退出状态；**不能**在 PR-E 仍用 **`output().await` 外包一层 `timeout`** 作为唯一手段（拿不到可靠 kill 句柄时行为未定义）。见 **§6.2**。
- **`timed_out=true`**（或等价）与 **截断** 进入同一框图分支，与 strengthen 计划 §2.4 一致。
- **`persisted_output_path`**：超限全文路径回填 JSON / 文本尾注。

---

## 10. 测试矩阵（验收）

| 维度 | 用例（实际函数名 / 路径） | 状态 |
|------|---------------------------|------|
| 原语成功 / 禁止 | `suite_test::execute_bash_success`、`suite_test::execute_bash_forbidden` | ✅ |
| gate + 审计 | `gate_suite_test::gate_bash_forbidden_blocks`、`gate_suite_test::execute_bash_audit_records_bash_scope`、`gate_suite_test::gate_bash_approval_allow_once` | ✅ |
| 路径 RHS 预检 | `tests/bash_assignment_deny.rs::bash_assignment_rhs_denied_in_all_supported_positions` | ✅ |
| 集成 echo / argv | `tests/primitives_tools_tests.rs::test_primitive_executor_execute_bash_echo_succeeds`、`test_primitive_executor_execute_bash_argv_echo` | ✅ |
| dispatcher | `dispatch_with_extension_test::dispatch_execute_bash_with_primitive_returns_ok`、`dispatch_execute_bash_with_argv_calls_primitive` | ✅ |
| gate 单测 | `core/permission/tests/gate_test.rs` 中 `bash_forbidden_blocks_self_escalation`、`bash_approval_required_layer2` 等 | ✅ |
| catalog / prompt | `catalog_test`（`execute_bash` 条目）、`system_prompt_test`（含 `execute_bash` 字面量） | ✅ |
| E2E CLI | `tests/cli_tests.rs::test_user_asks_pi_to_run_bash_command` | ✅ |
| Agent loop 容错 | `tests/agent_loop_tests.rs`（`execute_bash` mock 流） | ✅ |
| **T1** 超时杀进程 | `suite_test::bash_wallclock_timeout_kills_process` | ✅(2026-05-07) |
| **T1** 输出截断落盘 | `suite_test::bash_output_truncation_keeps_head_tail`、`suite_test::bash_persists_full_output_when_truncated` | ✅(2026-05-07) |
| **T2** 后台三件套 | `submodules_test::tool_exec_bash_background_full_lifecycle`、`tool_exec_bash_background_without_registry_returns_friendly_error`、`tool_exec_task_output_without_registry_returns_friendly_error`、`tool_exec_task_list_without_registry_returns_friendly_error` + `bash_task::tests::*`（registry CRUD） | ✅(2026-05-07) |
| **P1** registry race-free wait | `bash_task_test::wait_for_change_returns_new_output_after_pump_flush`、`wait_for_change_returns_finished_on_natural_exit`、`subscribe_lifecycle_emits_once_per_task`、`tail_log_returns_suffix` | ✅(2026-05) |
| **P1** `task_output(block=true)` 契约 | `submodules_test::task_output_block_true_returns_finished_on_natural_exit`、`task_output_block_true_timeout_is_non_terminal_wait_slice`、`task_output_timeout_zero_is_equivalent_to_non_blocking`、`task_output_timeout_ms_cap_does_not_block_indefinitely` | ✅(2026-05) |
| **P1** claim-on-entry 状态机 | `submodules_test::task_output_block_true_claims_completion_route_on_finished`、`task_output_block_true_releases_claim_on_timeout`、`task_output_block_true_skips_wait_when_lifecycle_already_delivered` | ✅(2026-05) |
| **P1** CLI 倒计时 | `cli_turn_renderer::tests::tool_update_emits_dim_countdown_line_on_stderr` | ✅(2026-05) |
| **P1** 真 LLM CLI 黑盒门禁 | `cli_tests::test_user_background_bash_autofeed_real_llm_cli`、`test_user_background_bash_blocking_waitslice_real_llm_cli`、`test_user_background_bash_multiple_timeout_slices_real_llm_cli` | ✅(2026-05) |
| **P2** `BackgroundTaskOutputReady` / running 摘要 | （待 P2） | PENDING |
| **T3** AST allowlist | `suite_test::bash_ast_allowlist_denies_compound_command_short_circuit`、`suite_test::bash_ast_default_empty_lists_keeps_legacy_behavior`、`suite_test::bash_ast_heredoc_returns_unsupported_error` + `bash_ast::tests::*`（14 例） | ✅(2026-05-07) |

§1 观察指标 **G1–G6**：未 ✅ 的行对应目标仍为路线图；合入后把状态改为日期并回填函数名。

---

## 11. 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|------------------|--------|
| 超长 stdout/stderr | OOM / 卡顿 | PR-E：`EndTruncatingAccumulator` + 落盘 + 字符上限 | 输出太大就砍并指到文件。 |
| 无超时 | 子进程永久挂起 | PR-E：`tokio::time::timeout` + kill；默认 120s | 到点强杀。 |
| `args` schema 缺失 | 文档与模型可见能力不一致 | **PR-A 优先**补齐 schema，再 PR-E 对齐样例与测试 | 别让模型以为不能传 argv。 |
| 重命名破坏 transcript | 旧会话工具名失效 | 与 read 同：**不重定向**；session/transcript 层 **`warn`** + 未知工具错误；文档与 prompt 只教 **`bash`** | 老会话别指望长名还能静默跑。 |
| `extract_paths` 盲区 | 动态路径未预检 | 保留 T-147 类提示词防御 + T3 AST | 静态抠不全的路径靠别的层补。 |
| 后台通知风暴 | 重复唤醒 / busy loop | completion 按 `task_id` 去重；P2 对 output-ready 事件做 coalesce / rate limit | 别让一堆后台任务把 agent 吵醒到停不下来。 |

---

## 12. 历史决策（已被本方案取代或待定）

- ~~strengthen 计划 §1「`execute_bash` 单迭代 fallback」~~ → **否**：与 [read.md](read.md) **PR-RA** 一致——**无运行时重定向**；legacy 名 **`warn` + 未知工具**（或等价），避免双轨审计。
- ~~`BASH_TIMEOUT_SECS = 30` 已生效~~ → **否**：常量 **`#[allow(dead_code)]`**，**未**包裹 `output()`；PR-E 改为可配 **`timeout_ms`**（默认 120s）。
- ~~输出无上限整段进 String~~ → **否**：PR-E 引入有界累积 + 落盘。
- ~~仅依赖 regex forbidden/approval~~ → **偏否**：T3 叠加 AST 分段规则；regex 路径保留。
- ~~后台任务第一期就做 auto-background~~ → **否**：strengthen 明确先手动 `run_in_background`，不抄 cc-fork 自动预算。

---

## 13. 关联文档

- 兄弟工具：[read.md](read.md) · [write.md](write.md) · [edit.md](edit.md) · [search_files.md](search_files.md)
- 权限总览：[../permission-system.md](../permission-system.md)
- 总计划：[strengthen-four-core-tools_b51c9eae.plan.md](../../../../../.cursor/plans/strengthen-four-core-tools_b51c9eae.plan.md)
- 五仓对比：[agent-tools-comparison.md](../../reports/agent-tools-comparison.md)
- 派生工具目录：[tool-catalog.md](../../tool-catalog.md)
- 看板目录（长任务叙事）：[`agents/TASK_BOARD_002/README.md`](../../../agents/TASK_BOARD_002/README.md)

---

**一句话总结**：**现状**在 **`bash.rs`** 串起 **cwd → bash gate → 路径预检 → spawn → 审计**；**`tool_exec`** 负责 **argv 解析与结果 stringify**；路线图 **先 PR-A 改名** 再 **PR-E → PR-I → PR-L**；对外名 **终态 `bash`**，以 **catalog + types.rs** 为契约锚点。

---

## 附录：与 strengthen 计划章节对照

| strengthen `strengthen-four-core-tools_b51c9eae` 章节 / PR | 本文位置 |
|------------------------------------------------------------|----------|
| §0.5 bash 维度表 / §1 差距 bash 行 | §2.2 |
| §1 命名 PR-A（**本方案要求最先实施**；**fallback 句以本文 + read 为准**） | §2.3、§2.4 表首行、§2.4.1、§3、§4、§11、§12 |
| §2.4 bash T1 | §2.4 PR-E、§2.4.3、§6.2、§9.2、§10 PENDING |
| §3.4 bash T2 | §1 G4、§4.2、§10 PENDING |
| §4.3 bash T3 | §1 G5、§2.4 PR-L、§2.4.5 |
