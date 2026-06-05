# `web_search` 工具：多供应商检索、归一化与 OpenAI server-side 注入

本文档是内置 **`web_search`** 工具的技术方案（OpenSpec **B 类**：`docs/architecture/tools/`），与兄弟文档 [`web_fetch.md`](web_fetch.md) **拆为两份独立满额文档**——双工具协议、PR 节奏、风险表互不依赖；共享术语（`url` / `cache key` / `SSRF` 等）在两篇各自完整书写，便于单篇审阅、单篇冻结。

**文首声明（与 `read.md` 全篇闭环口吻不同）**：

- **§3、§5**：描述本期 PR-WS-A/S/O/W 合入后的**目标态行为**与代码锚点；与实现不一致处以 **`src/` 代码为准**。
- **§1 观察指标表、§2.3–§2.4、§9、§10、§11**：描述**契约草案与路线图**（与 002 看板 [T2-P1-012](../../agents/TASK_BOARD_002/tasks/T2-P1-012.md) 一致）；合入后以 PR 更新本文状态列。

写作约定见 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)（B 类：术语、调研、目标、**§4.1/§4.2**、One-Glance、测试、风险）。

> **实现锚点校准（2026-06 重构后核对，落地时以此为准）**：本文起草后 `src/` 与参考仓库均有重构，以下锚点已漂移——
>
> 1. **`tool_exec` 已从单文件升为目录模块**：`src/core/agent_loop/tool_exec/`。中央分发在 [`tool_exec/mod.rs::execute_tool_tuple_full`](../../../src/core/agent_loop/tool_exec/mod.rs) 的 `match tc.name.as_str()`；每个工具的处理函数放在 `tool_exec/branches/<tool>.rs`（形如 `handle_read` / `handle_bash`），并在 [`tool_exec/branches/mod.rs`](../../../src/core/agent_loop/tool_exec/branches/mod.rs) 注册。**本文凡提「`tool_exec.rs` 加 `match "web_search"`」均指**：新增 `tool_exec/branches/web_search.rs`（`handle_web_search`）→ 在 `branches/mod.rs` 导出 → 在 `tool_exec/mod.rs` 的 match 增一臂。
> 2. **工具配置类型已移位**：`src/infra/config/types.rs` → [`src/infra/config/types/tools.rs`](../../../src/infra/config/types/tools.rs)；聚合结构为 `ToolsConfig { read, write, bash }`（各 `Tools*Config` 子表）。`ToolsWebSearchConfig` 应作为新子表加进 `ToolsConfig`。
> 3. **PR-WS-O 已落为 sidecar 路径**：hosted 请求不再通过 [`openai_responses/mod.rs`](../../../src/core/llm/openai_responses/mod.rs) / [`openai_responses/payload.rs`](../../../src/core/llm/openai_responses/payload.rs) 注入当前对话请求；实际落点是 [`web_search/mod.rs::execute_openai_hosted`](../../../src/core/tools/web_search/mod.rs) 负责选 hostedCandidateModel 与凭证，再委托 [`web_search/openai_server.rs::search_openai_hosted`](../../../src/core/tools/web_search/openai_server.rs) 构造并发送独立 `POST /v1/responses`，随后由 `parse_server_tool_blocks` 归一化结果。
> 4. **`registry.rs` 行号**：`PROVIDERS` 常量现位于 [`registry.rs` L53-56](../../../src/core/llm/registry.rs)（原文 L47-50 已失效）；`("openai", …)` / `("openai-responses", …)` 两条不变。
> 5. **新依赖**：`Cargo.toml` 当前**无** `moka`（无任何 LRU 工具）——PR-WS-S 的缓存须新增该依赖；`reqwest 0.12` / `xxhash-rust(xxh32)` 已在。
> 6. **openclaw 门闩已变更**（见 §2.2 / §2.4.3 已就地更新）：`isCodexNativeSearchEligibleModel` 现仅判 `modelApi === "openai-chatgpt-responses"`。
> 7. **新增的子 Agent 工具白名单门闩**（重构引入）：[`tool_exec/guard.rs`](../../../src/core/agent_loop/tool_exec/guard.rs) 的 `is_reviewer_whitelisted_tool` / `is_verifier_whitelisted_tool` 会**拒绝**白名单外的任何工具。`web_search` 默认**不在**白名单内——PR-WS-A 须显式决定：reviewer/verifier 子 Agent 是否允许联网检索（多数审查场景应**保持禁止**，则无需改 guard；若要放开则在此二函数补名）。`tool_exec_unknown_tool_for_web_search_without_backend` 之外应另加一条「reviewer 调 web_search 被拒」断言。
> 8. **【重大设计修订】server-side 资格从「看当前模型 wire + capability」改为「看项目级 hosted 候选模型」**——这是本次复核后的新裁决，已就地改写 §1 / §1.1 G2 / §2.2 / §2.3 / §2.4.2 / §2.4.3 / §3 / §4.1 / §7 / §8 / §10 / §11。背景四概念（这次明确拆开）：
>    - **currentModel（当前对话模型）** = 当前会话正在用来推理的模型；**只负责主对话**，**不再参与** `backend=auto` 的 hosted 首选判断。
>    - **hostedCandidateModel（项目级 hosted 候选模型）** = 合并后的 model catalog（`models.toml` + builtin）中首个 `capabilities.web_search == true` 的模型；若有多个，按合并后的顺序取首个；若没有，则 `auto` 没有 hosted 首选。
>    - **wire（管线）= 某个模型自己的 `api` 字段**（`openai` = Chat Completions wire / `openai-responses` = Responses wire）。resolver（[`resolver.rs::build_provider_config` L280](../../../src/core/llm/resolver.rs)）把 `entry.api` 灌进 `LlmConfig.provider`，再由 [`registry.rs`](../../../src/core/llm/registry.rs) 选实现——**现在只关心 hostedCandidateModel 自己的 wire**，不再拿当前对话模型的 wire 做 `auto` 门闩。
>    - **vendor（厂商）= `models.toml` 的 `provider` 字段**：**仅**用于选 `<VENDOR>_API_KEY`（如 `provider="mimo"`→`MIMO_API_KEY`），**与能不能 server-side 检索无关**。
>    - **capability（能力）= 模型是否真支持 hosted `web_search` tool**：**事实源是各厂商官方文档**，登记到 [`models.toml` 的 `capabilities`](../../../src/core/llm/catalog.rs)（新增 `web_search` 位，默认 `false`）。
>
>    **正确规则 = 项目里存在 hostedCandidateModel → `auto` 先尝试 `openai(hosted)`；项目里不存在 hostedCandidateModel → 直接进入 HTTP `auto` 链（优先 Tavily，再 Brave，再 Serper）**。当前对话模型的 `api` 是否 `openai-responses` **不再参与** `auto` 资格判断。若 hostedCandidateModel 自身配置错误或运行时不可用，`auto` 再回 HTTP 链；显式 `openai` 则报错。**只看当前模型 wire 会错**：用户可能正用 DeepSeek / Chat Completions 对话，但项目里已经单独配置了一个可做 hosted `web_search` 的模型；这时 `auto` 仍应先用那个项目级 hosted 候选，而不是因为当前模型不是 Responses 就放弃 hosted。

---

## 目录

- [1. 目标与设计原则](#1-目标与设计原则)
- [2. 竞品 / 选型对比](#2-竞品--选型对比)
  - [2.1 检索工具的典型关切](#21-检索工具的典型关切)
  - [2.2 常见实现横向对比](#22-常见实现横向对比)
  - [2.3 落地选型决策表](#23-落地选型决策表)
  - [2.4 实施点（路线图）](#24-实施点路线图)
  - [2.4.1 PR-WS-A：catalog 注册与 system_prompt](#241-pr-ws-acatalog-注册与-system_prompt)
  - [2.4.2 PR-WS-S：自家 HTTP backends（Tavily / Brave / Serper）](#242-pr-ws-s自家-http-backendstavily--brave--serper)
  - [2.4.2.1 HTTP 上游字段速查（实现必读）](#2421-http-上游字段速查实现必读)
  - [2.4.3 PR-WS-O：OpenAI server-side 注入与归一化](#243-pr-ws-oopenai-server-side-注入与归一化)
  - [2.4.4 PR-WS-W：域名守卫与 SSRF 拒接](#244-pr-ws-w域名守卫与-ssrf-拒接)
- [3. 术语统一](#3-术语统一)
- [4. 协议（入参 / 出参 / Schema）](#4-协议入参--出参--schema)
- [5. One-Glance Map（文件职责总览）](#5-one-glance-map文件职责总览)
- [6. 调度时序（运行时图）](#6-调度时序运行时图)
- [7. 状态机（backend 选择）](#7-状态机backend-选择)
- [8. 配置与环境变量](#8-配置与环境变量)
- [9. 错误模型 / 截断 / 警告](#9-错误模型--截断--警告)
- [10. 测试矩阵（验收）](#10-测试矩阵验收)
- [11. 风险与应对](#11-风险与应对)
- [12. 历史决策（已被本方案取代或待定）](#12-历史决策已被本方案取代或待定)
- [13. 关联文档](#13-关联文档)

---

## 1. 目标与设计原则

**一句话**：让模型一句 `query` 拿到一组**结构归一**、**可审计**、**可缓存**的网页 hits，**多 backend 透明切换**——只要项目级 model catalog 中存在 `capabilities.web_search == true` 的 hosted 候选模型，就把它作为 `auto` 的首选 backend，发起一笔独立的 hosted search 请求；若项目里不存在该候选，或该候选运行时不可用，则进入 HTTP `auto` 链：优先 Tavily，当前候选若缺 key、认证失败、429 / 5xx、timeout 或 transport fail，再顺序降到 Brave、Serper；不抓正文（正文走 [`web_fetch.md`](web_fetch.md)）。**判定跟项目里有没有 hosted 候选走，不跟当前对话模型的 `api` / vendor 名走**——详见文首校准块第 8 条。

### 1.1 观察指标表（与 §10 验收一一对应）

| 目标 | 观察指标（落地后可核对） | 说人话 |
|------|--------------------------|--------|
| G1 query→hits 闭环 | catalog 注册 `web_search`；同一 `query` 经任一 backend 都返回相同形状的 `hits[]`（含 `title/url/snippet/position`，可选 `published_at`） | 不管换哪个 backend，模型读到的都是同一份字段。 |
| G2 多 backend 透明切换 | `backend=auto`：若项目里存在 `capabilities.web_search == true` 的 hosted 候选模型 → `openai(hosted)` 为首选；否则进入 HTTP 链 `tavily → brave → serper`；若 hosted / HTTP 当前候选缺 key、401/403、429/5xx、timeout、transport fail 则继续下一家；显式 `backend=…` 不降级；切换记 `warnings[]` | hosted 首选看项目里有没有可联网模型，不看当前聊天模型；自动模式会继续找能用的 backend。 |
| G3 hits 归一化 | 输出 `{ hits, query, backend, stats, truncated, warnings }` 单一 schema；上游各 provider 的特异字段在 adapter 内吃掉 | 模型不需读三套 JSON，调用方一份解析即可。 |
| G4 缓存命中 | 进程内 LRU + TTL（默认 5 min / 50 条）；key=`(backend, query, count, freshness, country, language, domain_filter, allowed_domains, blocked_domains)`；命中 → `stats.cached=true` 不再发 HTTP | 同会话短时间内重复检索免账单、免速率限制；配置域约束变化不复用旧缓存。 |
| G5 SSRF 守卫 | hits.url 归一化阶段解析 + 拒任意 IP literal / loopback / 私网 / 内网保留 hostname / 无 host；`allowed_domains` / `blocked_domains` 在结果集级别过滤 | 别让模型以为 `http://127.0.0.1` 或 `https://metadata.google.internal` 是合法搜索结果。 |
| G6 cost 与限速归一化 | `count` 默认 5、上限 20；429 / 5xx → `truncated=true + warning`，不抛 `Err`；超 `max_result_size_chars` 软上限做 snippet 截断 + warning | 别让一次检索扛上整轮上下文，也别一报错就整轮 fail。 |

### 1.2 非目标

| 非目标 | 推给 | 说人话 |
|--------|------|--------|
| 抓取并解析单个 URL 的正文 | [`web_fetch.md`](web_fetch.md) | 检索是「找路标」，抓正文是另一个工具。 |
| 服务端 domain blocklist API | 003 迭代（自托管栈无对应服务端） | 不远程拉黑名单，靠本地配置。 |
| Firecrawl / Parallel / Exa 全量接入 | 路线图，按需 | hermes 把 4 家 backend 一锅端，pi 暂只做 Tavily/Brave/Serper 三家 + OpenAI server-side。 |
| 浏览器 / 渲染型抓取 | 002 看板后续任务（headless browser） | 检索工具不跑浏览器，渲染由专用工具承担。 |
| MCP 转接同名工具 | 003 迭代 | 内置 vs MCP 同名冲突另起 ADR；本期内置版优先。 |
| LLM 二次摘要 hits | system_prompt 引导即可 | 模型自己看 hits 列表，工具不替它再摘一次。 |

---

## 2. 竞品 / 选型对比

对标过 [agent-tools-comparison.md](../../reports/agent-tools-comparison.md) 中 **cc-fork-01 / hermes-agent / openclaw / pi-mono / pi_agent_rust** 五栈的 web 工具策略。下表为**已写入路线图的决策**，不是待办 brainstorm。

### 2.1 检索工具的典型关切

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  本地 web_search 类工具通常要同时解决的四类问题                              │
├────────────────────┬─────────────────────────────────────────────────────┤
│  Provider 异构     │  Tavily / Brave / Serper / Anthropic native /         │
│                    │  OpenAI server-side：字段不同、限速不同、付费不同     │
│  归一化            │  上游 5 套 hits 形状 → 模型只想读一套；schema 单一事实源 │
│  上下文 cost       │  10 条结果 × 200 字 snippet ≈ 2 KiB；count + 软上限    │
│  安全与合规        │  API key 泄漏 / SSRF (hits.url 指向私网) / 限速归一化   │
└────────────────────┴─────────────────────────────────────────────────────┘
```

### 2.2 常见实现横向对比

| 来源 / 形态 | Provider 抽象 | 默认 backend | 输入字段 | 安全 / 限速 | 备注 |
|-------------|--------------|--------------|----------|-------------|------|
| **cc-fork-01** | 内嵌 Anthropic server-side `web_search` 工具 | Anthropic native | `query` + `allowed_domains` + `blocked_domains` | server 侧代搜，限速跟模型走 | 见 [`WebSearchTool.ts`](../../../../cc-fork-01/src/tools/WebSearchTool/WebSearchTool.ts)；fetch 工具单独存在 |
| **hermes-agent** | `_get_backend()` 多分支：Parallel / Exa / Tavily / Firecrawl | `config.yaml` 的 `web.backend` 优先；否则按 **Firecrawl→Parallel→Tavily→Exa** 顺序挑**首个**有 env/gateway 的（[`_get_backend` L128-143](../../../../hermes-agent/tools/web_tools.py)）；若全无则仍返回 `"firecrawl"` 字符串（[`L145`](../../../../hermes-agent/tools/web_tools.py)） | `query` + `limit` | 各 backend 自带；`web_extract` 另走 `is_safe_url` 等 | 见 [`web_tools.py`](../../../../hermes-agent/tools/web_tools.py)，多 backend 抽象最宽；**与 pi「非 Responses 默认 Tavily」不同——属产品默认取舍，非实现错误** |
| **openclaw** | `provider-web-search-contract` plugin SDK + Codex native 注入 | Codex native **仅当** `isCodexNativeSearchEligibleModel` 为真——**现仅判** `modelApi === "openai-chatgpt-responses"`（见 [`codex-native-web-search-core.ts` L35-49](../../../../openclaw/src/agents/codex-native-web-search-core.ts)；**2026-06 已简化**，旧版的 `modelProvider === "openai-codex"` / `modelApi === "openai-codex-responses"` 已删）/ 否则 managed `web_search` 工具 | `query` + 上下文 size + freshness + filters | 限速由 OpenAI server 兜底 | 见 [`codex-native-web-search-core.ts`](../../../../openclaw/src/agents/codex-native-web-search-core.ts)（`buildCodexNativeWebSearchTool` 现 L160）；**pi 对齐其「仅特定 API 管线可走 native」思路，但 provider 字符串映射到本仓 `openai` vs `openai-responses`** |
| **pi-mono** | — | — | — | — | **没做** web 工具；通过 MCP 转接外部检索（如 Tavily MCP server） |
| **pi_agent_rust** | — | — | — | — | **没做** web 工具；本仓本期补齐 |
| **本仓库 `web_search`**（路线图） | `trait WebSearchBackend` + 4 适配器 | `backend=auto`：项目里若存在 `capabilities.web_search == true` 的 hosted 候选模型 → `openai(hosted)` 首选；否则 → Tavily→Brave→Serper 降级链 | `query` + `count` + `freshness` + `country` + `language` + `domain_filter` | hits.url 归一化阶段 **本仓（tomcat）增补** SSRF + 限速 → `truncated+warning` | managed schema 对齐 openclaw；**hosted 首选 = 项目级 hosted 候选模型**，不再绑定当前对话模型的 wire，见文首校准块第 8 条 |

**结论（写入路线图）**：**多 backend 抽象**对齐 **hermes-agent**；**OpenAI server-side 注入路径**对齐 **openclaw**；**字段集**对齐 **openclaw**（query + freshness + country + language + filters）；**统一 hits 归一化**取 **cc-fork-01** 的 `Output { query, results, durationSeconds }` 思路；**默认 backend `auto`** 兼顾两路。

### 2.3 落地选型决策表（维度取舍）

**代码落点、交付物、阶段**见 **[§2.4](#24-实施点路线图)**，与 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§4.1 / §4.2** 分工一致。**`决策`** 列钉本行裁决结论（**SHOULD**）。

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| **工具与文档拆分** | `web_search` / `web_fetch` 是否合一 | **采用** `web_search` / `web_fetch` 双工具双文档。 | cc-fork / hermes / openclaw（双工具）+ 本仓库文档惯例 | schema、权限（fetch 按 domain）、缓存键（query vs url）分离；**一工具一 md** 与 read/write/edit/bash 一致 | × 单文件双口吻、PR 互相拖拽 | 一个工具一份文档；找路标 vs 抓正文是两件事。 |
| **多供应商抽象** | 换 key / 换模型是否丢检索能力 | **采用** `WebSearchBackend` trait + 多 adapter。 | hermes-agent + 本仓库路线图 | `WebSearchBackend` + OpenAI server-side / Tavily / Brave / Serper；OpenAI 系可走注入免自建 HTTP | × 仅单一 HTTP / 仅 server-side | 一套接口、四个 provider；坏一个不影响其他。 |
| **默认 backend** | 用户无配置时走哪条路 | **采用** `auto`：项目里有 hosted 候选模型 → server-side；否则 → Tavily。 | 用户指定的项目级 hosted backend 取向 + openclaw「eligible 才 native」思路 | **`auto`**：只要合并后的 model catalog 中存在 hostedCandidateModel（首个 `capabilities.web_search == true` 的模型）→ `openai(hosted)` 作为首选；否则 → Tavily；`[tools.web_search] backend` 可强制 | × 把 hosted 资格绑死在当前对话模型；× 没项目级 hosted 候选时还强行走 hosted | hosted 是项目能力，不是当前模型身份。 |
| **hosted 候选选择** | 项目里多个 `capabilities.web_search == true` 模型时选谁 | **采用** 合并后 catalog 首个匹配项；当前对话模型不参与资格判断。 | model merge 顺序 + 实现可预测性 | 项目自定义条目 / 覆盖后的顺序天然可控；无需新增第二套 selector 配置就能稳定选出一个候选 | × 跟当前模型走；× 多个候选时无规则挑选 | 只要项目里配了可联网 hosted 模型，就按固定顺序挑一个先用。 |
| **上游 API 契约来源** | adapter 该按什么 header / body / query 接 | **采用** 各 provider 官方 API 文档 + 工作区参考实现双锚。 | Tavily / Brave / Serper 官方文档 + openclaw / hermes | 认证头、参数名、响应 shape 都有权威来源；工作区实现只做交叉校对 | × 只看社区博客；× 凭经验猜字段 | 接哪家先看官方 docs，再用仓库实现对照。 |
| **密钥配置位置** | 三家 HTTP backend 的 key 放哪、怎么让用户配 | **采用** per-provider env secrets：运行时读 `~/.tomcat/assets/.env` 或进程 env，本地开发 / 真 API 用例读仓库根 `.env`（由 `.env.example` 提供样板）。 | 本仓 `.env` / `user-guide.md` / 测试现状 / 安全要求 | 多 backend 可同时持有多家 key，适配 `auto` 链；secret 不进 schema、不进共享 `api_key` 字段、不进审计 | × 单个共享 `api_key`；× 把 secret 写进 `tomcat.config.toml` 或 tool args | key 放 `.env` / secret 管理里，别让模型或配置文件看到。 |
| **优先级与运行时降级** | 一个 backend 不通时怎么办 | **采用** `auto` 下 `openai(hosted, project candidate) → tavily → brave → serper → disabled`；候选若缺 key、401/403、429/5xx、timeout、transport fail 则顺序切下一个；显式 backend 不降级。 | hermes 多 backend 思路 + 用户指定的项目级 hosted 优先级 | 自动模式重可用性，手动模式保用户意图；不把“点名某家”偷偷改成别家 | × `auto` 只试一家就失败；× 显式 backend 静默切别家 | 自动模式先试项目里那台能做 hosted 搜索的模型，不行再试 HTTP。 |
| **输入字段集** | LLM 常用约束是否进 schema | **采用** 6 字段 schema（query/count/freshness/country/language/domain_filter）。 | openclaw `web-search.ts` 字段集 | `query`+`count`+`freshness`+`country`+`language`+`domain_filter`；跨 provider 归一化 | × 一锅端塞 `page_token` 等易混字段 | 6 个字段够用；不要让模型猜怎么 query。 |
| **输出归一化** | 模型是否要学多套 JSON | **采用** 统一 `{ hits, stats, warnings, backend }`。 | cc-fork-01（Output 思路）+ hermes | 单一 `{ hits, stats, warnings, backend }`；`warnings` 透传截断/限速 | × 原样三套 shape；× 仅 title+url 过短 | 模型只学一套字段。 |
| **缓存策略** | 同会话重复 query 成本 | **采用** 进程内 LRU + TTL（key 含 backend 与全参）。 | cc-fork-01 `utils.ts` LRU 思路 | 进程内 LRU + TTL；key 含 backend 与全参数字段 | × 不缓存烧钱；× 落盘持久化非目标 | 同样的搜索别花两次钱。 |

### 2.4 实施点（路线图）

**实施顺序**：**① PR-WS-A**（catalog 注册 + system_prompt + tool_exec match）→ **② PR-WS-S**（trait + 自家 HTTP 三 backend + 缓存）→ **③ PR-WS-O**（OpenAI server-side 注入）→ **④ PR-WS-W**（域名守卫 + SSRF）。**先注册再补 backend**——避免 PR-WS-S 后续测试与 prompt 反复改字面量。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
| --- | --- | --- | --- | --- |
| **PR-WS-A**（命名 + catalog） | **交付物**：6 字段 schema；占位 friendly err。**落地点**：catalog / `tool_exec` / system_prompt | [`catalog.rs`](../../../src/core/tools/contract/catalog.rs)、[`tool_exec/mod.rs`](../../../src/core/agent_loop/tool_exec/mod.rs)（match 增臂）+ 新 [`tool_exec/branches/web_search.rs`](../../../src/core/agent_loop/tool_exec/branches/mod.rs)、[`system_prompt.rs`](../../../src/core/llm/system_prompt.rs) | `core::tools::contract::catalog::tests::web_search_registered`、`core::agent_loop::tests::submodules_test::tool_exec_web_search_requires_runtime_injection`（**PASS, 2026-06-04**） | 先把名字 / schema / 占位放好，后面 PR 接 backend 不改字面量。 |
| **PR-WS-S**（自家 HTTP backends） | **交付物**：trait + 三 HTTP adapter；LRU/TTL；归一化 hits。**落地点**：`core/tools/web_search/*`、`ToolsWebSearchConfig` | 新模块 `core/tools/web_search/{mod,types,backend,tavily,brave,serper,cache}.rs`、[`infra/config/types/tools.rs`](../../../src/infra/config/types/tools.rs) 的 `ToolsConfig` 加 `ToolsWebSearchConfig` 子表；缓存需新增 `moka` 依赖 | `tavily_runtime_maps_request_and_normalizes_hits`、`auto_backend_falls_back_to_brave_and_then_hits_cache`、`auto_backend_falls_back_after_brave_timeout`、`explicit_tavily_rate_limit_returns_degraded_output`、`runtime_explicit_tavily_works_from_public_api`、`runtime_explicit_serper_works_from_public_api`（**PASS, 2026-06-04**） | 三个 provider 接进来，模型不知道里面跑哪家；`auto` 先看项目里有没有 hosted 候选。 |
| **PR-WS-O**（OpenAI hosted backend 调度与归一化） | **交付物**：project-level hosted 候选模型请求；server_tool 结果→统一 hits。**落地点**：`core/tools/web_search/{mod,openai_server}.rs` | [`web_search/mod.rs::execute_openai_hosted`](../../../src/core/tools/web_search/mod.rs)（解析 hosted 候选模型、凭证与 fallback）+ [`openai_server.rs`](../../../src/core/tools/web_search/openai_server.rs)（构造 sidecar hosted 请求 + 归一化） | `discover_hosted_candidate_uses_merged_catalog_order`、`auto_backend_uses_project_hosted_candidate`、`explicit_openai_requires_project_candidate`、`parse_server_tool_blocks_handles_openai_and_server_tool_shapes`、`runtime_explicit_openai_uses_project_hosted_candidate`（**PASS, 2026-06-04**） | `auto` 只要项目里有 hosted 候选就优先用它；当前对话模型是不是 Responses 不重要。 |
| **PR-WS-W**（域名守卫 + SSRF） | **交付物**：hits 阶段 URL 守卫与域过滤。**落地点**：`web_search/cache.rs`、`types.rs` | `web_search/cache.rs`、`web_search/types.rs` | `normalize_hits_filters_private_hosts_and_domain_rules`（**PASS, 2026-06-04**） | 别让搜出来的 URL 指向 127.0.0.1。 |

下文按实施点展开**技术要点与示意图**；**交付边界与代码落点仍以表为准**。

#### 2.4.1 PR-WS-A：catalog 注册与 system_prompt

- **交付**：`BUILTIN_TOOL_CATALOG` 增加条目 **`name = "web_search"`**；`web_search_parameters()` 输出 6 字段 JSON Schema（见 §4.1）；[`system_prompt.rs`](../../../src/core/llm/system_prompt.rs) 工具描述对齐 [openclaw 的 description](../../../../openclaw/src/agents/tools/web-search.ts)（强调「source attribution」、「current date={today}」）；`tool_exec` 添加 `match "web_search"` 占位分支返回 friendly error，**不**注册 backend——直到 PR-WS-S/O 接入。
- **历史回放**：旧 transcript 若出现 `web_search`（无注册）→ 走 **未知工具** 路径（与 read 的 `read_file` 同口径，不重定向）。
- **与后续 PR 的衔接**：PR-WS-S 的 backend trait + adapter 直接挂到本步占位的 `tool_exec` 分支；PR-WS-O 替换其中 OpenAI 系的分支。

```text
  LLM / transcript
        │
        ▼
┌───────────────────┐     注册名仅 "web_search"
│  catalog.rs       │──────────────────────────────┐
└───────────────────┘                              │
        │                                            ▼
        ▼                               ┌────────────────────┐
  tool_exec  match "web_search"         │ "websearch" 等拼错 │
        │（PR-WS-A 占位 friendly err）    │ → UnknownTool 错误 │
        ▼                                └────────────────────┘
   PR-WS-S/O 接入后
   → backend.search(args)
```

**说人话**：先把名字、6 个参数、system prompt 文案放进去；后面接 backend 不再动 catalog。

#### 2.4.2 PR-WS-S：自家 HTTP backends（Tavily / Brave / Serper）

- **交付**：定义 `trait WebSearchBackend { async fn search(&self, args: &WebSearchArgs) -> Result<WebSearchOutput, AppError> }`；3 个 adapter（Tavily **`POST {base}/search`**、Brave **`GET …/res/v1/web/search`**、Serper **`POST https://google.serper.dev/search`**）经 reqwest 0.12（已存在依赖）调用；`backend.rs` 提供 `discover_hosted_candidate(model_catalog) -> Option<HostedCandidateModel>` + `pick_backend(hosted_candidate, cfg) -> …`（命名可调整，语义不能变）。`hostedCandidateModel` 的规则是：**合并后的 model catalog 中首个 `capabilities.web_search == true` 的模型**，与当前对话模型无关。若 `backend ∈ {auto, openai}` 且存在 hostedCandidateModel → **defer 到 PR-WS-O**（不在此模块起 reqwest）；否则 `auto` 按 **`tavily → brave → serper`** 选第一个可用 HTTP backend。若 `auto` 当前候选出现**缺 key、401/403、429/5xx、timeout、transport fail**，则记 `warnings += "backend_unavailable:<name>, fallback=<next>"` 后继续下一家；**400/422 等请求契约错误直接 `Err`**，避免把实现 bug 伪装成降级成功。显式 `tavily/brave/serper` 恒走 HTTP 且**不降级**。进程内 LRU `MokaCache<CacheKey, WebSearchOutput>` 默认 `5 min TTL / 50 entries`；每次 HTTP backend 调用包 `tokio::time::timeout(default 12s, …)`。
- **限速归一化**：在 **显式 backend** 下，429 / 5xx 不抛 `Err`，写入 `warnings += "rate_limited (backend=tavily,status=429)"` + `truncated=true`；在 **`auto`** 下，429 / 5xx 归入「当前候选不可用」并进入下一家，直到链路耗尽才按最终状态返回。
- **配置承接**：`ToolsWebSearchConfig { backend, count, freshness, country, language, domain_filter, blocked_domains, allowed_domains, cache_ttl_secs, cache_capacity, timeout_ms, tavily_base_url, brave_base_url, serper_base_url }` 作为新子表加到 [`infra/config/types/tools.rs`](../../../src/infra/config/types/tools.rs) 的 `ToolsConfig`；env 优先级 `TOMCAT__TOOLS__WEB_SEARCH__*`。**credentials 不进 config**：运行时从 `~/.tomcat/assets/.env` / 进程 env 读取 `TAVILY_API_KEY` / `BRAVE_API_KEY` / `SERPER_API_KEY`，本地 real-API / 开发测试可用仓库根 `.env`（由 `.env.example` 提供样板）；OpenAI hosted 路径则复用 **hostedCandidateModel 对应** 的 key source，而不是当前对话模型的 key source。

```text
  WebSearchArgs
       │
       ▼
┌──────────────────┐         hit
│  cache.lookup    │─────────────▶  return cached + stats.cached=true
│  (backend, args) │
└────────┬─────────┘
         │ miss
         ▼
┌──────────────────┐
│ pick_backend()   │  auto / explicit
└────────┬─────────┘
         │
   ┌─────┴─────┬───────────┬───────────┐
   ▼           ▼           ▼           ▼
 tavily.rs   brave.rs   serper.rs   openai_server.rs (PR-WS-O)
   │           │           │           │
   └─────┬─────┴─────┬─────┘           │
         │           │                  │
         ▼           ▼                  ▼
   reqwest GET/POST + tokio::timeout    inject + parse blocks
         │           │                  │
         └─────┬─────┴──────────────────┘
               ▼
         normalize → WebSearchOutput
               │
               ▼
         cache.put + return
```

**说人话**：trait + 三个适配器把上游 JSON 揉成同一份 `hits[]`；超时和 429 都归一化成 `warnings`，不抛错；命中缓存就跳过 HTTP。

#### 2.4.2.1 HTTP 上游字段速查（实现必读）

本节把 **`WebSearchArgs` → 各 vendor 真实 HTTP 形态**钉死，避免 adapter 里「字段名想当然」。**权威顺序**：各 vendor 官方文档 > 下表引用的 **仓库内参考实现** > 本文句子；若官方改版，以官方为准并回写本节。

##### 官方 API 文档入口与密钥来源

| backend | 官方文档 | 鉴权 / 获取 key | 运行时 secret 读取位置 | 说人话 |
|---------|----------|-----------------|------------------------|--------|
| Tavily | [Search API](https://docs.tavily.com/documentation/api-reference/endpoint/search)、[Quickstart](https://docs.tavily.com/documentation/quickstart) | 官方 OpenAPI 为 `Authorization: Bearer <TAVILY_API_KEY>`；key 从 Tavily dashboard 领取 | 运行时优先 `~/.tomcat/assets/.env` 或进程 env 的 `TAVILY_API_KEY`；本地开发 / 真 API 用例读仓库根 `.env` | 实现以官方 Bearer 头为准，不沿用旧 body `api_key` 写法。 |
| Brave | [Quickstart](https://api-dashboard.search.brave.com/documentation/quickstart)、[Web Search API](https://api-dashboard.search.brave.com/api-reference/web/search/post) | `X-Subscription-Token: <BRAVE_API_KEY>`；key 在 Brave dashboard 创建 | 运行时优先 `~/.tomcat/assets/.env` 或进程 env 的 `BRAVE_API_KEY`；本地开发 / 真 API 用例读仓库根 `.env` | Brave 是 auto 链第二个 HTTP 候选。 |
| Serper | [Serper Docs](https://docs.serper.dev)、[Serper 官网](https://serper.dev) | `X-API-KEY: <SERPER_API_KEY>`；key 在 Serper 控制台领取 | 运行时优先 `~/.tomcat/assets/.env` 或进程 env 的 `SERPER_API_KEY`；本地开发 / 真 API 用例读仓库根 `.env` | Serper 是 auto 链第三个 HTTP 候选。 |

**说人话**：这张表解决两个问题: 一是 adapter 的字段和鉴权以哪份文档为准，二是用户到底去哪里放 key。结论就是「官方 docs 定协议，`.env` / secret 管理定凭证」。

##### Tavily Search API

| 项 | 内容 |
|----|------|
| **方法 / 路径** | `POST` `{TAVILY_BASE_URL}/search`；默认 base `https://api.tavily.com`（hermes 用 env `TAVILY_BASE_URL`，见 [`web_tools.py` L324-340](../../../../hermes-agent/tools/web_tools.py)；openclaw 用 `resolveTavilyBaseUrl`，见 [`tavily-client.ts` L52-65、L148-155](../../../../openclaw/extensions/tavily/src/tavily-client.ts)）。 |
| **鉴权** | Tavily 官方 OpenAPI `securitySchemes.bearerAuth` 明确要求 **`Authorization: Bearer <TAVILY_API_KEY>`**（见上表官方文档；Quickstart curl 也使用 Bearer 头）。hermes / openclaw 的旧样例可作为**请求字段 / 结果归一化**参考，但**鉴权实现以官方 Bearer 头为准**，不再设计为 JSON body `api_key`。 |
| **请求体（JSON）** | 见 openclaw [`runTavilySearch` 构造 `body`](../../../../openclaw/extensions/tavily/src/tavily-client.ts) **L124-145**：必填语义 **`query`**、**`max_results`**（1–20，openclaw 在 L98-101 clamp）；可选 **`search_depth`**、`topic`、`include_answer`（bool）、**`time_range`**、`include_domains`、`exclude_domains`。openclaw 字段名即 Tavily API **snake_case**（`search_depth`、`time_range`、`include_domains`、`exclude_domains`）。 |
| **hermes 当前最小子集** | [`web_tools.py` L1152-1157](../../../../hermes-agent/tools/web_tools.py)：`query`、`max_results`、`include_raw_content: false`、`include_images: false`。pi 可保留后两项默认 `false` 以贴近 hermes，或与 openclaw 对齐仅发「有值的可选字段」——**二选一写进 adapter 注释即可**。 |
| **响应 → `Hit`（normalize）** | Tavily 每条常见字段 **`title` / `url` / `content`**（正文式摘要）；openclaw 将 `content`→`snippet`（[`tavily-client.ts` L157-167](../../../../openclaw/extensions/tavily/src/tavily-client.ts)）；可选 **`published_date`**。hermes 归一化见 [`_normalize_tavily_search_results` L347-361](../../../../hermes-agent/tools/web_tools.py)（`content`→`description`）。**pi `Hit.snippet` 统一取自 `content`**。 |
| **`freshness` 映射** | 工具入参 `day \| week \| month \| year` ↔ Tavily **`time_range`** 字符串 **`day` / `week` / `month` / `year`**（与 openclaw 文档 [`docs/tools/tavily.md` L77](../../../../openclaw/docs/tools/tavily.md) 一致）；`null` → **省略** `time_range`。 |
| **`domain_filter` 映射** | 非空时 → Tavily **`include_domains`: string[]**（openclaw L140-141）；**不要**误用成 Brave/Serper 参数名。 |
| **`country` / `language`** | Tavily `/search` **无**与 openclaw  generic `web_search` 对齐的一等字段；**省略**或 `warnings += "tavily_ignores_country_language"`（与 §4.1 表格一致）。 |

##### Brave Web Search API

| 项 | 内容 |
|----|------|
| **方法 / URL** | **`GET https://api.search.brave.com/res/v1/web/search`**（Brave Search API **Web Search**；**不是** `/v1/web/search` 这种省略前缀路径）。 |
| **鉴权** | Header **`X-Subscription-Token: <BRAVE_API_KEY>`**（Brave 控制台文档常用名；实现前以 [Brave Search API 文档](https://api-dashboard.search.brave.com/app/documentation/web-search/get-started) 为准）。 |
| **Query 参数（全部 query string，无 JSON body）** | 至少 **`q`**（查询串）。常用：**`count`**（1–20）、**`country`**（ISO-3166-1 alpha-2 或文档允许字面量）、**`search_lang`**、**`ui_lang`**、**`freshness`**（时间过滤：`pd` / `pw` / `pm` / `py` 等）、`offset`、`safesearch` 等——完整列表以官方 **Web Search Query Parameters** 为准。 |
| **`WebSearchArgs` 映射** | `query`→**`q`**；`count`→**`count`**；`country`→**`country`**；`language`→**`search_lang`**（Brave 期望语言代码，与 ISO 639-1 多数情况兼容）；`freshness` **枚举需映射为 Brave 字面量**：`day→pd`，`week→pw`，`month→pm`，`year→py`；`null`→省略 `freshness`。 |
| **`domain_filter`** | Brave **无** Tavily 式 `include_domains` 的等价一对一字段；MVP 可在 adapter **改写 `q`**（例如拼接 `(site:foo.com OR site:bar.org) …`）并 `warnings += "brave_domain_filter_via_query_rewrite"`，**或**文档化「仅 Tavily 路径应用 `domain_filter`」——**须在 `brave.rs` 顶部注释择一并贯彻**。 |

##### Serper.dev（Google Search）

| 项 | 内容 |
|----|------|
| **方法 / URL** | **`POST https://google.serper.dev/search`**（官方 Serper「Google Search」端点；自建兼容网关若存在则走配置 base，但 **path 仍为 `/search`**）。 |
| **鉴权** | Header **`X-API-KEY: <SERPER_API_KEY>`** + `Content-Type: application/json`（社区与官方示例一致；实现前核对 [serper.dev](https://serper.dev/) 当前说明）。 |
| **请求体（JSON）** | 至少 **`q`**。常用：**`num`**（条数）、**`gl`**（地域，小写国家码）、**`hl`**（界面/结果语言）、**`tbs`**（时间过滤，Google `tbs` 语法，如过去一天常用 **`qdr:d`**）。 |
| **`WebSearchArgs` 映射** | `query`→**`q`**；`count`→**`num`**（clamp 1–20）；`country`→**`gl`**；`language`→**`hl`**；`freshness`→**`tbs`**：`day→qdr:d`，`week→qdr:w`，`month→qdr:m`，`year→qdr:y`；`null`→省略 `tbs`。 |
| **响应 → `Hit`（normalize）** | 主流在 **`organic`** 数组；元素常见字段 **`title` / `link` / `snippet`**（`link` 映射为 `Hit.url`）。 |
| **`domain_filter`** | 与 Brave 类似，**无**一等价字段；可在 **`q` 内拼接 `site:foo.com` 约束**（与上表 Brave 策略一致），或仅对 Tavily 应用 `include_domains`——**与 `brave.rs` 同一产品策略**，避免三 adapter 三套 silently 不同语义。 |

##### 横比小结（给 `tavily.rs` / `brave.rs` / `serper.rs` 文件头注释用）

| `WebSearchArgs` 字段 | Tavily JSON body | Brave GET query | Serper JSON body |
|---------------------|------------------|-----------------|------------------|
| `query` | `query` | `q` | `q` |
| `count` | `max_results` | `count` | `num` |
| `freshness` | `time_range` (`day`…) | `freshness` (`pd`…) | `tbs` (`qdr:d`…) |
| `country` | （省略 + warning） | `country` | `gl` |
| `language` | （省略 + warning） | `search_lang` | `hl` |
| `domain_filter` | `include_domains` | 见上（改写 `q` 或不支持） | 见上 |

**关联源码（工作区内）**：Tavily 以 **openclaw `extensions/tavily/src/tavily-client.ts`** + **hermes `tools/web_tools.py`（`_tavily_request` + `_normalize_tavily_search_results`）** 为双参考；Brave/Serper 以 **官方 API 文档** 为主（本工作区 hermes/openclaw **未**检出独立 Brave/Serper HTTP 客户端，故不硬造「对标某文件」链接）。openclaw 对 **generic `web_search` 工具 schema** 的字段名（`count`/`country`/…）见 [`src/agents/tools/web-search.ts`](../../../../openclaw/src/agents/tools/web-search.ts)（与 pi catalog 对齐用）。

#### 2.4.3 PR-WS-O：OpenAI hosted backend 调度与归一化

- **交付**：[`web_search/mod.rs::execute_openai_hosted`](../../../src/core/tools/web_search/mod.rs) 不再看**当前对话模型**，而是先解析 **hostedCandidateModel**（合并后的 model catalog 中首个 `capabilities.web_search == true` 的模型），再结合候选模型对应凭证与 base URL，委托 [`core/tools/web_search/openai_server.rs::search_openai_hosted`](../../../src/core/tools/web_search/openai_server.rs) 发起一笔**专用** hosted sidecar 请求：`model = hostedCandidateModel.id`，请求体中的 `tools = [{ "type": "web_search", "filters": {...}, "search_context_size": "medium", "user_location": ... }]`（字段形状参考 [openclaw `buildCodexNativeWebSearchTool`:160-200](../../../../openclaw/src/agents/codex-native-web-search-core.ts)）。响应回包含 `server_tool_use` + `web_search_tool_result` 块时，由 `openai_server.rs::parse_server_tool_blocks` 归一化为 `WebSearchOutput { hits, query, backend: "openai", stats: { elapsed_ms, cached:false }, warnings, truncated }`（块遍历与错误分支参考 [cc-fork-01 `WebSearchTool.ts`:86-150](../../../../cc-fork-01/src/tools/WebSearchTool/WebSearchTool.ts)）。`WebSearchOutput` 类型在 PR-WS-S 已定义，本 PR 复用该类型，不修改正常对话的 `openai_responses` 工具注入路径。
- **关键差异**：自家 HTTP backend 走 `tool_exec` → backend.search → 返回；hosted 路径则在 tool 内部先**选项目级 hosted 候选模型**，再发一笔 sidecar Responses 请求，由该模型代搜，然后解析 `server_tool_use`/`web_search_tool_result` 块。**两条路径在 `tool_exec` 出口处汇合到统一 `WebSearchOutput`**——模型读不到差异。
- **降级 / 致命错误**：
  - **候选存在门**：项目里**不存在** hostedCandidateModel → `auto` 直接进入 HTTP 链；显式 `openai` 时 `Err(AppError::Tool("no hosted web_search model configured; set capabilities.web_search=true on one models.toml entry"))`。
  - **候选有效性门**：hostedCandidateModel 存在，但其 provider / model 配置无法执行 hosted `web_search`（例如 key 缺失、模型声明与厂商能力不符、实现不支持）→ `auto` 记 `warnings += "hosted_candidate_unavailable, fallback=tavily"` 后回 HTTP 链；显式 `openai` 时 `Err(AppError::Tool("hosted web_search model <id> is misconfigured or unavailable"))`。
  - **运行时不可用**：401/403、429/5xx、timeout、transport fail 按 §7 状态机处理：`auto` 回 HTTP 链；显式 `openai` 不切别家，仅返回 degraded warning。

```text
  ┌──────────────────────────────────────────────────────────────────┐
  │ 自家 HTTP 路径（PR-WS-S）                                          │
  └──────────────────────────────────────────────────────────────────┘
   tool_exec → web_search.search → tavily/brave/serper.search
                                      │
                                      ▼
                                 reqwest call → normalize
                                      │
                                      ▼
                            WebSearchOutput { hits, ... }

  ┌──────────────────────────────────────────────────────────────────┐
  │ OpenAI hosted 路径（PR-WS-O）                                      │
  └──────────────────────────────────────────────────────────────────┘
  backend.discover_hosted_candidate  ┐
       -> hostedCandidateModel.id    │
                                     ▼
              web_search.mod.execute_openai_hosted
                   + resolve auth/base_url
                                     │
                                     ▼
                openai_server.search_openai_hosted
                 POST /v1/responses (sidecar)
                                     │
                                     ▼
                   blocks: [server_tool_use, web_search_tool_result, ...]
                                     │
                                     ▼
               openai_server.parse_server_tool_blocks
                                     │
                                     ▼
                          WebSearchOutput { hits, ... }
                                     │
    tool_exec ◀─── 同一形状汇合 ─────┘
```

**说人话**：OpenAI 这条路不再绑在当前聊天模型上，而是先找项目里那台被声明为 `capabilities.web_search=true` 的 hosted 候选模型，再用它发一笔专用搜索请求；回来我们把结果捏成跟自家 HTTP 一样的输出。

#### 2.4.4 PR-WS-W：域名守卫与 SSRF 拒接

- **交付**：归一化层（`web_search/types.rs::normalize_hits`）增加 URL 解析步骤——
  1. `Url::parse` 失败 → `warnings += "skipped_invalid_url"`，丢弃该 hit；
  2. 解析 `host` → 命中任意 IP literal（含公网 / 私网 / loopback）→ `warnings += "ssrf_filtered"`，丢弃；
  3. 单段 hostname（如 `localhost`、`server`，不含 `.`）或保留内网后缀（`.localhost` / `.local` / `.internal` / `.localdomain` / `.home.arpa`）→ 同上拒；
  4. 配置 `[tools.web_search] blocked_domains` 命中（按子域 suffix） → `warnings += "domain_blocked:<host>"`，丢弃；
  5. 配置 `[tools.web_search] allowed_domains` 非空时仅保留命中的 hits，其它 → `warnings += "domain_filtered:<host>"` 丢弃。
- **与 `web_fetch` 的关系**：`web_search` 的 `allowed/blocked_domains` 是**结果集级别后过滤**（HTTP 已经发出去，只是不把结果给模型）；[`web_fetch.md`](web_fetch.md) 的 `PermissionGate::Domain` 是**请求前权限**（请求都不发出去）。两者都需要、不互斥、配置项**独立**。
- **测试覆盖**：`normalize_hits_filters_private_hosts_and_domain_rules`（loopback / 公网 IP literal / `.internal` / allow+block 规则都被过滤）/ `cache_key_tracks_allowed_and_blocked_domains`（配置域约束变化不会误命中旧缓存）。

```text
  hits[] 原始（adapter 出口）
        │
        ▼
   for each hit:
     parse url
       │
       ├─ parse fail ──▶ skip + warning
       │
       ├─ host = loopback/私网/单段 ──▶ skip + ssrf warning
       │
       ├─ blocked_domains 命中 ──▶ skip + warning
       │
       ├─ allowed_domains 非空且未命中 ──▶ skip + warning
       │
       └─ pass ──▶ 保留
        │
        ▼
   hits[] 归一化后 → WebSearchOutput
```

**说人话**：Tavily 给我们什么我们都过一遍 IP/host 黑名单 + 配置黑/白名单，垃圾 URL 不进模型上下文；warning 写明哪条被丢了。

---

## 3. 术语统一

| 术语 | 语义（大白话） | 数据载体 | 行为约束 | 说人话 |
|------|----------------|----------|----------|--------|
| **`query`** | 模型给的检索词字符串 | `WebSearchArgs.query: String` | 必填、长度 ≤512、不空白；与 `count/freshness/...` 一起组成 cache key | 想搜啥就写啥。 |
| **`hits`** | 归一化的搜索结果列表 | `WebSearchOutput.hits: Vec<Hit>` | `Hit { title, url, snippet, position, published_at? }`；位置从 1 起 | 一组带标题 + 链接 + 摘要的小卡片。 |
| **`snippet`** | 结果内的文字摘要 | `Hit.snippet: String` | 上游不一定给（Brave 偶尔无）→ 空串；不抓正文 | 搜索结果页那段灰色小字。 |
| **`backend`** | 实际跑这次检索的 provider | `WebSearchOutput.backend: String`；`auto/tavily/brave/serper/openai` 五值 | `auto` **不**出现在 output（必落到具体 provider），仅出现在 input/config；`openai` 表示项目级 hosted 候选模型路径 | 这条结果是哪家给的；审计里能看到。 |
| **hostedCandidateModel** | 项目级 hosted 搜索候选模型 | 合并后的 model catalog 中首个 `capabilities.web_search == true` 的 model entry | `auto` 先看它是否存在；若有多个按合并顺序取首个；当前对话模型不参与资格判断 | 项目里只要配了可联网 hosted 模型，auto 就先用它。 |
| **server-side 注入** | 把 `{type:"web_search",...}` 写进**项目级 hosted 候选模型**的专用 Responses 请求，由该模型代搜 | [`web_search/openai_server.rs`](../../../src/core/tools/web_search/openai_server.rs) | 仅当项目里存在 `hostedCandidateModel` 且 `backend ∈ {auto, openai}` 时启用；返回块由 `parse_server_tool_blocks` 解析 | 当前聊天模型是不是 Responses 不重要；hosted 搜索走的是一笔独立请求。 |
| **`cache key`** | 缓存命中判定键 | `(backend, query, count, freshness, country, language, domain_filter, allowed_domains, blocked_domains)` 元组 hash | key 含 backend 与配置域约束 → 切换 backend 或调整 allow/block 不会误命中旧缓存；TTL 默认 5 min | 「同样的 backend + 同样的 query + 同样的过滤 / 域约束」才算同一次。 |
| **SSRF（Server-Side Request Forgery）** | 模型让工具去连内网 / loopback 的攻击形态 | `web_search/types.rs::normalize_hits` 里的 host 黑名单 | **hits**：归一化阶段拒（loopback / 私网 / 单段 hostname）。**fetch**：`validate.rs` 增补 IP 段拒（cc-fork `validateURL` 客户端无此项，见 [`utils.ts:139-168`](../../../../cc-fork-01/src/tools/WebFetchTool/utils.ts)） | 别让搜来的 URL 指内网。 |
| **`warnings`** | 一组「不致命但模型应当知道」的标签 | `WebSearchOutput.warnings: Vec<String>` | backend 切换 / 截断 / 限速 / SSRF 过滤 / domain 过滤 都进 | 「这次搜索发生了点小事，但还是给你结果」。 |

**「LLM 收到 tool 结果后」**：指 **`tool_exec` 已把 `WebSearchOutput` 序列化为 tool 消息文本（JSON）**、写入会话历史、**即将进入下一轮模型推理之前**。

---

## 4. 协议（入参 / 出参 / Schema）

**单一事实源**：

- JSON Schema（模型可见）：[`catalog.rs::web_search_parameters`](../../../src/core/tools/contract/catalog.rs)（PR-WS-A 添加）→ [`docs/tool-catalog.md`](../../tool-catalog.md) 派生。
- Rust 类型：`core/tools/web_search/types.rs`（PR-WS-S 新增）的 `WebSearchArgs` / `WebSearchOutput` / `Hit`。

### 4.1 入参（工具 arguments）

| 字段 | JSON 类型 | 必填 | 默认 | 说明 | 说人话 |
|------|-----------|------|------|------|--------|
| `query` | string | **是** | — | 检索词；非空、长度 ≤512 | 想搜啥就写啥。 |
| `count` | integer | 否 | 5 | 期望返回 hits 数；范围 1..=20 | 默认 5 条；最多 20。 |
| `freshness` | enum `day` \| `week` \| `month` \| `year` \| null | 否 | null | 时间范围筛选；映射到各 backend 的对应字段（Tavily `time_range` / Brave `freshness` / Serper 时间） | 想要新闻就传 day / week。 |
| `country` | string | 否 | null | ISO 3166-1 alpha-2，如 `us`/`cn`；映射 Brave `country` / Serper `gl` / Tavily ignored | 限制结果国家。 |
| `language` | string | 否 | null | ISO 639-1，如 `en`/`zh`；映射 Brave `search_lang` / Serper `hl` / Tavily ignored | 限制语言。 |
| `domain_filter` | string[] | 否 | `[]` | 白名单域名（含子域 suffix 匹配）；与 `[tools.web_search] blocked_domains` 互补；本字段空时仅 config 生效；本字段非空时**叠加**到 config 之上 | 「只搜这几个站」。 |

`auto` 三态语义（`backend` 字段在 catalog **不暴露给模型**，仅 `[tools.web_search] backend` 配置可写）：

- 缺省 / 显式 `auto`：动态 pick——若合并后的 model catalog 中存在 hostedCandidateModel（首个 `capabilities.web_search == true` 条目）→ 先走 server-side hosted 请求（PR-WS-O）；若项目里不存在该候选，或该候选运行时不可用 → Tavily（若无 key / 不可用 → Brave → Serper 顺序降级）。**当前对话模型的 `api` 是否 `openai-responses` 不参与判断**。
- 显式 `tavily/brave/serper/openai`：强制对应 backend；**`openai` 但项目里不存在 hostedCandidateModel** → §9「致命错误」（禁止静默改走 HTTP，以免用户以为走了「官方联网」）。

### 4.2 出参（Rust：`WebSearchOutput`）

| 字段 | 类型 | 说明 | 说人话 |
|------|------|------|--------|
| `hits` | `Vec<Hit>` | 归一化结果（已过 SSRF + 域名过滤） | 主菜：一组结果。 |
| `query` | `String` | echo 入参 query | 我搜的是啥。 |
| `backend` | `String` | 实际跑的 provider，四值之一（`tavily` / `brave` / `serper` / `openai`）；**`auto` 不出现在出参**——`pick_backend` 必落到具体 provider 后才发起调用 | 哪家给的。 |
| `stats` | `Stats { elapsed_ms: u64, cached: bool, total_before_filter?: usize }` | 性能与命中信息 | 多久 / 是不是缓存里取的。 |
| `truncated` | `bool` | 限速 / 超时 / count 限制是否触发截断 | 是否没拿全。 |
| `warnings` | `Vec<String>` | 标签列表（见 §3） | 有啥小事故。 |

**`Hit` 子结构**：

```text
Hit {
  title:        String         // 上游缺失则用 url 的 host 兜底
  url:          String         // 必有，已通过 SSRF + domain 过滤
  snippet:      String         // 上游缺失 → 空串
  position:     u32             // 1-based
  published_at: Option<String> // ISO 8601；上游未给 → None
}
```

### 4.3 调用样例（jsonc）

**最简检索**：

```jsonc
{
  "query": "GPT-5.5 release notes"
}
```

**带筛选**：

```jsonc
{
  "query": "rust async runtime benchmarks",
  "count": 10,
  "freshness": "month",
  "country": "us",
  "language": "en",
  "domain_filter": ["github.com", "blog.rust-lang.org"]
}
```

**典型出参（Tavily 路径）**：

```jsonc
{
  "hits": [
    {
      "title": "Introducing GPT-5.5",
      "url": "https://openai.com/index/introducing-gpt-5-5/",
      "snippet": "GPT-5.5 brings ...",
      "position": 1,
      "published_at": "2026-04-22T00:00:00Z"
    }
  ],
  "query": "GPT-5.5 release notes",
  "backend": "tavily",
  "stats": { "elapsed_ms": 312, "cached": false, "total_before_filter": 12 },
  "truncated": false,
  "warnings": ["domain_filtered:newsapi.com"]
}
```

**OpenAI server-side 路径**（hits 由 `web_search_tool_result` 块解出，shape 同上，仅 `backend="openai"`、`stats.elapsed_ms` 来自块的耗时字段）。

---

## 5. One-Glance Map（文件职责总览）

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/llm/system_prompt.rs                                             │
│  • web_search 工具说明：source attribution / current date={today}           │
└────────────────────────────────────────────────────────────────────────────┘
        │
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/tools/contract/catalog.rs                                        │
│  • BUILTIN_TOOL_CATALOG: name = "web_search"                                │
│  • web_search_parameters(): 6 字段 JSON Schema                              │
└────────────────────────────────────────────────────────────────────────────┘
        │
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/agent_loop/tool_exec/  (目录模块)                                 │
│  • mod.rs::execute_tool_tuple_full → match "web_search"                      │
│  • branches/web_search.rs::handle_web_search → executor.web_search.search()  │
│  • 序列化 WebSearchOutput 为 JSON 字符串作为 tool 消息文本                    │
└───────────────────────────────┬────────────────────────────────────────────┘
                                │
                                ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/tools/web_search/                                                 │
│  ├ mod.rs            • search(args) 入口；命中缓存早返；调 backend.search      │
│  ├ types.rs          • WebSearchArgs / WebSearchOutput / Hit / Stats        │
│  │                   • normalize_hits: SSRF + domain 过滤                    │
│  ├ backend.rs        • trait WebSearchBackend + pick_backend(provider, cfg) │
│  ├ tavily.rs         • POST /search → normalize                             │
│  ├ brave.rs          • GET api.search.brave.com/res/v1/web/search → normalize │
│  ├ serper.rs         • POST google.serper.dev/search → normalize             │
│  ├ openai_server.rs  • build/request/parse hosted sidecar（PR-WS-O）         │
│  └ cache.rs          • Moka LRU + TTL；CacheKey 元组                         │
└───────────────────────────────┬────────────────────────────────────────────┘
                                │
                ┌───────────────┴───────────────┐
                ▼                               ▼
┌──────────────────────────────────┐  ┌──────────────────────────────────────┐
│ src/core/tools/web_search/       │  │  src/infra/config/types/tools.rs     │
│ {mod,openai_server}.rs           │  │  • ToolsConfig 加 ToolsWebSearchConfig│
│ • execute_openai_hosted          │  │    { backend, count, freshness,      │
│ • search_openai_hosted: sidecar  │  │      country, language, domain_filter,│
│ • parse_server_tool_blocks       │  │      blocked_domains, allowed_domains,│
│   （不修改正常对话 openai wire）  │  │      cache_ttl_secs, cache_capacity, │
└──────────────────────────────────┘  │      timeout_ms, *_base_url }        │
                                      └──────────────────────────────────────┘

  + tests:
    src/core/tools/web_search/tests/                  (per-adapter 单元)
    tests/web_search_tool_tests.rs                    (集成 mock HTTP)
    E2E-WEB-SEARCH-001                                (真 Tavily, PI_LIVE_WEB_SEARCH=1)
```

**阅读顺序（说人话）**：模型先看到 **catalog** 里 `web_search` 名字与 6 字段；调起后 **`tool_exec`** 把 args 解出来，转给 **`web_search/mod`** 的 `search`；`mod` 先查 cache、再让 `backend.rs::pick_backend` 选具体 provider；HTTP adapter 起 reqwest 拿原始 JSON、各自 normalize 进 `Hit`；`types.rs::normalize_hits` 跑一遍 SSRF + 域名过滤；最后 `tool_exec` 把 `WebSearchOutput` 序列化回 LLM。**OpenAI hosted 路径**则由 `openai_server.rs` 走一笔独立 sidecar 请求：挑项目级 hosted 候选模型、把 `type:web_search` 工具挂到该请求上，再把返回的 `web_search_call` / `web_search_tool_result` 归一化到同一份 `WebSearchOutput`。

---

## 6. 调度时序（运行时图）

### 6.1 自家 HTTP backend 路径（默认 Tavily）

```text
LLM           tool_exec           web_search/mod        backend(adapter)        cache       network
 │                │                     │                     │                  │            │
 │ web_search(q)  │                     │                     │                  │            │
 │───────────────▶│ parse args          │                     │                  │            │
 │                │────────────────────▶│ build CacheKey      │                  │            │
 │                │                     │────────────────────────────────────────▶│            │
 │                │                     │                     │     hit?         │            │
 │                │                     │◀────────────────────────────────────────│            │
 │                │                     │ miss → pick_backend(...)                │            │
 │                │                     │────────────────────▶│                   │            │
 │                │                     │                     │ reqwest GET/POST  │            │
 │                │                     │                     │──────────────────────────────▶│
 │                │                     │                     │◀──────────────────────────────│
 │                │                     │                     │ adapter.normalize │            │
 │                │                     │◀────────────────────│                   │            │
 │                │                     │ types::normalize_hits (SSRF + domain)   │            │
 │                │                     │ cache.put + return WebSearchOutput      │            │
 │                │◀────────────────────│                     │                   │            │
 │◀───────────────│ JSON tool 消息       │                     │                   │            │
```

### 6.2 OpenAI hosted 路径（项目级候选模型）

```text
tool_exec/web_search.mod   backend.rs          web_search/mod         web_search/openai_server
│                              │                        │                           │
│ auto/openai                   │ discover hostedCandidate                     │
│─────────────────────────────▶│───────────────────────▶│ resolve auth/base_url   │
│                              │                        │ build sidecar request    │
│                              │                        │──────────────────────────▶│ POST /v1/responses
│                              │                        │                           │
│                              │                        │                           ▼
│                              │                        │                OpenAI Responses API
│                              │                        │                           │
│                              │                        │◀──────────────────────────│ parse + normalize
│◀─────────────────────────────│ return WebSearchOutput（与自家 HTTP 路径同形）          │
```

**两条路径在 `tool_exec` 出口处汇合到同一 `WebSearchOutput`**——下游 LLM 看到的字段一致。

---

## 7. 状态机（backend 选择）

```text
                   ┌──────────────────┐
                   │   入参 backend?   │
                   └────────┬──────────┘
                            │
            ┌───────────────┼─────────────────────┐
            │               │                     │
       缺省 / "auto"   "tavily/brave/serper"   "openai"
            │               │                     │
            ▼               ▼                     ▼
  项目里有 hostedCandidate? 候选 backend 可用? 项目里有 hostedCandidate?
            │               │                     │
   ┌────────┴───────┐       │              ┌──────┴──────┐
   │                │       │              │             │
    yes             no      │              yes            no
   │                │       │              │             │
   ▼                ▼       ▼              ▼             ▼
 openai(hosted)  Tavily→Brave→Serper  具体 backend  openai(hosted)  Err 致命
   │              (auto 链)             │               │          (无候选模型)
   │                                    │               │
   └─ hosted 不可用? ───────▶ HTTP 链     └─ 不可用? ───▶ degraded
          (key/auth/429/5xx/
           timeout/transport)

> **hostedCandidateModel** = 合并后的 model catalog 中首个 `capabilities.web_search == true` 的条目；
> 当前对话模型**不参与** `auto` 的 hosted 首选资格判断。
> `auto` 下有候选就先试 `openai(hosted)`，没有候选再进 HTTP 链；显式 `openai` 下没有候选则致命 Err。
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `auto` + 项目里存在 hostedCandidateModel | hosted 候选可用 | `openai` | 发起一笔 hosted sidecar 请求 | 只要项目里配了 hosted 候选，auto 就先用它。 |
| `auto` + 项目里存在 hostedCandidateModel | hosted 路径缺 key、401/403、429/5xx、timeout 或 transport fail | `tavily`（再不通继续 `brave→serper`） | `warnings += "openai_unavailable, fallback=tavily"` | hosted 不通就切回 HTTP 链。 |
| `auto` + 项目里不存在 hostedCandidateModel | Tavily key / 可用性 ok | `tavily` | 直接用 Tavily | 没有 hosted 候选就默认 HTTP backend。 |
| `auto` + 当前候选 HTTP backend | 缺 key、401/403、429/5xx、timeout 或 transport fail | 下一个候选（若存在） | `warnings += "backend_unavailable:<current>, fallback=<next>"` | 一家不通就继续找下一家。 |
| `auto` + 全链无任何已配置候选 | 无 hostedCandidateModel，且 Tavily / Brave / Serper 均无 key | `disabled` | `Err(Tool("no web_search backend configured"))` | 一个能用的都没；告诉模型搜不了。 |
| `auto` + 至少一个候选已尝试 | 所有候选都运行时不可用 | `degraded` | `truncated=true + warnings += "all_backends_unavailable"`；返回空 `hits[]` | 已经都试过了，只能告诉模型这轮没搜成。 |
| 显式 `openai` | 项目里不存在 hostedCandidateModel | `incompatible` | `Err(Tool("no hosted web_search model configured; set capabilities.web_search=true on one models.toml entry"))` | 你点名要 hosted，就得先在项目里配一个 hosted 候选模型。 |
| 显式 `openai` | hostedCandidateModel 存在但配置错误 | `incompatible` | `Err(Tool("hosted web_search model <id> is misconfigured or unavailable"))` | 有候选但不能用，也不能假装切别家。 |
| 显式 `openai` / `tavily` / `brave` / `serper` | 401/403、429/5xx、timeout 或 transport fail | `degraded` | `truncated=true + warnings += "backend_unavailable:<current>"`；不切别家 | 手动点名就不偷偷换别家，但也不把整轮直接打死。 |
| 任何 backend | 400 / 422 等请求契约错误 | `failed` | `Err(...)` | 这是实现或参数 bug，不能用降级掩盖。 |

**降级顺序**（`auto`）：`openai(hosted, project candidate if any) → tavily → brave → serper → disabled`；HTTP 链与 hosted 都只对**可用性 / 认证 / retryable** 问题降级，**不**对 400 / 422 这类契约错误降级。若全链**无候选**则报 `no web_search backend configured`；若全链**都尝试过但都暂不可用**，则返回空 `hits[]` + `truncated/warnings`。显式指定时**不降级**，只返回当前 provider 的归一化 warning/错误。

---

## 8. 配置与环境变量

**总则**：`env > config > 默认`。

| 来源 | 键 | 含义 | 备注 | 说人话 |
|------|-----|------|------|--------|
| `tomcat.config.toml` | `[tools.web_search] backend` | `auto` / `tavily` / `brave` / `serper` / `openai` | 默认 `auto` | 不写就跟模型走。 |
| env | `TOMCAT__TOOLS__WEB_SEARCH__BACKEND` | 同上，运行时覆盖 | env > config | 容器里临时换 backend。 |
| `tomcat.config.toml` | `[tools.web_search] tavily_base_url` / `brave_base_url` / `serper_base_url` | 各 backend 的 base URL override | 默认官方 endpoint；只改 host / base，不放 secret | 兼容自建网关或代理。 |
| env | `TOMCAT__TOOLS__WEB_SEARCH__TAVILY_BASE_URL` 等 | 同上，运行时覆盖 | env > config | 容器里临时切网关。 |
| runtime `.env` / process env | `TAVILY_API_KEY` / `BRAVE_API_KEY` / `SERPER_API_KEY` | per-provider HTTP key | 运行时从 `~/.tomcat/assets/.env` 或进程 env 读取 | auto 链可同时持有多家 key。 |
| repo 根 `.env` | 同上 | 本地开发 / 真 API 测试用 | 由 `.env.example` 提供样板；测试会加载仓库根 `.env` | 开发时复制样板即可。 |
| runtime `.env` / model env | hostedCandidateModel 对应的 `OPENAI_API_KEY`（或 `<PROVIDER>_API_KEY`） | OpenAI hosted 搜索凭证 | 复用 **hostedCandidateModel** 的 key source；**不**由 `[tools.web_search]` 单独配 | hosted 这条路跟项目级候选模型走，不跟当前聊天模型走。 |
| `tomcat.config.toml` | `[tools.web_search] count` | 默认期望 hits 数 | 1..=20，默认 5 | 一次默认搜几条。 |
| `tomcat.config.toml` | `[tools.web_search] freshness` / `country` / `language` | 缺省筛选 | 入参可覆盖 | 配置层定个默认；模型可改。 |
| `tomcat.config.toml` | `[tools.web_search] domain_filter` | 默认白名单 | 入参可叠加 | 「这台机只搜这几个站」。 |
| `tomcat.config.toml` | `[tools.web_search] blocked_domains` | 黑名单 | 与 `domain_filter` 互补；与 `web_fetch.allowed_domains` **不共享** | 拒搜某些站。 |
| `tomcat.config.toml` | `[tools.web_search] cache_ttl_secs` / `cache_capacity` | LRU TTL 与容量 | 默认 300 / 50 | 缓存活多久、能装多少。 |
| `tomcat.config.toml` | `[tools.web_search] timeout_ms` | 单次 backend 超时 | 默认 12_000 | 等多久还没回来就算超时。 |

**用户在 `tool_exec` 入参里 `count/freshness/country/language/domain_filter` 都可逐次覆盖**——配置只决定缺省。

> **server-side 资格不在 `[tools.web_search]` 里配**：`auto` 是否有 hosted 首选，取决于**合并后的 model catalog** 中是否存在 `capabilities.web_search == true` 的条目；若有多个，按顺序取首个作为 hostedCandidateModel。`capabilities.web_search` 是**新增能力位**（[`catalog.rs` Capabilities](../../../src/core/llm/catalog.rs)，默认 `false`），与现有 `vision/files/tools/reasoning` 并列；**登记前必须先查该厂商/该模型的官方文档**（见 §13），不得仅凭「endpoint 兼容 Responses wire」就置 `true`。`[tools.web_search] backend` 只能在「资格已具备」时把 `auto` 收窄/或强制走 HTTP，**不能**反向赋予一个不支持的模型 server-side 能力。**当前对话模型无论是不是 Responses，都不再影响 `auto` 是否尝试 hosted 首选。**

> **能否「纯配置接入新 backend」**（回应选型期提问）：**仅对 wire 同构的网关成立**。Tavily 兼容网关（同 `POST {base}/search` 形状）可仅靠 `[tools.web_search] tavily_base_url` + `TAVILY_API_KEY` 接入，无需改码；Brave / Serper 若只是换 host 也可通过各自 `*_base_url` 覆盖。但**异构 HTTP backend**（字段/鉴权/分页不同的全新供应商）仍需在 `core/tools/web_search/` 新增 adapter——这与「新增 LLM 模型仅改 `models.toml`」**不同**，因为 LLM 侧已有 `openai`/`openai-responses` 等 wire 实现可复用，而检索供应商各家 HTTP 形状不归一。

---

## 9. 错误模型 / 截断 / 警告

```text
                    web_search 请求
                            │
        ┌───────────────────┼───────────────────┐
        ▼                   ▼                   ▼
   参数非法              无可用 backend       backend 返回
   (query 空 / count 越界) (no_key / openai     │
   AppError::Tool         不兼容)               │
                          AppError::Tool        │
                                                │
        ┌───────────────────┬───────────────────┴────────────────────┐
        ▼                   ▼                   ▼                    ▼
   429 / 5xx          tokio::timeout      上游解析失败            正常返回
   truncated=true     truncated=true      AppError::Tool          (含 warnings)
   warnings+="rate_  warnings+="timeout"  （致命：JSON shape       │
    limited"                               变了）                   │
        │                   │                   │                    │
        └───────────┬───────┴───────────────────┘                    │
                    │                                                 │
                    ▼                                                 ▼
         tool_exec 序列化 WebSearchOutput            tool_exec 序列化 WebSearchOutput
              （hits 可能为空但有 warnings）              （正常 hits + 可能的 warnings）
                                                                      │
                            归一化 hits 阶段                            │
                  ┌──────────────────┬──────────────────┐             │
                  ▼                  ▼                  ▼             │
            URL parse 失败      SSRF / 私网命中    domain 黑/白名单    │
            warnings+="skipped_ warnings+="ssrf_   warnings+=         │
            invalid_url"        filtered"           "domain_blocked"  │
                  │                  │                  │              │
                  └──────────────────┴──────────────────┘              │
                                     │                                  │
                                     └──────────────────────────────────┘
```

**`tool_exec` 视角**：

- `Err(_)` → tool 消息文本为错误描述（「致命」类 4 路：参数非法 / 无可用 backend / hosted 候选缺失或配置错误 / 上游解析致命）。
- `Ok(WebSearchOutput)` → JSON 序列化为 tool 消息文本（含可能为空的 hits + warnings）。

**§1 G1–G6 的「锁死它的测试」**全部位于 §10。

---

## 10. 测试矩阵（验收）

**当前状态（2026-06-04）**：PR-WS-A / S / O / W 的本地实现已完成，并已通过 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`./scripts/run-integration-tests.sh lib`、`./scripts/run-integration-tests.sh integration`，以及 `PI_LIVE_WEB_SEARCH=1 cargo test --test web_search_tool_tests live_tavily_search_smoke -- --nocapture`。

| 维度 | 用例（实际函数名） | 状态 | 说人话 |
|------|---------------------|------|--------|
| catalog 注册 | `core::tools::contract::catalog::tests::web_search_registered`、`core::agent_loop::tests::submodules_test::tool_exec_web_search_requires_runtime_injection`、`core::agent_loop::tests::submodules_test::tool_exec_web_search_routes_to_runtime` | PASS（2026-06-05） | 名字注册了；runtime 缺失时报明确错误，注入后会真实路由到 `web_search` runtime。 |
| Tavily 解析 | `tavily_runtime_maps_request_and_normalizes_hits`、`runtime_explicit_tavily_works_from_public_api` | PASS（2026-06-04） | Tavily header/body 映射和公开 API 路径都已跑通。 |
| Brave 解析 | `auto_backend_falls_back_to_brave_and_then_hits_cache`、`runtime_auto_routes_to_http_fallback_chain` | PASS（2026-06-04） | Brave 既覆盖 auto fallback，也覆盖集成层公共 runtime。 |
| Serper 解析 | `runtime_explicit_serper_works_from_public_api` | PASS（2026-06-04） | Serper 请求映射与结果归一化已通过 mock HTTP。 |
| backend.auto（project hosted 候选） | `auto_backend_uses_project_hosted_candidate`、`incompatible_hosted_candidate_falls_back_to_tavily` | PASS（2026-06-04） | auto 先看项目里的 hosted 候选，候选不可用时再回 HTTP 链。 |
| hosted 候选选择 | `discover_hosted_candidate_uses_merged_catalog_order`、`merged_catalog_preserves_override_slot_and_web_search_capability`、`explicit_openai_requires_project_candidate` | PASS（2026-06-04） | 多个候选按合并后顺序取首个，没有候选时显式 `openai` 会报错。 |
| 缓存 | `auto_backend_falls_back_to_brave_and_then_hits_cache`、`cache_key_tracks_allowed_and_blocked_domains` | PASS（2026-06-05） | 同 query 二次命中 cache，不再重复发起上游请求；配置域约束变化不会误复用旧缓存。 |
| 超时 | `auto_backend_falls_back_after_brave_timeout` | PASS（2026-06-04） | 超时会记 warning，并在 `auto` 链里继续尝试下一个 backend。 |
| 限速归一化 | `explicit_tavily_rate_limit_returns_degraded_output` | PASS（2026-06-04） | 显式 backend 遇到 429 会返回 degraded output，不会把整轮工具直接打崩。 |
| OpenAI hosted | `build_hosted_request_body_includes_filters_and_location`、`parse_server_tool_blocks_handles_openai_and_server_tool_shapes`、`runtime_explicit_openai_uses_project_hosted_candidate` | PASS（2026-06-04） | 项目级 hosted 候选模型请求与 server-side block 归一化都已落地。 |
| SSRF 守卫 | `normalize_hits_filters_private_hosts_and_domain_rules` | PASS（2026-06-05） | 任意 IP literal、内网保留 hostname、单段 host 会在 hits 归一化时被剔除。 |
| 域名过滤 | `normalize_hits_filters_private_hosts_and_domain_rules` | PASS（2026-06-05） | `allowed_domains` / `blocked_domains` 已按 host / subdomain 规则过滤。 |
| 集成（mock HTTP） | `runtime_explicit_tavily_works_from_public_api`、`runtime_auto_routes_to_http_fallback_chain`、`runtime_explicit_serper_works_from_public_api` | PASS（2026-06-04） | mock Tavily / Brave(auto) / Serper 的 public runtime 集成路径都跑通了。 |
| 集成 catalog | `core::agent_loop::tests::submodules_test::tool_exec_web_search_requires_runtime_injection`、`core::agent_loop::tests::submodules_test::tool_exec_web_search_routes_to_runtime` | PASS（2026-06-05） | `tool_exec` 已真实接入 `web_search` 分支，缺 runtime 时错误友好，注入后成功路径也有对称回归。 |
| 配置解析 | `infra::config::tests::tools_cfg_test::tools_web_search_toml_override`、`infra::config::tests::validate_test::validate_config_rejects_invalid_web_search_backend` | PASS（2026-06-04） | `[tools.web_search]` 的默认值、覆写和非法 backend 校验都已覆盖。 |
| E2E（live） | `live_tavily_search_smoke`（`PI_LIVE_WEB_SEARCH=1` gate） | PASS（2026-06-04） | 已在本机载入真实 Tavily key 后跑过一次 live smoke。 |

§1 观察指标 **G1–G6** 与本表逐行对应：G1↔catalog/Tavily/Brave/Serper/集成；G2↔backend.auto/openai server-side；G3↔三家解析；G4↔缓存；G5↔SSRF/域名过滤；G6↔超时/限速/集成。

---

## 11. 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|------------------|--------|
| **API key 泄漏到 transcript / 日志** | 凭证外泄、账号被盗用 | env 直读，**不**写入审计 message；error 文案做 redaction（参考 [hermes `url_safety.py`](../../../../hermes-agent/tools/url_safety.py) 的 secret-prefix scan：错误里若包含 `Bearer xxx` / `sk-` 前缀字符串 → 替换为 `<redacted>`）；catalog schema **不**含 `api_key` 字段（不让模型直接传） | 永远不让 key 进模型上下文。 |
| **SSRF（hits.url 指向 loopback / 私网）** | 模型若顺手喂给 fetch 工具会打内网 | 在 `web_search/types.rs::normalize_hits` 解析 url → 拒 `127.0.0.0/8` / `10.0.0.0/8` / `172.16.0.0/12` / `192.168.0.0/16` / `::1` / `fc00::/7` / 单段 hostname；参考 [cc-fork-01 `utils.ts:139-169`](../../../../cc-fork-01/src/tools/WebFetchTool/utils.ts) 的 `isPrivateIP`；命中 → 丢 hit + `warnings += "ssrf_filtered"` | 别让搜来的 URL 指内网。 |
| **限速 / 账号配额耗尽** | 整轮 tool 失败、模型 retry 风暴 | 429 / 5xx → `truncated=true + warnings += "rate_limited (backend=<x>,status=<y>)"`，**不抛 `Err`**；hits 可能为空，模型自行决定下一步 | 限速了别整轮塌；告诉模型「这次没拿到」。 |
| **上下文 cost 超限** | 一次 20 条 × 长 snippet 占满上下文 | `count` 默认 5、上限 20；**软上限** `max_result_size_chars=60_000`：每条 hit 的 snippet 超 4 KiB → 截断到 4 KiB + `warnings += "snippet_truncated"`；总字符超软上限 → 减少 hits 数 + warning（**永远先减条数再减 snippet 长度**） | 别一次搜搜出 8 万字。 |
| **把 hosted 资格错误地绑到当前对话模型，或把 capability 误配到不支持 hosted 的模型** | 当前对话模型不是 Responses 时，即便项目里已配 hosted 候选也永远用不到 server-side；或 capability 误配后，hosted 请求会报错 / 静默失败 → 整轮 fail 或模型以为联网了其实没搜 | `auto` 先扫描项目级 hostedCandidateModel（首个 `capabilities.web_search == true`），**当前对话模型不参与 hosted 首选判断**；`capabilities.web_search` 只能在**查证厂商官方文档**后置 `true`；候选不存在或不可用时 `auto` 回 HTTP 链，显式 `openai` 下报致命 Err。**已知事实**：OpenAI 官方 Responses 支持但**按模型分**（见 §13）；Azure / 多数第三方兼容栈未必实现该 hosted 工具 | auto 该看项目里有没有可联网 hosted 模型，不该看当前聊天用的是谁。 |

---

## 12. 历史决策（已被本方案取代或待定）

- ~~合并 `web` 工具单 schema：参数二选一（`query` 或 `url`）~~ → **否**：schema 双口（必填字段不同）会让模型频繁参数错；权限粒度（fetch 按 domain）对不上；缓存键不同（query vs url）。**`web_search` 与 `web_fetch` 拆两个工具**（与 cc/hermes/openclaw 三家一致）。
- ~~`web_search` 与 `web_fetch` 写在同一份 `web.md` 文档~~ → **否**：长文双口吻冲突（[ARCHITECTURE_SPEC §14 No-Stale](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)）；与 read/write/edit/bash/search_files **一文件一工具**习惯不一致。**拆为两份独立满额文档**（本文 + [`web_fetch.md`](web_fetch.md)）；共享术语与风险在两篇各自完整书写；**各文仍须具备完整 §4.1 / §4.2**，**不**合并为单稿后省略。
- ~~默认 backend 单一硬编码（如永远 Tavily）~~ → **否**：项目里只要已经配置了 `capabilities.web_search == true` 的 hosted 候选模型，`auto` 就应先用它；只有项目里**没有** hosted 候选时才直接走 HTTP 链。**默认 `auto`** 跟着**项目里有没有 hosted 候选模型**走，不是跟着当前对话模型是谁走。
- ~~只看当前对话模型的 wire / capability 判 server-side~~ → **否**（本次设计改写）：当前对话模型可能是 DeepSeek / Completions，但项目里已经额外配置了 hosted 搜索模型。**改项目级选择**：扫描合并后的 model catalog，取首个 `capabilities.web_search == true` 的条目作为 hostedCandidateModel；若不存在，再进 HTTP 链。详见文首校准块第 8 条与 §11。
- ~~持久化 search 缓存（落盘）~~ → **否**：检索结果有时效性，跨进程命中反而坑；进程内 LRU + TTL 即可。
- ~~server-side 默认 + 由 server 代取 fetch~~ → **否**：cc-fork-01 的 server-side 抓取（Anthropic native）只能在 Anthropic 系生效；fetch 单独走自家 reqwest（[`web_fetch.md`](web_fetch.md)），统一行为。
- ~~hits 不归一化、各 backend 透传~~ → **否**：模型读三套 JSON 形状会频繁出错；归一化是单一事实源（`web_search/types.rs::Hit`）。

**跨文档修订**：

- 本文新增的 catalog 条目 `web_search` 触及 [`docs/tool-catalog.md`](../../tool-catalog.md)（派生文档，由 `build_function_definitions()` 自动生成）；不需手动改。
- 本文不修改 [`read.md`](read.md) / [`write.md`](write.md) / [`edit.md`](edit.md) / [`bash.md`](bash.md) / [`search_files.md`](search_files.md) 已冻结正文。

---

## 13. 关联文档

- **HTTP 字段实现对照**：本文 **[§2.4.2.1](#2421-http-上游字段速查实现必读)**；Tavily 参考实现 [openclaw `tavily-client.ts`](../../../../openclaw/extensions/tavily/src/tavily-client.ts)、[hermes `web_tools.py`](../../../../hermes-agent/tools/web_tools.py)（`_tavily_request`）；openclaw 工具侧 schema [`web-search.ts`](../../../../openclaw/src/agents/tools/web-search.ts)；Tavily 插件参数表 [openclaw `docs/tools/tavily.md`](../../../../openclaw/docs/tools/tavily.md)。
- **hosted `web_search` 厂商能力事实源**（登记 `capabilities.web_search` 前**必读**，§2.4.3 / §11 双门闩依据）：
  - OpenAI Responses API · Web search tool：<https://platform.openai.com/docs/guides/tools-web-search>（支持**按模型**而定；返回含 `web_search_call` + 带 `url_citation` annotations 的 message）。
  - OpenAI Responses API 参考（`tools[].type = "web_search"` 字段形状）：<https://platform.openai.com/docs/api-reference/responses/create>。
  - Azure OpenAI Responses API：<https://learn.microsoft.com/azure/ai-foundry/openai/how-to/responses>（hosted web search 可用性受限、`external_web_access` 等约束随区域/模型变化——**默认按不支持登记**，逐模型查证）。
  - 自托管 / 第三方「OpenAI 兼容」栈（vLLM / Ollama / Together 等）：通常**仅**实现 Chat/Responses 文本 wire，**不**实现 hosted `web_search` 工具 → `capabilities.web_search` 保持 `false`，走自家 HTTP backend。
- 兄弟工具：[`web_fetch.md`](web_fetch.md) · [`read.md`](read.md) · [`bash.md`](bash.md) · [`search_files.md`](search_files.md) · [`write.md`](write.md) · [`edit.md`](edit.md)
- 权限总览：[`../permission-system.md`](../permission-system.md)
- 看板叙事：[`docs/agents/TASK_BOARD_002/README.md`](../../agents/TASK_BOARD_002/README.md)、[`T2-P1-012.md`](../../agents/TASK_BOARD_002/tasks/T2-P1-012.md)
- 五仓对比：[`agent-tools-comparison.md`](../../reports/agent-tools-comparison.md)
- Cursor 内置工具参考：[`cursor-builtin-tools-reference.md`](../../reports/cursor-builtin-tools-reference.md)
- 派生工具目录：[`tool-catalog.md`](../../tool-catalog.md)
- 规范：[`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)

---

**一句话总结**：`web_search` 在 **`tool_exec`** 解参数与序列化、在 **`web_search/mod`** 查缓存与挑 backend、在 4 个 adapter 里跑 reqwest 或注入 OpenAI server-side、在 **`types::normalize_hits`** 做 SSRF + 域名过滤；协议以 **`catalog.rs` + `web_search/types.rs`** 为单一事实源，配置走 `[tools.web_search]` 子表，限速 / 超时归 `warnings`，**`auto`** 跟着**项目级 hosted 候选 + HTTP 降级链**走，跟兄弟工具 [`web_fetch.md`](web_fetch.md) 拆开各自负责一件事。
