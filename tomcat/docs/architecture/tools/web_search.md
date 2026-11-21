# `web_search` 工具：插件后端、按序回退与结果归一化

**快速理解**：在 VS Code 打开「设置 → 工具 → 网页搜索」，打开你想用的服务、填 Key，再用上下箭头调整顺序。只开一家，就是指定该服务；开多家，才会在失败时按顺序尝试下一家。首次启动不需要手动运行 `tomcat init`。

本文只描述现行实现；与代码不一致处以 `src/` 为准。写作约定见 [`ARCHITECTURE_SPEC.md`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)（B 类）。

---

## 目录

- [0. 当前实现：启动、配置与搜索](#0-当前实现启动配置与搜索)
- [1. 目标与非目标](#1-目标与非目标)
- [2. 选型决策](#2-选型决策)
- [3. 术语](#3-术语)
- [4. 协议](#4-协议)
- [5. One-Glance Map（文件职责总览）](#5-one-glance-map文件职责总览)
- [6. 调度时序](#6-调度时序)
- [7. 状态机（逐项尝试）](#7-状态机逐项尝试)
- [8. 配置与凭据](#8-配置与凭据)
- [9. 错误、截断与警告](#9-错误截断与警告)
- [10. 测试矩阵](#10-测试矩阵)
- [11. 风险与应对](#11-风险与应对)
- [12. 已放弃的方案](#12-已放弃的方案)
- [13. 关联文档](#13-关联文档)

---

## 0. 当前实现：启动、配置与搜索

```text
首次启动 CLI / Serve（无需 init）
  └─ 安装内置搜索插件：plugin.json + main.js + README.md

插件声明 ──→ 设置页显示服务、Key 状态、开关、尝试顺序
                                     │ 保存有序名单
                                     ▼
web_search ──→ 预检每一项（Key / 模型 / 地址冲突）──→ 查缓存
                                     │ 未命中
                                     ▼
              按顺序调用插件：返回 { hits, warnings } 或抛错
                                     │
              第一个正常响应（含空结果）──→ 过滤、归一化、缓存
```

- **首启即用**：启动会补齐内置搜索插件文件。即使用户已有其他插件的登记簿，内置插件也默认可见；登记簿明确禁用时仍不加载。
- **谁能收到搜索词**：只有名单里的服务。单项就是指定；多项才会顺序回退。未保存名单（包括旧 `backend = "auto"`）时使用默认顺序：DeepSeek → Tavily → Brave → Serper → MiMo → OpenAI。已有名单不追加后端，旧的具体名称映射为单项名单。
- **谁实现服务调用**：生效插件的 `web_search.backend` 函数。声明写在 `functions[].metadata.backends`；REST 后端用 `pi.fetch`，模型后端使用宿主传入的模型。当前聊天模型不决定搜索目标。
- **Key 和顺序各管一件事**：保存 Key 不会启用后端。托管层和 Agent 层插件可使用已保存的 Key；项目层插件只能用 CLI 的显式环境变量；共享 Serve 不从继承环境找 Key。`{{secret:NAME}}` 只在请求头的值里展开，搜索词所在的请求体原样发送。
- **地址冲突预检**：每个 Key 型后端声明了接收 Key 的 HTTPS 主机。若某个模型用同一个 Key 名、却指向别的地址（或没有地址），宿主不把这把 Key 交给该后端，设置页提示去「模型」给那个模型换一个 Key 名。内置模型都指向官方地址，默认不会冲突。
- **OpenAI**：插件用 `gpt-6.2-sol` 直接调用官方 Responses，Key 固定为 `OPENAI_API_KEY`；不经模型网关，也不认模型的 `capabilities.web_search`。
- **缓存**：键是请求参数加上「生效顺序及每项预检是否通过」。补上或删除 Key、改顺序、出现地址冲突都会换键；只换 Key 的值不换键，旧结果在 TTL（默认 5 分钟）内仍可命中。
- **实现入口**：[`builtin.rs`](../../../src/ext/plugin/builtin.rs)、[`declarations.rs`](../../../src/core/tools/web_search/declarations.rs)、[`mod.rs`](../../../src/core/tools/web_search/mod.rs)、[随包插件说明](../../../assets/plugins/web-search-backends/README.md)。

---

## 1. 目标与非目标

**一句话**：让模型用一个 `query` 拿到一组结构统一、可审计、可缓存的网页结果；搜索词只发给用户在设置页启用的服务。正文抓取走 [`web_fetch.md`](web_fetch.md)。

| 目标 | 观察指标 | 说人话 |
|------|----------|--------|
| G1 query→hits 闭环 | catalog 注册 `web_search`；任一后端返回同一形状的 `hits[]`（`title/url/snippet/position`，可选 `published_at`） | 换哪家，模型读到的字段都一样。 |
| G2 按序回退 | 多项名单逐项尝试，预检失败或调用失败都换下一项；单项失败直接报该项的错 | 开几家试几家，只开一家就是指定。 |
| G3 凭据不外流 | Key 只进请求头；同名 Key 被别的地址使用时不发出；项目层插件不碰已保存的 Key | Key 只去它该去的地方。 |
| G4 缓存 | 进程内 LRU + TTL（默认 5 分钟 / 50 条）；命中时 `stats.cached=true`，不调用插件 | 短时间重复搜索不重复花钱。 |
| G5 SSRF 与域名过滤 | 归一化阶段丢弃 IP literal、内网主机名和不合规域名 | 搜出来的链接不指向内网。 |
| G6 上下文成本 | `count` 默认 5、上限 20；单条摘要 4 KiB、总量 60 000 字符软上限 | 一次搜索不撑爆上下文。 |

| 非目标 | 去向 | 说人话 |
|--------|------|--------|
| 抓取单个 URL 的正文 | [`web_fetch.md`](web_fetch.md) | 检索是找路标，抓正文是另一个工具。 |
| 宿主内置 HTTP 适配器 | 插件 `src/backends/*.js` | 新增供应商改插件，不改 Rust。 |
| 按对话模型自动选搜索服务 | 不做 | 用户看不出搜索词发给了谁。 |
| LLM 二次摘要 hits | system prompt 引导 | 模型自己读结果。 |

---

## 2. 选型决策

调研对象见 [agent-tools-comparison.md](../../reports/agent-tools-comparison.md)（cc-fork-01、hermes-agent、openclaw、pi-mono、pi_agent_rust）。

| 维度 | 决策 | 理由 | 未采用 | 说人话 |
|------|------|------|--------|--------|
| 工具拆分 | `web_search` / `web_fetch` 双工具双文档 | schema、权限和缓存键都不同 | 一个 `web` 工具二选一参数 | 找路标和抓正文分开。 |
| 后端归属 | 官方插件 `tomcat.web-search-backends` 拥有全部后端，宿主只管顺序、预检、超时、归一化和缓存 | 加供应商只改插件；宿主不感知厂商 | Rust 内置 Tavily/Brave/Serper 适配器 | 宿主是调度员，插件是干活的。 |
| 选择方式 | 设置页保存的有序名单；未保存用默认顺序 | 用户看得见、能排序 | 插件内 `autoOrder`；按当前模型推断 | 你排的顺序就是搜索顺序。 |
| 插件错误协议 | 成功返回 `{ hits, warnings }`，失败就抛错；宿主不区分失败种类，只区分超时 | 回退规则只看「单项还是多项」，不需要错误分类 | `unsupported_backend` 标记、`__missing_key__` 等哨兵警告、按 HTTP 状态分类 | 插件说成功或失败就够了。 |
| 凭据绑定 | Key 型后端声明固定 `secret` 和 `host`；宿主统一做地址冲突预检 | 所有后端同一条规则，`pi.fetch` 不需要知道模型目录 | `{{host_key}}` 按目的主机反查模型目录 | Key 名写死，冲突在发请求前就拦下。 |
| 缓存键 | 请求参数 + 生效顺序 + 每项预检结果 | Key 增删、冲突、改顺序自动换键；不需要代次计数器 | 键里带插件、模型目录、凭据代次 | 能不能用变了才换键。 |
| 输出 | 统一 `WebSearchOutput` | 模型只学一套字段 | 透传上游 JSON | 一套字段走天下。 |

---

## 3. 术语

| 术语 | 含义 | 载体 | 说人话 |
|------|------|------|--------|
| **后端（backend）** | 插件声明的一个搜索服务，以 `id` 标识 | `BackendDeclaration` | DeepSeek、Tavily 这些。 |
| **有序名单** | 用户保存的后端 `id` 列表；未保存时取声明里 `defaultAuto=true` 的项 | `tools.web_search.backends` | 尝试顺序。 |
| **预检** | 调插件前检查后端是否声明、模型是否存在、Key 是否可用、是否有地址冲突 | `declarations.rs::key_status`、`mod.rs::target_for` | 不满足条件的不打扰插件。 |
| **地址冲突** | Key 型后端的 `secret` 也被某个模型使用，而该模型的地址不是声明的 `host` | `declarations.rs::used_at_other_host` | 同一把 Key 不能发给两个地方。 |
| **生效插件** | 同一 `point` 下优先级最高的插件函数：项目层 > Agent 层 > 托管层，同层先发现者胜 | `PluginCatalog::host_functions` | 只认一个赢家，不跨插件兜底。 |
| **`warnings`** | 不致命但模型应知道的标签 | `WebSearchOutput.warnings` | 回退、截断、过滤都记在这。 |
| **SSRF 过滤** | 归一化时丢弃指向 IP literal、内网或保留主机名的结果 | `types.rs::normalize_hits` | 别让结果指内网。 |

---

## 4. 协议

**单一事实源**：模型可见 schema 在 [`catalog.rs::web_search_parameters`](../../../src/core/tools/contract/catalog.rs)，派生到 [`tool-catalog.md`](../../tool-catalog.md)；Rust 类型在 [`types.rs`](../../../src/core/tools/web_search/types.rs)；插件契约在 [`plugin_backend.rs`](../../../src/core/tools/web_search/plugin_backend.rs)。

### 4.1 入参（工具 arguments）

| 字段 | 类型 | 必填 | 默认 | 说明 | 说人话 |
|------|------|------|------|------|--------|
| `query` | string | **是** | — | 非空，≤512 字符 | 想搜啥就写啥。 |
| `count` | integer | 否 | 5 | 1..=20 | 默认 5 条。 |
| `freshness` | `day` \| `week` \| `month` \| `year` \| null | 否 | null | 时间范围，各后端自行映射 | 要新的就传 day / week。 |
| `country` | string | 否 | null | ISO 3166-1 alpha-2 | 限制国家。 |
| `language` | string | 否 | null | ISO 639-1 | 限制语言。 |
| `domain_filter` | string[] | 否 | `[]` | 只要这些域名（含子域），叠加到配置的 `domain_filter` 上 | 只搜这几个站。 |

搜索目标不暴露给模型，由有序名单决定。

### 4.2 出参（`WebSearchOutput`）

| 字段 | 类型 | 说明 | 说人话 |
|------|------|------|--------|
| `hits` | `Vec<Hit>` | 已过 SSRF 与域名过滤 | 结果。 |
| `query` | `String` | 回显 | 搜的是啥。 |
| `backend` | `String` | 实际给出结果的后端 `id` | 哪家给的。 |
| `stats` | `{ elapsed_ms, cached, total_before_filter? }` | 耗时与缓存命中 | 多久、是否走缓存。 |
| `truncated` | `bool` | 摘要截断、超出 `count` 或总量上限时为真 | 有没有删减。 |
| `warnings` | `Vec<String>` | 此前失败项（`backend_unavailable:<id> (<原因>)`）+ 插件警告 + 归一化警告 | 过程中的小事。 |

`Hit { title, url, snippet, position (1 起), published_at? }`；上游缺标题时用 URL 兜底，缺摘要为空串。

### 4.3 插件契约

**声明**（`plugin.json` → `functions[]`，`point = "web_search.backend"`，`function = "webSearchBackend"`）：

```jsonc
{ "id": "tavily", "label": "Tavily", "secret": "TAVILY_API_KEY", "host": "api.tavily.com", "defaultAuto": true }
{ "id": "mimo",   "label": "MiMo",   "model": "mimo/mimo-v2.6-pro", "defaultAuto": true }
```

- 每项必须是 `secret` + `host`（合法环境变量名、HTTPS 默认端口的 DNS 主机）或 `model` 二者之一；不合规或重复 `id` 的项忽略并记日志。
- `secret` 须列入 `requiredSecrets`，`host` 须列入 `allowedHosts`；内置插件由 `declarations_match_network_grants_and_catalog` 锁定。

**调用**：宿主传 `{ backend, query, count, freshness, country, language, domainFilter, model? }`，`model` 只在模型后端出现。

**返回**：`{ hits: [{ title, url, snippet?, published_at? }], warnings?: string[] }`；任何失败都 `throw new Error(message)`。宿主把 `pi.fetch` 的 `timeout` 错误码映射为 `search.timeout`，其余失败包装为 `plugin.searchFailed`（带后端、插件 ID 和原始信息）；返回体不符合形状时为 `search.runtime`。

**凭据**：REST 后端在请求头写 `Bearer {{secret:NAME}}`。占位符出现在 URL、query 或请求头名会被 `forbidden_secret` 拒绝；请求体不展开；不跟随重定向；明文 Key 不回传给插件。

---

## 5. One-Glance Map（文件职责总览）

```text
src/core/tools/contract/catalog.rs        web_search 名字与 6 字段 schema
src/core/agent_loop/tool_exec/branches/web_search.rs
                                          解参数 → WebSearchRuntime::search → 序列化结果
src/core/tools/web_search/
  ├ mod.rs            WebSearchRuntime：生效顺序、预检、缓存、逐项调用、超时、回退
  ├ declarations.rs   声明解析、默认顺序、key_status（含地址冲突预检）
  ├ plugin_backend.rs PluginSearchInvoker trait、请求载荷、返回体解析
  ├ cache.rs          Moka LRU + TTL；CacheKey
  └ types.rs          WebSearchArgs/Request/Output/Hit；normalize_hits（SSRF + 域名 + 截断）
src/ext/plugin_search_invoker.rs          取生效插件函数并执行；错误映射
src/ext/plugin/catalog.rs                 PluginCatalog::host_functions：按层级选每个 point 的赢家
src/ext/dispatcher/net.rs                 pi.fetch：allowedHosts、requiredSecrets、请求头内展开 secret
src/ext/plugin/builtin.rs                 启动时安装内置插件
src/api/serve/web_search_settings.rs      设置读取、保存顺序（Serve 命令）
assets/plugins/web-search-backends/       官方插件：src/backends/*.js → tomcat plugin build → main.js
tomcat-vscode-ext/gui/src/settings/WebSearchSettingsView.tsx
                                          设置页：开关、顺序、Key、冲突提示
```

**阅读顺序（说人话）**：模型在 catalog 里看到 `web_search` → `tool_exec` 解参数交给 `WebSearchRuntime` → 它按名单逐项预检、查缓存，再经 `ExtPluginSearchInvoker` 调生效插件 → 插件用 `pi.fetch` 或宿主模型完成搜索 → `normalize_hits` 过滤截断 → 结果序列化回模型。

---

## 6. 调度时序

```text
LLM        tool_exec      WebSearchRuntime            ExtPluginSearchInvoker     插件 / pi.fetch
 │ web_search(q)│                │                              │                       │
 │─────────────▶│ parse args     │                              │                       │
 │              │───────────────▶│ 读名单、声明、模型目录        │                       │
 │              │                │ 逐项预检 → routes             │                       │
 │              │                │ cache.get(请求 + routes)      │                       │
 │              │                │── 命中 → cached=true 返回     │                       │
 │              │                │ 未命中：对第 i 项             │                       │
 │              │                │─────────────────────────────▶│ 取生效函数并执行      │
 │              │                │                              │──────────────────────▶│
 │              │                │                              │◀── {hits,warnings}/抛错│
 │              │                │◀─────────────────────────────│ 错误映射              │
 │              │                │ 成功 → normalize_hits → 缓存 → 返回                  │
 │              │                │ 失败 → 单项直接返回错误；多项记 warning 换下一项       │
 │◀─────────────│ JSON tool 消息 │                              │                       │
```

---

## 7. 状态机（逐项尝试）

实现见 [`mod.rs::search`](../../../src/core/tools/web_search/mod.rs)。

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| 第 i 项 | 预检失败（未声明、缺模型、缺 Key、地址冲突） | 下一项 | 不调用插件；记录原因 | 没配好的直接跳过。 |
| 第 i 项 | 插件正常返回（含空结果） | 结束 | 返回结果，`warnings` 前置此前失败原因；有结果或未截断时写缓存 | 第一个正常回来的就用它。 |
| 第 i 项 | 插件抛错或超时 | 下一项 | `warnings += "backend_unavailable:<id> (<原因>)"` | 一家不通试下一家。 |
| 名单只有一项 | 任意失败 | 失败 | 直接返回该项的错误原文 | 只选一家就不偷偷换。 |
| 全部失败 | — | 失败 | `search.allUnavailableDetails` 列出每项原因；不缓存 | 都试过了，告诉模型没搜成。 |

---

## 8. 配置与凭据

| 来源 | 键 | 含义 | 说人话 |
|------|-----|------|--------|
| `tomcat.config.toml` | `[tools.web_search] backends` | 有序名单，由设置页写入 | 尝试顺序。 |
| `tomcat.config.toml` | `[tools.web_search] backend` | 旧键，仅在没有 `backends` 时生效：`auto` = 默认顺序，具体名称 = 单项名单 | 老配置照样能用。 |
| `tomcat.config.toml` | `count` / `freshness` / `country` / `language` / `domain_filter` | 入参缺省值 | 模型可逐次覆盖。 |
| `tomcat.config.toml` | `allowed_domains` / `blocked_domains` | 结果集级别过滤，与 `web_fetch` 独立 | 拒搜或只搜某些站。 |
| `tomcat.config.toml` | `cache_ttl_secs` / `cache_capacity` | 默认 300 / 50 | 缓存多久、多大。 |
| `tomcat.config.toml` | `timeout_ms` | 每项调用超时，默认 12 000 | 等多久算超时。 |
| 已保存 Key / 进程环境 | `DEEPSEEK_API_KEY` / `TAVILY_API_KEY` / `BRAVE_API_KEY` / `SERPER_API_KEY` / `OPENAI_API_KEY` | REST 后端凭据；设置页与模型共用同一私有存储 | 在设置页填一次即可。 |
| 模型目录 | `mimo/mimo-v2.6-pro` 及其 Key | MiMo 后端经宿主模型调用 | 模型配好就能搜。 |

旧的 `tavily_base_url` / `brave_base_url` / `serper_base_url` 键被忽略。

**凭据来源规则**：

- 托管层、Agent 层插件：先读已保存的 Key，再读显式进程环境变量。
- 项目层插件：只读显式进程环境变量。
- 共享 Serve：不读从启动者继承的环境变量。
- Key 型后端：若有模型用同一 Key 名却指向别的地址，该后端预检失败，提示「Give that model its own key in Settings → Models」。

---

## 9. 错误、截断与警告

| 情形 | 结果 | 说人话 |
|------|------|--------|
| 参数非法（空 query、count 越界、非法 freshness/域名） | `AppError::Tool`，不调用任何后端 | 先把参数改对。 |
| 单项名单失败 | 该项的错误原文（预检、`plugin.searchFailed`、`search.timeout`、`search.runtime`） | 原因直达模型。 |
| 多项名单全部失败 | `search.allUnavailableDetails`，逐项列原因 | 每家为什么不行都写清。 |
| 某项失败但后续成功 | 成功输出，`warnings` 带 `backend_unavailable:<id> (...)` | 换了一家，模型知道。 |
| URL 无效 / SSRF / 域名规则 | 丢弃该条，`skipped_invalid_url` / `ssrf_filtered` / `domain_blocked:<host>` / `domain_filtered:<host>` | 坏链接不进上下文。 |
| 摘要 > 4 KiB / 超出 `count` / 总量 > 60 000 字符 | 截断，`snippet_truncated` / `count_limited` / `max_result_size_chars`，`truncated=true` | 太长就删减并标明。 |

`tool_exec` 把 `Err` 作为工具错误文本，`Ok` 序列化为 JSON；两者都不会中断对话回合。

---

## 10. 测试矩阵

| 维度 | 用例 | 说人话 |
|------|------|--------|
| 注册与路由 | `web_search_registered`、`tool_exec_web_search_requires_runtime_injection`、`tool_exec_web_search_routes_to_runtime`、`reviewer_blocks_web_search_tool` | 工具在、能路由、审查子 Agent 不能用。 |
| 声明与冲突预检 | `declarations_ignore_invalid_and_duplicate_entries`、`secret_backend_is_blocked_when_a_model_at_another_host_shares_its_key`、`declarations_match_network_grants_and_catalog` | 声明合法、冲突被拦、内置声明与授权一致。 |
| 回退与错误 | `ordered_route_skips_failures_and_stops_on_empty_with_hot_updates`、`single_backend_returns_its_error_unchanged`、`auto_exhausted_returns_tool_error_and_does_not_cache`、`malformed_plugin_output_is_a_backend_failure`、`only_the_timeout_code_maps_to_a_timeout` | 多项回退、单项直报、全失败聚合。 |
| 缓存 | `cache_follows_key_availability_but_not_key_values`、`cache_key_tracks_allowed_and_blocked_domains`、`deepseek_requests_use_the_current_saved_key` | Key 增删换键，换值不换键。 |
| 凭据 | `openai_search_key_is_refused_while_a_gateway_model_shares_it`、`project_search_winner_cannot_borrow_saved_key_from_user_layer`、`rejects_placeholders_outside_header_values` | Key 只去对的地址、对的层。 |
| 插件实现 | `committed_main_matches_source_bundle`、`deepseek_request_maps_filters_and_counts_unicode_points`、`openai_bundle_sends_official_request_and_keeps_citations_first`、`tavily/brave/serper_parser_fixture_matches_expected_hits` | 打包产物与源码一致，各家映射正确。 |
| 不跨插件兜底 | `test_web_search_backend_does_not_fallback_to_shadowed_provider`、`ext_plugin_search_invoker_does_not_fall_back_to_shadowed_provider` | 赢家失败不找下层插件。 |
| 归一化 | `normalize_hits_filters_private_hosts_and_domain_rules` | SSRF 与域名过滤。 |
| 集成 / E2E | `tests/web_search_tool_tests.rs`（本地 HTTPS + 真 QuickJS）、`tests/cli_tests.rs` 中 `test_chat_path_*web_search*`、`serve_shared_e2e::web_search_settings_are_scoped_but_route_and_keys_are_shared` | 从对话和 Serve 走完整链路。 |
| 真网 smoke | `real_tavily_plugin_web_search` 等（有 Key 才跑） | 只证明今天还能通。 |

---

## 11. 风险与应对

| 风险 | 影响 | 应对 | 说人话 |
|------|------|------|--------|
| Key 进入 transcript / 日志 | 凭证外泄 | Key 只在 `pi.fetch` 请求头展开并标记敏感；schema 不含 Key 字段；明文不回传插件 | 永远不让 Key 进模型上下文。 |
| Key 被发到别的地址 | 网关 Key 被带去官方地址，或反过来 | 声明固定 `secret` + `host`；同名 Key 被别的地址使用时预检失败；`allowedHosts` 限制出口 | 一把 Key 只给一个地址。 |
| 项目插件借用用户 Key | 打开陌生仓库即泄露 Key | 项目层插件只读显式进程环境变量 | 别人的仓库拿不到你的 Key。 |
| SSRF | 模型把内网链接喂给 fetch | `normalize_hits` 丢弃 IP literal、内网和保留主机名 | 结果不指内网。 |
| 上下文成本 | 一次搜索占满上下文 | `count` ≤ 20；单条 4 KiB、总量 60 000 字符 | 先减条数再减长度。 |
| 换 Key 后命中旧结果 | 最多 TTL 内看到旧 Key 搜到的结果 | 可接受：结果与 Key 值无关；Key 增删、冲突会换键 | 结果不变，只是没重搜。 |
| OpenAI 搜索偏慢 | 12 秒超时可能不够 | 实测后只调默认值 | 先观察再调。 |

---

## 12. 已放弃的方案

- **Rust 内置 Tavily/Brave/Serper 适配器、宿主托管 OpenAI**：已删除。供应商实现全部在插件里，宿主不感知厂商。
- **插件内 `auto` / `autoOrder`**：已删除。顺序只由宿主的有序名单决定。
- **`unsupported_backend` 标记与 `__missing_key__` / `__unauthorized__` 哨兵警告**：已删除。缺 Key 由宿主预检发现；其余失败插件直接抛错。
- **`{{host_key}}`（按目的主机反查模型目录取 Key）**：已删除。它让 `pi.fetch` 依赖模型目录，且只保护 OpenAI 一家；现由固定 `secret` + 统一地址冲突预检替代。
- **缓存键带插件、模型目录、凭据代次**：已删除。改为键里带每项预检结果，无需全局计数器。
- **按对话模型或模型能力标记选搜索服务**：搜索目标会随模型配置隐式变化，用户看不出搜索词发给了谁。
- **持久化搜索缓存**：结果有时效性，进程内 LRU + TTL 足够。

---

## 13. 关联文档

- 插件协议与新增后端步骤：[`web-search-backends/README.md`](../../../assets/plugins/web-search-backends/README.md)
- `pi.fetch` 与宿主函数：[`js-bridge-and-host-api.md`](../plugin-system/js-bridge-and-host-api.md)、[`plugin-system-overview.md`](../plugin-system-overview.md)
- OpenAI Responses Web search：<https://platform.openai.com/docs/guides/tools-web-search>
- DeepSeek Search API：<https://api-docs.deepseek.com/zh-cn/guides/search>
- 兄弟工具：[`web_fetch.md`](web_fetch.md)
- 权限总览：[`../permission-system.md`](../permission-system.md)
- 五仓对比：[`agent-tools-comparison.md`](../../reports/agent-tools-comparison.md)
- 派生工具目录：[`tool-catalog.md`](../../tool-catalog.md)

---

**一句话总结**：`web_search` 由宿主按用户保存的顺序逐项预检、调用生效插件中的后端，第一个正常结果经 SSRF 与域名过滤后返回并缓存；Key 型后端只使用声明的固定 Key，且该 Key 不能同时被别的地址使用。
