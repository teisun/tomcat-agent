# 同类 Agent 的规划收敛、子 Agent 结果控制与验证纪律调研

> 状态：调研报告，**不含实施决策**。产出目的是为后续单独立项的「Plan 模式收敛与探索/验证效率」计划提供事实基础。
>
> 调研范围：`cc-fork-01`(Claude Code fork)、`cline`、`codex`、`continue`、`opencode`、`langgraph`、`langchain`、`pi`、`pi_agent_rust`、`vscode`(Copilot Chat) 共 10 个实现。
>
> 方法：全程只读。每条结论附 `文件:行号`；提示词一律照抄原文不转述——**措辞本身就是调研对象**。「未发现」项列出实际使用的检索词，负面结果同样是结论。

---

## 触发这次调研的三个病理

来自一次 27 小时会话的实测数据：

- **P1 规划膨胀**：4 条用户需求被拆成 70 条 todo（首个快照 16 条 → 最终 70 条，+337%）。其中一个里程碑独占 29 条，吃掉执行阶段 **47%** 的 LLM 轮次。tomcat 的 planner 提示词写的是 "break each milestone down into detailed todos so nothing is missed"——一个**没有上限的目标**；且全局没有任何数量上限、预算或按复杂度分支。
- **P2 子 Agent 结果膨胀**：派发 37 个 explorer 子 Agent，其 transcript 合计 37.6 MB，回灌主上下文 901 KB，同一个文件被重复读了 22 次。并行 explorer 之间没有去重，报告没有大小上限，跨波次没有派发预算。超过通用 10000 字符工具结果阈值的报告会被落盘、只留 500 字符预览——**超大的探索会静默丢掉自己的发现**。
- **P3 验证重跑打转**：57 次后台测试/构建任务，失败率 53%；`cargo check` 重跑 11 次、lint 10 次、install-e2e 8 次；轮询任务输出消耗 **2.05 小时**墙钟，而阻塞等待的 API 早就存在、提示词也早就这么要求了。

---

## 摘要

- **todo 数量硬上限：0/10。** 没有任何实现在 schema 层校验或拒绝写入过多 todo。**只有 VS Code Copilot** 在写入时对 `<3` 与 `>10` 条返回**软警告**（`manageTodoListTool.ts:282-287`）。
- **反过度拆解的提示词：6/10 有明确措辞。** 共同模式是「3+ steps / trivial 别用 / 单步别用」。但 cc-fork 与 opencode 结尾都有 "When in doubt, use it" 这类**反向放大**的话。
- **codex 是唯一对「步骤粒度」给正向约束的实现**：要求每步 "no more than 5-7 words each"，并给出 high/low quality plan 的正反例（`default.md:271`）。仍无步数硬上限。
- **子 Agent 派发预算：只有 codex 有 session 级天花板**（V2 默认 `max_concurrent_threads_per_session = 4`，有效 spawn 预算 3）。tomcat 的 37 次无上限属于离群值。
- **子 Agent 报告回灌上限**：cc-fork 的 fork 子 Agent 有 **500 words** 明文上限；codex 通用工具输出默认 **10 KB 中间截断**；opencode **50 KB**；cc-fork 父上下文单结果 **~400 KB 溢出落盘 + 2 KB 预览**（注意：是 2 KB，不是 tomcat 的 500 字符）。
- **跨并行子 Agent 的「已探索路径」共享：10/10 全都没有。** 这是 tomcat 若要做就属于自创的部分。
- **命令/测试结果缓存（「没改动就别重跑」）：10/10 全都没有。** 各家清一色靠提示词纪律。这同样是 tomcat 的可创新点，但也意味着没有现成范式可抄。
- **「禁止轮询、用阻塞等待」是跨实现共识**：cc-fork `TaskOutput block=true`（默认）、vscode `mode=sync`（强烈推荐）、codex `wait_agent`、opencode 完成通知，四家提示词都明文禁止 poll/sleep。

---

## 一、任务拆解的收敛控制

### 1.1 各实现现状

#### cc-fork-01（Claude Code fork）

工具是 `TodoWrite`（`TodoWriteTool/TodoWriteTool.ts:31-32`）；Plan 模式另有 `EnterPlanMode` / `ExitPlanMode`。

分解指令原文（`TodoWriteTool/prompt.ts:6-25`）：

```
## When to Use This Tool
Use this tool proactively in these scenarios:

1. Complex multi-step tasks - When a task requires 3 or more distinct steps or actions
2. Non-trivial and complex tasks - Tasks that require careful planning or multiple operations
...
## When NOT to Use This Tool

Skip using this tool when:
1. There is only a single, straightforward task
2. The task is trivial and tracking it provides no organizational benefit
3. The task can be completed in less than 3 trivial steps
4. The task is purely conversational or informational

NOTE that you should not use this tool if there is only one trivial task to do.
In this case you are better off just doing the task directly.
```

但同一份提示词末尾又有反向放大的一句（`prompt.ts:180`）：

```
When in doubt, use this tool. Being proactive with task management demonstrates attentiveness...
```

数量上限：`TodoWriteTool.ts:15` 只有 `TodoListSchema()` 数组，**无 `maxItems`**；`call()` 不校验条数（`:65-94`）。

复杂度分支：`EnterPlanModeTool/prompt.ts` 有两套提示词——external 版是 "Prefer using EnterPlanMode"（`:27`），内部版是 "When in doubt, prefer starting work... over entering a full planning phase"（`:136`）。用 Plan 还是直接干，由模型判断 + 用户批准共同决定。

Plan 审查有否决权：`ExitPlanModeV2Tool.ts:432-447`，teammate 提交计划后**必须等 lead 批准**；被拒则 "refine your plan based on the feedback"。

#### codex

工具是 `update_plan`（`core/src/tools/handlers/plan_spec.rs:42-47`）。

分解指令原文（`protocol/src/prompts/base_instructions/default.md:54-56, 267-275`）：

```
You have access to an `update_plan` tool which tracks steps and progress...
Do not use plans for simple or single-step queries that you can just do or answer immediately.

To create a new plan, call `update_plan` with a short list of 1-sentence steps
(no more than 5-7 words each) with a `status` for each step
(`pending`, `in_progress`, or `completed`).
There should always be exactly one `in_progress` step until everything is done.
```

反 padding（同文件 `:101-121`）：

```
Note that plans are not for padding out simple work with filler steps or stating the obvious.
...
If you need to write a plan, only write high quality plans, not low quality ones.
```

续跑模板（`prompts/templates/goals/continuation.md:23`）：

```
Skip planning overhead for trivial one-step progress, and do not treat a plan update
as a substitute for doing the work.
```

数量上限：`plan.rs` / `plan_spec.rs` **无步数校验**；"5-7 words" 只是提示词，不是运行时约束。

#### opencode

工具是 `todowrite`（`packages/core/src/tool/todowrite.ts:32-33`），提示词在 `packages/opencode/src/tool/todowrite.txt`。

原文节选（`todowrite.txt:5-16, 44`）：

```
## When to use
Use proactively when:
- The task requires 3+ distinct steps or actions
  (not just 3 tool calls for a single conceptual step)
...
## When NOT to use
Skip when:
- The work is a single, straightforward task (or <3 trivial steps)
...
When in doubt, use it.
```

注意括号里那句 "not just 3 tool calls for a single conceptual step"——它在纠正一个具体的误判方式，措辞比其他几家更精确。

数量上限：`todowrite.ts:47` 替换式写入，**无上限**。

#### vscode（Copilot Chat）

工具是 `manage_todo_list`（`manageTodoListTool.ts:28, 69`）。

`modelDescription` 原文节选（`:69`）：

```
When NOT to use:
- Single, trivial tasks that can be completed in one step
- Purely conversational/informational requests
- When just reading files or performing simple searches
...
Use this tool VERY frequently to ensure task visibility and proper planning.
```

Schema 约束（`:46-51`）：title 要求 "3-7 words"；`in-progress` 描述含 "max 1"——都只是描述，不 enforce。

**唯一的运行时软限制（`:282-291`）：**

```typescript
if (todoList.length < 3) {
  warnings.push('Warning: Small todo list (<3 items). This task might not need a todo list.');
}
else if (todoList.length > 10) {
  warnings.push('Warning: Large todo list (>10 items). Consider keeping the list focused and actionable.');
}
if (changes > 3) {
  warnings.push('Warning: Did you mean to update so many todos at the same time? ...');
}
```

这是全部 10 个实现里**唯一**在运行时对 todo 规模做出反馈的机制。它不拦、不报错，只是把警告写进工具返回值让模型看见。第三条警告（一次改超过 3 条）针对的是另一个病理：批量刷状态而不是逐条推进。

Plan 审查：Agent host 有 `planReview` UI（`agentHostChatContribution.test.ts:3442+`），用户可批准或要求修订。

#### cline

`sdk/task-proxy.ts:145-146` 明确写着 `Focus chain checklist (stub — focus chain removed)`——**todo 机制已被移除**，只剩 telemetry 与 file-utils 残留。`shared/tools.ts:25` 里 `TODO = "focus_chain"` 枚举还在，实现已 stub。

Plan 模式原文（`sdk/cline-session-factory.ts:52-63`）：

```
# Plan Mode

You are in Plan mode. Your role is to explore, analyze, and plan -- not to execute.
...
- Do NOT edit files, write code, run destructive commands, or make any changes
...
Once the user has reviewed your plan and explicitly approved it ... use the switch_to_act_mode tool
```

**一个实现曾经有 todo 机制、后来整个移除了**，这本身是值得注意的信号（未查移除原因，见开放问题）。

#### langchain（v1）

middleware `write_todos`（`agents/middleware/todo.py:139-149`）。

原文节选（`WRITE_TODOS_TOOL_DESCRIPTION:52-79, 112-113`）：

```
Only use this tool if you think it will be helpful in staying organized.
If the user's request is trivial and takes less than 3 steps,
it is better to NOT use this tool and just do the task directly.
...
Remember: If you only need to make a few tool calls to complete a task,
and it is clear what you need to do, it is better to just do the task directly
and NOT call this tool at all.
```

system prompt（`:125-127`）：

```
For simple objectives that only require a few steps, it is better to just complete
the objective directly and NOT use this tool.
Writing todos takes time and tokens, use it when it is helpful for managing complex
many-step problems! But not for simple few-step requests.
```

**这是 10 家里语气最克制的一份**：它不但说了什么时候别用，还说明了理由（takes time and tokens），而且没有 "when in doubt, use it" 的反向放大。

运行时约束（`:313-333`）：`after_model` **拒绝同一轮内多次并行调用 `write_todos`**——不是数量上限，是防止批量刷写。

与 cc-fork 的一处分歧（`:85`）：langchain 允许多个不相关任务同时 `in_progress`。

#### continue / pi / pi_agent_rust / langgraph

- **continue**：CLI 有 `Subagent` 工具（`extensions/cli/src/tools/subagent.ts:15-27`），**无内置 todo/plan 工具**。
- **pi**：默认 system prompt（`src/core/system-prompt.ts:121-129`）只列 read/bash/edit/write 与通用准则，**无 todo/plan 工具**。Plan mode 与 subagent 是 extensions 示例，不是 core。
- **pi_agent_rust**：`grep write_todo|update_plan` 无结果。`validation_broker` 里的 plan latency 预算是 CLI 校验基础设施，与 agent 拆解无关。
- **langgraph**：subgraph 是图拓扑，**无 agent todo 工具或 plan 提示词**。

### 1.2 横向对比

**「什么时候该用 todo/plan」的阈值表述**

- 「3+ steps」这个具体阈值：cc-fork、opencode、langchain 三家一致；vscode 间接表达
- 「trivial / single-step 别用」：上述三家 + codex（禁 plan padding）+ vscode，共 5 家
- 「拿不准就用」这类反向放大：cc-fork `prompt.ts:180`、opencode `todowrite.txt:44`、vscode "VERY frequently"——三家有，且与 tomcat 的 "nothing is missed" 同向

**数量与粒度约束**

- 硬上限：0/10
- 软警告：1/10（仅 vscode，`>10` 与 `<3`）
- 粒度约束（措辞）：codex "5-7 words each"；vscode title "3-7 words"
- 防批量刷写：langchain（禁止同轮并行调用）、vscode（一次改 >3 条给警告）

**复杂度分支怎么判**

- 显式模式切换：cline Plan/Act、cc-fork EnterPlanMode、vscode planReview
- 提示词内启发式：codex "trivial one-step → skip planning"、cc-fork 内部版 "straightforward → 别进 plan"
- **没有任何实现用运行时启发式（如需求长度、涉及文件数）自动判定复杂度**——全部交给模型判断

**计划审查的否决权**

- 用户/lead 可拦：cc-fork ExitPlanMode、cline switch_to_act_mode、vscode planReview
- 无独立 critique agent 具备否决权：codex、opencode、langchain、continue、pi、langgraph

### 1.3 对 tomcat 的启示

- **tomcat 的措辞在这 10 家里偏激进。** 多数实现用「3+ steps / trivial 不用」这种**有下界的**表述，而 tomcat 用的是 "break down so nothing is missed"——一个**没有上界的**表述。cc-fork 里最接近的一句是 "Break complex tasks into smaller, manageable steps"（`prompt.ts:174`），也没有「不遗漏」这种无限目标。
- **可直接引用的两条证据**：codex 的「每步 5-7 词 + 低质量计划反例」是唯一对粒度的正向约束；vscode 的 `>10` 软警告是唯一的运行时反馈。两者都不拦截，都只是让膨胀「被看见」。
- **langchain 的语气值得抄**：它解释了为什么不该过度使用（花时间和 token），而不是只下命令。给出理由的提示词通常比给出规则的更稳。
- **注意 cline 把 todo 机制整个移除了**——在决定给 tomcat 加更多 todo 治理机制之前，值得先搞清楚 cline 为什么放弃。
- **没有 prior art 支持硬上限。** 0/10 有硬上限这一事实，本身就是反对加硬上限的证据；但也要注意，没有一家公开过 A/B 数据说明硬上限会不会伤害成功率。

---

## 二、子 Agent 派发与结果规模控制

### 2.1 各实现现状

#### cc-fork-01

派发：`Agent` 工具（`AgentTool/prompt.ts:248, 271`），提示词鼓励 "Launch multiple agents concurrently whenever possible"。

并发与数量：

- 工具并行度 `CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY` 默认 **10**（`toolOrchestration.ts:8-11`）
- 子 Agent `maxTurns` 可选（`loadAgentsDir.ts:89`）；`general-purpose` 与 `Explore` **未设**（`generalPurposeAgent.ts:25-34`、`exploreAgent.ts:64-83`）；fork agent 设 **200**（`forkSubagent.ts:65`）
- **无会话级派发总数预算**

子 Agent 报告上限——**这是本调研里最直接可对标的一条**（`forkSubagent.ts:185-186`）：

```
8. Keep your report under 500 words unless the directive specifies otherwise.
   Be factual and concise.
```

父上下文工具结果限制（`constants/toolLimits.ts:13-49`、`toolResultStorage.ts:109, 309`）：

- 单个结果上限 `MAX_TOOL_RESULT_BYTES = 100_000 tokens × 4 ≈ 400 KB`，溢出则落盘 + 预览
- `PREVIEW_SIZE_BYTES = 2000`（tomcat 是 500 字符）
- 单条 message 内所有工具结果合计 `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS = 200_000`

读文件去重（`FileReadTool/FileReadTool.ts:536-567`、`prompt.ts:7-8`）：

```
FILE_UNCHANGED_STUB =
  'File unchanged since last read. The content from the earlier Read tool_result
   in this conversation is still current — refer to that instead of re-reading.'
```

mtime 未变且 offset/limit 相同时返回 `file_unchanged`，不把内容重复读进上下文。

跨 explorer 去重：**未发现**共享的已探索集合；fork 只做 `readFileState` 的 clone/merge（`fileStateCache.ts:128-141`）。

#### codex

派发：`spawn_agent` / multi-agent V2（`multi_agents_spec.rs:689-699, 717-723`）。

并发常量（`config/mod.rs:205, 1140, 1428-1431`）：

- `DEFAULT_MULTI_AGENT_V2_MAX_CONCURRENT_THREADS_PER_SESSION = 4`
- `effective_agent_max_threads(V2) = max_concurrent - 1` → 有效 spawn 预算 **3**
- V1：`DEFAULT_AGENT_MAX_THREADS = Some(6)`
- 触顶报错（`agent_jobs.rs:121-122`）：`"agent thread limit reached; this session cannot spawn more subagents"`

委派提示词里的反重复条款（`multi_agents_spec.rs:681-693`）：

```
- Avoid issuing multiple delegate calls on the same unresolved thread unless the new
  delegated task is genuinely different and necessary.
- Do not repeatedly wait by reflex.
```

工具输出截断：默认 `TruncationPolicy::Bytes(10_000)`（`openai_models.rs:674`），**中间截断**保留首尾（`output-truncation/src/lib.rs:25-28`）。

读文件：提示词要求 apply_patch 之后不要 re-read（`default.md:143`），**无跨子 Agent 的 read cache**。

#### opencode

派发：`task` 工具（`tool/task.ts:24-41, 58-60`）。

背景执行原文（`task.ts:31-34, 59-60`）：

```
The task is working in the background. You will be notified automatically when it finishes.
DO NOT sleep, poll for progress, ask the task for status, or duplicate this task's work
```

最后半句 "or duplicate this task's work" 是对重复派发的直接约束，措辞比其他几家更明确。

结果规模（`tool/truncate.ts:15-16, 85-140`）：

- 默认 `MAX_LINES = 2000`、`MAX_BYTES = 50 * 1024`
- 溢出后写进 truncation 目录 + 预览 + 提示；若会话里有 Task 工具，提示会建议**派发子 Agent 去读那个溢出文件**（`:129-130`）

最后这条设计值得注意：它把「结果太大」从丢失变成了「可检索」——溢出内容仍然可达，只是要多一步。

并发/数量上限：提示词鼓励并行，**无 session 级预算**。

#### continue

`subagent.ts:80-112`：子 session 跑完后把 `result.response` **全文**拼进父工具输出，**无大小上限**。

bash 输出（`runTerminalCommand.ts:26-27, 184-185`）：默认 50,000 字符 / 1000 行；并行调用时**按并行数均分**配额——这个细节其他家没有。

#### pi_agent_rust

工具截断（`src/tools.rs:220-224, 263-267`）：`DEFAULT_MAX_LINES = 2000`、`DEFAULT_MAX_BYTES = 1_000_000`，超阈值溢出为 artifact。

读缓存（`tools.rs:8264-8292` 测试）：同路径二次读命中缓存，文件被改写后缓存失效。

子 Agent 与跨 Agent 去重：**未发现**。

#### vscode / langchain / langgraph / pi

- **vscode**：Agent host 有多 session 与子 agent thread（测试覆盖），但本次未在 `contrib/chat` 找到独立的 Task spawn 工具常量（见开放问题）。终端输出强推 `mode=sync` 一次拿全量（`runInTerminalTool.ts:297-299, 454`）。
- **langchain**：`subagent` 是 model role + middleware，**无派发预算或结果上限**。
- **langgraph**：subgraph 是图拓扑不是 LLM explorer，**无统一的工具结果截断层**（属框架使用者职责）。
- **pi**：core 无 subagent 工具。

### 2.2 横向对比

**派发上限**

- 有 session 级硬上限：codex（V2 有效 3 个，V1 6 个）——**唯一一家**
- 只有工具并行度上限：cc-fork（10）
- 完全无上限：opencode、continue、cc-fork 的 Agent 派发

**子 Agent 报告进入父上下文的规模**

- 明文字数上限：cc-fork fork 子 Agent **500 words**
- 通用工具截断兜底：codex **10 KB**、opencode **50 KB**、cc-fork **~400 KB**、continue bash **50 K 字符**、pi_agent_rust **1 MB**
- 无任何上限：continue 的 subagent 结果（全文拼接）

**溢出之后发生什么**

- cc-fork：落盘 + **2 KB** 预览 + 磁盘路径
- opencode：落盘 + 预览 + **提示可派子 Agent 去读溢出文件**
- codex：中间截断，保留首尾
- tomcat 现状：落盘 + **500 字符**预览——是这组里最激进的丢弃

**去重**

- 单会话内读文件去重：cc-fork（mtime + 窗口）、pi_agent_rust（输出缓存）
- 跨并行子 Agent 的已探索路径共享：**0/10**

### 2.3 对 tomcat 的启示

- **500 字符预览是这组里的离群值。** cc-fork 用 2 KB，且同时给出磁盘路径；opencode 更进一步，明确提示可以派子 Agent 去读溢出文件。tomcat 的 500 字符预览意味着一份超大的 explorer 报告实际上等于没做——这是隐性的信息丢失，不只是效率问题。
- **派发预算有唯一 prior art**：codex 的 session 级天花板。37 次无上限在这 10 家里确实是离群值。
- **子 Agent 报告上限有唯一 prior art**：cc-fork 的 500 words。这个数量级（约 3-4 KB）远小于 tomcat 现在的无上限。
- **跨 explorer 去重没有 prior art。** 0/10 意味着如果 tomcat 要做，是在没有参考实现的情况下自创机制，应当从最小、最保守的形态开始（只做建议不做强拦），并准备好推翻它。
- **单会话读文件去重有两家 prior art**（cc-fork 的 `file_unchanged`、pi_agent_rust 的 read cache），说明「同一文件被读 22 次」这个问题应该在 **read 工具层**解决，而不是靠 explorer 侧的提示词。tomcat 其实已有 `ReadFileState` 去重，需要查的是为什么没生效（不同 offset/limit 视为不同读取，是已知的可能原因）。

---

## 三、验证与重跑纪律

### 3.1 各实现现状

#### codex——这一项里最完整的一份

原文（`default.md:149-163`）：

```
When testing, your philosophy should be to start as specific as possible to the code
you changed so that you can catch issues efficiently, then make your way to broader
tests as you build confidence.
...
For all of testing, running, building, and formatting, do not attempt to fix unrelated bugs.
...
- When running in the non-interactive approval mode **never**, proactively run tests, lint ...
- When working in interactive approval modes like **untrusted**, or **on-request**,
  hold off on running tests or lint commands until the user is ready...
- When working on test-related tasks ... you may proactively run tests regardless of approval mode.
```

三点值得注意：一是「**先窄后宽**」写得很具体（start as specific as possible → broader as you build confidence）；二是按 approval mode 分支决定要不要主动跑测试；三是明确禁止顺手修无关 bug——那也是重跑的一个来源。

运行时重跑 gate：**未发现**命令结果缓存。

#### cc-fork-01

后台任务（`BashTool/prompt.ts:317-319`）：

```
If waiting for a background task you started with `run_in_background`,
you will be notified when it completes — do not poll.
```

`TaskOutput`（`TaskOutputTool.tsx:173-178`）：

```
DEPRECATED: Prefer using the Read tool on the task's output file path instead.
...
- Use block=true (default) to wait for task completion
```

注意 `block=true` 是**默认值**——它把正确用法做成了默认，而不是靠提示词提醒。

验证子 Agent：`TodoWriteTool.ts:77-107`，当所有 todo 完成且没有 verification 项时，工具返回值里会 nudge 模型去 spawn 一个 verifier。这是「用独立角色收口」而不是「反复跑 lint」。

命令缓存：**无**（WebFetch 有 15 分钟缓存，与测试无关）。

#### vscode

`runInTerminalTool.ts:297-299, 320, 454` 原文：

```
For ALL one-shot commands (builds, tests, installs, compilation, linting, downloads,
scripts), use mode='sync' and omit timeout. The tool waits for the command to complete
and returns full output inline. This is the default and strongly preferred mode.
...
NEVER run sleep or similar wait commands in a terminal. You will be automatically
notified on your next turn when async terminal commands ... complete ... Do NOT poll
for completion.
```

同样是把阻塞做成默认 + 明文禁 poll。

#### 其余

- **opencode**：Task 背景执行 + 完成通知，禁 sleep/poll（见 2.1）。未发现测试专用提示词。
- **cline**：Plan 模式禁止执行，天然不跑测试；Act 模式**未发现**「勿重复全量」的专用提示词。
- **continue**：bash 默认阻塞（`runTerminalCommand.ts:189-209`），超时转后台；**无重跑纪律提示词**。
- **langchain / langgraph / pi / pi_agent_rust**：**无**专门的验证纪律提示词或测试缓存（pi_agent_rust 有 120s bash 超时，不是缓存）。

### 3.2 横向对比

**测试策略提示词**

- 最完整：codex（先窄后宽 + approval mode 分支 + 禁修无关 bug）
- 禁 poll / 用阻塞：cc-fork、vscode、opencode 三家措辞高度一致
- 按改动文件自动缩小测试范围：**0/10**

**运行时防打转机制**

- 命令结果缓存：**0/10**（确认的负面结果）
- 读文件的过期保护：cc-fork、pi_agent_rust 有，但与测试重跑无关

**阻塞等待 API 的存在性与默认值**

- cc-fork `TaskOutput block=true`——**默认阻塞**
- vscode `mode=sync`——**默认且强烈推荐**
- codex `wait_agent`——存在，但提示词要求 "very sparingly"
- continue——默认 await 子进程

这里有个规律：**把正确用法做成默认值的实现（cc-fork、vscode、continue），比只在提示词里推荐的实现（codex），更不容易出现轮询打转。**

### 3.3 对 tomcat 的启示

- **2.05 小时轮询在 cc-fork / vscode / opencode 都被明确当作反模式**，三家都提供阻塞路径。tomcat 的能力已经有了（`task_output` 支持 `block=true`，最长 10 分钟），差的是**默认值**——参考实现的共同做法是把阻塞设为默认，而不是靠提示词要求模型选。
- **「先窄后宽」有可直接抄的原文**（codex `default.md:149-163`），包括禁止顺手修无关 bug 这条。
- **重跑缓存没有 prior art。** 0/10 意味着如果 tomcat 要做命令结果缓存（比如按 git tree hash + 命令指纹），是自创；风险在于缓存失效判断错了会让模型看到过期的绿灯，比多跑几次更危险。建议先把提示词与默认值这两条低风险的做掉，再评估要不要做缓存。
- **cc-fork 的 verifier nudge 值得对比**：它是在 todo 全部完成时提示 spawn 一个独立验证角色，而不是让执行者自己反复跑 lint。tomcat 的 verifier 目前是下线状态（有测试锁定），重新上线是独立决策。

---

## 四、未找到的机制（负面结果）

以下是检索后确认**在调研范围内不存在**的机制。列出实际使用的检索词，便于复核。

- **todo 条数硬上限 / 拒绝写入**——检索 `maxItems`、`max.*todo`、`todoList.length >`，10/10 无；仅 vscode 有 warning
- **里程碑级 todo 预算**——检索 `milestone`、`todo.*budget`、`max.*plan.*step`，10/10 无
- **跨并行子 Agent 的已探索/已读共享状态**——检索 `explored`、`already covered`、`dedup.*explorer`，10/10 无
- **子 Agent 报告的强制字数上限**（cc-fork fork 的 500 words 除外）——codex、opencode、continue 均无
- **命令 / 测试结果缓存**——检索 `command.*cache`、`nothing changed since last run`、`rerun.*gate`，10/10 无
- **按改动文件自动圈定测试范围**——检索 `affected.*test`、`changed files.*test`，10/10 无
- **按复杂度自动分支的运行时启发式**（非模型判断）——10/10 无
- **replay 安全性的工具分类**（只读 vs 写）——10/10 无
- **cline 现行 focus chain 提示词**——已 stub（`task-proxy.ts:145`）
- **continue / pi 的 core todo 工具**——检索 `TodoWrite`、`write_todos`、`manage_todo`，无
- **langgraph 的 agent todo/plan 工具**——无
- **codex 的 plan 步数运行时校验**——检索 `plan.rs` 里 `validate`、`max.*step`，无

---

## 五、开放问题

1. **cline 为什么移除 focus chain？** 当前 tree 已 stub，需查 git history 才能拿到移除前的原始措辞和移除理由。这是本调研里最有价值的未解问题——一个实现主动放弃 todo 机制，理由可能直接适用于 tomcat。
2. **codex 子 Agent 最终答案注入父 thread 前的精确 byte 预算。** spawn/wait 路径有 truncation policy，但子 Agent 整段 transcript 是否参与 compaction 需要读 `thread_manager.rs` 与 session compaction 测试。本调研未量化出与 tomcat「901 KB 注入」直接可比的数值。
3. **VS Code Copilot Agent host 的子 Agent 并发与结果上限。** spawn 在 agent host 服务里，不在 `manage_todo_list` 同目录，需单独读 agent host protocol。本报告只确认了 planReview 与终端 sync 纪律。
4. **continue 的 GUI 与 CLI 是否不同。** 本报告以 `extensions/cli` 为准，GUI 侧的截断与后台策略未查。
5. **pi_agent_rust 生产 agent loop 的默认提示词。** `tools.rs` 之外是否还有验证相关段落，需读 agent session 构建路径。
6. **硬上限的缺失是刻意还是疏忽。** 「3+ steps」「5-7 words」全是软提示；0/10 有硬上限是事实，但没有任何一家公开过 A/B 数据证明硬上限不伤成功率。在给 tomcat 加硬约束前，这个问题应当先有答案。

---

## 附：与 tomcat 现状的差异一览

仅列出**已验证的数值差异**，不含推断。

- 子 Agent 报告上限：cc-fork 500 words / tomcat 无上限
- 工具结果溢出预览：cc-fork 2 KB、codex 中间截断保留首尾 / tomcat 500 字符
- 单结果落盘阈值：cc-fork ~400 KB、opencode 50 KB、codex 10 KB / tomcat 10000 字符
- 子 Agent session 级派发预算：codex 3（V2）/ tomcat 无
- 单次并行子 Agent 数：codex 4、cc-fork 工具并行 10 / tomcat 6
- todo 数量运行时反馈：vscode `>10` 警告 / tomcat 无
- 步骤粒度约束：codex "5-7 words" / tomcat 无
- 阻塞等待默认值：cc-fork `block=true` 默认、vscode `mode=sync` 默认 / tomcat `block` 需显式指定
