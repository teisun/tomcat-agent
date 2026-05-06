# `read` 工具：分页、去重、多模态与陈旧检测

本文档是内置工具 **`read`** 的冻结版技术方案（OpenSpec **B 类**：`openspec/specs/architecture/tools/`），承接看板子项 **T2-P0-tools-read**（[`TASK_BOARD_002.md`](../../../../agents/TASK_BOARD_002.md)）与计划 [`strengthen-read-tool_92f396c7.plan.md`](../../../../../.cursor/plans/strengthen-read-tool_92f396c7.plan.md)、[`tools-read-spec-migration_cb4d7b57.plan.md`](../../../../../.cursor/plans/tools-read-spec-migration_cb4d7b57.plan.md)。**实现以仓库代码为准**；计划文档保留讨论过程与 PR 治理顺序，本文只保留**已定稿的行为与契约**。

---

## 目录

- [1. 目标与设计原则](#1-目标与设计原则)
- [2. 竞品 / 选型对比](#2-竞品--选型对比)
  - [2.1 Agent 读文件的典型关切](#21-agent-读文件的典型关切)
  - [2.2 常见实现横向对比](#22-常见实现横向对比)
  - [2.3 落地选型决策表](#23-落地选型决策表)
  - [2.4 实施点（已闭环）](#24-实施点已闭环)
- [3. 术语统一](#3-术语统一)
- [4. 协议（入参 / 出参 / Schema）](#4-协议入参--出参--schema)
- [5. One-Glance Map（文件职责总览）](#5-one-glance-map文件职责总览)
- [6. 调度时序（运行时图）](#6-调度时序运行时图)
- [7. 状态机与会话表](#7-状态机与会话表)
- [8. 配置与环境变量](#8-配置与环境变量)
- [9. 错误模型 / 截断 / Stub](#9-错误模型--截断--stub)
- [10. 测试矩阵（验收）](#10-测试矩阵验收)
- [11. 风险与应对](#11-风险与应对)
- [12. 历史决策（已被本方案取代）](#12-历史决策已被本方案取代)
- [13. 关联文档](#13-关联文档)
- [附录：旧节号 → 本版对照](#附录旧节号--本版对照)

---

## 1. 目标与设计原则

**一句话**：让模型在本地读盘时 **可控体量、可读错误、可续读、少重复刷屏**，并在改文件前有机会发现「磁盘已变」——而不是把整份大文件或裸二进制直接倒进上下文。


| 原则（可观察）            | 说明                                                                                                     |
| ------------------ | ------------------------------------------------------------------------------------------------------ |
| **单名对外**           | 内置 catalog 仅注册 `read`；`read_file` 得到与拼错名一致的**未知工具**类错误                                                 |
| **窗口可控**           | `offset`（1-based 行）+ `limit`（1..=10000，默认 2000）；截断时正文尾附带 `resume with offset=<next>, limit=<same>`     |
| **裸读有上限**          | 未传窗口时 `metadata().len()` 超过文本上限（默认 25 MiB）→ 结构化错误并提示分窗；**已传** `offset` 或 `limit` 时可绕过（允许只读大文件的一小段）     |
| **二进制可诊断**         | 非 UTF-8 文本路径 → `AppError::Tool`，文案含 first-byte 十六进制与可执行建议（如 `bash file`），避免裸 `invalid utf-8`           |
| **行级可定位**          | 默认 `cat -n`（6 格右对齐行号 + Tab）；`hashline=true` 时为 `行号#双字符哈希:正文`（与 `line_numbers` 互斥，**hashline 优先**）      |
| **多模态走 OpenAI 约束** | 图 / PDF：`tool` 消息里占位句；真实 `InputImage` / `InputFile` 注入**下一条** `user` 的 `Parts`（`role=tool` 不接受图像 part） |
| **会话去重**           | 同一 `(path, offset, limit)` 且磁盘 `mtime+size` 未变 → 第二次起返回 `FILE_UNCHANGED` 短 stub                        |
| **陈旧检测底座**         | `read_state` 存上次成功 read 的指纹；`write` / `edit` 入口可比对，防止按旧上下文误改                                           |


### 1.1 观察指标表（与 §10 验收一一对应）


| 目标            | 观察指标（落地后可核对）                                           |
| ------------- | ------------------------------------------------------ |
| G1 工具名统一      | catalog 仅 `read`；`read_file` → 未知工具错误                  |
| G2 大文件可控      | `offset` + `limit`；截断带续读尾注                             |
| G3 裸读有上限      | 无窗口超 `max_bytes` → 结构化错误；有窗口可绕过                        |
| G4 二进制可诊断     | Tool 错误含 hex 与建议                                       |
| G5 行级可定位      | `cat -n` 或 hashline 二选一渲染                              |
| G6 多模态 inline | magic + 扩展名路由；图 4.5 MiB、PDF 25 MiB 在 **metadata 阶段**拒绝 |
| G7 OpenAI 路径  | 图 / PDF 占位 + 下一条 `user` 注入                             |
| G8 会话去重       | 同窗口未变 → `FileUnchanged` stub                           |
| G9 陈旧检测       | `read_state` 供写改前比对                                    |


### 1.2 非目标


| 非目标                         | 说明                                     |
| --------------------------- | -------------------------------------- |
| 服务端缩放图片                     | 不引入 `image` crate；大图由上游或用户预处理          |
| PDF 文本抽取 / Notebook         | 不解码 PDF 为文本；不解析 `.ipynb`               |
| `read_file` 运行时别名           | 不重定向；历史回放仅 warn（见代码注释）                 |
| Anthropic `tool_result` 内嵌图 | 当前主线为 OpenAI Responses；Anthropic 另接时再扩 |


---

## 2. 竞品 / 选型对比

对标过 pi-mono、pi_agent_rust、openclaw、hermes、cc-fork 等读文件策略。下列表格为 **已写入代码的决策**，不是待办 brainstorm。

### 2.1 Agent 读文件的典型关切

本地 `read` 类工具通常要同时解决四类问题：**体量**、**编码与类型**、**模型重复调用**、**写改一致性**。本方案用 **offset/limit + metadata 门 + read_state（mtime/size 快路径）+ 下一条 user 注入多模态** 四条线分别收口。

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  本地 read 工具通常要同时解决的四类问题                                    │
├────────────────────┬─────────────────────────────────────────────────────┤
│  体量              │  整文件进上下文 → OOM / 费 token → 需要分页与硬上限      │
│  编码与类型        │  UTF-8 文本 vs 二进制 vs 图 / PDF → 路由与可读错误    │
│  模型行为          │  重复 read 同一窗口 → 需要软 dedup，而非误伤合法重试   │
│  写改一致性        │  文件在磁盘已变 → 需要指纹，供 edit/write 前陈旧拦截   │
└────────────────────┴─────────────────────────────────────────────────────┘
```

### 2.2 常见实现横向对比


| 来源 / 形态                | 分页与上限                  | 行号 / 锚点               | 重复读                   | 多模态                                 | 备注                      |
| ---------------------- | ---------------------- | --------------------- | --------------------- | ----------------------------------- | ----------------------- |
| **cc-fork 系**          | 大行数窗口 + 续读提示           | 常见 `cat -n`           | 软 stub 省 token        | 视产品而定                               | 工程上验证「窗口默认 ~2k 行」较稳     |
| **pi_agent_rust**      | 类似 Agent 读盘            | **hashline**（xxh 短指纹） | 可与 edit 对齐            | 依部署                                 | 本仓库 **hashline 算法对齐**   |
| **Claude / Cursor 内置** | 产品化分页策略                | 因产品而异                 | 通常由宿主去重               | 强多模态                                | 协议细节不公开处用「占位 + 侧信道」思路参照 |
| **本仓库 `read`**         | 默认 2000 行 + 25 MiB 裸读门 | `cat -n` 或 hashline   | `FILE_UNCHANGED` stub | OpenAI：**tool 占位 + 下一条 user Parts** | wasm 友好、单测锁行为           |


### 2.3 落地选型决策表


| 决策点         | 默认选择                             | 主要替代方案              | 选择理由（为何不是替代）                                            |
| ----------- | -------------------------------- | ------------------- | ------------------------------------------------------- |
| 工具对外名       | 仅 `read`                         | `read_file` 并存或别名   | 单名减少双轨、审计与 prompt 分叉；历史调用用 transcript fallback **warn** |
| 分页默认上限      | 2000 行                           | 512 / 整文件           | 与「一屏 + α」及 cc-fork 实践一致；整文件易炸 wasm 堆                    |
| 裸读字节上限      | 25 MiB（`[tools.read] max_bytes`） | 256 KB / 100 MiB    | 比过小阈值实用；比过大上限省内存                                        |
| 行号默认        | `cat -n` 6 格 + Tab               | 无前缀 / LSP 风格        | 与 IDE、diff、人工扫读一致                                       |
| 同窗口重复读      | 软 stub（`FileUnchanged`）          | 硬拒绝或全文重发            | 省 token；合法「再确认」仍可得全文（磁盘变则重读）                            |
| dedup 判定    | `mtime_ms + size` 快路径            | `content_hash` 参与命中 | 后者会迫使短路前再读全文，与省读目标矛盾；hash 仍存作诊断与 edit 纵深                |
| 图片策略        | 不缩放、不解码；metadata 限长              | 引入 `image` 缩放       | 控制依赖与编译体积；上限对齐 OpenAI inline                            |
| hashline    | xxh32 + 双字符表                     | 无行级指纹 / MD5 前缀      | 与 pi_agent_rust 生态对齐、锚点短                                |
| 图 / PDF 进模型 | 注入下一条 `user`                     | 塞进 `role=tool`      | OpenAI API **不接受** tool 消息中的图像 part                     |


### 2.4 实施点（已闭环）

下列顺序与 [`TASK_BOARD_002.md`](../../../../agents/TASK_BOARD_002.md) **T2-P0-tools-read** 及 `strengthen-read-tool_92f396c7.plan.md` §6–§8 一致；**2026-05-05** 已全量合入主线（见看板 Changelog 行）。


| 实施点            | 交付范围                                                                                 | 主要代码落点                                      | 验收锚点（示例）                                                          |
| -------------- | ------------------------------------------------------------------------------------ | ------------------------------------------- | ----------------------------------------------------------------- |
| **PR-RA**      | `read_file` → `read` 命名统一；历史 transcript 仅 fallback warn                              | `catalog.rs`、`tool_exec`、未知工具路径             | `tool_exec_legacy_read_file_returns_unknown_tool_error`           |
| **PR-RB（T1）**  | `offset` / `limit`、二进制结构化 hint、memchr 单循环抽窗、`[tools.read] max_bytes` 默认 25 MiB       | `primitive/executor/read.rs`、`infra/config` | `read_window_test::*`、`read_with_offset_bypasses_max_bytes_check` |
| **PR-RF（T2）**  | `cat -n` 行号、`read_state`（`ReadStamp` / `ReadFileState`）、`FILE_UNCHANGED_STUB`、会话结束清理 | `read_state.rs`、`tool_exec.rs`              | `tool_exec_dedup_test::*`                                         |
| **PR-RJ-0**    | `image_b64` / `file_b64` 统一为 `(mime, &Path)`，metadata 白名单 + 读盘 base64 单点             | `types.rs`                                  | `src/core/llm/tests/types_test.rs`                                |
| **PR-RJ T3-a** | `ReadResult` 四态枚举（Text / Image / Pdf / FileUnchanged）                                | `primitive/types.rs`、wire 翻译                | `read_routes_*`                                                   |
| **PR-RJ T3-b** | PNG/JPEG/GIF/WebP/PDF magic 路由；metadata 阶段 `IMAGE_MAX_BYTES` / `FILE_MAX_BYTES` 拒绝   | `executor/read.rs`、`types.rs`               | `read_oversize_image_rejected_at_metadata_stage`                  |
| **PR-RJ T3-c** | `tool_exec` 返回可携带 `Vec<ChatMessageContentPart>`；`tool_dispatcher` 注入**下一条** `user`   | `tool_exec.rs`、agent loop 调度                | `tool_exec_image_result_injects_into_next_user_message_parts` 等   |
| **PR-RM**      | `hashline: bool`（xxh32）；与 `line_numbers` 互斥且 **hashline 优先**                         | `executor/read.rs`、`catalog`                | `read_with_hashline_renders_hash_prefixed_lines`                  |
| **PR-RS**      | 文档合入                                                                                 | 本 `read.md` 等                               | 不计代码 PR                                                           |


集成与并发组登记见 `tests/read_tool_tests.rs`、`scripts/test-groups.sh`（看板条目中已列门禁结果）。

下文按实施点展开**技术要点与示意图**；**交付边界与代码落点仍以表为准**，避免与表冲突。

#### 2.4.1 PR-RA：对外单名 `read`

- **交付**：catalog / system_prompt / 相关字面量统一为短名 `read`；`tool_exec` 仅匹配 `"read"`；`"read_file"` 走**未知工具**路径，语义与拼错工具名一致。
- **历史回放**：`session` 侧用 `OnceLock` 对 legacy 名打 **`tracing::warn`**（不重定向、不静默改写调用），避免双轨审计。

```text
  LLM / transcript
        │
        ▼
┌───────────────────┐     注册名仅 "read"
│  catalog.rs       │──────────────────────────────┐
└───────────────────┘                              │
        │                                            ▼
        ▼                               ┌────────────────────┐
  tool_exec  match "read"                │ "read_file" 等    │
        │                                │ → UnknownTool 错误 │
        ▼                                └────────────────────┘
   正常 read 路径
```

#### 2.4.2 PR-RB（T1）：分页、二进制 hint、流式抽窗与裸读上限

- **`offset` / `limit`**：1-based 行窗口；截断时在正文尾附带续读提示（见 §4、§9）。
- **流式与内存**：分块读盘 + `memchr` 找换行，**单循环**抽出窗口内行，避免先把整文件读进 `String` 再切行（wasm / 大文件友好）。
- **`[tools.read] max_bytes`**：默认 25 MiB；**仅当** primitive 入参里 **`offset` / `limit` 均未显式出现**（`has_window = offset.is_some() || limit.is_some()` 为假）时，在 metadata 阶段用 `len()` 拒绝过大文本路径；**显式传 `offset` 或 `limit` 之一**即可绕过，用于「只窥一角」读大文件。
- **二进制 / 非 UTF-8**：返回结构化 `AppError::Tool`，带首字节 hex 与运维向建议，避免裸 `invalid utf-8` 污染模型上下文。

```text
  open(path)
      │
      ▼
 metadata.len()  +  has_window?
      │
      ├─ has_window=false 且 len > max_bytes ──▶ Tool Err（提示 offset/limit）
      │
      └─ 否则 ──▶ 分块读 + memchr ──▶ 窗口内 UTF-8 文本 + 行号/hashline
```

#### 2.4.3 PR-RF（T2）：`cat -n`、会话表与 `FILE_UNCHANGED`

- **行号**：`format_with_line_numbers` → `{:>6}\t{content}`，默认 `line_numbers=true`。
- **`read_state.rs`**：`ReadStamp { mtime, size, content_hash, offset, limit, is_partial_view }` + `ReadFileState`（`RwLock<HashMap<PathBuf, ReadStamp>>`）+ 常量 `FILE_UNCHANGED_STUB`。
- **挂载**：`Arc<ReadFileState>` 挂在 `AgentLoopConfig` / `ChatContext`，跨轮复用；会话结束清理，避免表无限涨。
- **dedup**：`tool_exec` 在调 primitive 前查表；命中且磁盘指纹未变 → 直接 `ReadResult::FileUnchanged`（**不调** executor）。

```text
        read 请求 (path, offset, limit)
                    │
                    ▼
            ReadFileState.lookup
                    │
         ┌──────────┴──────────┐
         ▼                     ▼
    mtime/size 变          key 命中且未变
         │                     │
         ▼                     ▼
   executor/read          FileUnchanged stub
   + put_stamp             （不调 primitive）
```

#### 2.4.4 PR-RJ-0 与 T3-a / b / c：多模态与 OpenAI 注入边界

- **PR-RJ-0**：`ChatMessageContentPart::image_b64` / `file_b64` 统一为 **`(mime, &Path)`**（及 `file_b64` 的文件名参数）：helper 内 **metadata 二次校验 + 读盘 + base64**，避免 read 与 LLM 客户端重复 IO、重复校验。
- **T3-a**：`ReadResult` 四态 **`Text` / `Image` / `Pdf` / `FileUnchanged`**；primitive 只产出前三者，`FileUnchanged` **仅** `tool_exec` 构造。
- **T3-b**：`detect_inline_mime`（magic + 扩展名）路由 PNG/JPEG/GIF/WebP/PDF；在 metadata 阶段用 `IMAGE_MAX_BYTES` / `FILE_MAX_BYTES` 拒绝超限，**不**加载全字节。
- **T3-c**：`tool_exec` 返回值升级，可附带 `Vec<ChatMessageContentPart>`；`tool_dispatcher` 在 tool 消息之后向**下一条 user**追加 image/file part——对齐 OpenAI「tool 里不能塞图」的硬约束。

```text
  ReadResult::Image | Pdf
            │
            ├─▶ role=tool 文本：短占位说明（无 binary part）
            │
            └─▶ 紧随其后的 role=user：Parts += InputImage / InputFile
```

#### 2.4.5 PR-RM：`hashline`（xxh32）

- **依赖**：`xxhash-rust`（`xxh32`）。
- **算法**：行内容经 whitespace-stripped 后做 xxh32，取 nibbles 映射为**双字符**指纹前缀；渲染 `{:>6}#XX:{content}`。
- **优先级**：`hashline=true` 时 **覆盖** `line_numbers`（schema、system prompt、executor 分支一致）。

```text
  line_numbers=true, hashline=false     →  "    42\tcode"
  hashline=true（优先）                 →  "    42#Ab:code"
```

#### 2.4.6 PR-RS：文档

- 将冻结 spec 合入 `openspec/specs/architecture/tools/read.md`，并与 tool catalog、看板、集成测试登记交叉引用（见 §13）。

---

## 3. 术语统一


| 术语                      | 语义（人话）                                                     | 数据载体                                                                    | 行为约束                                                                                                  |
| ----------------------- | ---------------------------------------------------------- | ----------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| **窗口**                  | 从第几行开始、最多读几行                                               | `offset: Option<u64>`, `limit: Option<u64>`                             | `offset` 缺省等价 1；`limit` 缺省等价默认 2000；**显式** `limit` 才参与「是否分窗」判定（与 `read_state` 的 `is_partial_view` 一致） |
| **dedup（重复读短路）**        | 同一窗口、文件看起来没变，就不再塞全文                                        | [`read_state::ReadFileState`](../../../../src/core/tools/pipeline/read_state.rs) | 命中 → `ReadResult::FileUnchanged`（在 **tool_exec** 短路，不调 primitive）                                     |
| **staleness（陈旧）**       | 模型脑中的内容已不是磁盘最新                                             | 同上表中的 `ReadStamp`                                                       | **edit/write 入口**查表：指纹不一致 → 拒绝并要求先 `read`                                                             |
| **FILE_UNCHANGED stub** | 告诉模型「别读了，跟上一次一样」                                           | 常量 [`FILE_UNCHANGED_STUB`](../../../../src/core/tools/pipeline/read_state.rs)    | 非错误；模型应引用上一轮 read 结果                                                                                  |
| **hashline**            | 行号 + 内容指纹前缀，供精细 edit 锚点                                    | executor 文本渲染分支                                                         | `hashline=true` 时覆盖 `line_numbers`；算法对齐 pi_agent_rust                                                 |
| **「LLM 收到 tool 结果后」**   | 指 **`tool_exec` 已把 `ReadResult` 落成 chat 消息、即将进入下一轮模型推理之前** | —                                                                       | 与去重 / 注入时序讨论绑在此边界                                                                                     |


---

## 4. 协议（入参 / 出参 / Schema）

**单一事实源**：

- JSON Schema：[`catalog.rs::read_parameters`](../../../../src/core/tools/contract/catalog.rs) → `build_function_definitions()` → [`docs/tool-catalog.md`](../../../../docs/tool-catalog.md)。
- Rust 类型：[`primitive/types.rs`](../../../../src/core/tools/primitive/types.rs) 中 `ReadResult` / `ReadTextResult` / `ReadBinaryResult`。

### 4.1 入参（工具 arguments）


| 字段             | JSON 类型           | 必填    | 默认      | 说明                                    |
| -------------- | ----------------- | ----- | ------- | ------------------------------------- |
| `path`         | string            | **是** | —       | 绝对或相对路径；经 `PermissionGate` Read       |
| `offset`       | integer ≥ 1       | 否     | 1       | 从第几行开始读（1-based）                      |
| `limit`        | integer 1..=10000 | 否     | 2000    | 最多返回多少行；截断则附续读尾注                      |
| `line_numbers` | boolean           | 否     | `true`  | `cat -n` 风格前缀；与 `hashline` 互斥         |
| `hashline`     | boolean           | 否     | `false` | `行号#XX:内容`；开启时 **优先于** `line_numbers` |


### 4.2 出参（Rust：`ReadResult`）

判别式枚举四种结局（wire / UI 再序列化）：

```text
ReadResult
├── Text(ReadTextResult)
│     • content      — 已带行号或 hashline、可能带截断尾注的最终字符串
│     • start_line   — 窗口起始行号（1-based）
│     • num_lines    — 本响应实际行数
│     • truncated    — 是否因 limit 截断
│     • remaining_lines — 截断时后面还剩多少行；未截断为 0
├── Image(ReadBinaryResult)   — mime + size + path + filename（primitive 不 base64）
├── Pdf(ReadBinaryResult)     — 同上，mime 为 application/pdf
└── FileUnchanged { path }    — 仅 tool_exec dedup 路径构造；primitive **不**产出此变体
```

**Image / Pdf 与 helper**：`tool_exec` 用路径调用 `ChatMessageContentPart::image_b64(mime, &Path)` / `file_b64(filename, mime, &Path)` 完成读盘与 base64（[`types.rs`](../../../../src/core/llm/types.rs)）。

### 4.3 调用样例（jsonc）

**文本分页**：

```jsonc
{
  "path": "src/lib.rs",
  "offset": 1,
  "limit": 80
}
```

**精细锚点（hashline）**：

```jsonc
{
  "path": "src/lib.rs",
  "offset": 10,
  "limit": 40,
  "hashline": true
}
```

---

## 5. One-Glance Map（文件职责总览）

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/llm/system_prompt.rs                                             │
│  • 工具说明里使用短名 `read`，引导 offset/limit / hashline                  │
└────────────────────────────────────────────────────────────────────────────┘
        │
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/tools/contract/catalog.rs                                                 │
│  • BUILTIN_TOOL_CATALOG：`name = "read"`，`read_parameters()` JSON Schema   │
└────────────────────────────────────────────────────────────────────────────┘
        │
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/agent_loop/tool_exec.rs                                          │
│  • match `"read"`：解析 offset/limit/line_numbers/hashline，越界早失败       │
│  • dedup：命中 ReadFileState → 直接 FileUnchanged                           │
│  • ReadResult::Image|Pdf → 占位 tool 文本 + 注入下一条 user Parts            │
└───────────────────────────────┬────────────────────────────────────────────┘
                                │
                                ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/tools/primitive/executor/read.rs                                 │
│  • read / read_file_impl：gate → metadata 上限 → 路由 Text|Image|Pdf       │
│  • 文本：分块读 + memchr 找换行，单循环抽窗；UTF-8 校验；尾注；行号/hashline │
│  • 二进制拒绝：结构化 Tool 错误                                             │
└───────────────────────────────┬────────────────────────────────────────────┘
                                │
              ┌─────────────────┴──────────────────┐
              ▼                                    ▼
┌──────────────────────────────┐      ┌──────────────────────────────────────┐
│  src/core/tools/pipeline/read_state.rs │      │  src/core/llm/types.rs               │
│  • ReadFileState / ReadStamp   │      │  • image_b64 / file_b64(path 签名)    │
│  • put_stamp / check_stamp     │      │  • metadata 尺寸白名单 + 读盘 base64 │
└──────────────────────────────┘      └──────────────────────────────────────┘
              ▲
              │ Arc 挂在 AgentLoopConfig（见 src/api/chat/mod.rs 装配）
```

**怎么读这张图**：请求先进 **`tool_exec`**（参数与去重），再进 **`executor/read`**（真 IO 与渲染），会话指纹记在 **`read_state`**；图 / PDF 的昂贵编码只在 **`types.rs` helper** 做一次。

---

## 6. 调度时序（运行时图）

### 6.1 首次 read（文本窗口）

```text
LLM          tool_exec              executor/read           read_state
 │               │                       │                    │
 │ read(args)    │                       │                    │
 │──────────────>│ 校验 offset/limit      │                    │
 │               │──────────────────────>│ gate + open       │
 │               │                       │ 流式扫行/拼 content │
 │               │                       │───────────────────>│ put_stamp
 │               │<──────────────────────│ ReadResult::Text   │
 │<──────────────│ tool 消息文本         │                    │
```

### 6.2 同窗口第二次 read（dedup）{#sec-read-dedup}

```text
LLM          tool_exec                       read_state
 │               │                               │
 │ read(同路径同窗口) │ lookup：mtime+size 未变      │
 │──────────────>│──────────────────────────────>│
 │               │<──────────────────────────────│ hit
 │               │ 组装 FileUnchanged（不调 executor）
 │<──────────────│ stub 文本
```

### 6.3 edit 前陈旧检查（概念）

```mermaid
sequenceDiagram
    autonumber
    participant L as LLM
    participant E as tool_exec
    participant S as read_state

    L->>E: edit(path, ...)
    E->>S: check_stamp(path)
    alt mtime+size 等与 stamp 一致
        S-->>E: ok
    else 不一致
        S-->>E: stale
        E-->>L: 请重新 read
    end
```



---

## 7. 状态机与会话表

**dedup 命中**（简化条件，完整见 `ReadStamp::matches_request`）：

```text
           ┌─────────────┐
  首次 read │  no record  │
           └──────┬──────┘
                  │ put_stamp
                  ▼
           ┌─────────────┐
  再次 read │  key 对齐   │──mtime/size 变──▶ 全量重读 + 更新 stamp
           │  且磁盘未变 │
           └──────┬──────┘
                  │ 同窗口 + 未变
                  ▼
           ┌─────────────┐
           │ FILE_UNCHANGED stub
           └─────────────┘
```


| 字段 / 概念             | 作用                                         |
| ------------------- | ------------------------------------------ |
| `mtime_ms` + `size` | dedup 快路径：未变才允许短路                          |
| `content_hash`      | 存储供诊断与后续 hashline_edit；**dedup 命中不强制重算比对** |
| `is_partial_view`   | 分窗读与整文件读不互相 dedup                          |


---

## 8. 配置与环境变量

**总则**：`env > config > 默认`（若某 env 未实现则省略该行）。


| 来源               | 键                                               | 含义                | 备注                                                                                           |
| ---------------- | ----------------------------------------------- | ----------------- | -------------------------------------------------------------------------------------------- |
| `pi.config.toml` | `[tools.read] max_bytes`                        | 文本路径**无窗口**时的字节上限 | 默认 25 MiB；[`infra/config/types.rs`](../../../../src/infra/config/types.rs) `ToolsReadConfig` |
| 代码常量             | `IMAGE_MAX_BYTES` 等                             | 图 / PDF inline 上限 | [`types.rs`](../../../../src/core/llm/types.rs)                                              |
| 测试               | `DefaultPrimitiveExecutor::with_read_max_bytes` | 缩小阈值注入执行器         | 避免造 25 MiB fixture                                                                           |


`line_numbers` / `hashline` **不进** config：由模型按次决定，避免管理员静默改变模型上下文形状。

---

## 9. 错误模型 / 截断 / Stub

```text
                    read 请求
                        │
        ┌───────────────┼───────────────┐
        ▼               ▼               ▼
   参数非法         权限 / IO        文本非 UTF-8
   AppError::Tool   Permission/IO   AppError::Tool（结构化 hint）
        │               │               │
        └───────────────┴───────────────┘
                        │
        ┌───────────────┴───────────────┐
        ▼                               ▼
  裸读超 max_bytes                 limit 截断
  AppError::Tool（提示 offset/limit）  Ok(Text{truncated=true, 尾注})
        │
        ▼
  dedup 命中
  Ok(FileUnchanged) — 非错误
```

---

## 10. 测试矩阵（验收）


| 维度             | 用例（实际函数名）                                                                                                                                                                                                                                                                                                                                                                                    | 状态           |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------ |
| 分页 / 尾注        | `read_window_test::read_offset_limit_returns_window`、`read_offset_beyond_eof_returns_empty`、`read_limit_truncates_with_resume_hint`、`read_with_offset_bypasses_max_bytes_check`                                                                                                                                                                                                              | ✅ 2026-05-05 |
| 大文件 / 二进制 hint | `read_window_test::read_no_offset_large_file_rejected_with_hint`、`read_binary_returns_structured_hint`                                                                                                                                                                                                                                                                                       | ✅ 2026-05-05 |
| 行号 / hashline  | `read_window_test::read_default_renders_cat_n_line_numbers`、`read_offset_window_uses_absolute_line_numbers`、`read_with_hashline_renders_hash_prefixed_lines`                                                                                                                                                                                                                                 | ✅ 2026-05-05 |
| 路由 Image/Pdf   | `read_window_test::read_routes_png_to_image_variant`、`read_routes_pdf_to_pdf_variant`、`read_unknown_extension_falls_back_to_text`、`read_oversize_image_rejected_at_metadata_stage`                                                                                                                                                                                                           | ✅ 2026-05-05 |
| 集成（黑盒）         | `tests/read_tool_tests.rs`：`read_text_offset_limit_window_with_line_numbers`、`read_binary_returns_structured_hint`、`read_hashline_renders_two_char_hash_prefix`、`read_png_routes_to_image_and_can_build_input_image_part`、`read_pdf_routes_to_pdf_and_can_build_input_file_part`、`read_oversize_image_rejected_before_loading_bytes`                                                         | ✅ 2026-05-05 |
| tool_exec 参数   | `submodules_test::tool_exec_read_returns_content`、`tool_exec_legacy_read_file_returns_unknown_tool_error`、`tool_exec_read_offset_zero_returns_bound_error`、`tool_exec_read_limit_over_max_returns_bound_error`                                                                                                                                                                               | ✅ 2026-05-05 |
| dedup / 注入     | `tool_exec_dedup_test::tool_exec_read_second_call_returns_unchanged_stub`、`tool_exec_read_after_mtime_bump_refetches`、`tool_exec_read_partial_then_full_does_not_dedup`、`tool_exec_read_different_window_does_not_dedup`、`tool_exec_read_state_clear_resets_dedup`、`tool_exec_image_result_injects_into_next_user_message_parts`、`tool_exec_pdf_result_injects_into_next_user_message_parts` | ✅ 2026-05-05 |
| helper 签名      | `src/core/llm/tests/types_test.rs` 中 `image_b64` / `file_b64`                                                                                                                                                                                                                                                                                                                                | ✅ 2026-05-05 |
| 配置解析           | `infra/config/tests/tools_cfg_test.rs`（`[tools.read] max_bytes` 覆盖）                                                                                                                                                                                                                                                                                                                          | ✅ 2026-05-05 |


§1.1 观察指标表与本表可逐行对照（G1–G9）。

---

## 11. 风险与应对


| 风险                | 影响          | 应对（已实现或约定）                                             |
| ----------------- | ----------- | ------------------------------------------------------ |
| 模型坚持传 `read_file` | 工具调用失败      | catalog 仅 `read`；`submodules_test` 锁未知工具语义；prompt 写明短名 |
| 大文件 OOM           | wasm / 主机内存 | metadata 门 + 分块扫行；禁止整文件读后再判大小                          |
| OpenAI tool 消息塞图片 | API 拒收 / 浪费 | 图 / PDF **仅**注入下一条 `user` Parts；单测锁注入行为                |
| `mtime` 欺骗式不变     | 陈旧漏判        | 存 `content_hash` + hashline 给 edit 侧纵深；§7 写明边界         |
| 重复 read 烧 token   | 成本          | dedup stub；`ReadFileState` 按会话释放                       |


---

## 12. 历史决策（已被本方案取代）

- ~~整文件 `read_file_utf8` 唯一路径~~ → **否**：LLM 工具走 `read()` + 窗口 + `ReadResult`。
- ~~`read_file` 与 `read` 双注册~~ → **否**：避免双轨与审计分叉。
- ~~非 UTF-8 仅裸错误~~ → **否**：改为结构化 Tool 错误 + hex 提示。
- ~~图片在 primitive 内 base64~~ → **否**：统一到 `ChatMessageContentPart` helper，单点限长与白名单。
- ~~用 content_hash 做 dedup 命中条件~~ → **否**：会迫使「短路前再读一遍全文」，与省读目标矛盾；改用 `mtime_ms + size` 快路径（`read_state.rs` 头注释）。

---

## 13. 关联文档

- 兄弟工具：[search_files.md](search_files.md)
- 权限：[../permission-system.md](../permission-system.md)
- 派生工具目录：[../../../../docs/tool-catalog.md](../../../../docs/tool-catalog.md)
- 看板：[TASK_BOARD_002.md](../../../../agents/TASK_BOARD_002.md)（`T2-P0-tools-read`）

---

**一句话总结**：`read` 在 **`tool_exec`** 做参数与去重、在 **`executor/read`** 做流式窗口与渲染、在 **`read_state`** 记下指纹供后续写改校验；图 / PDF 走 **helper + 下一条 user 注入**；协议以 **`primitive/types.rs` + `catalog.rs`** 为单一事实源。

---

## 附录：旧节号 → 本版对照

仓库里部分 `//!` / 错误文案仍写「`read.md` §2.x / §3.2 / §4.x」（计划期编号）。本版按 [`ARCHITECTURE_SPEC.md`](../../guides/workflow/ARCHITECTURE_SPEC.md) 与 `search_files.md` 重排章节，按下表跳转即可。


| 旧锚点                           | 本版位置                                                                            |
| ----------------------------- | ------------------------------------------------------------------------------- |
| §0 / §0.A 对标与决策表              | [§2](#2-竞品--选型对比)                                                               |
| §1 命名切换                       | [§1](#1-目标与设计原则) 原则表 · G1                                                       |
| §2.1 分页 offset/limit          | [§4.1](#41-入参工具-arguments)、G2                                                   |
| §2.2 续读尾注                     | G2、[§9](#9-错误模型--截断--stub)                                                      |
| §2.3 二进制 hint                 | G4、[§9](#9-错误模型--截断--stub)                                                      |
| §2.4 分块流式                     | [§5 One-Glance](#5-one-glance-map文件职责总览)（`executor/read.rs`）                    |
| §2.5 metadata 上限              | [§8](#8-配置与环境变量)、G3、[§9](#9-错误模型--截断--stub)                                     |
| §2.6 tool_exec 参数校验           | [§4.1](#41-入参工具-arguments)；边界见 [§10](#10-测试矩阵验收) `tool_exec_read_*_bound_error` |
| §3.1 cat-n 行号                 | [§4.1](#41-入参工具-arguments) `line_numbers`、G5                                    |
| §3.2 readFileState、dedup、stub | [§3](#3-术语统一)、[§7](#7-状态机与会话表)、[§6.2](#sec-read-dedup)                          |
| §4.1 多模态路由                    | [§4.2](#42-出参rustreadresult)、G6                                                 |
| §4.2 tool→user 注入             | G7、[§6](#6-调度时序运行时图)                                                            |
| §4.3 / §4.4 hashline          | [§3](#3-术语统一)、[§4.1](#41-入参工具-arguments) `hashline`                             |
| 上一版「§7 选型摘要」                  | [§2.3](#23-落地选型决策表)                                                             |
| 上一版无独立「实施排期」节                 | [§2.4](#24-实施点已闭环)                                                         |


