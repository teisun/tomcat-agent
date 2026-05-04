# search_files 工具：双实现 + 启动预检

本文档是 `search_files` 工具的最终技术方案（架构 spec），承接计划
[`search_files_兜底选型_c8b4a778.plan.md`](../../../../.cursor/plans/search_files_兜底选型_c8b4a778.plan.md)
与 [`TASK_BOARD_002.md`](../../../agents/TASK_BOARD_002.md) T2-P0-005 子项「search_files 兜底与预检」。

> **位置说明**：计划文档（plan）记录决策过程与待办清单；本文为冻结后的技术方案，包含协议、调度逻辑、竞品分析、One-Glance Map 与运行时图。

---

## 目录

- [1. 目标与设计原则](#1-目标与设计原则)
- [2. 竞品 / 选型对比](#2-竞品--选型对比)
- [3. 协议（入参 / 出参 / Schema）](#3-协议入参--出参--schema)
- [4. One-Glance Map（文件职责总览）](#4-one-glance-map文件职责总览)
- [5. 调度时序（运行时图）](#5-调度时序运行时图)
- [6. Tier1 / Tier2 行为对照](#6-tier1--tier2-行为对照)
- [7. 启动预检（pi chat）](#7-启动预检pi-chat)
- [8. 预检事件状态机](#8-预检事件状态机)
- [9. 平台决策树（桌面 / Termux / 普通 Android App）](#9-平台决策树桌面--termux--普通-android-app)
- [10. 配置与环境变量](#10-配置与环境变量)
- [11. 错误模型 / 截断 / 警告](#11-错误模型--截断--警告)
- [12. 测试矩阵（实现 ↔ 用例）](#12-测试矩阵实现--用例)
- [13. 历史决策（已被本方案取代）](#13-历史决策已被本方案取代)
- [14. 关联文档](#14-关联文档)

---

## 1. 目标与设计原则

- **对外仅一个工具名 `search_files`**，一套 JSON Schema；内部 **Tier1 → Tier2 自动回落**。LLM 永远只看到一个工具，避免乱选。
- 缺 `rg`/`fd` 时**不抛错**：自动切到进程内的纯 Rust 实现，让"没装 / 装不上 / 没网"的用户依然能搜索。
- 进入 `pi chat` 时**后台**探测并尝试安装 `rg` / `fd`，**全程非阻塞**会话；不做 LLM/全网连通性探测。
- 普通 Android App（非 Termux）**不自动装**外部二进制，全部走 Tier2。
- 预检由 `[preflight] auto_install_search_tools` + `PI_SKIP_SEARCH_TOOLS_PREFLIGHT` 双开关控制（**env > config > 默认 true**）。
- 输入/输出 Schema **跨 Tier 一致**；实现差异通过 `warnings` 表达，不在协议层分裂。

---

## 2. 竞品 / 选型对比

### 2.1 行业内"代码搜索"工具的三种典型形态

```text
┌──────────────────────────────────────────────────────────────────────────┐
│                        代码搜索能力的三种形态                             │
├───────────────────┬──────────────────────────────────────────────────────┤
│  A. 进程内（嵌入）│ 不依赖外部二进制；纯库；跨平台；性能中等              │
│                   │  - ripgrep 自带的 `ignore` / `globset` / `grep` crate │
│                   │  - GitHub Tantivy（倒排）                             │
│  B. spawn 通用    │ 调系统已有 grep/find；POSIX/GNU/BSD 方言差；安全面大  │
│  C. spawn 现代    │ 调 ripgrep / fd；性能最佳；需用户预装                  │
└───────────────────┴──────────────────────────────────────────────────────┘
```

### 2.2 常见同类工具横向对比

| 工具 / 项目 | 形态 | 协议层 | 默认 `.gitignore` | 正则方言 | 大仓库性能 | 跨平台 | 我们的借鉴点 |
|--------------|------|--------|------------------|----------|-----------|--------|---------------|
| **GNU `grep` + `find`** | spawn POSIX | shell 字符串 | 否 | BRE / ERE / PCRE | 慢 | 差（Win/移动端） | 不采用：方言差 + 注入面 |
| **`ripgrep` (rg)** | spawn 现代 | CLI 参数 + 行式输出 | **是** | Rust regex | 很快 | 高 | **Tier1 内容主力** |
| **`fd` / `fdfind`** | spawn 现代 | CLI 参数 | **是** | glob | 快 | 高 | **Tier1 文件名主力** |
| **`ignore` + `globset` + `regex`** | 进程内 crate | 库 API | **是** | Rust regex | 中 | 高 | **Tier2 兜底主力** |
| **Cursor Instant Grep** | 内置（自家二进制） | IDE 内部 | 是 | 内部 | 极快 | 跨平台桌面 | 协议层提示「优先内置 grep」的设计思想 |
| **Claude Code `Grep` / `Glob`** | spawn ripgrep / 进程内 | 工具 API（JSON） | 是 | Rust regex | 快 | 高 | 单工具统一入口；分页/截断/警告字段 |
| **Codex / Aider grep** | spawn ripgrep | shell + 解析 | 是 | rg | 快 | 高 | spawn 方案的工程参考 |
| **GitHub Code Search / Tantivy** | 后端服务 / 倒排索引 | HTTP / SDK | — | 自家 | 极快 | 服务端 | 不适用：本地 Agent 场景 |

### 2.3 三条候选路线（最终选 C+A）

| 维度 | A. 进程内（`ignore` + globset + regex） | B. spawn `find` + `grep` | C. spawn `rg` + `fd`（旧主线） |
|------|-----------------------------------------|----------------------------|---------------------------------|
| 适用面 | 任何可读磁盘 | Unix 较好；Windows 差 | 需用户已装 rg/fd |
| files 实现 | globset + WalkBuilder | find 名称匹配（与 fd 不一致） | 与 fd 一致 |
| content 实现 | regex（与 rg 有方言差） | grep -R（差异更大） | 与 rg 一致 |
| `.gitignore` 默认 | 是（`ignore` crate 自带） | 否 | 是 |
| 注入面 / 跨平台 | **零注入** / 高 | 高 / 低 | 受限 / 高 |
| 维护成本 | 一套纯 Rust 双轨测试 | 三平台 shell 差异，最贵 | 当前最低 |
| 适合作为 | **Tier2 兜底** | （不采用） | **Tier1 主力** |

> **最终决策**：**Tier1 = C（rg + fd）+ Tier2 = A（`ignore::WalkBuilder` + globset + regex）自动回落**；**B 不采用**。LLM 只看到一个 `search_files`，schema 跨 Tier 一致；语义差通过 `warnings` 字段表达。

### 2.4 与其它 Agent 工具的协议差异点

```text
┌─────────────────────────────────────────────────────────────────────────┐
│ 协议设计差异                                                            │
├──────────────────────┬──────────────────────────────────────────────────┤
│ Claude Code Grep     │ output_mode = files_with_matches | content | count │
│  ↑ 直接被我们沿用    │ context = -A/-B/-C；head_limit；type=rust/py/...  │
├──────────────────────┼──────────────────────────────────────────────────┤
│ Cursor Grep          │ 简单 query + path；强调「内置 grep 优先」        │
├──────────────────────┼──────────────────────────────────────────────────┤
│ pi search_files      │ 单工具双 target（content / files）+ 同上 + 增加：  │
│                      │   • include_hidden、case_insensitive 显式开关     │
│                      │   • offset / next_offset 稳定分页                 │
│                      │   • warnings 透传 Tier 切换 / 方言差 / 截断原因   │
│                      │   • implementation 写入审计                       │
└──────────────────────┴──────────────────────────────────────────────────┘
```

---

## 3. 协议（入参 / 出参 / Schema）

> 单一事实源：[`src/core/tools/primitive/types.rs`](../../../src/core/tools/primitive/types.rs)；
> 派生：[`src/core/tools/catalog.rs::search_files_parameters`](../../../src/core/tools/catalog.rs)
> → `build_function_definitions()` → [`docs/tool-catalog.md`](../../../docs/tool-catalog.md)。

### 3.1 入参 `SearchFilesArgs`

| 字段 | JSON 类型 | 必填 | 默认值 | 适用 target | 说明 |
|------|-----------|------|--------|-------------|------|
| `pattern` | string | **是** | — | both | `target=content`：ripgrep regex；`target=files`：路径 glob（如 `*.rs`、`src/**/*.rs`） |
| `target` | enum `"content"` \| `"files"` | 否 | `"content"` | both | `content` 在文件**内**搜；`files` 搜**路径** |
| `path` | string | 否 | 当前工作区 | both | 搜索根；必须通过 Read 权限 gate |
| `glob` | string | 否 | — | content | 文件路径过滤，如 `*.rs`、`**/*.md` |
| `type` | string | 否 | — | content | ripgrep 文件类型 alias，如 `rust` / `js` / `py` / `md` / `toml` / `json` / `yaml` |
| `output_mode` | enum `"content"` \| `"files_with_matches"` \| `"count"` | 否 | `"files_with_matches"` | content | 返回行 / 命中文件集 / 每文件计数 |
| `context` | integer ≥ 0 | 否 | 0 | content（仅 `output_mode=content`） | 命中行前后各 N 行；Tier2 不实现 → warning |
| `head_limit` | integer 1..=1024 \| `null` | 否 | content=64, files=128 | both | `null` = 不限；`0` 校验失败；与 `offset` 搭配 |
| `offset` | integer ≥ 0 | 否 | 0 | both | 跳过结果项数；与 `next_offset` 配套实现稳定翻页 |
| `case_insensitive` | boolean | 否 | false | content | 等价 `rg -i` |
| `include_hidden` | boolean | 否 | false | both | 是否包含 dotfiles（`.gitignore` 仍生效） |

> **`head_limit` 三态语义**（Rust 表示 `Option<Option<usize>>`）：
> - 字段缺省 → 用 target 默认值；
> - 显式 `null` → 不限；
> - 显式整数 → 上限（≤ 1024，0 拒绝）。

### 3.2 出参 `SearchFilesOutput`

| 字段 | 类型 | 说明 |
|------|------|------|
| `mode` | enum `"content_files"` \| `"content_lines"` \| `"content_count"` \| `"files"` | 与 `target × output_mode` 组合一一对应（见下表） |
| `query` | `SearchFilesQuery` | **回显查询**：归一化后的 pattern / target / path / glob / type / output_mode / head_limit / offset / case_insensitive / include_hidden |
| `files` | `string[]?` | 当 `mode=files` 或 `mode=content_files` 时填充 |
| `matches` | `SearchFileMatch[]?` | 当 `mode=content_lines` 时填充 |
| `counts` | `SearchFileCount[]?` | 当 `mode=content_count` 时填充 |
| `stats` | `SearchFilesStats` | `scanned_files`、`elapsed_ms` |
| `truncated` | boolean | 是否被 `head_limit` 或 Tier2 墙钟截断 |
| `next_offset` | integer? | `truncated=true` 时给出后续偏移；否则为 null |
| `warnings` | `string[]` | 实现 / 方言 / 截断 / 跳过文件等可读说明 |

#### `mode` 取值规则

| target | output_mode | mode |
|--------|-------------|------|
| `files` | —（忽略） | `files` |
| `content` | `files_with_matches`（默认） | `content_files` |
| `content` | `content` | `content_lines` |
| `content` | `count` | `content_count` |

#### 子结构

```text
SearchFileMatch
  • path: string         // 相对 root 的路径（用 "/" 分隔）
  • line: u64            // 1-based
  • text: string         // 命中行（去尾部 \r\n）
  • before: string[]     // 前置上下文行（Tier2 暂为空 + warning）
  • after:  string[]     // 后置上下文行（同上）

SearchFileCount
  • path: string
  • count: u64           // 命中行数

SearchFilesStats
  • scanned_files: usize
  • elapsed_ms: u128
```

### 3.3 状态字段 vs 结果字段（一图看清）

```text
┌───────────────────────────────── SearchFilesOutput ─────────────────────────────────┐
│                                                                                     │
│  mode  ──┐                                                                          │
│          │                                                                          │
│          ▼                                                                          │
│   ┌──────────────┬─────────────┬──────────────┬──────────────┐                      │
│   │  files       │ content_    │ content_     │ content_     │                      │
│   │              │ files       │ lines        │ count        │                      │
│   ├──────────────┼─────────────┼──────────────┼──────────────┤                      │
│   │ files: [...] │ files:[...] │ matches:[..] │ counts: [..] │  ← 三选一字段        │
│   │ matches:null │ matches:nil │ files: null  │ files: null  │                      │
│   │ counts: null │ counts: nil │ counts: null │ matches:null │                      │
│   └──────────────┴─────────────┴──────────────┴──────────────┘                      │
│                                                                                     │
│  query (回显)：归一化后的查询参数（含 head_limit/offset/include_hidden 等实际取值)│
│  stats (实测)：scanned_files / elapsed_ms                                           │
│  truncated + next_offset：分页 / 墙钟截断协同                                       │
│  warnings：implementation=tier1|tier2、regex 方言、跳过的二进制 / 大文件、墙钟到点  │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

### 3.4 调用样例

**输入**：`target=content`，三种 `output_mode` 同一份 pattern。

```jsonc
{
  "pattern": "TODO\\(.+\\)",
  "target": "content",
  "path": "src/",
  "glob": "*.rs",
  "output_mode": "files_with_matches",
  "head_limit": 50,
  "include_hidden": false
}
```

**输出**（Tier2 路径示例，节选）：

```jsonc
{
  "mode": "content_files",
  "query": {
    "pattern": "TODO\\(.+\\)", "target": "content", "path": "src",
    "glob": "*.rs", "type": null, "output_mode": "files_with_matches",
    "head_limit": 50, "offset": 0, "case_insensitive": false, "include_hidden": false
  },
  "files": ["src/lib.rs", "src/agent_loop/run.rs"],
  "matches": null,
  "counts": null,
  "stats": { "scanned_files": 1284, "elapsed_ms": 312 },
  "truncated": false,
  "next_offset": null,
  "warnings": [
    "implementation=tier2 rust-fallback; regex dialect is Rust regex and may differ from ripgrep; .gitignore/.ignore are respected by default"
  ]
}
```

---

## 4. One-Glance Map（文件职责总览）

```text
┌──────────────────────────────────────────────────────────────┐
│ src/api/cli/chat_cmd.rs            ── 入口                   │
│  • fn run_chat(resume, cfg)                                  │
│  ★ 启动后台预检：preflight::start_search_tools_preflight     │
│  • 不阻塞 chat_loop，事件经 EventBus 推到 stderr             │
└──────────────────────────────────────────────────────────────┘
                 │ tokio::spawn / std::thread::spawn
                 ▼
┌──────────────────────────────────────────────────────────────┐
│ src/api/chat/preflight.rs          ── 后台预检               │
│  • start_search_tools_preflight(cfg, bus)                    │
│  • should_skip_preflight: env > config > 默认 true           │
│  • missing_search_tools: find_binary("rg"/"fd"/"fdfind")     │
│  • Unix：nohup 重定向 + sh -c spawn，不等待安装结束；         │
│    并发仅进程表（Homebrew 窄匹配），可选 log-path marker     │
│  • Windows：Command::output() 阻塞；v1 无 detached           │
│  • 日志：Unix 由 shell 实时写 preflight-file-log-*.log；     │
│    Windows 由 pi 在 output 后汇总写入                        │
│  • tracing target：pi_wasm_preflight（RUST_LOG=debug）       │
│  • install_plan: cfg!(target_os) + TERMUX_VERSION 决策       │
│      macOS  → brew install ripgrep fd                        │
│      Win    → winget install (UAC 弹窗为系统行为)            │
│      Linux  → apt-get / dnf / pacman / zypper                │
│      Termux → pkg install ripgrep fd                         │
│      Android (非 Termux) → 不自动装；走 Tier2                │
│  • 事件 wire::WIRE_SEARCH_TOOLS_PREFLIGHT                    │
└──────────────────────────────────────────────────────────────┘
                 │ 事件
                 ▼
┌──────────────────────────────────────────────────────────────┐
│ src/api/chat/events/stderr.rs      ── CLI/TUI 反馈           │
│  • 监听 WIRE_SEARCH_TOOLS_PREFLIGHT，向 stderr 输出          │
│    ready/start/progress/success/failed 与 detached/        │
│    already_installing（灰字 + log + tail 提示）              │
└──────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────┐
│ src/core/tools/primitive/executor.rs ── 调度层               │
│  • find_binary(&[...])  ★ 即时读 PATH（无缓存）              │
│  • async fn search_files(args, ctx)                          │
│  ★ 缺一策略：rg/fd 任一缺 → 该 target 走 Tier2；都缺 → Tier2 │
│  • 写 audit.implementation = tier1 | tier2                   │
│  • Tier2 入 tokio::task::spawn_blocking，不阻塞 runtime      │
└──────────────────────────────────────────────────────────────┘
                 │ 失败 / 缺二进制
                 ▼
┌──────────────────────────────────────────────────────────────┐
│ Tier2: search_files_fallback + collect_fallback_files        │
│  • ignore::WalkBuilder（默认遵守 .gitignore / .ignore）      │
│  • filter_entry 阶段对 deny 路径剪枝（避免越权 IO）          │
│  • globset：路径匹配；regex (RegexBuilder)：内容匹配         │
│  • 大文件 5 MiB / 二进制 NUL 嗅探 → 跳过 + warning           │
│  • 单查询墙钟 10s（PI_SEARCH_TIER2_DEADLINE_MS 可覆盖）      │
│  • regex 编译失败 → 空命中 + warning（不 panic / 不 Err）    │
└──────────────────────────────────────────────────────────────┘
                 │
                 ▼
┌──────────────────────────────────────────────────────────────┐
│ src/core/tools/primitive/types.rs   ── 同一 schema           │
│  • SearchFilesArgs / SearchFilesOutput / SearchFilesQuery /  │
│    SearchFilesStats / SearchFileMatch / SearchFileCount      │
│  ★ Tier2 复用同一结构，差异写 warnings；不开新工具名         │
└──────────────────────────────────────────────────────────────┘
                 ▲
                 │ catalog 派生
┌──────────────────────────────────────────────────────────────┐
│ src/core/tools/catalog.rs           ── 单一事实源            │
│  • search_files 描述含「双实现 / .gitignore / warnings」     │
│  • build_tool_definitions() / docs/tool-catalog.md 派生不变  │
└──────────────────────────────────────────────────────────────┘
```

**阅读顺序（12 岁原则）**：用户进 chat（`chat_cmd.rs`）→ 后台偷偷装 `rg/fd`（`preflight.rs`，事件经 `stderr.rs` 显示）→ LLM 真要搜的时候由 `executor.rs` 决定走 Tier1（rg/fd）还是 Tier2 兜底（`search_files_fallback`）→ 不管走哪条，返回 JSON 都来自 `types.rs`，描述给 LLM 的话术来自 `catalog.rs`。

---

## 5. 调度时序（运行时图）

### 5.1 启动期：pi chat 后台预检

```text
 用户            CLI (chat_cmd)        EventBus       Preflight 线程     PATH/包管理器
  │  pi chat ──▶ │                       │                  │                    │
  │              │ 注册 stderr listeners ▶│                  │                    │
  │              │ start_preflight()      │                  │                    │
  │              │────── spawn ──────────────────────────────▶│                   │
  │              │                       │                  │ find_binary(rg/fd)  │
  │              │ 进入 chat_loop（不等）│                  │  ──── 缺失 ────────▶│
  │              ◀── prompt 立即返回 ────│                  │                    │
  │  >> 提问 ───▶│                       │ Started ◀────────│                    │
  │              │                  ◀────│                  │ spawn brew/pkg/... │
  │              │ stderr 渲染 Started   │                  │  ◀── stdout/err ───│
  │              │                  ◀────│ Progress ◀───────│                    │
  │  ...继续聊天 │                       │                  │                    │
  │              │                  ◀────│ Succeeded /Failed│                    │
  │              │ stderr 渲染最终状态   │                  │                    │
```

### 5.2 工具调用期：search_files 的 Tier 决策与回落

```text
 LLM             tool_exec            executor.search_files       Tier1 子进程        Tier2 (spawn_blocking)
  │ tool_call ──▶│                          │                          │                        │
  │              │ dispatch search_files ──▶│                          │                        │
  │              │                          │ find_binary(rg) -> Some  │                        │
  │              │                          │ find_binary(fd) -> Some  │                        │
  │              │                          │ tier1_warning            │                        │
  │              │                          │── target=content ───────▶│ rg ... → JSON          │
  │              │                          ◀── stdout/exit ───────────│                        │
  │              │                          │ audit.implementation=tier1                        │
  │              ◀── SearchFilesOutput ─────│                                                   │
  │ tool_result◀─│                                                                              │
  │                                                                                             │
  │              缺 rg 的另一种路径：                                                            │
  │              │                          │ find_binary(rg) -> None                           │
  │              │                          │ fallback_warning                                  │
  │              │                          │── target=content ──────────────────────────────────▶│ ignore::WalkBuilder
  │              │                          │                                                    │ + globset + regex
  │              │                          │                                                    │ deadline_hit?
  │              │                          ◀──── SearchFilesOutput (truncated/warnings)─────────│
  │              │                          │ audit.implementation=tier2                         │
  │              ◀── SearchFilesOutput ─────│                                                   │
```

---

## 6. Tier1 / Tier2 行为对照

| 维度 | Tier1（rg + fd） | Tier2（ignore::WalkBuilder + globset + regex） |
|------|------------------|-------------------------------------------------|
| `target=files` | `fd --glob`（或 `fdfind`） | `WalkBuilder` 遍历 + `globset` 匹配 |
| `target=content` | `rg` 正则 + 参数映射 | 文件 + `regex` 行级匹配 |
| `output_mode` | `--files-with-matches` / `--count` / 默认 content | 三种模式同款，输出结构一致 |
| `context` | `-C N` | **不支持**（写 warning） |
| `offset` | rg 输出后切片 | 按 `path+line` 稳定排序后切片 |
| `include_hidden` | `--hidden` | `WalkBuilder::hidden(!include_hidden)` |
| 正则方言 | ripgrep / Rust regex 生态 | Rust `regex` crate（**非 rg 超集**） |
| 不支持的正则 | rg 报错 | `regex::Error` → warning + 空命中，**不 panic、不 Err、不超时** |
| `.gitignore` | fd/rg 默认尊重 | **`ignore` crate 默认遵守** |
| 权限 / deny | gate + 结果过滤 | `filter_entry` 阶段剪枝（拒绝目录直接不递归）+ 叶子复检 |
| 大文件 / 二进制 | rg 流式跳过 | > 5 MiB 或前 8 KiB 含 NUL → 跳过 + warning |
| 墙钟 | rg 自身控制 | 默认 10s；`PI_SEARCH_TIER2_DEADLINE_MS=<ms>` 可覆盖；超时 `truncated=true` + warning |
| 输出结构 | `SearchFilesOutput` | **同一结构**，`warnings` 标 `implementation=tier2 rust-fallback` |
| 缺一策略 | — | 仅缺 `fd` → `target=files` 走 Tier2；仅缺 `rg` → `target=content` 走 Tier2；都缺 → 全 Tier2 |

---

## 7. 启动预检（pi chat）

| 点 | 说明 |
|----|------|
| 入口 | `chat_loop` 注册完 stderr 监听后调用 `preflight::start_search_tools_preflight(cfg, bus)`；与会话循环并行；不影响首屏 |
| 探测 | `find_binary("rg")` / `find_binary("fd")` / `find_binary("fdfind")`；即时读 `PATH`，无缓存 |
| 安装（Unix） | 后台线程内对包管理器使用 **`nohup … >> ~/.pi_/agents/main/logs/preflight-file-log-<ts>.log 2>&1 &`**，经 `/bin/sh -c` **`spawn`**；**不**阻塞等待安装结束，退出 `pi chat` / 结束 `pi` 后安装可继续 |
| macOS Homebrew | **`brew install --force-bottle ripgrep fd`**，且 detached shell 前缀 **`HOMEBREW_NO_BUILD_FROM_SOURCE=1`**：仅用 bottle，**禁止**从源码构建（避免 llvm 等超长后台编译）；无 bottle 时安装失败，会话仍可用 Tier2 |
| 安装（Windows） | v1 仍为 **`Command::output()` 阻塞**直至结束；detached 留代码 TODO（PowerShell `Start-Process -NoWait`） |
| 并发（Unix / Homebrew） | 仅依据 **进程表**（如 `pgrep -f` 匹配 `brew.rb` / Homebrew `build.rb` 等窄模式）判断是否已有安装/编译；为真则 emit `already_installing`，**不**再起第二套 nohup |
| log-path marker（UX） | 可选文件 `preflight-detached-log.marker`（与日志同目录）仅存**一行**本次 detached 日志绝对路径，便于 `already_installing` 时提示 `tail -f`；**从不**单独作为「正在安装」的判定依据 |
| 日志清理 | v1 **不**自动删除历史 `preflight-file-log-*.log`（保持实现简单） |
| 平台决策 | `cfg!(target_os)` + 运行期 `TERMUX_VERSION` env；详见 §9 决策树 |
| 事件 | `WIRE_SEARCH_TOOLS_PREFLIGHT`：`ready` / `start` / `progress`（主要为 Windows）/ `success` \|\| `failed`（Windows 阻塞路径）/ **`detached`** / **`already_installing`**（Unix 后台路径） |
| 取消 | 复用 `ctx.cancel_token`；Ctrl+C 软中断同时取消子进程 |
| 竞态 | 装完前用户调用 `search_files`：行为不变（缺一策略回落 Tier2） |
| sudo / 权限 | Linux 不抢 root；失败发事件，不静默 |
| Windows UAC | 系统行为不可绕过；用户取消归类 `failed`，不影响进程 |
| 关闭方式 | `[preflight] auto_install_search_tools = false` 或 `PI_SKIP_SEARCH_TOOLS_PREFLIGHT=1`（env > config > 默认 true） |

---

## 8. 预检事件状态机

实现侧 wire `payload.status` 字符串（非下图字面量）：`ready`、`start`、`progress`、`success`、`failed`、`detached`、`already_installing`。

```text
                       ┌──────────────────┐
                       │   not started    │
                       └─────┬────────────┘
              should_skip?   │ no
                ┌────yes─────┘
                ▼
          ╔══════════════╗
          ║   skipped    ║（探测仍发生，不安装）
          ╚══════════════╝

      missing_search_tools ──┐
                             ▼
                ┌──────────────────────────┐
                │       start             │  emit start（仍缺工具）
                └──────────────┬──────────┘
                               │
              ┌────────────────┴────────────────┐
              │ Unix（v1）                     │ Windows（v1）
              ▼                                ▼
   install_plan 有方案？              install_plan 有方案？
       │ no → failed                         │ no → failed
       │ yes                                 │ yes
       ▼                                     ▼
   Homebrew 安装中？                  progress → output() 阻塞
       │ yes → already_installing              │
       │ no                                  ┘
       ▼                                  success / failed
   spawn nohup + detached
       │
       └────────► chat 继续（Tier1 就绪后自动接管 / 否则 Tier2 兜底）

```

> 状态机不影响 `chat_loop`：所有节点都通过 `EventBus` 异步推送，主循环只负责渲染 / 不阻塞。

---

## 9. 平台决策树（桌面 / Termux / 普通 Android App）

```text
                       ┌────────────────────────┐
                       │ start_search_tools_    │
                       │   preflight(cfg, bus)  │
                       └──────────┬─────────────┘
                                  │
                  should_skip_preflight?
            (env PI_SKIP_... > cfg.auto_install)
                                  │
                ┌────── yes ──────┴──────── no ──────┐
                ▼                                    ▼
        ╔═══════════════╗                missing_search_tools()?
        ║  skip install ║                            │
        ╚═══════════════╝               ┌───── none ─┴── some ─────┐
                                        ▼                          ▼
                                ╔════════════╗     ┌──────────────────────────┐
                                ║ no-op done ║     │   match cfg!(target_os)  │
                                ╚════════════╝     └──────────┬───────────────┘
                                                              │
                ┌─────────────────────────┬────────┬──────────┼───────────┬──────────┐
                ▼                         ▼        ▼          ▼           ▼          ▼
             macos                     windows   linux+    linux+      android+    其它
                │                         │      TERMUX     非 Termux   非 Termux   (freebsd…)
                │                         │      VERSION                            │
                ▼                         ▼        ▼          ▼           ▼         ▼
        brew install            winget install   pkg install   /etc/os-     不自动装  仅探测
        ripgrep fd              ripgrep fd       ripgrep fd    release →    走 Tier2 不安装
                                (UAC 弹窗)                     apt/dnf/
                                                              pacman/zypper
                │                         │        │          │           │         │
                └────────────┬────────────┴────────┴──────────┴───────────┴─────────┘
                             ▼
                  Unix：start → detached / already_installing / failed（不阻塞）
                  Windows：start → progress → success / failed（阻塞至 output 返回）
                  (chat_loop 永远不被预检线程阻塞)
```

| 形态 | `search_files` 主路径 | 预检 / 安装 |
|------|------------------------|-------------|
| **macOS / Linux / Windows** | 用户已装则 Tier1，否则 Tier2 | brew / winget / apt / dnf / pacman / zypper |
| **Termux** | 优先 Tier1；缺则 Tier2 | `pkg install ripgrep fd` |
| **普通 Android App**（沙箱 APK） | 直接 Tier2 | **不自动装**，仅 warn 或后续后台解压内置二进制 |

运行期 Termux 判定：

- 优先：`std::env::var("TERMUX_VERSION").is_ok()`。
- 兜底：构建 feature `termux`（编译期强制走 Termux 分支）。
- `/data/data/com.termux` 路径探测、`runtime_profile` 等增强**仅留 TODO**，不阻塞首版。

---

## 10. 配置与环境变量

```toml
[preflight]
# 默认 true：进入 pi chat 时缺 rg/fd 后台尝试安装
auto_install_search_tools = true
```

| 变量 | 取值 | 含义 | 优先级 |
|------|------|------|--------|
| `PI_SKIP_SEARCH_TOOLS_PREFLIGHT` | `1` / `true` | 跳过后台**安装**（探测仍发生） | env（最高） |
| `[preflight] auto_install_search_tools` | `bool` | 开/关后台安装 | config |
| `PI_SEARCH_TIER2_DEADLINE_MS` | 整数毫秒 | Tier2 单查询墙钟覆盖（默认 10000） | env |

> 优先级总则：**env > config > 默认**。CI 镜像建议显式设置 `PI_SKIP_SEARCH_TOOLS_PREFLIGHT=1`，避免拉包卡住流水线（即便不设也不会阻塞，最多多发几条事件）。

---

## 11. 错误模型 / 截断 / 警告

```text
                   search_files 调用 → 五种归一化结局
   ┌───────────────────────────────────────────────────────────────────────┐
   │ 1. 正常返回（Tier1）                                                  │
   │      warnings += "implementation=tier1 rg/fd"                          │
   │ 2. 正常回落（Tier1 缺 → Tier2）                                       │
   │      warnings += "implementation=tier2 rust-fallback; 方言/.gitignore" │
   │ 3. 截断（head_limit 命中）                                            │
   │      truncated=true, next_offset=Some(n), warnings 描述截断原因        │
   │ 4. 截断（Tier2 墙钟到点）                                              │
   │      truncated=true, warnings 含 "wall-clock budget exhausted"        │
   │ 5. 容忍错误（regex 编译失败 / 大文件 / 二进制 / deny）                │
   │      命中集合可能为空，但 query/stats/warnings 完整，**不抛 Err**     │
   └───────────────────────────────────────────────────────────────────────┘

   何时返回 Err（少数）：
     • `head_limit = Some(Some(0))` → 参数校验前置失败
     • path 越权（gate 阶段拒绝）
     • IO 致命错误（root 不存在等）
```

---

## 12. 测试矩阵（实现 ↔ 用例）

集成测试 [`tests/search_files_tests.rs`](../../../tests/search_files_tests.rs)：

| ID | 用例名 | 覆盖目标 |
|----|--------|----------|
| T1 | `test_search_files_target_files_uses_fd_glob` | Tier1 `target=files` 简单 glob |
| T2 | `test_search_files_content_files_with_matches_paginates_and_filters_denied` | Tier1 `target=content` + 分页 + deny |
| T3 | `test_search_files_tier2_count_and_deny` | Tier2 count + deny `filter_entry` 剪枝 |
| T4 | `test_search_files_content_lines_and_count_modes` | Tier1 三种 `output_mode` |
| T5 | `test_search_files_missing_binary_uses_tier2_content_fallback` / `test_search_files_missing_fd_uses_tier2_files_fallback` | 缺一策略：rg / fd 任一缺 → 该 target Tier2 |
| T6 | （由 `ignore` crate 默认行为覆盖） | `.gitignore` 仓库 Tier2 默认跳过 `target/` / `node_modules/` |
| T8 | `test_search_files_tier2_lookaround_returns_empty_with_warning` | regex 编译失败 → 空命中 + warning，**不 panic** |
| T9 | `test_search_files_tier2_skips_binary_and_large_files` | 二进制 NUL 嗅探 + > 5 MiB 跳过 + warning |
| T10 | `test_search_files_tier2_include_hidden_toggle` | `include_hidden=true/false` 与 Tier1 `--hidden` 对齐 |

预检相关单元测试（[`src/api/chat/tests/preflight_test.rs`](../../../src/api/chat/tests/preflight_test.rs)，经 `preflight.rs` 末尾 `#[path]` 挂载；见 `RUST_FILE_LINES_SPEC` §A.9）：

- `should_skip_preflight_when_config_disables_auto_install`
- `trim_for_event_limits_long_messages`
- （Unix）`nohup_shell_quotes_log_path_with_spaces`：`shell_words` 拼接 nohup 重定向路径；**brew** 计划含 `HOMEBREW_NO_BUILD_FROM_SOURCE=1` 与 `--force-bottle`
- （Unix）`nohup_shell_non_brew_has_no_homebrew_env_prefix`：非 brew 计划不注入 Homebrew 环境变量

配置加载测试（[`src/infra/config/tests/load_test.rs`](../../../src/infra/config/tests/load_test.rs)）：

- `load_config_accepts_preflight_section`

---

## 13. 历史决策（已被本方案取代）

- ~~双工具名 `search_files_01 / _02`~~ → 否：合并为单工具 + 双实现，避免模型乱选。
- ~~chat 入口"按 y"强制确认~~ → 否：无确认、后台自动安装 + 事件反馈。
- ~~chat 入口 LLM/全网连通性探测~~ → 否：本地模型场景会被误杀；LLM 失败用运行时报错兜住。
- ~~只在 `pi init` 检查 / 安装 rg/fd~~ → 否：与 `search_files` 触达点错位；改为 `pi chat` 入口预检。
- ~~引入 `walkdir` crate~~ → 否：改用 `ignore` crate 的 `WalkBuilder`，自带 `.gitignore` 支持。
- ~~Tier2 超时返回 Err~~ → 否：改为 `truncated=true` + warning，与分页截断同源处理。

---

## 14. 关联文档

- 计划：[`/Users/yankeben/.cursor/plans/search_files_兜底选型_c8b4a778.plan.md`](../../../../.cursor/plans/search_files_兜底选型_c8b4a778.plan.md)
- 工具目录：[`docs/tool-catalog.md`](../../../docs/tool-catalog.md)
- 用户指南：[`docs/user-guide.md`](../../../docs/user-guide.md)
- 看板：[`agents/TASK_BOARD_002.md`](../../../agents/TASK_BOARD_002.md) T2-P0-005
- 跨 Agent 工具描述报告：[`docs/reports/builtin-tool-description-cross-agent-study.md`](../../../docs/reports/builtin-tool-description-cross-agent-study.md)
- Cursor 内置工具参考：[`docs/reports/cursor-builtin-tools-reference.md`](../../../docs/reports/cursor-builtin-tools-reference.md)
- 相关架构：[`permission-system.md`](permission-system.md)（gate / deny 规则）、[`audit-log.md`](audit-log.md)（审计 `implementation` 字段）、[`interrupt-and-cancellation.md`](interrupt-and-cancellation.md)（取消令牌）
