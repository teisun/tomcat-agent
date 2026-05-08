# 多仓库 Agent 错误处理设计与实现对比报告

**版本**：1.0  
**日期**：2026-04-19  
**落盘路径**：`tomcat/docs/reports/agent_error_handling_cross_repo.md`  
**范围**：对比 **cc-fork-01、hermes-agent、openclaw、pi-mono、pi_agent_rust、tomcat** 在 **工具失败、LLM/流式失败、扩展/钩子、会话重试** 等场景下：错误如何分类、**是否让 agent loop 继续**、还是 **终止并待人介入**；并给出 **评分表** 与 **适合 tomcat 的优选组合**。

**相关报告**：[plugin_systems_openclaw_pi_mono_pi_agent_rust.md](plugin_systems_openclaw_pi_mono_pi_agent_rust.md)（三端插件体系）；本报告专注 **错误与恢复**，不重复扩展注册面。

---

## 1. 摘要

| 项目 | 核心模式（一句话） |
|------|-------------------|
| **tomcat** | `classify_error` → `Fatal` / `Retryable` / `Aborted`；**Attempt** 指数退避 + 可选 **L3 截断** 后重试；**工具** `Err` → 字符串 + `is_error` 进下一轮推理。 |
| **pi_agent_rust** | 流式 **API 失败** → 注入 assistant 错误文案 + `AgentEnd(error)` **并返回 Err**；**工具**多数 **Err→ToolOutput**；**扩展钩子**广泛 **fail-open（warn）**；`execute_tool_calls` **批次级 Err** → `AgentEnd` + **返回 Err**。 |
| **OpenClaw** | `resolveRunFailoverDecision`：**rotate_profile / fallback_model / surface_error / return_error_payload**；**Harness** 可先非 Pi 后端再 **fallback 嵌入式 Pi**；嵌入式 run 内含重试耗尽 → **用户可读 isError payload**。 |
| **pi-mono** | **可配置会话级自动重试**：正则匹配可重试错误 → 去掉最后一条 assistant → 退避 → **`agent.continue()`**；事件 `auto_retry_*`；TUI `showError`。 |
| **hermes-agent** | **API 无效响应** 循环重试 + **抖动退避**；耗尽后 **`_try_activate_fallback` 模型**；工具/网关注入 **局部重试**；最终 **dict failed**。 |
| **cc-fork-01** | 研究用 **Claude Code 快照**，CLI 遇错 **`exitWithError`**；**不做**与「长期运行宿主」同级的 loop 策略对比（见 §8）。 |

---

## 2. 术语

- **吸收为观测（tool result）**：工具执行失败不把整局判死，而是把错误文本作为 **tool 角色消息**（或等价结构）交给模型下一轮推理——**loop 继续**。
- **致命（fatal）**：当前轮/次 **无法在无用户动作下恢复**，向上返回错误（或展示 isError），**默认不自动再呼同一 LLM 请求**（除非另有重试层）。
- **可重试（retryable）**： transient（429/5xx、超时、上下文溢出等），由上层 **退避重试** 或 **截断后再请求**。
- **fail-open（扩展）**：扩展钩子失败 **只打日志 warn**，**不阻断**主 Agent——与「失败即停机」相反。

---

## 3. 总览矩阵

| 维度 | tomcat | pi_agent_rust | OpenClaw（嵌入式 Pi） | pi-mono | hermes-agent | cc-fork-01 |
|------|--------------|---------------|------------------------|---------|--------------|------------|
| **工具失败默认** | `execute_tool`：`Ok/Err` → `(String, is_error)`，推进 messages | `execute_tool`：`Err`→**ToolOutput 文本错误**；未知工具同上 | 多数在 Pi 层为 **tool result**（经 pi-agent-core） | Pi Agent 工具管线 → **toolResult** | 工具内 **局部重试** 或返回 JSON 错误字符串 | 依赖上游 SDK；本快照未展开统一 loop |
| **LLM/流失败** | `classify_error`；**Attempt** 重试；Fatal 则 `AgentEnd(error)` | `stream_assistant_response` **Err**：合成 assistant + **`AgentEnd` + `return Err`** | **failover-policy** + retry-limit；可 fallback 模型 / profile | **`_handleRetryableError`** + `continue()` | **run_conversation** 内重试 + **fallback 模型** | CLI `exitWithError` |
| **会话内自动重试 LLM** | 有（`max_attempts`、**AutoRetry** 事件） | 主要在 **单次 run** 内迭代与工具轮；流式 Err 路径见上 | 嵌入式 **多次 attempt** + 策略 | **可配置**，指数退避 | 有（**invalid response** 等） | 不适用 |
| **扩展/插件失败** | VM/插件路径另述；宿主以 **审计/事件** 为主 | **fail-open**：`startup`/`tool_call`/`session_start` 等 hook **warn** 不挡跑 | 插件加载失败偏 **诊断+配置**；Skills/工具分册 | ResourceLoader **冲突不卸载**（与错误处理正交） | Python 插件体系相对独立 | 非本报告重点 |
| **用户可见** | EventBus：`AutoRetry*`、`AgentEnd`；Chat 打印 | `AgentEvent` 序列 | **isError payload**、渠道投递 | **auto_retry** 事件、`showError` | `_emit_status`、日志、return dict | CLI 退出码 |

---

## 4. tomcat（本仓库）

**锚点**：[`src/core/agent_loop/convert.rs`](../../src/core/agent_loop/convert.rs)（`classify_error`）、[`src/core/agent_loop/run.rs`](../../src/core/agent_loop/run.rs)（`run` / `run_attempt_loop` / `run_reasoning_loop` / `execute_tool`）。

### 4.1 LLM / HTTP 错误

- **分类**：401 → `Fatal`；429/5xx/超时/上下文溢出启发 → **`Retryable`**（其余多为 Fatal，见 `classify_error`）。
- **Attempt 循环**：`run_attempt_loop` 对 `Retryable` **指数 sleep**；若判定为 **上下文溢出** 且有 `ContextState`，可走 **强制删旧 turn + 重建 messages（L3 语义）** 再试；用尽 `max_attempts` → **Fatal**。
- **用户中断**：`abort_signal` → `LoopError::Aborted` → `AgentEnd` + `AppError::Config("用户中断")`。

### 4.2 工具失败

- **不抛断整个 `run_reasoning_loop`**：`execute_tool` 内 `primitive.*.await.map_err(|e| e.to_string())` → **`(err_string, is_error: true)`**，再 **`ChatMessage::tool`** 追加（见 `run.rs` 约 665–688、718–829 行）。

### 4.3 ASCII：三层结构

```text
  run (Conversation)
      |
      v
  run_attempt_loop  <-------- Retryable + 退避 (+ 可选 L3 截断)
      |
      v
  run_reasoning_loop  <------ 多轮 tool round
      |
      +-- LLM stream Err --> classify_error --> Fatal|Retryable|Aborted
      +-- tool Err --------> tool message (is_error=true) --> 继续下一 LLM turn
```

---

## 5. pi_agent_rust

**锚点**：[`pi_agent_rust/src/agent.rs`](../../pi_agent_rust/src/agent.rs)（`run` 内循环、`stream_assistant_response`、`execute_tool_calls`、`execute_tool_without_hooks`）。

### 5.1 流式推理失败（网络/提供商错误）

- `stream_assistant_response(..).await` **`Err(err)`**：生成 **assistant 错误文本**、`TurnEnd`、`AgentEnd { error: Some(err_string) }`，并 **`return Err(err)`**（约 844–892 行）——**本轮对外是失败**，不是「假装成功再让模型接着聊」。
- **`StopReason::Error | Aborted`**（模型已返回助手消息但带错）：同样 `AgentEnd` + **`return Ok(assistant)`**（约 903–937）——语义为 **有序结束一轮**。

### 5.2 工具失败

- **单工具**：`execute_tool_without_hooks` 内 `tool.execute(..).await` **`Err(e)`** → **`ToolOutput` 文本「Error: …」+ `is_error: true`**（约 2243–2254 行）——**吸收为观测**。
- **批次级**：`execute_tool_calls(..).await` **`Err(err)`** → `AgentEnd { error }` + **`return Err(err)`**（约 1018–1052）——与「单工具 Err」层级不同。

### 5.3 扩展钩子 fail-open

- 多处 **`tracing::warn!(.. fail-open)`**：如 **`startup`/`session_start`/`tool_call`/`tool_result`/`session_compact`** 等（`agent.rs` 检索 `fail-open`）——**钩子异常不终止 Agent**。

### 5.4 ASCII：流式 Err vs 工具 Err

```text
        stream_assistant_response
                    |
          +---------+---------+
          | OK           Err |
          v                   v
   正常 assistant    合成错误 assistant + AgentEnd(error)
                             |
                             return Err(api_err)   <-- 待人/上层处理

        execute_tool (single)
                    |
          +---------+---------+
       Ok output          Err(e)
          |                   |
          v                   v
   ToolOutput           ToolOutput(is_error=true)
                             |
                        仍进入 tool_results，下轮 LLM 可自纠
```

---

## 6. OpenClaw

**锚点**：

- [`openclaw/src/agents/pi-embedded-runner/run/failover-policy.ts`](../../openclaw/src/agents/pi-embedded-runner/run/failover-policy.ts)：`resolveRunFailoverDecision`（**retry_limit / prompt / assistant** 三阶段不同分支）。
- [`openclaw/src/agents/harness/selection.ts`](../../openclaw/src/agents/harness/selection.ts)：`runAgentHarnessAttemptWithFallback` —— 非 Pi harness **异常**时可 **fallback 嵌入式 Pi**（约 129–137 行）。
- [`openclaw/src/agents/pi-embedded-runner/run/retry-limit.ts`](../../openclaw/src/agents/pi-embedded-runner/run/retry-limit.ts)：`handleRetryLimitExhaustion` → **fallback_model** 抛 `FailoverError` 或 **return_error_payload**（用户可读错误块）。

### 6.1 策略要点

- **重试耗尽**：若配置了 fallback 且原因满足 `shouldEscalateRetryLimit` → **换模型**；否则 **return_error_payload**（用户侧看到 **isError: true** 的文本块）。
- **Prompt/Assistant 阶段**：可 **rotate_profile**、**fallback_model**、**surface_error**；外部 abort → **surface_error**。
- **Harness**：某后端 runAttempt 抛错 → 可能 **降级到 Pi**（策略 `runtime: auto` 且允许 fallback）。

### 6.2 ASCII：failover 决策（简化）

```text
              retry_limit 阶段
                     |
       +-------------+-------------+
       |                           |
 escalation?+fallback      否则 return_error_payload
       |                           |
       v                           v
 fallback_model              isError 文案给用户
```

---

## 7. pi-mono（coding-agent）

**锚点**：[`pi-mono/packages/coding-agent/src/core/agent-session.ts`](../../pi-mono/packages/coding-agent/src/core/agent-session.ts)（`_handleRetryableError`、`isRetryableError` 正则约 2388–2392）、[`interactive-mode.ts`](../../pi-mono/packages/coding-agent/src/modes/interactive/interactive-mode.ts)（`auto_retry_end` → `showError`）。

### 7.1 要点

- **可配置**：`settingsManager.getRetrySettings()`；超限 → **`auto_retry_end` success:false**。
- **状态修复**：重试前 **删掉 agent.state 最后一条 assistant**（避免错误 assistant 占位），再 **`setTimeout(() => this.agent.continue())`**。
- **可重试错误**：基于 **errorMessage 正则**（overloaded、429、5xx、timeout、fetch failed 等）。

### 7.2 ASCII

```text
   agent_end + assistant.errorMessage
              |
              v
      isRetryableError? ---- 否 ---> 照常展示失败
              |
             是
              v
      auto_retry_start -> sleep(退避) -> agent.continue()
              |
              v
      成功下一轮 / auto_retry_end (false) -> showError
```

---

## 8. hermes-agent

**锚点**：[`hermes-agent/run_agent.py`](../../hermes-agent/run_agent.py)（无效 API 响应重试、**`_try_activate_fallback`**、最终 `failed: True`）；工具如 [`tools/terminal_tool.py`](../../hermes-agent/tools/terminal_tool.py) **执行异常重试**；[`gateway/platforms/base.py`](../../hermes-agent/gateway/platforms/base.py) **`_send_with_retry`**。

### 8.1 要点

- **对话级**：无效响应 → **计数重试 + jittered backoff**；失败可 **切换 fallback 模型**；仍失败 → **persist + return dict**（含 **`failed`**）。
- **工具级**：terminal 等对执行异常 **有限次重试 + sleep**。
- **渠道投递**：网络类失败 **指数退避重试**，失败通知用户文案。

---

## 9. cc-fork-01（附录：范围限定）

**定位**：[`cc-fork-01/README.md`](../../cc-fork-01/README.md) 自述为 **Claude Code 暴露快照 + 研究笔记**，不是与 **tomcat/pi_agent_rust** 同级的「自研宿主产品」。

**抽样**：[`cc-fork-01/src/entrypoints/cli.tsx`](../../cc-fork-01/src/entrypoints/cli.tsx) 对 `result.error` 走 **`exitWithError`**——偏 **CLI 进程级失败**，而非长生命周期 session 内的 **可恢复 loop 策略**。

**本报告用法**：作为 **对照「商业 CLI 如何把错误交给用户退出」** 的脚注，**不参与**评分表横向强对比。

---

## 10. 横向小结：「继续 loop」vs「停机待人」

```text
  吸收错误进上下文（tool/assistant 文本） --------> 模型可自纠，loop 继续
           ^                        |
           |                        |
  tomcat tools    pi_agent_rust 单工具
  OpenClaw (Pi tool)    pi-mono（重试前删 assistant）

  整轮失败 return Err / AgentEnd(error) --------> 需要用户或上层换新输入/配置
           ^
           |
  tomcat Fatal / Aborted
  pi_agent_rust stream Err, execute_tool_calls Err
  OpenClaw retry-limit payload / FailoverError
```

---

## 11. 评分表（1–5 分）与说明

**说明**：分数表示 **「该维度在『工程上可借鉴、且与 Wasm/窄宿主契合」的综合表现**——**非**「产品谁更强大」。**cc-fork-01** 标 **N/A**。

**图例**：1 = 弱/不适用；3 = 中；5 = 强且易借鉴。

| 维度 | tomcat | pi_agent_rust | OpenClaw | pi-mono | hermes-agent |
|------|--------------|---------------|----------|---------|--------------|
| **工具失败→观测、不崩局** | **5** 显式 `(String,is_error)`，与 transcript 一致 | **5** `ToolOutput` + 单工具 Err 文本化 | **4** Pi 惯例以 tool result 回填（经多层） | **4** Pi core 管线成熟 | **4** 工具多返回结构化错误串 |
| **LLM 错误分层（可重试/致命）** | **5** `classify_error` + Retryable + L3 路径清晰 | **4** 流式 Err 偏「终局失败」；与分类器不同风格 | **5** failover 政策与多阶段极全 | **4** 正则可重试 + 会话重试 | **4** 重试+换模，偏运行手册式 |
| **会话/Attempt 自动重试** | **5** `max_attempts` + 事件 | **3** 主在单次 run 内工具迭代 | **5** 嵌入式多 attempt | **5** `auto_retry` + `continue` | **4** 对话内循环 |
| **模型/后端降级（fallback）** | **2** 主要单 provider 配置 | **3** 扩展/多 profile（见 EXTENSIONS） | **5** fallback_model + Pi harness | **3** 视 ModelRegistry | **5** `_try_activate_fallback` |
| **扩展失败不拖死主流程** | **3** VM/插件另线；宿主以事件为主 | **5** fail-open 面广、可审计 | **4** 插件诊断+reload 叙事 | **3** 扩展冲突「不卸载」非 fail-open | **2** 非主维度 |
| **实现复杂度 vs 本仓库 Wasm 方向** | **5** 同仓库、Rust 枚举可维护 | **4** 可参考 fail-open 与 ToolOutput | **2** 策略面过大，不宜整锅端 | **3** TS 会话重试可局部学 | **2** Python 单体，难直搬 |

---

## 12. 适合 tomcat 的优选组合（结论）

以下按 **「保持窄宿主 + 清晰错误语义」**（与 [plugin_skills_first_principles_pi_rust_wasm.md](plugin_skills_first_principles_pi_rust_wasm.md) 的 **窄契约** 一致）给出 **推荐 / 慎用 / 不建议**。

### 12.1 建议 **保留并加强**（已具备）

- **`classify_error` + Attempt 退避 + 上下文溢出与 L3 修剪**（[`run.rs`](../../src/core/agent_loop/run.rs)）：与 **pi_agent_rust** 的「重试耗尽再失败」、**OpenClaw** 的 retry-limit 叙事 **同构**，适合作为 **Wasm 宿主** 的 **主防线**。
- **工具失败 → `(result, is_error)` 写入 tool 消息**：与 **pi_agent_rust** 单工具 **ToolOutput** 一致，**应维持**——这是 **让 loop 自愈** 的关键。

### 12.2 建议 **择机吸收**（按产品阶段）

- **pi-mono 式「可开关的会话级 LLM 自动重试」**：在 **TUI/配置** 中可选打开，**正则/可重试错误** 与 **`agent.continue()` 等价物**（在 tomcat 即 **再度 `AgentLoop::run`** 或内部 `continue` API）——**降低** transient 提供商错误对用户的心理摩擦；成本是 **状态机复杂度**，需与 **`max_attempts`** 分工（一层 HTTP、一层会话）。
- **pi_agent_rust 式扩展钩子 fail-open**：扩展/VM 路径逐步对齐 **`warn!` + 不阻断主循环**（已有部分事件模型时可映射）。

### 12.3 **慎用**

- **OpenClaw 全量 failover-policy + 多 harness**：能力最强，但依赖 **Gateway/多模型/账号 profile** 产品线；**仅当** tomcat 走向 **多通道控制台** 再分段引入。
- **hermes-agent 级「单体里叠大量工具局部重试」**：易增加 **不可预测延迟**；Wasm 宿主更适合 **统一退避策略**（已在 Attempt 层）而非每工具自成一套。

### 12.4 **不建议照搬**

- **pi_agent_rust `stream_assistant_response` Err 即 `return Err`**：对 **CLI 嵌入** 合理；若 tomcat 要强化 **「提供商瞬断自愈」**，应优先 **Attempt 层 Retryable**（你已部分实现），而非一上来终局失败。
- **cc-fork-01 CLI `exitWithError`**：适合 **一次性 CLI**，不适合 **长会话 agent**。

### 12.5 一句话路线图

**以当前 tomcat 三层循环为骨架，工具层保持「错误→tool 观测」；LLM 层继续用分类 + Attempt 重试与 L3；按配置增加「会话级软重试」可选包；扩展走 fail-open 审计。OpenClaw 级多模型 failover 等产品化能力留到真的出现 Gateway 需求再做。**

---

## 13. 参见与索引

| 文档 / 路径 | 用途 |
|-------------|------|
| [`docs/TODOS.md`](../TODOS.md) | P0/P1 中与中断、流式、重试相关条目（如 `#T-003`/`#T-006`/`#T-072` 等，以文内 `[x]` 为准） |
| [`plugin_systems_openclaw_pi_mono_pi_agent_rust.md`](plugin_systems_openclaw_pi_mono_pi_agent_rust.md) | 三端插件与注册面对照 |
| `pi_agent_rust/src/agent.rs` | Rust Agent 错误与钩子 fail-open |
| `openclaw/src/agents/pi-embedded-runner/run/failover-policy.ts` | OpenClaw failover 决策源 |
| `pi-mono/.../agent-session.ts` | 会话自动重试实现 |

---

*本报告基于 Tomcat 工作区内仓库快照；若上游分支变更，以各仓库源码为准。*
