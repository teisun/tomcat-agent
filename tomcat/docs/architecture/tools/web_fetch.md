# `web_fetch` 工具：URL 抓取、Markdown 化、域名权限与重定向收口

本文档是内置 **`web_fetch`** 工具的技术方案（OpenSpec **B 类**：`docs/architecture/tools/`），与兄弟文档 [`web_search.md`](web_search.md) **拆为两份独立满额文档**——双工具协议、PR 节奏、风险表互不依赖；共享术语（`url` / `cache key` / `SSRF` / `warnings` 等）在两篇各自完整书写，便于单篇审阅、单篇冻结。

**文首声明（与 `read.md` 全篇闭环口吻不同）**：

- **§3、§5**：描述本期 PR-WF-A/S/D/B 合入后的**目标态行为**与代码锚点；与实现不一致处以 **`src/` 代码为准**。
- **§1 观察指标表、§2.3–§2.4、§9、§10、§11**：描述**契约草案与路线图**（与 002 看板 [T2-P1-013](../../agents/TASK_BOARD_002/tasks/T2-P1-013.md) 一致）；合入后以 PR 更新本文状态列。
- **§2.4.5 PR-WF-P**：路线图条目（可选 LLM 摘要），**本期不实现**——仅占位字段并预留接口。
- **当前开发批次（2026-06 范围确认）**：本期先推进 **PR-WF-A / PR-WF-S / PR-WF-B**；**PR-WF-D（域名权限）** 与 **PR-WF-P（LLM 摘要）** 后置。本期安全边界仅包含 [`validate.rs`](../../../src/core/tools/web_fetch/validate.rs) 的 **URL 校验 / SSRF 守卫**。

写作约定见 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)（B 类：术语、调研、目标、**§4.1/§4.2**、One-Glance、测试、风险）。

> **实现锚点校准（2026-06 重构后核对，落地时以此为准）**：本文起草后 `src/` 有重构，以下锚点已漂移——
>
> 1. **`tool_exec` 已从单文件升为目录模块**：`src/core/agent_loop/tool_exec/`。中央分发在 [`tool_exec/mod.rs::execute_tool_tuple_full`](../../../src/core/agent_loop/tool_exec/mod.rs) 的 `match tc.name.as_str()`；每个工具的处理函数放在 `tool_exec/branches/<tool>.rs`（形如 `handle_read`），并在 [`tool_exec/branches/mod.rs`](../../../src/core/agent_loop/tool_exec/branches/mod.rs) 注册。**本文凡提「`tool_exec.rs` 加 `match "web_fetch"`」均指**：新增 `tool_exec/branches/web_fetch.rs`（`handle_web_fetch`）→ 在 `branches/mod.rs` 导出 → 在 `tool_exec/mod.rs` 的 match 增一臂。
> 2. **工具配置类型已移位**：`src/infra/config/types.rs` → [`src/infra/config/types/tools.rs`](../../../src/infra/config/types/tools.rs)；聚合结构为 `ToolsConfig { read, write, bash }`。`ToolsWebFetchConfig` 应作为新子表加进 `ToolsConfig`。
> 3. **权限锚点仍有效**：[`permission/types.rs::PermissionScope`](../../../src/core/permission/types.rs) 当前仍为五值（`Read/Write/Bash/BashApproval/Forbidden`）、[`primitive/types.rs::PrimitiveOperation`](../../../src/core/tools/primitive/types.rs) 仍为四值（`Read/Write/Edit/Bash`）、[`gate.rs::PermissionGate`](../../../src/core/permission/gate.rs) 仍是 `check` + `check_bash` 两法——PR-WF-D 的「新增第六值 `Domain` + `check_domain`」方案**与当前代码一致，可照常落地**。新增的 [`permission/url_like.rs`](../../../src/core/permission/url_like.rs)（`is_url_like` http/https 判定）可供 `validate.rs` 复用。
> 4. **新依赖**：`Cargo.toml` 当前**无** `moka`（无 LRU 工具）与 `html2md`——PR-WF-S 的缓存与 HTML→Markdown 须各自新增依赖；`reqwest 0.12` / `xxhash-rust(xxh32)` 已在。
> 5. **新增的子 Agent 工具白名单门闩**（重构引入）：[`tool_exec/guard.rs`](../../../src/core/agent_loop/tool_exec/guard.rs) 的 `is_reviewer_whitelisted_tool` / `is_verifier_whitelisted_tool` 会**拒绝**白名单外的任何工具。当前口径是：`web_fetch` 仍**不对 reviewer 开放**，但已对 **verifier** 开放，用于抓取静态网页证据；若后续要扩到 reviewer，再在 guard 白名单中补名并同步 prompt 文案。

---

## 目录

- [1. 目标与设计原则](#1-目标与设计原则)
- [2. 竞品 / 选型对比](#2-竞品--选型对比)
  - [2.1 抓取工具的典型关切](#21-抓取工具的典型关切)
  - [2.2 常见实现横向对比](#22-常见实现横向对比)
  - [2.3 落地选型决策表](#23-落地选型决策表)
  - [2.4 实施点（路线图）](#24-实施点路线图)
- [2.4.1 PR-WF-A：catalog 注册与工具说明](#241-pr-wf-acatalog-注册与工具说明)
  - [2.4.2 PR-WF-S：HTTP + Markdownify + 缓存 + 重定向](#242-pr-wf-shttp--markdownify--缓存--重定向)
  - [2.4.3 PR-WF-D：域名权限（PermissionGate::Domain）](#243-pr-wf-d域名权限permissiongatedomain)
  - [2.4.4 PR-WF-B：二进制 / PDF 持久化](#244-pr-wf-b二进制--pdf-持久化)
  - [2.4.5 PR-WF-P（路线图）：可选 LLM 摘要](#245-pr-wf-p路线图可选-llm-摘要)
- [3. 术语统一](#3-术语统一)
- [4. 协议（入参 / 出参 / Schema）](#4-协议入参--出参--schema)
- [5. One-Glance Map（文件职责总览）](#5-one-glance-map文件职责总览)
- [6. 调度时序（运行时图）](#6-调度时序运行时图)
- [7. 状态机（域名审批）](#7-状态机域名审批)
- [8. 配置与环境变量](#8-配置与环境变量)
- [9. 错误模型 / 截断 / 警告](#9-错误模型--截断--警告)
- [10. 测试矩阵（验收）](#10-测试矩阵验收)
- [11. 风险与应对](#11-风险与应对)
- [12. 历史决策（已被本方案取代或待定）](#12-历史决策已被本方案取代或待定)
- [13. 关联文档](#13-关联文档)

---

## 1. 目标与设计原则

**一句话**：让模型一句 `url` 拿到该网页的 **干净 Markdown**——当前实现先做 URL 校验 + 同源/`www.` 重定向守卫 + 正文/落盘分流；域名权限网关仍是后置路线图；二进制不污染上下文（PDF/图直接落盘返路径）；**不**做查询（查询走 [`web_search.md`](web_search.md)）。

### 1.1 观察指标表（与 §10 验收一一对应）

| 目标 | 观察指标（落地后可核对） | 说人话 |
|------|--------------------------|--------|
| G1 url→markdown 闭环 | catalog 注册 `web_fetch`；HTTP `text/html` / `text/markdown` 200 → 返回 `result` 字段为干净 markdown（已脱 `<script>` / `<style>` / `<nav>` 噪声） | 给个网址，回个 markdown。 |
| G2 重定向安全 | 仅同源 / `www.` 切换的 redirect 自动 follow（≤10 跳）；其它 → 不 follow，返回结构化 `redirect` 字段让模型显式重发；**不**走 reqwest 默认 redirect policy | 跨域跳转不悄悄跟，让模型自己选。 |
| G3 域名权限（路线图后置） | `PermissionGate` 新增 `Domain` op；配置 `[tools.web_fetch] allowed_domains / blocked_domains / preapproved_hosts` + 运行时 ask 三态决策；审计带 `permission_scope=Domain` | 这是 PR-WF-D 的目标态，不是当前批次已交付行为。 |
| G4 二进制不进上下文 | `Content-Type` 非 `text/*` / `application/json` / `application/xml` → 落盘到 `~/.tomcat/agents/<id>/tool-results/web-fetch-<hash>.<ext>`；返回体仅 `persisted_output_path` + 元数据；**不**把 base64 塞进 tool 消息 | PDF 落地、给路径，不灌进上下文撑爆 token。 |

### 1.2 非目标

| 非目标 | 推给 | 说人话 |
|--------|------|--------|
| 生成检索词并执行检索 | [`web_search.md`](web_search.md) | 找路标是另一个工具。 |
| 服务端 domain blocklist API | 003 迭代（自托管栈无对应服务端） | 不远程拉黑名单，靠本地配置 + 用户确认。 |
| Headless 浏览器渲染 | 002 看板后续 `web_browser` 工具 | 静态 HTML 抓取够用 80%；JS 重的页另起工具。 |
| 自动跨域 follow redirect（含 302→任意 host） | 不做 | open-redirect 风险大，宁可让模型显式重发。 |
| 抓 PDF 后做文本抽取 | 模型自己读路径或调 read 工具 | fetch 只负责落盘，不解析二进制。 |
| 鉴权 / cookie / Bearer 头自动注入 | 不做 | URL 含凭证一律拒；让用户用专门的 auth fetch 工具。 |
| MCP 转接同名工具 | 003 迭代 | 内置 vs MCP 同名冲突另起 ADR；本期内置版优先。 |

---

## 2. 竞品 / 选型对比

对标过 [agent-tools-comparison.md](../../reports/agent-tools-comparison.md) 中 **cc-fork-01 / hermes-agent / openclaw / pi-mono / pi_agent_rust** 五栈的抓取策略。下表为**已写入路线图的决策**，不是待办 brainstorm。

### 2.1 抓取工具的典型关切

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  本地 web_fetch 类工具通常要同时解决的四类问题                                │
├────────────────────┬─────────────────────────────────────────────────────┤
│  权限与外泄        │  任意 URL = 可能向外发本地数据 / 拉外部脚本             │
│                    │  → 域名网关 + 用户确认 + redirect 策略                │
│  HTML→可读         │  原 HTML 难读 → markdownify；噪声节点（script/nav）剥离 │
│  上下文 cost       │  10 MiB HTML → 100k markdown 也太大 → 落盘 + 续读       │
│  内容类型路由      │  text/html、application/pdf、image/png 不同处理         │
└────────────────────┴─────────────────────────────────────────────────────┘
```

### 2.2 常见实现横向对比

| 来源 / 形态 | HTTP 客户端 | HTML→Markdown | 重定向策略 | 权限 / 域名 | 二进制 | 备注 |
|-------------|------------|---------------|-----------|-------------|--------|------|
| **cc-fork-01** | `fetch` + `axios` 等价 | `turndown` + 噪声节点剥离 | **手写** `getWithPermittedRedirects` + `isPermittedRedirect`（仅同源/www.；其它返结构化）| 客户端 + 服务端 `domain_info` API + `domain:hostname` rule | PDF/图：base64 inline（勿盲目照搬 cc 实现） | **工程范本**最完整；见 [`utils.ts:62-77,99-128,212-243`](../../../../cc-fork-01/src/tools/WebFetchTool/utils.ts) |
| **hermes-agent** | `requests` / Firecrawl SaaS | Firecrawl 服务端 markdownify / Tavily extract | follow-all（HTTP 默认） | 仅 secret-prefix 拒（`sk-`/`ghp-` 等） | 不抓 | `web_extract_tool` 走 SaaS；见 [`web_tools.py:1156-1300`](../../../../hermes-agent/tools/web_tools.py) |
| **openclaw** | 内嵌 / native | 跟 search 一样接 plugin | follow-all | 弱 | 不抓 | 抓取功能弱、检索强 |
| **pi-mono** | — | — | — | — | — | **没做** fetch；通过 MCP 转接 |
| **pi_agent_rust** | — | — | — | — | — | **没做** fetch；本仓本期补齐 |
| **本仓库 `web_fetch`**（路线图） | reqwest 0.12（已存在） | 自家选型（见 §2.4.2） | 手写 `isPermittedRedirect`，仅同源/www.；off-host 返结构化 `redirect` | `PermissionGate::Domain` 新维度 + allow/block/preapproved 三档 | 落盘 `tool-results/`，返路径 | wasm 友好；与 cc-fork-01 数字对齐 |

**结论（写入路线图）**：**HTTP 与重定向策略**对齐 **cc-fork-01**；**不引 SaaS** 不走 hermes 的 Firecrawl 路；**域名权限**上做得比 cc-fork-01 更深——把它的「客户端 rule + 服务端 API」简化为「客户端配置 + 运行时确认」，复用现有 `PermissionGate` 三态机制。

### 2.3 落地选型决策表（维度取舍）

**代码落点、交付物、阶段**见 **[§2.4](#24-实施点路线图)**，与 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) **§4.1 / §4.2** 分工一致。**`决策`** 列钉本行裁决结论（**SHOULD**）。

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| **Backend 形态** | 正文抽取是否要多 vendor | **采用** reqwest + 自研 HTML→Markdown；**拒绝** 多 SaaS backend。 | hermes（Firecrawl）+ 三家 Agent 常见形态 | **reqwest** + 自家 HTML→Markdown；维护面可控；与自托管目标一致 | × 多 backend 维护成本高；× Firecrawl SaaS | 一条 reqwest 路打到底；不引 SaaS。 |
| **输出形态** | 超大 markdown 怎么交给模型 | **采用** 按 `MAX_MARKDOWN_LENGTH` 阈值**分流**：小页 inline、超大页**落盘完整 md + 回执 head**。 | hermes `web_extract` `use_llm_processing=False` 路径 + 本仓 `bash`/二进制落盘 pattern | `chars ≤ MAX_MARKDOWN_LENGTH` → 整篇 inline 进 `result`（1 轮，最常见）；`> 阈值` → 完整 md 落 `tool-results/`，`result` 仅给 head 预览 + `persisted_output_path` + `total_chars` + `warnings += "markdown_persisted"`，模型用 `search_files`(target=content) 定位 + `read` 分页精读；PR-WF-P 再接可选摘要 | × 纯尾部截断（超大页直接丢后半，有损）；× MVP 卷入 small model 摘要选型 | 小页直接给；大页落盘给路径 + 开头，模型自己搜着读，不丢内容也不撑爆上下文。 |
| **域名权限** | fetch 是否与 Read 共用一维 | **采用** `PermissionGate::Domain` 独立维度。 | cc-fork-01 `checkPermissions` + [`permission/types.rs::PermissionScope`](../../../src/core/permission/types.rs) | 新增 `PermissionGate::Domain` + config + confirm；与 Read/Write/Bash 并列 | × 无权限裸连；× 复用 Read 维度耦合错位 | 抓网络 != 读文件，权限要分开；老的 Read 维度别污染。 |
| **重定向策略** | open-redirect 与可用性平衡 | **采用** `Policy::none()` + 手写同源/`www.` 循环。 | cc-fork-01 `getWithPermittedRedirects` + `isPermittedRedirect` | **`Policy::none()` + 手写循环**：同源 / `www.` 允许；off-host **结构化** `redirect_off_host` | × 默认 follow-all；× 全拒过严 | 同源跳转跟，跨域跳转停下来问模型。 |
| **缓存策略** | 同 url 短时间重抓如何省掉、缓存放哪 | **采用** moka 进程内缓存，存 `WebFetchOutput` **信封**（含 head + path 元数据），**不**缓存盘上大文件本体。 | 本仓 [`web_search/cache.rs`](../../../src/core/tools/web_search/cache.rs) 同 pattern（`moka` 已在 [`Cargo.toml`](../../../Cargo.toml)） | `Cache<CacheKey, WebFetchOutput>`，key=`(canonical_url, format)`；TTL=15 min；weigher 50 MB 上限；命中 → `cached=true` 不发 HTTP、不重新落盘（复用上次 `persisted_output_path`）；信封小、与磁盘载荷正交分层（**一套缓存**，非两套） | × 磁盘持久化缓存（审计盲点 + 生命周期复杂）；× 无缓存（重复抓费流量 / 易触发限速） | 15 分钟内同 url+format 不重抓；缓存只存小信封，大文件留在盘上。 |
| **URL 校验 / SSRF 守卫** | 任意 url 可能拨内网 / 带凭证外泄 | **采用** `validate.rs` 入口纵深拒：凭证 / 非 `http(s)` / 单段 host / **任意 IP literal（私网 / 公网均拒）**；`http` 入参首跳前**就地升 `https`**。 | cc-fork-01 `validateURL` + tomcat 自加 IP literal 拒绝 | 比 cc-fork 多一道 IP literal 拒；redirect **每跳**重校验（防跳到 `http://10.0.0.1`）；与 [`web_search.md`](web_search.md) SSRF 守卫纵深 | × 裸连（SSRF）；× 仅靠远程 `domain_info` 预检（自托管无此服务） | 抓之前先把 IP 地址、内网地址、带密码的 url 挡掉。 |
| **二进制 / 非文本内容** | PDF / 图等是否进上下文 | **采用** content-type 非文本 → 落盘 `tool-results/`，回执仅 `persisted_output_path`，**不**塞 base64。 | cc-fork-01 `WebFetchTool` + 本仓 [`resolve_agent_trail_dir`](../../../src/infra/mod.rs) | base64 1 MB → 4 MiB token 会炸；落盘返路径、模型按需 `read`；content-type 不可信时 magic 字节兜底 | × base64 inline（撑爆上下文）；× 直接丢弃（丢信息） | PDF / 图存盘给路径，不灌进上下文。 |
| **错误 / 限速归一化** | 429 / 5xx / 超时是否让整轮 tool fail | **采用** 429 / 5xx / 超时 → `warnings`（+ 可能 `truncated=true`），**不抛 `Err`**。 | web_search 同口径 + [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) §10 | 一次抓失败不该让整轮 tool fail / 触发 retry 风暴；`result` 可空，模型自决下一步 | × 直接 `Err` 中断整轮；× 静默吞掉（模型不知情） | 抓不到就给个 warning，别把整轮对话搞崩。 |

### 2.4 实施点（路线图）

**实施顺序**：**① PR-WF-A**（catalog 注册 + 工具说明 + tool_exec match）→ **② PR-WF-S**（reqwest GET + URL 校验 / SSRF 守卫 + markdownify + 缓存 + 重定向）→ **③ PR-WF-D**（`PermissionGate::Domain` 新维度 + 运行时确认）→ **④ PR-WF-B**（二进制 / PDF 落盘）→ **⑤ PR-WF-P**（路线图：可选 LLM 摘要）。**先注册再补 backend**——避免后续 PR 反复改字面量与断言。

**当前开发批次**：按已确认范围，先做 **① PR-WF-A → ② PR-WF-S → ④ PR-WF-B**；**③ PR-WF-D** 与 **⑤ PR-WF-P** 后置。也就是说，本期先把 **URL 校验 / SSRF 守卫**、抓取主链、正文/二进制分流做通，不接域名授权 gate。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
| --- | --- | --- | --- | --- |
| **PR-WF-A**（命名 + catalog） | **交付物**：3 字段 schema；占位 err；工具说明由 `catalog.rs` 的 `description` 承载（对齐 cc-fork 文案；本期不单独改 `system_prompt.rs`）。**落地点**：catalog / `tool_exec` / catalog `description` | [`catalog.rs`](../../../src/core/tools/contract/catalog.rs)、[`tool_exec/mod.rs`](../../../src/core/agent_loop/tool_exec/mod.rs)（match 增臂）+ 新 [`tool_exec/branches/web_fetch.rs`](../../../src/core/agent_loop/tool_exec/branches/mod.rs) | `catalog_test::web_fetch_registered`、`submodules_test::tool_exec_web_fetch_requires_runtime_injection`、`submodules_test::tool_exec_web_fetch_routes_to_runtime` | 先把名字 / schema / 工具说明 / 占位接好。 |
| **PR-WF-S**（HTTP + URL 校验 / SSRF 守卫 + Markdownify + 缓存 + 重定向） | **交付物**：GET+UA+Accept；`Policy::none()` + 手写 redirect 循环；**URL 校验 / SSRF 守卫**；HTML→Markdown；LRU/TTL；体积/超时/分流常量（`MAX_MARKDOWN_LENGTH` inline 阈值 + `MARKDOWN_HEAD_CHARS` + `MAX_HTTP_CONTENT_LENGTH`）。**落地点**：`core/tools/web_fetch/*`、`ToolsWebFetchConfig` | 新模块 `core/tools/web_fetch/{mod,types,fetcher,markdownify,cache,validate,redirect}.rs`、[`infra/config/types/tools.rs`](../../../src/infra/config/types/tools.rs) 的 `ToolsConfig` 加 `ToolsWebFetchConfig`；新增 `moka` + `html2md` 依赖 | `web_fetch_url_to_markdown`、`web_fetch_cache_hit_skips_http`、`web_fetch_redirect_same_host_followed`、`web_fetch_redirect_off_host_returns_structured`、`web_fetch_oversize_markdown_persisted`、`web_fetch_url_with_credentials_rejected`（**PENDING**） | 抓 + URL/SSRF 硬挡板 + 转换 + 缓存 + 跳转策略一并接好。 |
| **PR-WF-D**（域名权限；路线图后置，当前开发批次不做） | **交付物**：新增 `check_domain` trait 方法 + `PermissionScope::Domain` 第六值；三档域名配置；审计 scope 串。**落地点**：`core/permission/*`、`web_fetch/fetcher.rs`、`infra/audit/mod.rs` | [`core/permission/`](../../../src/core/permission/)（新增第六值到 [`types.rs::PermissionScope`](../../../src/core/permission/types.rs) + 新增 `check_domain` 方法到 [`gate.rs::PermissionGate`](../../../src/core/permission/gate.rs) trait）、`web_fetch/fetcher.rs`（gate 调用）、[`infra/audit/mod.rs`](../../../src/infra/audit/mod.rs)（`PrimitiveAuditEntry::permission_scope` 映射新增 `"domain"` 串） | **后置（非本期验收）**；验收锚点见 §10 `Permission Domain` / `审计 scope=Domain` | 抓任何域名前先过闸；本期先不落这层。 |
| **PR-WF-B**（二进制 / PDF 持久化） | **交付物**：content-type 分流；`persisted_output_path`。**落地点**：`fetcher.rs`、`persist.rs`、`resolve_agent_trail_dir` | `web_fetch/fetcher.rs`、`web_fetch/persist.rs`、复用 `infra::resolve_agent_trail_dir` | `web_fetch_pdf_persisted_to_tool_results`、`web_fetch_html_inline_not_persisted`、`web_fetch_persist_path_in_response`、`web_fetch_image_persists_with_correct_ext`（**PENDING**） | PDF 直接给路径，别灌 base64 进上下文。 |
| **PR-WF-P（路线图）** | **交付物**：`use_llm_processing` + small model 摘要。**落地点**：`web_fetch/summarize.rs` + LLM 客户端 | `web_fetch/summarize.rs`（新增）、复用 LLM 客户端 small model 路径 | 留待该 PR 单独写 §10 行（**本期不实现**） | 落盘 + 分页读还嫌折腾就让 small model 摘一下；本期先占位。 |

下文按实施点展开**技术要点与示意图**；**交付边界与代码落点仍以表为准**。

#### 2.4.1 PR-WF-A：catalog 注册与工具说明

- **交付**：`BUILTIN_TOOL_CATALOG` 增加 **`name = "web_fetch"`**；`web_fetch_parameters()` 输出 3 字段 schema（`url` 必填、`prompt`/`format` 可选）；**当前实现由 [`catalog.rs`](../../../src/core/tools/contract/catalog.rs) 条目的 `description` 承载工具说明**（与 `web_search` 同口径；本期不单独改 [`system_prompt.rs`](../../../src/core/llm/system_prompt.rs)），文案对齐 [cc-fork-01 `prompt.ts`](../../../../cc-fork-01/src/tools/WebFetchTool/prompt.ts)：「private/authenticated URL（`http://localhost`、含 token 的）会失败；redirect 不会自动跨域 follow，遇到 `redirect_off_host` 请改用新 url 重发」；`tool_exec` 添加 `match "web_fetch"` 占位分支。
- **历史回放**：旧 transcript 含 `web_fetch`（无注册）→ 走 **未知工具** 路径（与 read 的 `read_file` 同口径，不重定向）。
- **与后续 PR 的衔接**：PR-WF-S 的 fetcher 直接挂到本步占位的 `tool_exec` 分支；PR-WF-D（后续批次）在 fetcher 调用前插 gate；PR-WF-B 在 fetcher 出口加 content-type 分流。

```text
  LLM / transcript
        │
        ▼
┌───────────────────┐     注册名仅 "web_fetch"
│  catalog.rs       │──────────────────────────────┐
└───────────────────┘                              │
        │                                            ▼
        ▼                               ┌────────────────────┐
  tool_exec  match "web_fetch"          │ "webfetch" 等拼错  │
        │（PR-WF-A 占位 friendly err）    │ → UnknownTool 错误 │
        ▼                                └────────────────────┘
   当前批次 PR-WF-S/B 接入后
   → validate(url) / SSRF 守卫 → fetcher.fetch(url)
   （PR-WF-D 后续再在 validate 与 fetcher 之间插 host gate）
```

**说人话**：先把名字、3 个参数和工具说明（当前放在 `catalog.description`）放进去；后面接 backend 不再动 catalog。

#### 2.4.2 PR-WF-S：HTTP + URL 校验 / SSRF 守卫 + Markdownify + 缓存 + 重定向

- **HTTP 客户端**：reqwest 0.12 builder：
  - `User-Agent`：`pi/{CARGO_PKG_VERSION} (web_fetch)`（pi-mono 同档；不模仿浏览器避免 cf 误识别）
  - `Accept`：`text/markdown, text/html;q=0.9, application/xhtml+xml;q=0.8, */*;q=0.5`
  - `redirect`：`reqwest::redirect::Policy::none()`——**不**让 reqwest 自动 follow；自己写循环。
  - `timeout`：`FETCH_TIMEOUT_MS=60_000`（与 cc-fork-01 一致）
- **URL 校验 / SSRF 守卫**（[`validate.rs`](../../../src/core/tools/web_fetch/validate.rs) 新增）：
  - **与 cc-fork-01 对齐的子集**（见 [`utils.ts:139-168`](../../../../cc-fork-01/src/tools/WebFetchTool/utils.ts) `validateURL`）：长度 ≤`MAX_URL_LENGTH=2000`；`URL` 可解析；**拒** `username`/`password`；hostname 至少两段（`parts.length < 2` 拒，拦 `localhost` 等单标签名）。cc-fork 注释写明「不在此校验协议」——实际请求前会 **http→https 升级**（见下条）。
  - **tomcat 纵深补强（cc-fork 客户端 `validateURL` 未做）**：解析出 **IP literal 一律拒绝**：loopback / RFC1918 / ULA（`127.0.0.0/8`、`10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`、`::1`、`fc00::/7`）返回 `private or loopback IP rejected`；其它公网 IP literal 返回 `IP literal host rejected`。cc-fork 另依赖 **Anthropic `domain_info` 预检 API**（[`utils.ts:176-203`](../../../../cc-fork-01/src/tools/WebFetchTool/utils.ts)）拦恶意域；本仓无该云端预检服务，故本期先落 **IP literal 拒绝 / URL 校验 / SSRF 守卫**，`PermissionGate::Domain` 留待后续 PR-WF-D 补位。
- **http→https 升级（对齐 cc-fork，且与 `isPermittedRedirect` 无关）**：在 **首次 GET 之前**（进入 `redirect` 循环前）若 `parsed.protocol == "http:"` 则就地改写为 `https:` 再发请求，与 cc-fork [`getURLMarkdownContent` L372-378](../../../../cc-fork-01/src/tools/WebFetchTool/utils.ts) 同序。**禁止**把「`Location: http→https`」误当成 `isPermittedRedirect` 放行——cc-fork 在 `isPermittedRedirect` 里 **协议不同即 `false`**（[`utils.ts:220-222`](../../../../cc-fork-01/src/tools/WebFetchTool/utils.ts)），`http→https` 的 Location 跳转应走 **`redirect_off_host` / 让模型显式重发**，除非产品后续单独开「允许协议升级 redirect」开关（默认关）。
- **重定向循环**（[`redirect.rs`](../../../src/core/tools/web_fetch/redirect.rs) 新增；对齐 cc-fork-01 [`utils.ts:212-260`](../../../../cc-fork-01/src/tools/WebFetchTool/utils.ts) `isPermittedRedirect` + `getWithPermittedRedirects`）：

  ```text
  loop count in 0..MAX_REDIRECTS (=10):
      resp = client.get(current_url).send().await?
      if resp.status() not in 3xx: break
      let next = resp.headers().get("Location")?
      if isPermittedRedirect(current_url, next):    // 同源或 www. 切换
          current_url = next
          continue
      else:
          return Ok(WebFetchOutput {
              redirect: Some(RedirectInfo { original_url, redirect_url: next, status_code }),
              code: status, content_type: None, result: "",
              warnings: ["redirect_off_host"], ...
          })
  if count == MAX_REDIRECTS:
      return Err(Tool("redirect loop > 10"))
  ```

- **`isPermittedRedirect(from, to)`** 真值表（**逐条对齐** cc-fork-01 [`utils.ts:212-239`](../../../../cc-fork-01/src/tools/WebFetchTool/utils.ts)；先比较 **protocol / port / 凭证**，再 `stripWww` 比较 hostname）：

  | from URL（节选） | to URL（节选，`Location` 解析后绝对化） | 允许 | 备注（源码依据） |
  |-----------|---------|------|------|
  | `https://example.com/a` | `https://example.com/b` | ✅ | 同 protocol/port，hostname 同 |
  | `https://example.com/...` | `https://www.example.com/...` | ✅ | `stripWww` 后相等 |
  | `https://www.example.com/...` | `https://example.com/...` | ✅ | 同上 |
  | `https://example.com/...` | `https://api.example.com/...` | ❌ | 子域 ≠ `www.` 切换 |
  | `https://example.com/...` | `https://evil.com/...` | ❌ | 主机名不同 |
  | `https://example.com/...` | `http://example.com/...` | ❌ | **协议不同 → `false`**（L220-222） |
  | `http://example.com/...` | `https://example.com/...` | ❌ | **同上**；**非**「Location 自动升级」。入参 `http:` 应在首跳前走 **就地升级**（上一段），不走 `isPermittedRedirect` 特判 |

- **重定向策略的作用**（为什么不是默认 follow-all）：

  | 作用 | 具体体现 | 说人话 |
  |------|----------|--------|
  | **防 open-redirect / 钓鱼跳转** | `reqwest::redirect::Policy::none()` + `isPermittedRedirect` 仅放行同源 / `www.` 切换；`example.com -> evil.com` 这类 off-host 跳转直接停下并返回 `RedirectInfo` | 不让工具悄悄被带到另一个站。 |
  | **防 redirect 绕过 SSRF 守卫** | redirect 循环里每一跳都重新做 URL 校验；即便首个 URL 合法，也不能借 302 跳到 `10.0.0.1` / `127.0.0.1` / 其它私网地址 | 不给“先放行、再跳内网”留后门。 |
  | **把控制权留给模型而不是 HTTP 客户端** | off-host 跳转不自动 follow，而是把 `redirect_url`、`status_code` 回给模型，让模型显式决定是否用新 URL 重发 | 是否继续抓新站点，要让模型自己拍板。 |
  | **兼顾正常站点可用性** | 同 host 和 `apex <-> www` 变体仍自动 follow，避免 `example.com -> www.example.com` 这类常见部署把工具变得过度保守 | 该跟的正常跳转还是跟，不至于一跳就“卡死”。 |
  | **避免循环和协议语义混乱** | 限制 `MAX_REDIRECTS=10`；`http` 入参在首跳前就地升级 `https`，而不是把 `Location: http -> https` 混进允许重定向规则里 | 跳太多就报错，协议升级也只走一条清晰路径。 |

  **说人话**：这套策略是在 **安全性** 和 **可用性** 之间划线: 同源 / `www.` 这类低风险跳转自动跟，跨 host / 跨协议这类高风险跳转停下来，把决定权交还给模型。

- **HTML→Markdown 选型钉死表**：

  | 候选 crate | wasm 兼容 | 体积 | 噪声剥离能力 | 决策 |
  |------------|-----------|------|---------------|------|
  | [`html2md`](https://crates.io/crates/html2md) | ✅ | 小 | 中（默认剥 `<script>`/`<style>`） | **MVP 选**——成熟、API 简单、无 unsafe |
  | `pulldown-cmark` 反向 | ✅ | 小 | 无（cmark 是 md→html 单向） | ✗ 反向需自写 |
  | 自写 turndown 子集 | ✅ | 大（自带规则集） | 完全可控 | ✗ 维护成本高 |
  | `scraper` + 手写规则 | ✅ | 中 | 完全可控 | ✗ 手写规则多 |

  **MVP 选 `html2md`**；输出后追加一道 post-process（regex 删 `<style>...</style>`、`<script>...</script>`、`<nav>...</nav>` 残留），与 cc-fork-01 turndown rules（`removeAttributes` / `removeStyleTags` / `removeScriptTags` / `removeNavTags` / `removeFooterTags`）等价。

- **缓存**（[`cache.rs`](../../../src/core/tools/web_fetch/cache.rs)）：MokaCache `<CacheKey, WebFetchOutput>`，key=`(canonical_url, format)`；TTL=15 min；total_weight 上限 50 MB（按 `result` + `persisted_output_path` 元数据估算）；命中 → `cached=true` 不发 HTTP，**也不**重新落盘（直接返回上次的 `persisted_output_path`）。
- **正文体积分流**（替代旧「纯尾部截断」）：markdownify 后按 `MAX_MARKDOWN_LENGTH (=100_000)` 字符**分流**——
  - `result.chars().count() ≤ MAX_MARKDOWN_LENGTH` → 整篇 inline 进 `result`，`persisted_output_path=null`、`total_chars=result.chars().count()`（最常见，1 轮拿到全文）。
  - `> MAX_MARKDOWN_LENGTH` → **不截断丢内容**：把**完整 md** 落盘到 `tool-results/web-fetch-<hash>.md`（与二进制落盘同一目录/注入框架，见 §2.4.4），`result` 仅放前 `MARKDOWN_HEAD_CHARS (=2_000)` 字符的 **head 预览**（在 UTF-8 字符边界切），并填 `persisted_output_path`、`total_chars=完整字符数`、`warnings += "markdown_persisted"`；head 尾部追加提示句「...full markdown persisted to <path> (total <N> chars); use search_files(target=content) to locate then read(offset=...) to page through」。
  - **为何不再纯截断**：落盘点在 `agent_trail_readonly_dirs` 内，对 `read` / `search_files`（均 `PermissionScope::Read`）由 gate Layer 3 直接 Allow（[`gate.rs::in_agent_readonly_set`](../../../src/core/permission/gate.rs)），模型可无损地分页精读，避免「超大页丢后半」。
- **HTTP body 上限**：响应流式读 → 累计 byte 数 > `MAX_HTTP_CONTENT_LENGTH=10 MiB` → 中断读取并 `truncated=true + warnings += "http_oversize"`；不解 markdownify 完整文档（`html2md` 只接收前 10 MiB 字节）。

```text
  validate(url) → ok / Err(URL校验 / SSRF 守卫)
        │
        ▼
  cache.lookup(key)
        │
        ├─ hit ──▶ 返回 (cached=true)
        │
        ▼ miss
  redirect 循环（≤10 跳；isPermittedRedirect）
        │
        ├─ off-host ──▶ 返回 RedirectInfo + warnings
        │
        ▼ landed
  resp = response
        │
        ▼
  content-type 路由（PR-WF-B）：
        │
        ├─ text/* / json / xml ──▶ html2md → post-process → 体积分流(inline / 落盘+head) → result
        │
        └─ 其它 ──▶ persist → persisted_output_path
        │
        ▼
  cache.put + return WebFetchOutput
```

> **当前开发批次说明**：PR-WF-D 的 host 级权限 gate 暂不接入此流程；本期网络安全边界仅由 `validate.rs` 的 URL 校验 / SSRF 守卫承担。

**说人话**：URL 先过校验（长度、凭证、scheme、hostname 形态、**IP literal（含公网）**——最后一项是 tomcat 比 cc-fork 客户端更严的地方），**http 入参在首跳前就地升 https**（对齐 cc-fork `getURLMarkdownContent`），缓存命中早返；reqwest **不**自动 follow redirect，自己一跳跳查 `isPermittedRedirect`（**含跨协议 Location → 停**）；正文走 `html2md` + 后处理删 script/style/nav；二进制走 PR-WF-B 落盘路径；正文超 100k 字符**整篇落盘**（不丢内容）+ 回执给 head 预览 + 路径 + `total_chars` + warning，模型再用 `search_files`/`read` 分页精读。

#### 2.4.3 PR-WF-D：域名权限（`PermissionGate::Domain`，路线图后置；当前开发批次不做）

- **当前开发批次范围**：本节保留为路线图设计与后续实现锚点；本期不改 `core/permission/*`、`session_grants.rs`、`infra/audit/mod.rs`，也不把 host gate 接入 `web_fetch` 主流程。

- **新增 scope 值**：[`permission/types.rs::PermissionScope`](../../../src/core/permission/types.rs) 当前枚举 `Read / Write / Bash / BashApproval / Forbidden` 五值；**新增** `Domain` 第六值（`#[serde(rename_all = "snake_case")] = "domain"`）。审计串新增对应映射在 [`infra/audit/mod.rs`](../../../src/infra/audit/mod.rs)（`PrimitiveAuditEntry::permission_scope: Option<String>` 接收 `"domain"` 串）。注意 [`PrimitiveOperation`](../../../src/core/tools/primitive/types.rs)（gate `check` 方法的入参类型，当前 4 值 `Read/Write/Edit/Bash`）**不**新增 `Domain` —— web_fetch 走独立 `check_domain` 方法（见下条），不复用 `check(op, path)` 的路径权限主路。
- **gate 接口**：[`PermissionGate`](../../../src/core/permission/gate.rs) 当前 trait 含 `check(op: PrimitiveOperation, path: &str) -> Result<PermissionDecision, AppError>` 与 `check_bash(command: &str) -> Result<PermissionDecision, AppError>` 两个核心检查方法。**新增** `check_domain(host: &str) -> Result<PermissionDecision, AppError>` 第三个方法，与现有两个平行；返回 `Allow / NeedConfirm / Deny` 三态，复用既有 `PermissionDecision` 枚举不动。
- **决策链**（参考 cc-fork-01 [`WebFetchTool.checkPermissions:104-180`](../../../../cc-fork-01/src/tools/WebFetchTool/WebFetchTool.ts) 的 `domain:hostname` rule 引擎）：

  ```text
  check_domain(host):
    1. blocked_domains 命中（子域 suffix） → Deny("blocked by config")
    2. preapproved_hosts 命中            → Allow(grant=BuiltinDefault, scope=Domain)
    3. session_grants 含 host           → Allow(grant=SessionScope, scope=Domain)
    4. allowed_domains 命中              → Allow(grant=BuiltinDefault, scope=Domain)
    5. allowed_domains 非空但未命中     → Deny("not in allowlist")
    6. （配置 require_confirm=true）     → NeedConfirm(reason="never seen this domain")
    7. 默认（require_confirm=false）     → Allow(grant=BuiltinDefault)
  ```

- **运行时确认 UX**（与 Read/Bash 同 trait，复用 [`session_grants.rs`](../../../src/core/permission/session_grants.rs) 三态）：
  - **Allow Once**：本次抓 + 不持久；
  - **Allow & persist for session**：写入 `session_grants` 内存表，本会话内同 host 跳确认；
  - **Deny**：本次拒；不持久。
- **审计**：每次 fetch 都 `record_primitive(scope=Domain, grant_type=..., trigger=...)` —— 与 bash 同形，但 `extract_paths` 那一套不需要（只一个 host）。

```text
  fetch(url)
       │
       ▼
  parse host
       │
       ▼
  permission_gate.check_domain(host)
       │
   ┌───┼───────────────┬────────────────┐
   ▼   ▼               ▼                ▼
 Allow NeedConfirm   Deny             已 session 授权
   │   │               │                │
   │   │               ▼                │
   │   │      record_primitive(failure)│
   │   │      Err(Domain blocked)      │
   │   ▼                                │
   │  prompt user (UI)                 │
   │   │                                │
   │   ┌───┴────────────────┐           │
   │   ▼                    ▼           │
   │  AllowOnce        AllowAndPersist  │
   │   │                    │           │
   │   │       session_grants.insert(host)
   │   │                    │           │
   │   └────────────────────┘           │
   ▼                                    ▼
 record_primitive(success, scope=Domain) → 进 fetcher（PR-WF-S）
```

**说人话**：新加一档权限，专管 fetch 域名；和 Read/Bash 共用一套 ask 流程，但配置 / 审计字段都独立。

#### 2.4.4 PR-WF-B：二进制 / PDF 持久化

- **content-type 路由**：

  | content-type | 路径 | result 字段 | persisted_output_path |
  |--------------|------|-------------|-----------------------|
  | `text/html` / `text/markdown` / `text/plain` | markdownify（PR-WF-S）→ 体积分流 | `≤阈值`：整篇 md；`>阈值`：head 预览 | `≤阈值`：`null`；`>阈值`：`tool-results/web-fetch-<hash>.md` |
  | `application/json` | 原样字符串 | UTF-8 lossy 字符串 | `null` |
  | `application/xml` / `text/xml` | 原样字符串 | UTF-8 lossy 字符串 | `null` |
  | `application/pdf` | 落盘 | `""`（空串） | `~/.tomcat/.../web-fetch-<hash>.pdf` |
  | `image/png` / `image/jpeg` / `image/gif` / `image/webp` | 落盘 | `""` | `~/.tomcat/.../web-fetch-<hash>.<ext>` |
  | 其它（默认） | 落盘 + warnings += "binary_persisted" | `""` | `~/.tomcat/.../web-fetch-<hash>.bin` |

- **magic 路由**（content-type 不可信时回退）：响应前 16 字节读出，若魔数命中（与 [`read.rs::detect_inline_mime`](../../../src/core/tools/primitive/executor/read.rs) 同表）→ 用魔数推断的 mime 覆盖响应头声明；与 [read.md PR-RJ T3-b](read.md) 思路一致。
- **落盘路径**：`resolve_agent_trail_dir(cfg)? .join("tool-results").join(format!("web-fetch-{hash}.{ext}"))`，其中 `cfg: &AppConfig` 由 executor 装配期注入（与 [`bash_persist_dir`](../../../src/core/tools/primitive/executor/mod.rs) 同一注入框架）；`hash` = `xxh32(url)` 6 位 hex（复用 [`Cargo.toml`](../../../Cargo.toml) 已有的 `xxhash-rust` 依赖与 `xxh32` feature）；`tool-results` 子目录在启动期由 [`ensure_work_dir_structure`](../../../src/infra/config/load.rs) 已 `create_dir_all`，本工具运行时仅做幂等性兜底；写入用 `tokio::fs::write` 一次性写。
- **不在落盘中的字段**：二进制落盘时 `persisted_output_path` 仅放绝对路径字符串、`result` 字段为空；`bytes` 字段（响应总字节数）正常填，方便模型决策是否要读全文。
- **与「超大正文落盘」的区别**：二进制落盘 `result=""`；而 §2.4.2 的超大 markdown 落盘 `result=head 预览`（非空）+ `total_chars`——两者都填 `persisted_output_path`，但前者扩展名按 content-type（`.pdf`/`.png`/…），后者固定 `.md`。模型据 `result` 是否为空即可区分「二进制（去 read 原始字节）」与「超大文本（去 search/read 续读 markdown）」。
- **与 [`read.md` 多模态注入](read.md#424-pr-rj-0-与-t3-a--b--c多模态与-openai-注入边界) 的关系**：fetch 落盘后**不**自动注入到下一条 user message 的 Parts——模型自己看 `persisted_output_path` 决定是否调 `read` 工具读出来；与 read 的 image/pdf 自动注入策略**不同**（read 是用户主动点开的本地文件，fetch 是模型从外部抓的，多了一道间接审批）。

```text
  HTTP response headers + body (≤10 MiB)
        │
        ▼
  read first 16 bytes → magic match?
        │
   ┌────┴─────┐
   ▼          ▼
  match    no match
   │          │
   │          └── trust Content-Type header
   │
   ▼
  mime ∈ text/html|markdown|plain|application/json|xml ?
        │
   ┌────┴─────┐
   yes        no
   │          │
   ▼          ▼
  markdownify  resolve_agent_trail_dir + xxh32(url)
   + 体积分流   │
   │          ▼
   │       tool-results/web-fetch-<hash>.<ext>
   │       fs::write(content)
   │          │
   │          ▼
   │       persisted_output_path = abs_path
   │
   ▼
  WebFetchOutput
```

**说人话**：text 类直接 markdownify；其它（PDF/图/二进制）算文件落到 `tool-results/`，回执只给路径不给字节，模型自己决定要不要再调 read 读出来。

#### 2.4.5 PR-WF-P（路线图）：可选 LLM 摘要

- **不在本期实现**——本节仅作占位说明。
- **目标**：当用户配置 `[tools.web_fetch] use_llm_processing=true` + 入参 `prompt` 非空时，markdownify 后的 markdown 不直接返回，而是调用 small/fast model（如 `gpt-4o-mini` 或本地小模型）按 `prompt` 生成摘要，再返回。
- **为何延后**：本期 [`registry.rs`](../../../src/core/llm/registry.rs) 暂只统一 main model；接 small model 涉及双 model 注册 / 默认 model 选择 / token 计费策略，**不在 web_fetch 主路径上**——单独 PR 处理。
- **占位**：catalog schema 已含 `prompt` 字段（PR-WF-A 时保留）；MVP 阶段 `prompt` **仅作回执文本提示**（写入 warnings 末尾），不参与摘要逻辑——与 hermes [`web_extract_tool:1210-1228`](../../../../hermes-agent/tools/web_tools.py) 的 `use_llm_processing=False` 默认分支同档。

```text
  （路线图，本期不实现）

  WebFetchOutput { result: <inline md 或 head 预览> }
        │
        ▼
  use_llm_processing=true && prompt is Some?
        │
   ┌────┴─────┐
   no         yes
   │          │
   │          ▼
   │       small_model.complete(prompt + markdown)
   │          │
   │          ▼
   │       result = summary
   │
   ▼
  return
```

**说人话**：MVP 小页直接给 markdown、大页落盘给 head + 路径让模型续读；路线图里再接 small model 做摘要——本期先把字段位留好。

---

## 3. 术语统一

| 术语 | 语义（大白话） | 数据载体 | 行为约束 | 说人话 |
|------|----------------|----------|----------|--------|
| **`url`** | 模型给的目标地址 | `WebFetchArgs.url: String` | 必填、长度 ≤2000、有 scheme + host、不含凭证、host 必须有 `.` 且**不能是任意 IP literal（私网 / 公网均拒）** | 一个标准 http(s):// URL，不许带密码，也不许直接写 IP。 |
| **`markdownify`** | HTML → Markdown 的过程 | `web_fetch/markdownify.rs` | 走 `html2md` + post-process（删 `<script>`/`<style>`/`<nav>`/`<footer>` 残留）；与 cc-fork-01 turndown 规则等价 | 把网页的代码壳子剥掉，留下文字。 |
| **preapproved hosts** | 启动即放行、不需问用户的域名 | `[tools.web_fetch] preapproved_hosts: Vec<String>` | gate 决策链 step 2；命中 → `Allow(grant=BuiltinDefault)` | 「这几个站永远直接抓，别问」。 |
| **domain rule** | gate 决策链上对域名的判断条件 | [`permission/path_rule.rs`](../../../src/core/permission/path_rule.rs) 同框架的 domain 子集 | 优先级：blocked > preapproved > session > allowed > 默认 | 几条规则按顺序对照，命中谁谁说了算。 |
| **redirect off-host** | 重定向到非同源/非 `www.` 的目标 | `redirect.rs::isPermittedRedirect` 返 false | 不 follow，返回结构化 `RedirectInfo { original_url, redirect_url, status_code }` 给模型 | 跨域跳转就停下来，让模型自己拿新 url 重发。 |
| **二进制持久化** | 把非文本响应落盘到 `tool-results/` 而不是塞进上下文 | `web_fetch/persist.rs` + `resolve_agent_trail_dir` | content-type 非 text/* / json / xml → 落盘；返回 `persisted_output_path` 而不是 base64 | PDF 等存到文件里，给路径就行。 |
| **`cache key`** | 缓存命中判定键 | `(canonical_url, format)` 元组 hash | TTL=15 min；total_weight 上限 50 MB | 「同一个 url + format 短时间内不重抓」。 |
| **SSRF（Server-Side Request Forgery）** | 模型让工具去连内网 / loopback 的攻击形态 | `web_fetch/validate.rs::validate_url` | 在 validate 阶段拒（loopback / 私网 / 单段 hostname / 凭证）；与 [`web_search.md`](web_search.md) 的归一化阶段拒共同形成纵深 | 别让 fetch 拨内网。 |
| **`warnings`** | 一组「不致命但模型应当知道」的标签 | `WebFetchOutput.warnings: Vec<String>` | 重定向 / 截断 / 限速 / 落盘 / domain 审批形态 都进 | 「这次 fetch 发生了点小事」。 |

**「LLM 收到 tool 结果后」**：指 **`tool_exec` 已把 `WebFetchOutput` 序列化为 tool 消息文本（JSON）**、写入会话历史、**即将进入下一轮模型推理之前**。

---

## 4. 协议（入参 / 出参 / Schema）

**单一事实源**：

- JSON Schema（模型可见）：[`catalog.rs::web_fetch_parameters`](../../../src/core/tools/contract/catalog.rs)（PR-WF-A 添加）→ [`docs/tool-catalog.md`](../../tool-catalog.md) 派生。
- Rust 类型：`core/tools/web_fetch/types.rs`（PR-WF-S 新增）的 `WebFetchArgs` / `WebFetchOutput` / `RedirectInfo`。

### 4.1 入参（工具 arguments）

| 字段 | JSON 类型 | 必填 | 默认 | 说明 | 说人话 |
|------|-----------|------|------|------|--------|
| `url` | string (URL) | **是** | — | 目标 URL；scheme 必须 `http`/`https`；长度 ≤2000；禁止内嵌 `username:password@`；host 必须有 `.` 且**不能是任意 IP literal（私网 / 公网均拒）** | 一个干净的网址。 |
| `prompt` | string \| null | 否 | null | 「想从这个页面提取什么」；MVP 仅作回执文本提示；PR-WF-P 接 small model 后用于摘要 | 想要什么写什么；本期不影响输出。 |
| `format` | enum `markdown` \| `text` | 否 | `markdown` | 输出形态；`text` 走 html→text 简化（无 markdown 语法）；非 HTML 内容（PDF / image）按二进制持久化路径走，本字段无效 | 多数情况留默认 markdown 就行。 |

**`prompt` 三态语义**：

- 缺省 / 显式 `null`：MVP 直接返回 markdown（小页 inline / 大页 head + 落盘路径）；
- 显式非空字符串（MVP）：进 `warnings` 末尾，返回的 markdown 体积分流行为不变；
- 显式非空字符串（PR-WF-P 后）：调 small model 摘要替换 `result`。

### 4.2 出参（Rust：`WebFetchOutput`）

| 字段 | 类型 | 说明 | 说人话 |
|------|------|------|--------|
| `url` | `String` | 最终 landed URL（经允许的同源/`www.` 重定向后） | 实际抓的是哪个。 |
| `code` | `u16` | HTTP 状态码（最后一跳） | 网页给的状态码。 |
| `code_text` | `String` | 状态码文本（如 `OK`、`Not Found`） | 易读版状态。 |
| `content_type` | `String` | 响应 `Content-Type` 头（含 charset） | MIME。 |
| `bytes` | `u64` | 响应总字节数（截断前；若 oversize 则为 `MAX_HTTP_CONTENT_LENGTH`） | 多大。 |
| `result` | `String` | markdown / text 内容（`≤MAX_MARKDOWN_LENGTH` 时为整篇；超阈值时为前 `MARKDOWN_HEAD_CHARS` 的 head 预览 + 续读提示句）；二进制时为空串 | 主菜：网页文字；太长时只给开头。 |
| `total_chars` | `u64` | markdown/text 正文的**完整**字符数（未截断前；inline 时等于 `result` 字符数，落盘时等于盘上文件字符数） | 全文有多少字，模型据此决定要不要续读。 |
| `duration_ms` | `u64` | 抓取总耗时（含重定向循环） | 多久。 |
| `cached` | `bool` | 是否缓存命中 | 是不是没真发 HTTP。 |
| `persisted_output_path` | `Option<String>` | 落盘绝对路径：二进制（`.pdf`/`.png`/…）或**超大正文**（`.md`）时填，否则 null | 全文/二进制存哪了，去 read/search_files 取。 |
| `redirect` | `Option<RedirectInfo>` | 当 off-host 重定向时填，`result` 为空 | 跨域跳转；模型应该自己改用新 url 重发。 |
| `truncated` | `bool` | **HTTP body 是否被字节级截断**（>`MAX_HTTP_CONTENT_LENGTH` 只读了前 10 MiB，此时正文有损）；markdown 超阈值落盘**不算** truncated（全文在盘上，无损） | 是否在网络层就没拿全。 |
| `warnings` | `Vec<String>` | 标签列表（见 §3） | 有啥小事故。 |

**`RedirectInfo` 子结构**：

```text
RedirectInfo {
  original_url: String  // 输入 url
  redirect_url: String  // 拒 follow 的目标
  status_code:  u16     // 301 / 302 / 307 / 308
}
```

### 4.3 调用样例（jsonc）

**最简抓取（HTML 页）**：

```jsonc
{
  "url": "https://openai.com/index/introducing-gpt-5-5/"
}
```

**带 prompt（MVP 仅作提示）**：

```jsonc
{
  "url": "https://openai.com/index/introducing-gpt-5-5/",
  "prompt": "list new features and pricing changes",
  "format": "markdown"
}
```

**典型出参（HTML 路径）**：

```jsonc
{
  "url": "https://openai.com/index/introducing-gpt-5-5/",
  "code": 200,
  "code_text": "OK",
  "content_type": "text/html; charset=utf-8",
  "bytes": 184321,
  "result": "# Introducing GPT-5.5\n\nGPT-5.5 brings ...",
  "duration_ms": 1287,
  "cached": false,
  "persisted_output_path": null,
  "redirect": null,
  "truncated": false,
  "warnings": ["domain:openai.com (preapproved)"]
}
```

**典型出参（off-host 重定向）**：

```jsonc
{
  "url": "https://example.com/old-path",
  "code": 301,
  "code_text": "Moved Permanently",
  "content_type": "",
  "bytes": 0,
  "result": "",
  "duration_ms": 312,
  "cached": false,
  "persisted_output_path": null,
  "redirect": {
    "original_url": "https://example.com/old-path",
    "redirect_url": "https://newsite.org/new-path",
    "status_code": 301
  },
  "truncated": false,
  "warnings": ["redirect_off_host"]
}
```

**典型出参（PDF 落盘）**：

```jsonc
{
  "url": "https://arxiv.org/pdf/2401.12345.pdf",
  "code": 200,
  "code_text": "OK",
  "content_type": "application/pdf",
  "bytes": 2_413_512,
  "result": "",
  "duration_ms": 1_902,
  "cached": false,
  "persisted_output_path": "/Users/me/.tomcat/agents/abc-123/tool-results/web-fetch-9f3a2b.pdf",
  "redirect": null,
  "truncated": false,
  "warnings": ["binary_persisted"]
}
```

---

## 5. One-Glance Map（文件职责总览）

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/llm/system_prompt.rs                                             │
│  • 本期不单独新增 web_fetch 描述；工具说明由 catalog.description 承载        │
└────────────────────────────────────────────────────────────────────────────┘
        │
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/tools/contract/catalog.rs                                        │
│  • BUILTIN_TOOL_CATALOG: name = "web_fetch"                                 │
│  • web_fetch_parameters(): url + prompt? + format? JSON Schema             │
└────────────────────────────────────────────────────────────────────────────┘
        │
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/agent_loop/tool_exec/  (目录模块)                                 │
│  • mod.rs::execute_tool_tuple_full → match "web_fetch"                       │
│  • branches/web_fetch.rs::handle_web_fetch → executor.web_fetch.fetch()      │
│  • 序列化 WebFetchOutput 为 JSON 字符串作为 tool 消息文本                    │
└───────────────────────────────┬────────────────────────────────────────────┘
                                │
                                ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/tools/web_fetch/                                                  │
│  ├ mod.rs            • fetch(args) 入口；调 validate → gate → fetcher        │
│  ├ types.rs          • WebFetchArgs / WebFetchOutput / RedirectInfo         │
│  ├ validate.rs       • validate_url: 长度 / 凭证 / scheme / 任意 IP literal / 单段 host│
│  ├ redirect.rs       • isPermittedRedirect + redirect 循环（≤10 跳）        │
│  ├ fetcher.rs        • reqwest GET 主流程 + content-type 路由                │
│  ├ markdownify.rs    • html2md + 后处理（剥 script/style/nav）               │
│  ├ cache.rs          • Moka LRU + TTL；50 MB / 15 min                       │
│  └ persist.rs        • 二进制落盘到 tool-results/，复用 resolve_agent_trail_dir│
└───────────────────────────────┬────────────────────────────────────────────┘
                                │
                ┌───────────────┴──────────────────┐
                ▼                                  ▼
┌──────────────────────────────────┐  ┌──────────────────────────────────────┐
│  src/core/permission/             │  │  src/infra/audit/mod.rs              │
│  ├ types.rs                       │  │  • PrimitiveAuditEntry               │
│  │  └ PermissionScope::Domain     │  │      .permission_scope = "domain"    │
│  │     （PR-WF-D 新增第六值）      │  │  • record_primitive 接 web_fetch    │
│  ├ gate.rs                         │  └──────────────────────────────────────┘
│  │  └ trait PermissionGate         │
│  │      ├ check(op, path)          │   ← 现有
│  │      ├ check_bash(cmd)          │   ← 现有
│  │      └ check_domain(host)       │   ← PR-WF-D 新增
│  └ session_grants.rs               │
│     └ session 内域名持久            │
└──────────────────────────────────┘
                │
                ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/infra/config/types/tools.rs                                           │
│  • ToolsConfig 加 ToolsWebFetchConfig                                       │
│    { allowed_domains, blocked_domains, preapproved_hosts,                  │
│      max_redirects, fetch_timeout_ms, max_http_content_bytes,              │
│      max_markdown_chars, cache_ttl_secs, cache_capacity_bytes,             │
│      use_llm_processing }                                                   │
└────────────────────────────────────────────────────────────────────────────┘

  + tests:
    src/core/tools/web_fetch/tests/                  (validate / redirect / markdownify / persist / fetcher mock 服务器)
    tests/web_fetch_tool_tests.rs                    (public roundtrip + env-gated live smoke)
    src/core/permission/tests/                       (Domain op gate 单元，PR-WF-D 后置)
    E2E-WEB-FETCH-001                                (真 https://example.com/, PI_LIVE_WEB_FETCH=1)
```

**阅读顺序（说人话）**：模型先看到 **catalog** 里 `web_fetch` 与 3 字段；调起后 **`tool_exec`** 把 args 解出来；**`web_fetch/mod`** 先调 `validate` 过 URL 闸（长度 / 凭证 / scheme / 私网 / 单段 host / secret-prefix warning），命中缓存则早返；未命中时 **`fetcher`** 起 reqwest GET、`redirect` 模块按同源策略循环跳转、**`markdownify`** 或 **`persist`** 按 content-type 分流；最后 `tool_exec` 把 `WebFetchOutput` 序列化回 LLM。**`PermissionGate::Domain` / `check_domain` 仍后置到 PR-WF-D，本批次尚未接入运行时链路。**

---

## 6. 调度时序（运行时图）

主时序图（合并重定向 + content-type 路由 + persist）：

```text
LLM        tool_exec      web_fetch/mod   validate   gate      fetcher    redirect    network    markdownify/persist    cache
 │             │                │            │         │         │          │          │              │                  │
 │ web_fetch   │                │            │         │         │          │          │              │                  │
 │────────────▶│ parse args     │            │         │         │          │          │              │                  │
 │             │───────────────▶│ validate(url) │       │         │          │          │              │                  │
 │             │                │────────────▶│         │         │          │          │              │                  │
 │             │                │◀────────────│ ok      │         │          │          │              │                  │
 │             │                │  cache.lookup────────────────────────────────────────────────────────────────────────▶│
 │             │                │◀────── miss ────────────────────────────────────────────────────────────────────────────│
 │             │                │  permission_gate.check_domain(host)                                                      │
 │             │                │──────────────────────▶│         │          │          │              │                  │
 │             │                │◀──────────────────────│ Allow / NeedConfirm UI / Deny                                    │
 │             │                │  fetcher.fetch(url)                                                                       │
 │             │                │─────────────────────────────────▶│        │           │              │                  │
 │             │                │                                  │ loop ≤10           │              │                  │
 │             │                │                                  │  GET───────────────▶│              │                  │
 │             │                │                                  │  ◀──────── 3xx ────│              │                  │
 │             │                │                                  │ isPermittedRedirect │              │                  │
 │             │                │                                  │  ┌─ same-host ─────┘              │                  │
 │             │                │                                  │  │   continue                      │                  │
 │             │                │                                  │  └─ off-host ─▶ return RedirectInfo│                  │
 │             │                │                                  │  ◀──────── 200 ────│              │                  │
 │             │                │                                  │ inspect content-type              │                  │
 │             │                │                                  │  ┌─ text/* ─▶─────────────────────▶│ html2md + 分流   │
 │             │                │                                  │  │                                 │                  │
 │             │                │                                  │  └─ binary ─▶─────────────────────▶│ persist 落盘     │
 │             │                │◀─────────────────────────────────│ WebFetchOutput                    │                  │
 │             │                │  cache.put────────────────────────────────────────────────────────────────────────────▶│
 │             │◀───────────────│ Output                                                                                  │
 │◀────────────│ JSON tool 消息                                                                                            │
```

**事件 / 状态迁移发布点**：

- `permission_gate.check_domain` → 触发 `RequestUserConfirmation` 事件（与 Read/Bash 同 trait），UI 监听并拉起 ask 弹窗。
- `redirect_off_host` → 不发事件，直接通过 `WebFetchOutput.redirect` 字段告诉模型。
- `binary_persisted` → 不发事件（不像 read 的 image/pdf 注入到下一条 user message）；模型读 `persisted_output_path` 自行决定是否调 `read` 工具。

---

## 7. 状态机（域名审批）

```text
                    ┌────────────────┐
                    │  入 fetch(url) │
                    └────────┬───────┘
                             │
                             ▼
                    ┌────────────────┐
                    │ validate ok?   │
                    └────────┬───────┘
                             │ ok
                             ▼
                    ┌─────────────────────┐
                    │ gate.check_domain    │
                    └────────┬─────────────┘
                             │
       ┌─────────────────────┼─────────────────────┐
       ▼                     ▼                     ▼
   blocked              preapproved /          allowed_domains
   → Deny                session_grants          非空但未命中
                          → Allow                → Deny
                                                  ("not in allowlist")
                             │
                             ▼
                    ┌─────────────────────┐
                    │ require_confirm?     │
                    └────────┬─────────────┘
                             │ true / domain 未见过
                             ▼
                    ┌─────────────────────┐
                    │ NeedConfirm（UI 弹）  │
                    └────────┬─────────────┘
                             │
              ┌──────────────┼──────────────────────────┐
              ▼              ▼                          ▼
       AllowOnce        AllowAndPersist              Deny
       │                │                              │
       ▼                ▼                              ▼
  fetcher.fetch    session_grants.insert(host)    Err(Domain blocked)
                   fetcher.fetch
                                                       │
                                                       ▼
                                               record_primitive(failure)
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| `init` | 入 fetch + validate ok | `awaiting_gate` | — | 闸门排队。 |
| `awaiting_gate` | `blocked_domains` 命中 | `denied` | record_primitive(failure, scope=Domain, grant_type=BashRegexConfig 占位) | 配置黑名单，直接拒。 |
| `awaiting_gate` | `preapproved_hosts` 或 `session_grants` 命中 | `granted` | record_primitive(success) | 之前已经放行过；不弹 ask。 |
| `awaiting_gate` | `allowed_domains` 非空 + 命中 | `granted` | record_primitive(success) | 在白名单上。 |
| `awaiting_gate` | `allowed_domains` 非空 + 不命中 | `denied` | record_primitive(failure) | 不在白名单上，拒。 |
| `awaiting_gate` | 全无规则 + `require_confirm=true` | `awaiting_user` | UI 拉起 ask 弹窗 | 第一次见这个站，问一下。 |
| `awaiting_user` | 用户选 AllowOnce | `granted` | — | 这次过、不写 session。 |
| `awaiting_user` | 用户选 AllowAndPersist | `granted` | session_grants.insert(host) | 本会话内同 host 不再问。 |
| `awaiting_user` | 用户选 Deny / 取消 | `denied` | record_primitive(failure, trigger=UserConfirm) | 用户拒了。 |
| `granted` | fetcher 成功 | `done` | record_primitive(success) | 抓完。 |
| `granted` | fetcher off-host redirect | `done` | warnings += "redirect_off_host" | 跳出去了，让模型自己处理。 |
| `granted` | fetcher timeout / 5xx | `done` | warnings += "timeout" / "rate_limited"; truncated=true | 出小事但不抛错。 |

---

## 8. 配置与环境变量

**总则**：`env > config > 默认`。

| 来源 | 键 | 含义 | 默认 | 说人话 |
|------|-----|------|------|--------|
| `tomcat.config.toml` | `[tools.web_fetch] max_redirects` | 重定向跳数上限 | 10 | 转太多次就放弃。 |
| `tomcat.config.toml` | `[tools.web_fetch] fetch_timeout_ms` | 单次抓取墙钟超时 | 60_000 | 1 分钟还没回来就算超时。 |
| `tomcat.config.toml` | `[tools.web_fetch] max_http_content_bytes` | HTTP 响应字节上限 | 10 * 1024 * 1024 | 10 MiB 以上不再读。 |
| `tomcat.config.toml` | `[tools.web_fetch] max_markdown_chars` | markdown **inline 阈值**：正文 ≤ 此值整篇 inline，超过则落盘 + 回执 head | 100_000 | markdown 多长以内直接给。 |
| `tomcat.config.toml` | `[tools.web_fetch] markdown_head_chars` | 超阈值落盘时 `result` 的 head 预览字符数 | 2_000 | 大页只在回执里给开头多少字。 |
| `tomcat.config.toml` | `[tools.web_fetch] cache_ttl_secs` | LRU TTL | 900 (15 min) | 缓存活多久。 |
| `tomcat.config.toml` | `[tools.web_fetch] cache_capacity_bytes` | LRU total weight 上限 | 50 * 1024 * 1024 | 缓存一共占多少内存。 |
| `tomcat.config.toml` | `[tools.web_fetch] use_llm_processing` | 是否启用 PR-WF-P 摘要（**本期 false**） | false | 路线图：将来可开。 |
| `tomcat.config.toml` | `[llm] proxy` | 共享 outbound HTTP(S) 代理；当前 `web_fetch` / `web_search` 共用这条出网链路 | `None` | 需要统一走企业代理时在这里配。 |
| env | `TOMCAT__TOOLS__WEB_FETCH__MAX_HTTP_CONTENT_BYTES` 等 | 上述 cap / timeout / cache 字段的运行时覆盖 | — | 容器里临时调阈值。 |

**用户在入参里没有可覆盖的字段**——除了 `format`，其它都是阈值 / 路由类配置，模型不应能动。

> 说明：旧设计草案里提到的 `[tools.web_fetch] allowed_domains` / `blocked_domains` / `preapproved_hosts` / `require_confirm` 以及 `permission_gate.check_domain(host)` 仍属于 **PR-WF-D 后置设计**，当前批次的真实实现只有 URL 校验、同源重定向守卫、正文/落盘分流与缓存。

---

## 9. 错误模型 / 截断 / 警告

```text
                    web_fetch 请求
                            │
        ┌───────────────────┼───────────────────┐
        ▼                   ▼                   ▼
   URL 校验失败         凭证嵌入             SSRF / 私网 / 单段 host
   AppError::Tool       AppError::Tool       AppError::Tool
   ("URL too long"等)   ("creds rejected")    ("private IP rejected")
        │                   │                   │
        └───────────────────┴───────────────────┘
                            │
                            ▼
                    permission_gate.check_domain
                            │
                ┌───────────┴───────────┐
                ▼                       ▼
          domain_blocked            ask 用户拒
          AppError::Tool            AppError::Tool
          ("blocked by config")     ("user denied")
                ▼ Allow / persist
                redirect 循环（≤10 跳）
                            │
        ┌───────────────────┼───────────────────┐
        ▼                   ▼                   ▼
   redirect_off_host    redirect 循环 >10        正常 200
   Ok(redirect=Some,    AppError::Tool          继续 content-type 分流
    result="",          ("redirect loop")        │
    warnings=...)                                │
                                ┌────────────────┴────────────────┐
                                ▼                                 ▼
                          tokio::timeout                   429 / 5xx
                          truncated=true                   truncated=true
                          warnings+="timeout"              warnings+="rate_limited"
                          (Ok)                             (Ok)
                                │                                 │
                                └────────────────┬────────────────┘
                                                 │
                                content-type 路由（PR-WF-B）
                                                 │
                ┌───────────────┬─────────────────┴────────────────┐
                ▼               ▼                                  ▼
          html2md             persist                      markdown 超阈值落盘
          失败                 失败（IO/磁盘满）              warnings+="markdown_persisted"
          warnings+=          AppError::Tool                result=head 预览
          "markdownify_       ("persist failed: ...")        + persisted_output_path
          partial"                                          + total_chars (Ok)
          (Ok, 半成品 result)
```

**`tool_exec` 视角**：

- `Err(_)` → tool 消息文本为错误描述（「致命」类 6 路：URL 校验 / 凭证 / SSRF / domain_blocked / user_denied / redirect_loop / persist 失败）。
- `Ok(WebFetchOutput)` → JSON 序列化为 tool 消息文本（含可能为空的 result + warnings + 可能的 redirect 字段）。

**§1 G1–G4 的「锁死它的测试」**全部位于 §10。

---

## 10. 测试矩阵（验收）

| 维度 | 用例（实际函数名） | 状态 | 说人话 |
|------|---------------------|------|--------|
| catalog 注册 | `catalog_test::web_fetch_registered`、`submodules_test::tool_exec_web_fetch_requires_runtime_injection`、`submodules_test::tool_exec_web_fetch_routes_to_runtime` | PASS（2026-06-05） | 名字注册了、未注入 runtime 时会给显式错误，注入后成功路径会真实路由到 runtime。 |
| URL 校验 | `validate_test::{url_too_long_rejected,url_with_credentials_rejected,url_invalid_scheme_rejected,url_single_segment_host_rejected,url_private_ip_rejected,url_public_ip_literal_rejected,http_url_upgraded_to_https_before_first_get}` | PASS（2026-06-05） | 当前 URL 闸与 http→https 首跳升级都有自动化覆盖。 |
| 重定向同源 follow | `fetcher_test::{redirect_same_host_followed,redirect_apex_to_www_followed,redirect_www_to_apex_followed}`、`redirect_test::{redirect_same_host_followed,redirect_apex_to_www_followed,redirect_www_to_apex_followed}` | PASS（2026-06-05） | 同源 / `www.` 切换跟着跳；**http→https 是入参侧升级**，不是 `Location` 跨协议 follow。 |
| 重定向 off-host 拒 | `fetcher_test::redirect_off_host_returns_structured`、`redirect_test::{redirect_to_subdomain_returns_structured,redirect_cross_scheme_rejected}` | PASS（2026-06-05） | 跨域跳停下来给 `RedirectInfo`。 |
| 重定向循环 | `fetcher_test::redirect_loop_over_10_returns_err` | PASS（2026-06-05） | 转太多次报错。 |
| Markdownify | `markdownify_test::{html_with_script_style_and_nav_stripped,html_with_table_kept,markdown_format_text_mode_strips_basic_markup,json_content_is_returned_verbatim,xml_content_is_returned_verbatim_for_plus_xml_types}` | PASS（2026-06-05） | HTML 噪声节点会被剥离，文本 / JSON / XML 路由也都有回归。 |
| 正文体积分流 | `fetcher_test::{markdown_under_threshold_inlined,markdown_over_threshold_persisted_with_head}` | PASS（2026-06-05） | 小页 inline；超阈值落盘 + head 预览。 |
| Persist 二进制 | `fetcher_test::pdf_persisted_to_tool_results`、`persist_test::{pdf_persisted_to_tool_results,png_persisted_to_tool_results,markdown_text_persist_uses_requested_extension}` | PASS（2026-06-05） | PDF / PNG 落盘，Markdown 文本持久化扩展名也有覆盖。 |
| Magic 路由 | `fetcher_test::magic_overrides_content_type_when_mismatch`、`persist_test::magic_overrides_content_type_when_mismatch` | PASS（2026-06-05） | content-type 不可信时按魔数。 |
| 缓存 | `cache_test::{cache_hit_skips_http,cache_miss_after_ttl,cache_key_includes_format,cache_capacity_evicts_oldest,cache_hit_without_prompt_drops_cached_prompt_warning,redirect_output_is_not_cacheable}` | PASS（2026-06-05） | 命中、过期、容量、warning 清洗与 redirect 不缓存都有覆盖。 |
| 超时 | `fetcher_test::fetch_timeout_returns_truncated_warning` | PASS（2026-06-05） | 超时不抛错。 |
| 限速归一化 | `fetcher_test::{http_429_returns_warning_not_err,http_5xx_returns_warning_not_err}` | PASS（2026-06-05） | 429/5xx 归 warning。 |
| Permission Domain | `permission/tests/gate_test::check_domain_blocked`、`check_domain_preapproved_skips_ask`、`check_domain_session_grants_persist`、`check_domain_allowed_only`、`check_domain_unknown_returns_need_confirm` | 后置（PR-WF-D，本期不验收） | 5 条 domain rule 决策链全覆盖。 |
| Public runtime / live smoke | `tests/web_fetch_tool_tests::{public_output_roundtrip_preserves_fields,live_example_fetch_smoke}` | READY（env-gated） | 当前仓内 public API 回归以 roundtrip + `PI_LIVE_WEB_FETCH=1` live smoke 为准；离线 mock HTTP 集成主锚点仍在 `fetcher_test.rs`。 |
| 集成 catalog | `submodules_test::tool_exec_web_fetch_routes_to_runtime` | PASS（2026-06-05） | `tool_exec` 已能把 `web_fetch` 正确路由到 runtime。 |
| 配置解析 | `infra/config/tests/tools_cfg_test.rs`（`[tools.web_fetch]` 现有字段覆盖） | PASS（2026-06-05） | 当前 TOML 字段反序列化无字段丢失。 |
| 审计 scope=Domain | `infra/audit::tests::audit_includes_domain_scope_for_web_fetch` | 后置（PR-WF-D，本期不验收） | 审计字段对。 |
| E2E（live） | `E2E-WEB-FETCH-001`：真 `https://example.com/` 抓 markdown（`PI_LIVE_WEB_FETCH=1` gate；CI 默认跳） | READY（env-gated） | 上线前真跑一次；默认仓内用例不假装离线 mock 已覆盖这条公网抓取路径。 |

§1 观察指标 **G1–G4** 与本表逐行对应：G1↔URL 校验/Markdownify/集成；G2↔重定向同源 follow/off-host 拒/循环；G3↔Permission Domain/审计；G4↔Persist/Magic/集成 PDF。

**当前开发批次验收范围**：本期先验收 **G1 / G2 / G4** 与 `validate.rs` 的 URL 校验 / SSRF 守卫；**G3（Permission Domain / 审计）** 后置到 PR-WF-D。

---

## 11. 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|------------------|--------|
| **API key / token 泄漏到 transcript** | 凭证外泄、账号被盗用 | URL 校验阶段：URL 含 `username:password@` → `Err(Tool("URL with credentials rejected"))`；query string 含 `sk-` / `ghp_` / `Bearer ` 前缀 → `warnings += "secret_prefix_in_url"`（沿用 [hermes `url_safety.py`](../../../../hermes-agent/tools/url_safety.py) 的 secret-prefix scan 规则）；error 文案做 redaction（`Bearer xxx` → `<redacted>`） | 永远不让 key 进模型上下文。 |
| **SSRF（IP literal / loopback / 私网 / 单段 host）** | 内网信息泄漏、绕过网络隔离 | `validate.rs::validate_url` 拒 **任意 IP literal**：`127.0.0.0/8` / `10.0.0.0/8` / `172.16.0.0/12` / `192.168.0.0/16` / `::1` / `fc00::/7` 返回私网 / loopback 专用错误，其它公网 IP literal 亦拒；单段 hostname 同样拒绝——**tomcat 在客户端比 cc-fork 多一道**（cc-fork [`validateURL`](../../../../cc-fork-01/src/tools/WebFetchTool/utils.ts) **无** IP literal 拒，另依赖 Anthropic `domain_info` 预检）；redirect 循环里**每跳**重新 validate（防 redirect 到 `http://10.0.0.1`）；与 [`web_search.md`](web_search.md) 的 SSRF 守卫共同形成纵深 | 别让 fetch 直接拨 IP 或内网；redirect 也得每跳查。 |
| **DNS 解析型 SSRF 残留（域名 → 私网 IP / rebinding）** | 仍可能借“看起来合法”的域名落到内网 | 本期 `validate.rs` 只挡单段 host / 任意 IP literal，**不**校验 DNS 解析结果；因此 `safe.example` 若解析到私网 IP，当前主链不会在 validate 阶段拦截。该缺口已在本文显式登记，真正的 host 级防线留待 **PR-WF-D** 的 `PermissionGate::Domain` / `check_domain`；如需更深一层拦截，再单独补 DNS policy | 眼下能拦“直接写 IP”，还拦不住“域名解析后落到内网”。 |
| **限速 / 服务器 5xx** | 整轮 tool 失败、模型 retry 风暴 | 429 / 5xx → `truncated=true + warnings += "rate_limited (status=<x>)"`，**不抛 `Err`**；result 可能为空，模型自行决定下一步 | 限速了别整轮塌；告诉模型「这次没拿到」。 |
| **上下文 cost 超限** | 单次 fetch 灌入超大正文占满上下文 | 正文 > `MAX_MARKDOWN_LENGTH=100_000` 字符 → **整篇落盘**（`tool-results/*.md`）+ `result` 仅给 `MARKDOWN_HEAD_CHARS=2_000` head + `total_chars` + `warnings += "markdown_persisted"`，模型按需 `search_files`/`read` 续读（无损、不撑上下文）；HTTP body `MAX_HTTP_CONTENT_LENGTH=10 MiB` 字节级截断 + `truncated=true` + warning；二进制不进上下文（PR-WF-B 落盘） | 超大 markdown 落盘给路径 + 开头，不丢内容也不灌爆；二进制干脆不进。 |
| **redirect open-redirect 风险** | 模型按工具返回的 url 自动重发，若全 follow 会被钓鱼站劫持到外站 | `reqwest::redirect::Policy::none()` + 自定义 `isPermittedRedirect`：仅同源 / `www.` 切换 follow；其它返结构化 `RedirectInfo` 让模型显式重发；参考 [cc-fork-01 `utils.ts:248-260`](../../../../cc-fork-01/src/tools/WebFetchTool/utils.ts) 的 PSR 论证 | 跨域跳转停下来问模型，不要默默跟着跳。 |
| **fetch cache 内存爆炸** | 50 MB 上限若不严格执行会撑爆进程 | MokaCache `weigher` 按 `result.len() + persisted_output_path.len()` 估算；TTL=15 min 强制过期；进程退出释放；**不**持久化到磁盘（避免审计盲点） | 缓存有上限 + 超时清理 + 进程退出归零。 |
| **二进制泄漏到模型上下文** | base64 编码后 1 MB 二进制 → 4 MiB token，瞬间炸 | content-type 非文本类一律走 persist，**回执仅给路径**，不把 base64 塞进 tool result；与 [cc-fork-01 `WebFetchTool.ts:283-285`](../../../../cc-fork-01/src/tools/WebFetchTool/WebFetchTool.ts) 同档；与 [`read.md` PR-RJ T3-c](read.md) 的多模态注入策略**不同**——fetch 不自动注入到下一条 user message Parts，让模型显式决定是否调 `read` 工具读出来 | PDF 等只给路径；模型想看自己 read。 |

---

## 12. 历史决策（已被本方案取代或待定）

- ~~合并 `web` 工具单 schema：参数二选一（`query` 或 `url`）~~ → **否**：schema 双口（必填字段不同）会让模型频繁参数错；权限粒度（fetch 按 domain）对不上；缓存键不同（query vs url）。**`web_fetch` 与 `web_search` 拆两个工具**（与 cc/hermes/openclaw 三家一致）。
- ~~`web_search` 与 `web_fetch` 写在同一份 `web.md` 文档~~ → **否**：长文双口吻冲突（[ARCHITECTURE_SPEC §14 No-Stale](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)）；与 read/write/edit/bash/search_files **一文件一工具**习惯不一致。**拆为两份独立满额文档**（[`web_search.md`](web_search.md) + 本文）；共享术语与风险在两篇各自完整书写；**各文仍须具备完整 §4.1 / §4.2**，**不**合并为单稿后省略。
- ~~沿用 `PermissionScope::Read` 维度做 fetch 域名权限~~ → **否**：fetch 是「访问外网」语义，Read 是「读本地文件」语义，复用会让权限规则纠缠；session_grants 表的 grant 类型也对不上。**新增 `PermissionScope::Domain` 第六值**，并行于 Read/Write/Bash/BashApproval/Forbidden。
- ~~默认 `reqwest::redirect::Policy::limited(10)` follow-all~~ → **否**：reqwest 默认 policy 不区分同源跳转，open-redirect 风险大；改 **`Policy::none()`** + 自家 `isPermittedRedirect` 循环，仅同源 / `www.` 切换 follow，跨域返 `redirect_off_host`。
- ~~PDF / 图片直接 base64 inline 进 tool result~~ → **否**：1 MB PDF → 4 MiB token；与 [`read.md` PR-RJ T3-c](read.md) 的多模态注入策略也不同（fetch 是从外部抓的，多了一道间接审批）。**改为落盘 + `persisted_output_path`**，模型自行决定是否调 `read` 读出来。
- ~~引入 SaaS 抓取（Firecrawl）作为可选 backend~~ → **否**：与 pi「自托管 + 离线友好」目标冲突；hermes 的 Firecrawl 路在本期不引入；未来若有需要单独 PR + ADR。
- ~~`prompt` 字段 MVP 即接 small model 摘要~~ → **否**：double model 注册 / token 计费策略不在 web_fetch 主路径上；MVP 仅作回执提示，PR-WF-P 后再接。

**跨文档修订**：

- 本文新增的 `PermissionScope::Domain` 触及 [`permission-system.md`](../permission-system.md)（权限总览文档）——目前为「五值（Read/Write/Bash/BashApproval/Forbidden）」叙事；**修订意图**：合入 PR-WF-D 时同步修订 `permission-system.md` 的 scope 列表为「六值」，并新增 `Domain` 段说明其触发点（仅 web_fetch）、决策链、与 path_rule 的差异。**本期不在 PR-WF-D 内改 `permission-system.md` 正文**，只在文档中登记修订意图。
- 本文新增的 catalog 条目 `web_fetch` 触及 [`docs/tool-catalog.md`](../../tool-catalog.md)（派生文档，由 `build_function_definitions()` 自动生成）；不需手动改。
- 本文不修改 [`read.md`](read.md) / [`write.md`](write.md) / [`edit.md`](edit.md) / [`bash.md`](bash.md) / [`search_files.md`](search_files.md) 已冻结正文。

---

## 13. 关联文档

- 兄弟工具：[`web_search.md`](web_search.md) · [`read.md`](read.md) · [`bash.md`](bash.md) · [`search_files.md`](search_files.md) · [`write.md`](write.md) · [`edit.md`](edit.md)
- 权限总览：[`../permission-system.md`](../permission-system.md)（本文新增 `PermissionScope::Domain`，§12 已登记修订意图）
- 看板叙事：[`docs/agents/TASK_BOARD_002/README.md`](../../agents/TASK_BOARD_002/README.md)、[`T2-P1-013.md`](../../agents/TASK_BOARD_002/tasks/T2-P1-013.md)
- 五仓对比：[`agent-tools-comparison.md`](../../reports/agent-tools-comparison.md)
- Cursor 内置工具参考：[`cursor-builtin-tools-reference.md`](../../reports/cursor-builtin-tools-reference.md)
- 派生工具目录：[`tool-catalog.md`](../../tool-catalog.md)
- 规范：[`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)

---

**一句话总结**：`web_fetch` 在 **`tool_exec`** 解参数与序列化、在 **`web_fetch/mod`** 串起 **validate → cache → fetcher (含 redirect 循环) → markdownify / persist**；协议以 **`catalog.rs` + `web_fetch/types.rs`** 为单一事实源，配置走当前已落地的 `[tools.web_fetch]` 阈值字段，出网代理暂与 `[llm].proxy` 共用，重定向只跟同源 / `www.` 切换、跨域返结构化 `RedirectInfo`，二进制走 `tool-results/` 落盘**不进**上下文。**`PermissionGate::Domain` / `PermissionScope::Domain` 仍属 PR-WF-D 后置设计，当前文档不再把它写成已交付行为。**
