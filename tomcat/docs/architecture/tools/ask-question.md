# `ask_question` 工具：PLAN 模式下的结构化提问

本文档是内置工具 **`ask_question`** 的冻结版技术方案（OpenSpec **B 类**：`docs/architecture/tools/`）。承接 [`plan-runtime.md`](../plan-runtime.md) 与 [`planner.md`](./planner.md)：**在 PLAN 与 CHAT/Pending/Completed 模式均可见**（EXEC 模式不可见，避免 agent loop 阻塞在用户输入上），让模型以「单选 / 多选」结构化方式向用户索要明确决策，避免 prompt 里塞自然语言提问后模型自己脑补答案。**实现以仓库代码为准**；本文只保留**已定稿的行为与契约**。

末列 **「说人话」** 与 [`ARCHITECTURE_SPEC.md`](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§14.1** 对齐。

**说人话**：让模型在规划或日常对话时能「弹个选择题」给用户，而不是自己猜。CHAT 与 PLAN 都可见；EXEC 隐藏（执行态由 agent loop 自动推进，不应阻塞）。

---

## 目录

- [1. 术语统一](#1-术语统一)
- [2. 竞品 / 选型对比（调研）](#2-竞品--选型对比调研)
- [3. 目标与设计原则](#3-目标与设计原则)
- [4. 落地选型与实施（已定稿）](#4-落地选型与实施已定稿)
- [5. 协议（入参 / 出参 / Schema）](#5-协议入参--出参--schema)
- [6. One-Glance Map](#6-one-glance-map文件职责总览)
- [7. 调度时序](#7-调度时序运行时图)
- [8. 状态机](#8-状态机)
- [9. 配置与环境变量](#9-配置与环境变量)
- [10. 错误模型 / 截断 / 警告](#10-错误模型--截断--警告)
- [11. 测试矩阵（验收）](#11-测试矩阵验收)
- [12. 风险与应对](#12-风险与应对)
- [13. 历史决策](#13-历史决策)
- [14. 关联文档](#14-关联文档)

---

## 1. 术语统一

| 术语 | 语义（人话） | 数据载体 | 行为约束 | 说人话 |
|------|--------------|----------|----------|--------|
| **`ask_question` 工具** | 让模型向用户结构化提问的内置 LLM 工具 | `BUILTIN_TOOL_CATALOG` 中 `name = "ask_question"` | `mode ∈ {Planning, Chat, Pending, Completed}` 时可见（EXEC 隐藏）；`isReadOnly = true` / `requiresUserInteraction = true` | 模型问问题，不写盘。 |
| **Question** | 一道结构化题目 | `{ id, prompt, options[], allow_multiple? }` | `id` 单次调用内唯一；`options.length ∈ [2, 4]`（不含 UI 兜底的 `__custom__` 槽）；`options` 中必须**恰好一个** `recommended: true` | 一道题最少 2 个最多 4 个选项，且 LLM 必须先指明推荐项。 |
| **Option** | 题目的一个候选答案 | `{ id, label, recommended? }` | `id` 题内唯一；`recommended` 缺省 `false`，每题恰好一项为 `true`；UI 在 `recommended=true` 的 label 后追加 `— 推荐` 后缀；LLM **不能**显式声明 `id = "__custom__"` 的选项 | 选项 id 单题内不能重；必须给一个推荐项。 |
| **`__custom__` 自定义槽（UI 兜底）** | UI 在每题末尾**自动追加**的固定空选项 | `{ id: "__custom__", label: "自定义…", recommended: false, allow_custom_input: true }` | 由 runtime / UI 注入，**不**进 schema、**不**进 LLM 可见 `options`；选中时回填 `custom_text: string`（非空，≤ 500 字符） | 永远多一个让用户自己填的空格。 |
| **AskQuestionResult** | 工具返回结构 | `{ answers: [{ question_id, option_ids, custom_text? }] }` | 与 Question 顺序一致；多选时 `option_ids.length >= 1`；选中 `__custom__` 时必带 `custom_text`，其它情况不带 | 题型决定答案是单选还是多选；自定义答案另带文本。 |
| **isReadOnly / requiresUserInteraction** | 工具元属性 | catalog 注册时 `is_read_only = true`、`requires_user_interaction = true` | runtime 据此判断是否纳入「写权限审计」与「打断式 UI」 | 不写盘但要等用户。 |

---

## 2. 竞品 / 选型对比（调研）

### 2.1 Agent 提问工具的典型关切

```text
┌──────────────────────────────────────────────────────────────┐
│  本地 ask 类工具通常要解决的三类问题                          │
├──────────────────────┬──────────────────────────────────┤
│  减少模型脑补        │  自然语言追问 → 模型自己回答自己   │
│  保证答案可机器消费  │  用户回答要能回到工具结果里        │
│  限制提问规模        │  避免一次抛 20 道题给用户          │
└──────────────────────┴──────────────────────────────────┘
```

**说人话**：让模型问得有上限、问得结构化、问完真有人回答。

### 2.2 常见实现横向对比

| 来源 / 形态 | 工具名 | 题数上限 | 选项上限 | 多选 | 自定义输入槽 | 推荐项 | 可见时机 | 说人话 |
|-------------|--------|----------|----------|------|---------------|--------|----------|--------|
| **cc-fork-01** | `AskUserQuestion` | 4 | 4 | 支持 | 无 | 无 | PLAN 模式 | 4×4 是观察来的稳定上限。 |
| **claude-code 系** | 无独立工具 | — | — | — | — | — | 通过自然语言追问 | 退化方案，易脑补。 |
| **Cursor 内置** | `AskQuestion` | 多题 | 数个 | 支持 | **有**（每题自带「Custom answer」输入框） | 无 | 大多任务时机 | UI 永远多一个让用户自填的槽。 |
| **本仓库 `ask_question`** | `ask_question` | 4 | 4（+ UI `__custom__` 兜底） | 支持 | **有**（沿用 Cursor 风格：UI 端固定追加 `id="__custom__"`） | **强制每题恰好一个** `recommended: true` | **仅 `mode == Planning`** | cc-fork-01 题数 + Cursor 自定义槽 + 强制推荐项。 |

### 2.3 维度词典

| 维度 | 关切 | 说人话 |
|------|------|--------|
| Q1 题数上限 | 一次最多几题 | 4 是观察上限。 |
| Q2 选项数 | 单题 2-4 个 | 1 个不是选项题，5+ 烦人。 |
| Q3 单选/多选 | `allow_multiple: bool` | 默认单选；多选要显式开。 |
| Q4 可见时机 | 是否在执行态也开放 | 只 PLAN，执行态绝不开。 |
| Q5 阻塞 vs 非阻塞 | 等待 UI 回填 | 阻塞 await，但不占网络。 |
| Q6 与 transcript 关系 | 题目和答案是否进 transcript | 是，作为 plan.ask_question 事件落盘。 |
| Q7 自定义输入槽 | 是否允许用户在 LLM 给定选项外自己填 | UI 自动追加 `__custom__` 兜底，沿用 Cursor 习惯。 |
| Q8 推荐项 | 是否强制 LLM 先在选项里表态 | 每题恰好一个 `recommended: true`，UI 加「— 推荐」后缀。 |

---

## 3. 目标与设计原则

| ID | 目标 | 验证手段（§11） | 说人话 |
|----|------|------------------|--------|
| G1 | `mode ∈ {Planning, Chat, Pending, Completed}` 进 catalog（EXEC 隐藏） | `ask_question_visible_in_planning_and_chat`、`ask_question_hidden_in_executing` | CHAT/PLAN/Pending/Completed 都能弹；EXEC 隐藏不阻塞。 |
| G2 | 单次调用 `questions.length ∈ [1, 4]`；每题 LLM 给出 `options.length ∈ [2, 4]`；UI 兜底自动追加 1 个 `__custom__` 槽 | `ask_question_schema_bounds`、`ask_question_ui_appends_custom_slot` | 最多 4 题、每题 2–4 选项 + 1 个用户自定义槽。 |
| G3 | `requires_user_interaction = true`，工具 await 用户答复 | `ask_question_blocks_until_answered` | 必须等用户点完才返回。 |
| G4 | 题目 + 答案落 transcript 自定义事件（含 `custom_text` 与 `recommended` 标记） | `ask_question_emits_transcript_event` | 问答要进 transcript 方便回放。 |
| G5 | 用户中断（Ctrl+C）→ 返回 `cancelled` 而非 hang | `ask_question_handles_user_abort` | 用户取消不算工具 error。 |
| G6 | 每题 `options` 必须**恰好一个** `recommended: true`；缺失或多于一个 → tool error；LLM 也不允许把 `id` 设为保留值 `__custom__` | `ask_question_requires_exactly_one_recommended`、`ask_question_rejects_reserved_custom_id` | 模型必须先表态推荐项；保留 id 留给 UI。 |
| G7 | 选中 `__custom__` 时出参必带 `custom_text`（非空、≤ 500 字符）；非 `__custom__` 选项不得携带 `custom_text` | `ask_question_custom_text_required_when_custom_selected`、`ask_question_custom_text_forbidden_otherwise` | 自定义答案必须带文本，且只在自定义槽生效。 |

**说人话（§3 总览）**：规划阶段用结构化选择题向用户要决策，有上限、可回放、可取消，执行态不让模型再问。

### 3.1 非目标

| 非目标 | 说明 | 说人话 |
|--------|------|--------|
| 执行态提问 | 需追问 → `/plan exit` 或重进 PLAN | 干活时别弹选择题打断。 |
| 自由文本问答 | 由普通对话承担 | 开放问答不走本工具。 |
| 多模态选项 | 目前仅文本 label | 选项里先不放图。 |

---

## 4. 落地选型与实施（已定稿）

### 4.1 落地选型决策表

| 维度 | 取舍 | 拒因 | 说人话 |
|------|------|------|--------|
| Q1 题数上限 | 4 | cc-fork-01 实战上限；超过 4 用户疲劳。 | 一次别问太多。 |
| Q2 选项 | 2-4（不含自定义槽） | 1 个不是选择；超过 4 体验差。 | 每题 2–4 个选项。 |
| Q3 单选/多选 | 字段 `allow_multiple: bool`，默认 false | 模型默认单选更好控。 | 默认单选，多选要显式开。 |
| Q4 可见时机 | 仅 `Planning` | 执行态再问会破坏 todos 节奏。 | 只规划态可见。 |
| Q5 阻塞模型 | 阻塞 await UI | 与 cc-fork-01 / Cursor `AskQuestion` 一致 | 弹窗等用户答完。 |
| Q6 transcript 落盘 | 单条 `plan.ask_question` 事件包含题目 + 答案（含 `custom_text` 与是否选中推荐项） | 便于回放与调试 | 问答写进 transcript。 |
| **Q7 自定义答案槽（Cursor 风格）** | 每题在 LLM 给出的 `options` 之外，UI **自动追加**一个固定 `id = "__custom__"` 的空选项（label 缺省为「自定义…」），用户可现场编辑文本作为答案；选中时出参带 `custom_text: string`；LLM 入参**不需要**也**不允许**显式声明这个选项 | 完全照搬 Cursor `AskQuestion`：UI 兜底永远多一项「我自己写」，避免选项不足时强迫用户选最不差的 | 选项之外永远再给一个空格让用户自己填。 |
| **Q8 推荐项强制** | 每题 `options` 必须**恰好一个** `recommended: true`（UI 上 label 后缀显示「— 推荐」），代表 LLM 自己最倾向的选项；缺失或多于一个 → tool error；推荐项**不参与** §10 重复 id 校验之外的额外约束，单选 / 多选均适用；与 `__custom__` 槽**互斥**（自定义槽不带 recommended 标） | 让模型先表态再问人，避免「四个选项都看似中性、用户被迫读完所有 prompt」 | LLM 必须先在选项里指出哪个最推荐。 |

### 4.2 实施点（拟定）

> 与 [`plan-runtime.md`](../plan-runtime.md) **PR-PLB** 对齐；当前代码 **PENDING**。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **AQ-A** | catalog 注册 `ask_question`；`is_read_only=true`、`requires_user_interaction=true`；`mode ∈ {Planning, Chat, Pending, Completed}` 可见（EXEC 隐藏）；**交付**：`ToolMetadata` | `src/core/tools/contract/catalog.rs`、`src/api/chat/plan_runtime/catalog.rs` | 见 §11：`ask_question_visible_in_planning_and_chat` / `ask_question_hidden_in_executing`（PENDING） | CHAT/PLAN 都出现；执行态隐藏。 |
| **AQ-B** | 入参校验：`questions∈[1,4]`、每题 `options∈[2,4]`、id 唯一、**恰好一个 `recommended: true`**、`id` **不得**为保留值 `__custom__`；**交付**：校验错误文案 | `src/api/chat/plan_runtime/tool_exec.rs`（拟定） | 见 §11：`ask_question_schema_bounds`、`ask_question_requires_exactly_one_recommended`、`ask_question_rejects_reserved_custom_id`（PENDING） | 题数选项数越界 / 推荐项缺失 / 保留 id 直接拒。 |
| **AQ-C** | UI panel 阻塞 await 用户选择；CLI/IDE 双实现；**自动在每题末尾追加** `__custom__` 槽（label 默认「自定义…」，可输入文本）；推荐项在 label 后追加「— 推荐」后缀；**交付**：`AskQuestionPanel` trait | `src/api/chat/ui/ask_question_panel.rs`（拟定） | 见 §11：`ask_question_blocks_until_answered`、`ask_question_ui_appends_custom_slot`（PENDING） | 弹窗答完才返回 tool 结果；UI 永远多一个自定义槽，推荐项加后缀。 |
| **AQ-D** | transcript 写 `plan.ask_question`（题目 + 答案）；**交付**：事件 schema | `src/infra/transcript/...`（既有） | 见 §11：`ask_question_emits_transcript_event`（PENDING） | 问答落盘方便回放。 |
| **AQ-E** | 监听 `abort_signal` / UI「跳过」→ `{ cancelled: true }`（非 error）；**交付**：取消语义 | `tool_exec.rs` + UI 层 | 见 §11：`ask_question_handles_user_abort`（PENDING） | 用户取消不算工具失败。 |

下文按实施点展开**技术要点与示意图**；**交付边界与代码落点仍以表为准**。

#### 4.2.1 AQ-A：catalog 注册与元属性

- **交付**：`BUILTIN_TOOL_CATALOG` 新增 `ask_question`；`visible_tools_for_mode(Planning, …)` 包含；`Chat` / `Executing` / `Completed` / `Pending` 剔除（普通自由聊天用自然语言追问即可，不强制结构化提问）。
- **元属性**：`is_read_only = true`（不写 `PlanRecord`）；`requires_user_interaction = true`（阻塞 agent loop 直到 UI 回填）。

```text
  Planning 态 catalog 构造
        │
        ▼
  READ_ONLY_TOOLS + create_plan + ask_question
        │
        ▼
  Executing 态 → ask_question 被 filter 掉
```

**说人话**：只在规划态给模型这道「选择题」工具，且标明要等人答。

#### 4.2.2 AQ-B：入参校验

- **交付**：`tool_exec::ask_question` 在调 UI 前校验：
  - `1 ≤ questions.len() ≤ 4`；
  - 每题 `2 ≤ options.len() ≤ 4`（**不含** UI 兜底的 `__custom__` 槽）；
  - `question.id` / `option.id` 在单次调用内唯一；
  - 每题 `options` 中 `recommended: true` 的数量**恰好为 1**；
  - 入参 `option.id` **不得**等于保留值 `"__custom__"`。
- **失败**：统一 `AppError::Tool`，不把半合法题目交给 UI。

**说人话**：坏 schema、缺/多推荐项、占用保留 id 都在进 UI 前挡掉，避免弹出一半题。

#### 4.2.3 AQ-C：UI panel 与阻塞 await

- **交付**：`AskQuestionPanel::run(questions) -> AskQuestionResult`；`tool_exec` 在 async 上下文中 `await`；agent loop **暂停**直至返回或 cancel。
- **多选**：`allow_multiple=true` 时 `option_ids.len() ≥ 1`；默认单选时长度恒为 1。
- **UI 兜底**：渲染时在每题末尾追加一项 `{ id: "__custom__", label: "自定义…", allow_custom_input: true }`；用户选中时弹出文本输入框，限制 1–500 字符。
- **推荐项渲染**：`option.recommended == true` 时在 label 末尾追加「— 推荐」后缀，并默认聚焦该选项（单选默认勾选；多选默认勾选但允许取消）。
- **回填**：UI 在提交前组装 `picked_recommended`（用户最终勾选中是否包含 `recommended=true` 的那一项）；选中 `__custom__` 时附带 `custom_text`。

```text
  tool_exec::ask_question
        │
        ▼
  validate(questions)
        │
        ▼
  UI::show_panel ──await──▶ user answers / abort
        │
        ▼
  AskQuestionResult { answers, cancelled }
```

**说人话**：工具线程卡住等人点选项，答完才把结构化答案还给模型。

#### 4.2.4 AQ-D：transcript `plan.ask_question`

- **交付**：成功或 `cancelled` 均在返回 LLM 前写一条自定义事件，payload 含完整 `questions` + `answers`（或空 answers + `cancelled: true`）。
- **失败**：transcript 写失败 → warning-only，不阻塞 `ToolResult`。

**说人话**：不管答没答完，尽量把题目和选项记录进 transcript。

#### 4.2.5 AQ-E：abort 与取消语义

- **交付**：父 `abort_signal` 置位 → UI 关闭 → 返回 `{ answers: [], cancelled: true }`；**不**抛 `ToolError`。
- **配置**：`TOMCAT_ASK_QUESTION_TIMEOUT_MS`（默认 0）仅测试覆写。

**说人话**：Ctrl+C 或点跳过 = cancelled，不是工具挂了。

---

## 5. 协议（入参 / 出参 / Schema）

### 5.1 工具 JSON Schema

```json
{
  "name": "ask_question",
  "description": "Ask the user a small set of structured single- or multi-select questions. Callable in PLAN and CHAT/Pending/Completed modes (hidden in EXEC, where the agent loop must drive the plan without blocking on user input). Each question's options must include EXACTLY ONE option with `recommended: true` — the choice you (the model) believe is best given the current context. Do NOT submit any option with `id = \"__custom__\"`; the UI automatically appends a custom-input slot to every question so the user can type a free-form answer if none of your options fit. Returns when the user has chosen options or aborts.",
  "parameters": {
    "type": "object",
    "properties": {
      "questions": {
        "type": "array",
        "minItems": 1,
        "maxItems": 4,
        "items": {
          "type": "object",
          "properties": {
            "id":             { "type": "string", "description": "Unique within this call" },
            "prompt":         { "type": "string" },
            "allow_multiple": { "type": "boolean", "default": false },
            "options": {
              "type": "array",
              "minItems": 2,
              "maxItems": 4,
              "description": "Between 2 and 4 options. EXACTLY ONE must set `recommended: true`. The id `__custom__` is reserved for the UI's auto-appended custom-input slot and must not appear here.",
              "items": {
                "type": "object",
                "properties": {
                  "id":          { "type": "string", "description": "Unique within this question; must not equal the reserved value \"__custom__\"" },
                  "label":       { "type": "string" },
                  "recommended": { "type": "boolean", "default": false, "description": "Set to true on EXACTLY ONE option per question — the model's preferred choice. UI renders this with a `— 推荐` suffix." }
                },
                "required": ["id", "label"]
              }
            }
          },
          "required": ["id", "prompt", "options"]
        }
      }
    },
    "required": ["questions"]
  }
}
```

> **UI 端兜底（不进 schema）**：UI 在渲染每题时**自动**在末尾追加一项 `{ id: "__custom__", label: "自定义…", allow_custom_input: true }`，让用户可临时输入文本作为答案。该项**不**计入 `options.minItems/maxItems`，**不**带 `recommended` 标记，**不**对 LLM 可见。

### 5.2 出参

```jsonc
{
  "type": "object",
  "properties": {
    "answers": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "question_id": { "type": "string" },
          "option_ids":  {
            "type": "array",
            "items": { "type": "string" },
            "description": "Selected option ids. May include the reserved id `__custom__` if the user chose the UI's custom-input slot. Single-select questions have length == 1."
          },
          "custom_text": {
            "type": "string",
            "description": "Present iff `__custom__` is in `option_ids`; free-form user input (non-empty, ≤ 500 chars)."
          },
          "picked_recommended": {
            "type": "boolean",
            "description": "true iff the user picked the option you marked `recommended: true`. Convenience signal so the model does not have to cross-check ids."
          }
        },
        "required": ["question_id", "option_ids"]
      }
    },
    "cancelled": { "type": "boolean", "description": "true if user aborted before answering all questions" }
  }
}
```

### 5.3 元属性（catalog 注册时声明）

```rust
ToolMetadata {
    name: "ask_question",
    is_read_only:           true,   // 不写盘
    requires_user_interaction: true, // UI 必须打断
    visible_modes:          &[PlanMode::Planning],
}
```

**说人话**：只读、必须等人答；只在 Planning 时出现在工具列表里。

**说人话（§5 协议）**：入参是一组带选项的题目；出参是每题选中的 option_id 列表，或 `cancelled: true` 表示用户没答完就取消。

---

## 6. One-Glance Map（文件职责总览）

| 路径 | 职责 | 说人话 |
|------|------|--------|
| `src/api/chat/plan_runtime/catalog.rs`（拟定） | 在 `current_mode() == Planning` 时把 `ask_question` 注入 LLM 可见集 |
| `src/api/chat/plan_runtime/tool_exec.rs`（拟定） | 入参校验、UI 调用、abort 监听、transcript 写入 |
| `src/api/chat/ui/ask_question_panel.rs`（拟定） | UI 端结构化提问 panel（CLI / IDE 两套渲染） |
| `src/infra/transcript/...`（既有） | `plan.ask_question` 自定义事件写入 | 问答事件落 transcript。 |

**阅读顺序（说人话）**：catalog 只在 Planning 注入 → tool_exec 校验并弹 UI → 用户答完或取消 → 写 `plan.ask_question` → 把答案还给 LLM。

---

## 7. 调度时序（运行时图）

```
PLAN 模式中：
LLM ──tool_call("ask_question", { questions: [...] })──▶ tool_exec
                                                             │
                                       校验题数/选项数（拒入 → tool_error）
                                                             │
                                                             ▼
                                                       UI panel 弹出
                                                             │
                                          ┌──────────────────┴────────────────┐
                                          │                                    │
                                  用户答完                              用户 Ctrl+C
                                          │                                    │
                                          ▼                                    ▼
                            { answers: [...] }                  { answers: [], cancelled: true }
                                          │                                    │
                                          └──────────────┬─────────────────┘
                                                         ▼
                                       transcript 写 plan.ask_question 事件
                                                         │
                                                         ▼
                                            返回 ToolResult 给 LLM
```

**说人话**：模型出题 → 校验 → 弹窗等人 → 答完或取消 → 记 transcript → 把结构化答案塞回 tool 结果。

---

## 8. 状态机

`ask_question` 调用本身不持有跨调用状态；UI panel 的临时状态：

```
┌──────────┐    user picks    ┌──────────┐
│ Pending  │─────────────────▶│ Answered │
└────┬─────┘                  └──────────┘
     │ user abort
     ▼
┌──────────┐
│Cancelled │
└──────────┘
```

**说人话**：单次调用里 UI 只有「等用户选」→「答完」或「用户取消」三态，不跨调用记状态。

---

## 9. 配置与环境变量

| 名称 | 默认 | 语义 | 说人话 |
|------|------|------|--------|
| `TOMCAT_ASK_QUESTION_TIMEOUT_MS` | `0`（无超时） | 等待用户答复的超时；0 表示不超时（仅在受控测试环境下覆写） | 默认一直等；测试可设超时。 |

---

## 10. 错误模型 / 截断 / 警告

| 触发 | 反馈 | 说人话 |
|------|------|--------|
| `mode != Planning` | catalog 已不可见；强行调用返回 tool error，附 usage `先 /plan` | 非规划态调不了。 |
| `questions.length < 1` 或 `> 4` | tool error | 题数越界。 |
| `options.length < 2` 或 `> 4`（不含 UI 兜底的 `__custom__` 槽） | tool error | 选项数越界。 |
| 重复 `id`（题或选项） | tool error | id 不能重复。 |
| 某题 `options` 中 `recommended: true` 数量 ≠ 1（缺失 / 多于一个） | tool error，usage `每题必须恰好一个推荐项 (recommended: true)` | 推荐项必须唯一。 |
| 入参里出现 `option.id == "__custom__"` | tool error，usage `__custom__ 是 UI 保留槽位，模型不能显式提交` | 保留 id 留给 UI。 |
| UI 返回 `option_ids` 含 `__custom__` 但 `custom_text` 缺失 / 为空 / > 500 字符 | tool error（UI 端在提交前已校验，此处兜底） | 自定义答案必须带合法文本。 |
| UI 返回 `option_ids` 不含 `__custom__` 但携带 `custom_text` | tool error | 没选自定义就别带文本。 |
| 用户 abort | 返回 `{ answers: [], cancelled: true }`，**不**作为 error | 用户取消不算失败。 |
| transcript 写失败 | warning；工具仍正常返回 | 落盘失败不挡返回答案。 |

---

## 11. 测试矩阵（验收）

| 类型 | 测试 | 状态 | 说人话 |
|------|------|------|--------|
| 单元：catalog 可见性 | `ask_question_visible_in_planning_and_chat` / `ask_question_hidden_in_executing` | PENDING | EXEC 隐藏，其它模式可见。 |
| 单元：schema 边界 | `ask_question_schema_bounds`（待新增） | PENDING | 4×4 上下限要硬。 |
| 单元：阻塞与回填 | `ask_question_blocks_until_answered`（待新增） | PENDING | 没人答就别返回。 |
| 单元：abort 处理 | `ask_question_handles_user_abort`（待新增） | PENDING | Ctrl+C 不能 hang。 |
| 单元：transcript 事件 | `ask_question_emits_transcript_event`（待新增） | PENDING | 题目+答案都得落盘。 |
| 单元：推荐项校验 | `ask_question_requires_exactly_one_recommended`、`ask_question_rejects_reserved_custom_id`（待新增） | PENDING | 推荐项必须唯一；保留 id 不许占用。 |
| 单元：UI 自定义槽 | `ask_question_ui_appends_custom_slot`、`ask_question_custom_text_required_when_custom_selected`、`ask_question_custom_text_forbidden_otherwise`（待新增） | PENDING | UI 永远多一个槽；带 text 的边界要严。 |
| 集成：`picked_recommended` 回填 | `ask_question_result_carries_picked_recommended_flag`（待新增） | PENDING | 选了不推荐的也要告诉模型。 |

---

## 12. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| 模型每轮都问 4 题用户疲劳 | 中 | UI 层节流 + 文档约定「先想清楚再问」 | 别每轮塞满 4 题。 |
| 用户 hang 不答 | 中 | 默认无超时但允许测试覆写；UI 提供「跳过」按钮（返回 cancelled） | 可提供跳过/测试超时。 |
| 多选答案与单选混乱 | 中 | runtime 在出参侧统一为 `option_ids` 数组（单选时长度恒为 1） | 出参统一成 option_ids 数组。 |
| transcript 写失败导致回放缺题 | 低 | warning-only；不阻塞工具 | 记盘失败只 warning。 |

---

## 13. 历史决策

| 旧方案 / 分歧 | 结论 | 说人话 |
|---------------|------|--------|
| ~~把提问做成自然语言追问而非工具~~ | **否**：自然语言提问会让模型自己脑补答案；走结构化工具。 | 必须结构化，防脑补。 |
| ~~允许执行态调用 `ask_question`~~ | **否**：执行态需要追问 → `/plan exit` 回到对话或重进 PLAN。 | 执行态别用这个工具。 |
| ~~每题 5+ 选项~~ | **否**：4 是 cc-fork-01 实战上限；超过 4 体验差。 | 选项上限 4。 |
| ~~把 `ask_question` 拆成 `ask_choice` / `ask_multi_choice` 两个工具~~ | **否**：单工具 + `allow_multiple` 字段更简洁。 | 一个工具加 bool 就够。 |
| ~~让 LLM 自行声明「自定义输入」选项~~ | **否**：UI 端固定追加 `id = "__custom__"` 槽（参考 Cursor `AskQuestion` 的「Custom answer」行为）；模型不掺合，也无法绕过——`__custom__` 在 schema 入参侧是保留 id。 | 自定义槽 UI 兜底，不交给模型。 |
| ~~让推荐项靠自然语言提示（如 `prompt` 文末加「我建议 X」）~~ | **否**：改为 `options[].recommended: true` 结构化字段，UI 渲染时统一在 label 后追加「— 推荐」后缀；模型必须先在选项里表态，避免「四个看似中性的选项让用户读完所有 prompt 才能选」。 | 推荐项做成结构化字段，UI 统一展示。 |
| ~~允许零个或多个推荐项~~ | **否**：每题**恰好一个** `recommended: true`；零个会让用户失去快速决策的信号，多个等于没推荐。 | 推荐项必须唯一。 |

---

## 14. 关联文档

- PLAN 模式整体规范：[planner.md](./planner.md)
- 运行时编排：[plan-runtime.md](../plan-runtime.md)
- 写计划文件：[create-plan.md](./create-plan.md)
- 标杆写法：[read.md](./read.md)
- transcript 自定义事件：[session-storage.md](../session-storage.md)
- 任务卡：[T2-P1-002.md](../../../agents/TASK_BOARD_002/tasks/T2-P1-002.md)
- 文档规范：[ARCHITECTURE_SPEC.md](../../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)

**说人话**：规划时要问清楚用本文；写计划用 `create-plan.md`；模式切换看 `planner.md`。
