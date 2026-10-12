# 连接器模块 v2 · 渐进式披露（缓存前缀失稳 + MCP token 爆炸整改）

> 适用范围：v1（[v1-connector-foundation.md](./v1-connector-foundation.md)）把 MCP 工具**像插件一样注册进 `ToolRegistry`、随 `list_tools` 进每轮前缀**（§3.1 R4「Option B」）。上线后暴露出**一根病根、两个症状**：*易变且完整的 MCP 工具目录被放进了每次都要重发的缓存前缀* → ① 中转站 prompt 缓存基本打不中；② server 越接越多、前缀越堆越大、token 爆炸。v2 改走**渐进式披露**根治之。
> 关联：总纲/导航见 [../mcp-client.md](../mcp-client.md)；v1 基座设计见 [v1-connector-foundation.md](./v1-connector-foundation.md)；研发计划 `~/.cursor/plans/mcp缓存前缀失稳整改_66bd7b9d.plan.md`（以本文为单一真相，计划只承载 todos / 交付顺序 / 测试清单）。
> 当前运行时设置与信任规则见 [MCP 配置升级与项目信任](./mcp-upgrade-project-trust.md)。本文聚焦渐进式工具披露；早期关于 v1 R7 指纹/逐服务批准及 JSON 运行键的文字属于历史设计，不能作为当前实现依据。

**一句话定位**：v2 = 「让 MCP 的完整工具目录**根本不进缓存前缀**」的整改。前缀里只常驻**三个泛化元工具 `tool_search / tool_describe / tool_call` + 一条写死的 `connectors` skill 索引**；「有哪些 MCP、各有哪些工具」这类**易变**信息，改由**元工具结果**承载并做**两级渐进式披露**（`tool_search()` 列 source → `tool_search(source=…)` 列该 source 工具 → `tool_describe` 取 schema → `tool_call` 调用，随工具结果进消息体），`connectors` skill 保持**静态、只教方法**（与 `verify` 同一物化范式，skill 子系统对连接器零认知）；扇出/大数据场景由**复用现成插件 JS VM**（无新沙箱、无新 IPC）的代码执行承担。结果：前缀恒小恒稳、O(1) 与 server 数无关 —— 缓存失效与 token 爆炸两症状一并消失（关键设计取舍见 §2.3、发现示例见 §2.2）。

---

## 文首导读：v2 方案导图集

### 阅读顺序建议

1. **A.1 抽象总图**：先看职责与事实源——前缀里常驻什么、易变的目录放哪、模型怎么「发现→描述→调用」、两处关键分叉（发现方式 / 调用方式）。
2. **A.2 具体总图**：把同一条链路落到真实对象——`McpManager` 目录、三元工具 builtin、静态 `connectors` skill（verify 物化范式）、`tool_dispatcher` 图片回流、插件 rquickjs VM。
3. **§1 病根**：为什么 v1 会踩中（异步连接让易变目录进了前缀）。
4. **§2 主方案 + §3 落地选型与实施**：怎么做、为什么这么做、每一步落在哪个文件。
5. **§4–§6**：`tool_search` 打分答疑、代码执行、兜底/测试/调研。

### A.1 抽象 ASCII 总图（职责 / 事实源 / 分叉 / 终局）

> 专业：前缀只常驻「3 个 provider 无关的元工具 + 1 条 byte-stable 的 skill 索引」；MCP 完整目录只存在 `McpManager` 侧（运行时唯一事实源），仅经元工具触达（`connectors` skill 只教方法、静态、对连接器零认知）；连接/断开只更新目录、从不改前缀。
> 说人话：把「菜单上印所有菜」改成「菜单上只印一个『查菜』按钮」——你要什么菜现查现报，厨房有什么菜（会变）不再印在那张要反复复印的菜单上。

```text
【常驻前缀·恒稳·O(1)】            【按需层·进消息体·不入前缀缓存】        【运行时唯一事实源】
────────────────────            ──────────────────────────────       ────────────────────
内置工具(如常)                    L1 tool_search()      列 source         ┌─────────────────────┐
+ tool_search                    L2 tool_search(source) 列该source工具    │  McpManager 目录     │
+ tool_describe                        ────► 工具名 + 短描述        ◄──│  BTreeMap<name,      │
+ tool_call                      描述 tool_describe(名)                  │    McpToolDef>       │
+ <available_skills> 里一条             ────► 完整 inputSchema      ◄──│  (连接/断开只改这里, │
   【写死】connectors 索引        调用 tool_call(名, 参数)                │   从不动前缀)        │
   (静态·只教方法·无活数据)             ────► 真实结果(文本/图片)   ◄──└─────────────────────┘
                                 扇出 写 JS 在插件VM里 callTool 循环            │
                                       ────► 就地聚合, 只回终值    ──────► McpManager::call_tool
                                 (关键词直达: tool_search(query) 跨/内source) (温连接 + 同一 trust)

两级发现：L1 列 source ──► L2 列该 source 工具 ──► tool_describe(取 schema) ──► tool_call  (记得名字可用 query 直达)
分叉 调用方式：单发 tool_call（几步顺序）┈或┈ 代码执行（扇出/过滤大结果/长链）
关键：活地图（连了什么/有哪些工具）只走 tool_search 结果(消息体)；connectors skill 静态只教方法 → skill 不依赖连接器
终局：无论何时连上/断开/接多少 server，前缀逐字节不变 → 缓存稳 + 前缀 token O(1)
```

### A.2 具体 ASCII 总图（真实对象 / 模块 / 运行时约束）

> 专业：下图把 A.1 落到 Tomcat 真实代码。核心逆转是——`ConnectorRegistry::spawn_connect_all` **不再接收、更不会 `register_tool(mcp__{server}__{tool})` 到 `ToolRegistry`**（那正是 v1 污染前缀的路径），只让 `McpManager` 持有运行态目录；三元工具是 **builtin**（走 `tool_exec` 分支，不经注册表），`tool_call` 以 canonical name 查目录后直连 `McpManager::call_model_tool`。
> 说人话：左边是「这一步在干嘛」，右边括号是对应代码位置。

```text
开机：连上 MCP，只更新目录（【不】登记进注册表）
   代码：ConnectorRegistry 连 server
         →【只】刷新 McpManager 目录，不再 register_tool(具体工具)  (core/connector/mod.rs · 改)
        │
        ▼
每轮前缀：只多「3 元工具 + 1 条写死 skill 索引」
   代码：builtin catalog 静态登记 tool_search/describe/call         (core/tools/contract/catalog.rs · 新增)
         connectors【静态】skill 物化(verify 范式)，索引条目写死进 <available_skills>
                                                                    (assets/skills/connectors/*, builtin.rs · 新增)
         observe_tool_surface/list_tools 【看不到】具体 mcp__server__tool
         （`tool_call` 描述中的常量 canonical-name 语法除外）       (api/chat/run_loop/mod.rs · 天然)
        │  模型：L1 tool_search() 列 source → L2 tool_search(source) 列该 source 工具（skill 只教方法，不出活数据）
        ▼
发现：tool_search 查 McpManager 目录（skill 全程不参与 → 零耦合）
   代码：tool_search 三态 → McpManager::{list_servers | list_tools(server) | search(query,server,limit,offset)}
                                                                    (manager.rs · 新增 API)
         connectors skill 走【既有】磁盘发现/加载链，不碰 McpManager     (core/skill/* · 零改动)
        │  模型：tool_describe(名) → tool_call(名, 参数)
        ▼
描述/调用：查目录 → 直连 McpManager（【不】经 ToolRegistry）
   代码：tool_describe(names[]) → McpManager::describe_many(names) → 各 inputSchema  (tool_exec/branches/* · 新增)
         tool_call → canonical name 查目录 → McpManager::call_model_tool → call_tool
                                              (温连接 + 同一 trust + per-server 串行锁)
         └─ 返回 MCP CallToolResult JSON（含 text/image 块）
        │
        ▼
图片回流：内置路径自行跑「JSON 图片块 → InputImage」
   代码：tool_call handler 跑与 extract_tool_result_media 等价逻辑，
         按结果内容 {type:image} 块判定填 follow_up_parts             (tool_dispatcher.rs 逻辑复用)
        │
        ▼
扇出（可选）：写 JS 交插件 VM，就地聚合只回终值
   代码：tool_run_code(code) → rquickjs 插件 VM；await callTool hostcall
         直连 McpManager::call_model_tool（【非】registry 路径）      (tool_exec/branches/code.rs + ext/runtime/instance.rs · 新增)
```

> 阅读顺序（说人话）：开机连 MCP 只更新目录 → 前缀只多 3 元工具 + 1 条写死 skill 索引 → 模型两级发现 `tool_search()` 列 source → `tool_search(source=…)` 列该 source 工具（`connectors` skill 静态、只教方法，不出活数据）→ `tool_describe` 取 schema → `tool_call` 直连 `McpManager` 调用（图片走内容型回流）→ 需要扇出时写 JS 交插件 VM 就地聚合。**核心改动集中在**：`connector/mod.rs`（不再注册进前缀）、`catalog.rs`（3 元工具）、`manager.rs`（目录查询 API：`list_servers`/`list_tools(server)`/`search`）、`assets/skills/connectors`（静态 skill，verify 范式，`skill/*` 零改动）、`tool_exec`（元工具 handler + 图片回流）。

---

## 1. 病根复盘：一个「易变的完整目录」进了「每次都发的前缀」

### 1.1 背景：Prompt 前缀缓存怎么工作（先讲原理，后面全靠它）

大模型服务商（Anthropic / OpenAI）会把「一段请求的**开头**」缓存起来；下一条请求如果开头**逐字节完全相同**，这段就直接命中、不再重复计费与重算。关键词是**前缀（prefix）**：从第 1 字节起逐字节比对，**一旦某字节不同，从那里往后全部作废**。

```text
一次请求在线上的拼接顺序（以 Anthropic 为例）：

  [ tools 工具数组 ] → [ system 系统提示词(尾部可带运行态) ] → [ messages 对话历史 ]
        ▲最靠前                    ▲中间                              ▲最靠后
     这里一变，后面 system + messages 的缓存【全废】

结论：越靠前的位置抖动，杀伤越大。
tools 数组在最前面 —— 所以「工具目录每次不一样、或越堆越大」既【杀缓存】又【炸 token】。
```

### 1.2 一根病根，两个症状

```text
     病根：完整且易变的 MCP 工具目录，被放进了「每次都要发的前缀」
                            │
           ┌────────────────┴────────────────┐
     症状①缓存失效                        症状②token 爆炸
 (工具集随异步连接/断开漂移，前缀反复变)   (server 越接越多，前缀越堆越大)
           └────────────────┬────────────────┘
                            ▼
   正解：让 MCP schema 根本不进前缀 —— 渐进式披露（元工具按需发现/调用）
         前缀恒小又恒稳 → 两症状一起消失；异步连接/断开也不再影响前缀
```

### 1.3 证据（真实会话）

- 现象：新开 session 让 agent 写计划（Plan 模式、claude-opus-4-8），中转站后台缓存基本打不中；`#[ignore]` 的真实 LLM E2E（gpt-5.6-terra）反而能命中。
- 会话 `[transcript](/Users/yankeben/.tomcat/agents/main/sessions/1788089860215_7672ac42cd987ce9.jsonl)`：一个 user turn + 大量工具轮次 + 一个 plan_reviewer 子代理。后台 claude 请求输入从约 22K 涨到 47K，而**缓存读取恒定在约 2.5K–5K、不随历史增长** —— 说明靠前的大前缀（tools + system）每条都在变，只有很小的公共头命中。
- E2E 能命中的原因：它在发首条请求前 `wait_for_playwright_ready`（`tomcat/tests/ui_acceptance_real_llm_e2e.rs`），工具集全程不变；真实会话不等待 → 中途翻转。

### 1.4 为什么 v1（[v1 §3.1 R4](./v1-connector-foundation.md) Option B）会踩中

- MCP 连接**刻意非阻塞**（`core/connector/mod.rs::spawn_connect_all` 只 `tokio::spawn`，各 server `connect_with_backoff` 0/250/1000ms）。
- 连上后 lifecycle coordinator 收 `Ready` → 逐个 `register_tool(mcp__{server}__{tool}, plugin_id="mcp:{server}")`；收 `NotReady` → `unregister_plugin_tools`。**per-tool 全量注册进共享 `ToolRegistry`。**
- 每轮 `observe_tool_surface → list_tools(None) → ToolSurface::from_plugin_tools` 观测工具面（同 turn 冻结、跨 turn 重刷）。于是：

```text
turn1  MCP 还在连接  → 工具集 = 仅内置                → 前缀 P0
稍后   MCP Ready      → 注册 mcp__playwright__*(约20个,各带完整 schema)
turn2  refresh        → 工具集 = 内置 + MCP           → P1 ≠ P0  → MISS
抖动   NotReady        → 卸载                          → P2 ≠ P1  → MISS
重连   Ready           → 再装回                        → P3       → MISS
```

**判定**：不是「排序/内容每次不一样」（给定同一集合，工具面逐字节稳定：`list_tools` 按 `(plugin_id,name)` 排序、`from_plugin_tools` 按 name 排序、MCP 侧 `BTreeMap`、`serde_json` 未开 `preserve_order`，已有单测 `tool_surface_is_byte_stable_when_registry_observation_order_changes`）；**而是「完整目录进了前缀、且集合随异步连接漂移」** —— 一变则后面全废，同时埋下 token 爆炸（server 越多前缀越大）。

### 1.5 动工闸门：先复现、先定位（诊断先行）

在动任何代码前，用现成的**只读**手段确认「相邻两条缓存失效请求之间，到底是 (a) tools 数组 / (b) 系统正文 / (c) 运行态尾巴 哪个在变」，作为动工与 §1.6 是否启动的依据：

- `TOMCAT_PROMPT_PREFIX_FINGERPRINT=1`：`core/agent_loop/reasoning_loop.rs` 对整个 tools 数组算 `tool_hash`，比对相邻请求是否变化。
- `phase="prompt_runtime_snapshot"` 日志（`api/chat/run_loop/mod.rs`）的 `plugin_tools=` 字段：看缓存变差那轮是否增删了 `mcp:*`。
- 复现：真实配置一个 MCP（如 playwright），冷启动后**不预热**立即连发几轮；对照「先等 ready 再发」的一组。

**闸门判定**：(a)/(b) 由 §2 主方案根治；(c) 属 Anthropic 尾巴天花板（§1.6），仅在确认后才处理。

### 1.6 另一条独立天花板：Anthropic 运行态尾巴（条件性、非本次根因）

除了「工具目录进前缀」这条主病根，Anthropic 侧还有一条**独立、早已存在**的缓存天花板：`core/llm/anthropic/wire.rs` 把 ephemeral tail（workspace 状态 / 计划提醒 / 权限态）作为 **system 后缀**追加（注释明说「a state change deliberately invalidates one request」）。缓存顺序里 system 在 messages 之前，尾巴每轮变 → messages 层断点每轮无法命中，只有尾巴前的断点能命中——与「缓存读取恒定、不随历史增长」现象吻合。

- **它不是本次回归的根因**（E2E 走 OpenAI Responses 不受影响；且早于本期 MCP 迭代就存在），但会**限制修好前缀后 claude 的历史命中上限**。
- **仅当 §1.5 诊断确认差异来自尾巴才处理**；候选：让尾巴成为整个请求最末（messages 之后），或让最深消息断点位于尾巴之前使历史可缓存。需先查证 Anthropic 对 system/messages 顺序的实际缓存语义，避免又一个未经验证的架构假设。

---

## 2. 主方案：三元工具 + 动态 connectors skill

### 2.1 目标形态（一图）

```text
常驻前缀（恒小恒稳，O(1) 与 server/工具数无关）      按需（进消息体，不入缓存前缀）
────────────────────────────────────────────      ──────────────────────────────────
内置工具（如常）                                     L1 tool_search() 结果：已连接 source 清单（name+type+简介+tool_count）
+ tool_search / tool_describe / tool_call（3 个）     L2 tool_search(source=…) 结果：该 source 工具名 + 短描述
+ <available_skills> 里一条【写死】的 connectors 索引  tool_describe([names]) 结果：多个工具完整 inputSchema（批量）
  （静态 skill，只教方法、不含活数据）                tool_call 结果：工具真实输出（含图片回流）

模型工作流（两级发现 → describe→call）：
  ① 两级浏览：tool_search() ─列source─► tool_search(source=…) ─列tool─► tool_describe(名) ──► tool_call(名,参数)
  ② 关键词直达：tool_search(query="截图") ──候选名──► tool_describe(名) ──► tool_call(名,参数)
  connectors skill（静态）只教「怎么用/何时写代码」；活地图不进 skill，走 tool_search 结果

MCP 完整目录只存在 McpManager 侧，仅经三元工具触达（skill 不依赖连接器）；
连接/断开只更新目录，从不改前缀。
```

### 2.2 三个泛化元工具：`tool_search / tool_describe / tool_call`（回应决策 1）

**为什么叫 `tool_*` 而非 `mcp_*`（采纳决策 1）**：`search → describe → call` 是对**任意工具来源**都成立的通用模式，不是 MCP 独有。命名为 `tool_*` 让这三个工具成为**所有 deferred 工具的唯一稳定入口**：本期背后接 MCP；`ConnectorType` 里已预留的 CLI/A2A、以及将来「插件多到也想 defer」时，都能挂到同一组元工具背后，**前缀不再新增任何工具**。这正是「极致优雅」的收敛点——前缀里永远只有这 3 个通用工具。

| 元工具 | 职责 | 输入 | 输出 | 落点 |
|--------|------|------|------|------|
| `tool_search` | **三态、两级渐进式披露**（见下例） | 都可选：`query` / `source` / `limit` / `offset` | 视模式返回 source 列表 / 某 source 工具列表 / 关键词命中（均**不含 schema**） | 消息体 |
| `tool_describe` | **批量**取工具完整 schema（一次多个） | `names`：工具名数组（如 `["mcp__playwright__browser_click", "mcp__playwright__browser_type"]`；单个也传数组） | 每个工具的完整 `inputSchema` + 描述（未知名单独标错、不影响其余） | 消息体 |
| `tool_call` | 真正调用工具 | `name` + `arguments` | 工具真实结果（text/image…） | 工具结果 + 图片回流 |

**`tool_search` 三态**（一个工具承载「先 source、再 tool」的两级发现，避免一次吐出全部工具）。这里 **`source` = 连接器实例**（本期即一个 MCP server；未来含 CLI / A2A / 插件，用 `type` 区分）；对 MCP，`source` 就是工具名 `mcp__{server}__{tool}` 里的 `{server}` 段。用 `source` 而非 `server` 命名，是为让这三个元工具对**任意连接器类型**通用（参数不换、前缀不动）：

| 调用形态 | 语义（第几级） | 返回 |
|----------|----------------|------|
| `tool_search()`（无参数） | **L1 列 source** | 已连接的连接器清单：每个 `{name, type, title, description, tool_count}`（`type`∈`mcp`/`cli`/`a2a`…） |
| `tool_search(source="playwright")` | **L2 列某 source 的工具** | 该 source 工具清单：每个 `{name, description}`（无 schema） |
| `tool_search(query="截图")` | 关键词检索（可加 `source=` 限定域） | 跨/内 source 命中：每个 `{name, source, description}` |

- **静态注册**：三者进 `BUILTIN_TOOL_CATALOG`（`core/tools/contract/catalog.rs`），**永不随 MCP 生命周期增删**；executor 经 `global_services.connector_registry` 触达 `McpManager`。
- **gating（config 派生、跨会话稳定）**：仅当 `connector.enabled && 存在已配置 server` 时暴露（仿 `load_skill` 的 `builtin_tool_surface_with_policy(allow_load_skill)` 过滤先例）。因为「有没有配置 server」来自 config、不随连接状态变，所以**存在性对前缀稳定**。
- **`tool_describe` 批量**：入参是**名字数组**，一次取回多个 schema。原因：一个流程往往要连着用同 server 的好几个工具（如 playwright 的 `navigate`+`click`+`type`+`screenshot`），批量取避免 N 次 describe 往返（省 turn、省缓存扰动）。语义确定性：结果按**入参顺序**返回，未知名放 `errors` 单列、不影响其余；单个工具也统一传 `["name"]`。
- **`tool_call` 路由**：`name` 编码了来源（`mcp__{server}__{tool}`），handler 交给 `McpManager::call_model_tool` 以 canonical name 查目录，再复用既有 `call_tool`（温连接 + 同一 trust + per-server 串行锁）。**不再经 `ToolRegistry`**（§2.4 已把具体 MCP 工具从注册表移除），因此路由靠目录查表而非注册表前缀匹配。
- **分层边界（`source` vs `server`）**：三个元工具**对外**统一用通用参数 `source`（未来可聚合 MCP/CLI/A2A/插件）；`McpManager` **对内**只管 MCP、其 API 仍称 `server`。对 MCP，元工具的 `source` 直接映射到 `McpManager` 的 `server`（同名）；将来新增连接器类型时，由元工具 handler 按 `type` 分派到对应 manager，**前缀里的三个元工具与其参数名都不变**。

**元工具描述（写死在前缀，强制引导 load `connectors` skill）**。三者的 `description` 是**常量字符串**（进前缀、byte-stable），风格对齐现有 builtin（`catalog.rs`：英文、密集、只写「影响 LLM 正确/成功调用」的用法约束）。其中 `tool_search` 描述要求：当 `load_skill` 可用时，首次使用前必须 `load_skill("connectors")`。这让关闭 skills 的兼容会话仍可使用元工具，而默认会话里的模型从前缀就知道「怎么发现、去哪学方法」，无需盲猜：

```text
tool_search:
Discover deferred tools from connected connectors (MCP now; CLI/A2A/plugins later)
that are intentionally kept OUT of your tool set to keep the prompt small and cache-stable.
When load_skill is available, before first use in a session you MUST load the "connectors"
skill via load_skill("connectors") — it teaches the full search -> describe -> call workflow
and when to batch calls via code.
Modes: tool_search() lists sources (name, type, description, tool_count);
tool_search(source="…") lists that source's tool names + short descriptions (no schema);
tool_search(query="…") keyword-searches across sources (add source= to scope).
Then use tool_describe([names]) to fetch schemas and tool_call(name, arguments) to invoke.
Output returns in the conversation, never in the prompt prefix.

tool_describe:
Fetch full input schemas for one or more deferred tools found via tool_search.
Pass names (array; a single tool is ["name"]). Returns each tool's inputSchema + description
in input order; unknown names are reported in errors without failing the rest.
Batch related tools in one call to avoid repeated round trips, then tool_call to invoke.

tool_call:
Invoke one deferred tool found via tool_search/tool_describe. Pass name
(e.g. mcp__<source>__<tool>) and arguments matching its inputSchema. Runs through the same
trust/permission gate as native tools; image results stream back to you. To call one tool
many times (fan-out) or filter large results, prefer writing code (see the connectors skill)
over many tool_call rounds.
```

> 注：`tool_search` 描述里点名的 `connectors` skill，其**索引条目**（name+description）也写死在前缀的 `<available_skills>` 里（§2.3.5 缓存不变量），两处交叉引用、都是常量 → 前缀 byte-stable。

**发现示例（两级渐进式披露）**。设当前接入两个 MCP：`playwright`（`@playwright/mcp`，约 21 个 `browser_*` 工具）与 `github`（GitHub MCP，约 8 个工具）。

> 定位：本示例用真实 server/工具帮助理解形态。skill 里**可以**放一个类似的具体示例（更好懂），但须框定为『示例』、声明真实清单以 `tool_search` 运行时结果为准；**不可**把活状态（连了谁、几个工具）当事实写死（§2.3 铁律、§5.3 边界）。

L1 —— 模型先看「有哪些连接器」，`tool_search()` **不返回任何工具**，只返回 source 清单（每条带 `type`）：

```jsonc
// 请求：tool_search()          // 无参数
// 结果（进消息体，不进前缀）：
{
  "sources": [
    { "name": "playwright", "type": "mcp", "title": "@playwright/mcp",
      "description": "驱动真实浏览器：导航/点击/输入/截图/等待…", "tool_count": 21 },
    { "name": "github", "type": "mcp", "title": "GitHub MCP",
      "description": "查询仓库/Issue/PR/文件内容", "tool_count": 8 }
  ],
  "next": "tool_search(source=\"<name>\") 查看该 source 的工具；或 tool_search(query=\"关键词\") 跨 source 检索"
}
```

L2 —— 模型选定 `playwright`，`tool_search(source="playwright")` 才返回**该 source** 的工具名与短描述（仍**不含 schema**）：

```jsonc
// 请求：tool_search(source="playwright")
{
  "source": "playwright",
  "tools": [
    { "name": "mcp__playwright__browser_navigate",        "description": "打开 URL" },
    { "name": "mcp__playwright__browser_click",           "description": "按 ref/选择器点击元素" },
    { "name": "mcp__playwright__browser_take_screenshot", "description": "截图（回流给模型）" },
    { "name": "mcp__playwright__browser_snapshot",        "description": "取可访问性快照(结构)" }
    // …其余 browser_* 工具
  ],
  "next": "选定后 tool_describe([names…]) 批量取 inputSchema（可一次多个），再 tool_call(name, args)"
}
```

（可选）关键词直达 —— 记得大概叫什么时，`tool_search(query="截图")` 跨 source 命中，跳过 L1/L2：

```jsonc
// 请求：tool_search(query="截图")
{ "matches": [
  { "name": "mcp__playwright__browser_take_screenshot", "source": "playwright", "description": "截图（回流给模型）" }
] }
```

随后**批量**取 schema —— 一个流程常要连用好几个工具，一次 `tool_describe` 全取回：

```jsonc
// 请求：tool_describe(["mcp__playwright__browser_navigate", "mcp__playwright__browser_click", "mcp__playwright__browser_take_screenshot"])
{
  "tools": [
    { "name": "mcp__playwright__browser_navigate",        "inputSchema": { /* … */ }, "description": "打开 URL" },
    { "name": "mcp__playwright__browser_click",           "inputSchema": { /* … */ }, "description": "按 ref/选择器点击元素" },
    { "name": "mcp__playwright__browser_take_screenshot", "inputSchema": { /* … */ }, "description": "截图（回流给模型）" }
  ],
  "errors": []   // 未知名放这里、不影响其余；结果按入参顺序
}
```

拿到 schema 后 `tool_call(名, {...})` 真正调用。**全链路：`tool_search()`（列 source） → `tool_search(source=…)`（列 tool） → `tool_describe([names…])`（批量取 schema） → `tool_call`**，每一步只把「够决定下一步」的最小信息拉进消息体——发现本身也是渐进式披露，token 随「用到的深度」增长，而非一次吐满。

### 2.3 connectors skill 的耦合修正：动态地图走「工具结果通道」，skill 保持静态（回应决策 3 的耦合追问）

> 追问：让 `load_skill("connectors")` 现从 `McpManager` 渲染正文，会不会把**通用的 skill 加载路径**反向耦合到**具体的连接器功能**？会。下面把这个耦合从第一性原理拆掉。

#### 2.3.1 原「虚拟渲染」设想为什么耦合（证据）

原设想：给 skill 加一个「虚拟」来源，`load_skill_payload / handle_load_skill` 碰到 `name=="connectors"` 就去读 `McpManager` 目录、跳过 `read_file`。但看真实代码，skill 的加载路径**只认「读文件」这一个原语**、且 `SkillSource` 是**优先级**维度而非**内容来源**维度：

```text
handle_load_skill(ctx,args)      core/agent_loop/tool_exec/branches/load_skill.rs
  └─ ctx.skill_set.resolve(name)   只认 SkillSet(磁盘发现来的元数据)
  └─ load_skill_payload(primitive) core/skill/load.rs
       └─ primitive.read_file(p)   正文永远来自「读文件」
SkillSource = Project|Agent|Managed   ← 是【优先级】轴, 不是【内容来源】轴
```

要塞进「虚拟渲染」，就得让 `load.rs` 反向 import 并分支到 `McpManager`：

```text
   ┌─────────────┐  反向依赖(坏)   ┌──────────────┐
   │ skill 子系统 │ ─────────────► │ 连接器/McpMgr │
   └─────────────┘  通用机制依赖    └──────────────┘
        具体功能; 且 if name=="connectors" 是写死特例,
        再加第 2 个动态 skill(cli/plugins) 又要改 load.rs —— 违反开闭
```

#### 2.3.2 第一性原理重构：动态内容本就有专门通道，不必把 skill 改「活」

问一句最朴素的：*一段「随运行时状态变化、只需按需出现在消息体」的内容，系统里已经有承载它的通道吗？* —— **有，就是工具结果（tool result）**。工具结果天生具备我们要的三条性质：① 动态（每次调用现算）；② 只进消息体、绝不进前缀；③ 来源无关（谁产出都一样回流）。既然如此，「把 skill 正文改成动态」是在**重复造一条已存在的通道**，还附送 skill→connector 耦合。按本仓工程规则，这种「本不该存在」的抽象应当**删掉**，而不是去「优化」它（回收当初「虚拟渲染」的复杂度）。

于是把职责一刀切开，各归其位：

```text
       静态·教方法                      动态·给活地图
   ┌────────────────────┐        ┌──────────────────────────┐
   │ connectors skill    │        │ 元工具结果 tool_search()   │
   │ (verify 物化范式)   │        │  = 当前连了哪些 source     │
   │  怎么 search/       │        │    + 每个的工具名/短描述   │
   │  describe/call、     │        └──────────────────────────┘
   │  何时写代码          │            进消息体·不进前缀·
   └────────────────────┘            由 McpManager 现算
      进前缀的只有它【写死】的索引条目
```

#### 2.3.3 主选方案（推荐）：静态 skill + 元工具出地图

- **skill 侧零改动**：`connectors` skill 做成**静态 embed、物化到磁盘**，与 `verify` 完全同一范式（`materialize_builtin_skills` 写盘 → `discover` 扫到 → `load_skill_payload` 读盘）。正文只讲**方法**（search→describe→call、何时写代码），**不含任何随连接变化的内容**。索引条目天然写死、前缀天然稳。**skill 子系统对「连接器」零认知。**
- **活地图走工具通道（两级）**：`tool_search()` 无参数 = 列 **source 列表**（L1）；`tool_search(source=…)` = 列**该 source 的工具**（L2）；`tool_search(query=…)` = 关键词检索（见 §2.2 三态与示例）。实时目录从 `McpManager` 现算、随**结果**进消息体。三个元工具的**描述里静态写明**这条发现顺序，模型**从前缀就知道怎么发现**、不盲猜，且发现本身也是渐进式披露（先 source 再 tool）。
- **连接器知识只集中在元工具 handler**：`tool_search/describe/call` 本就住在 `tool_exec`、本就经 `global_services.connector_registry` 触达 `McpManager`（§2.2）——这是「连接器工具用连接器服务」的**正当且不可避免**的依赖，不是把通用 skill 机制污染成连接器专用。

依赖方向对比：

```text
坏(原设想):  skill 机制 ──► McpManager       通用反依赖具体 + 写死特例
好(主选):    skill 机制  ⟂  McpManager       互不依赖
             元工具handler ──► McpManager     连接器工具用连接器服务(正当)
```

#### 2.3.4 备选方案（Option 1）：`SkillContentProvider` 端口（依赖倒置）

若产品坚持「`load_skill("connectors")` 正文里就要直接出活地图」（省掉一次 `tool_search` 调用），那也**不该**在 `load.rs` 写死分支，而应做**依赖倒置**：把「内容来源」抽象成一个 skill 模块自己拥有的端口，连接器/装配层实现并注入：

```text
core/skill/ 定义端口(skill 拥有插槽)             组合根/装配层实现并注入(adapter)
┌──────────────────────────────────┐  register  ┌────────────────────────────┐
│ trait SkillContentProvider {       │◄───────────│ 用 McpManager 查询 API 实现 │
│   fn index_card() -> 静态 name+desc │            │ 该 trait, 注册进 skill 注册表│
│   async fn render(file)->String     │            └────────────────────────────┘
│ }                                   │
│ SkillSet 额外持 Vec<dyn Provider>   │
│ load 时按名 data-driven 分发, 无特例 │
└──────────────────────────────────┘
```

- 关键：trait **由 skill 模块拥有**（被扩展方定义插槽）；实现体放在**组合根/装配层**（同时持有 `McpManager` 与 skill 注册表），或退一步放连接器模块。无论哪种，**skill→connector 反向依赖被消除**（skill 只认自己的 trait）；加第 2 个动态 skill = 再实现一个 provider 注册，`load.rs` 不动（开闭）。
- 代价：多一层抽象与生命周期（provider 的注册/失效要与 discovery 合流）。

#### 2.3.5 选型结论 / 缓存不变量 / 推翻条件

- **默认主选方案**（静态 skill + 工具通道出地图）：概念最少（**不新增 skill 抽象**）、耦合为零、与既有 `verify` / `tool_exec` 完全同构——这才是「极致优雅」。
- **仅当**实测「模型频繁需要在不调用任何工具的前提下就看到活地图」成为体验瓶颈，才升级到 Option 1 的 provider 端口；即便如此也保持依赖倒置、**绝不**在 `load.rs` 写死连接器分支。
- **缓存不变量（两方案共同守）**：`prompt_snapshot_signature`（`core/llm/system_prompt.rs`）把渲染后的整段 `<available_skills>` 纳入前缀签名。故 connectors 的**索引条目（name+description）必须是常量字符串**；任何随连接变化的信息（几个 server、谁在线）**只能出现在工具结果 / skill 正文（消息体）**，绝不进这一行。主选方案天然满足（skill 静态）；Option 1 靠 trait 契约强制 `index_card()` 返回常量。

### 2.4 切断前缀污染（核心改动）

`core/connector/mod.rs` 的 `ConnectorRegistry::spawn_connect_all` 不再接收 `ToolRegistry`，也**不再把 per-tool `mcp__{server}__{tool}` 注册进共享 `ToolRegistry`**；连接/断开只更新 `McpManager` 目录。于是 `observe_tool_surface / list_tools` 再也看不到任何**具体** MCP 工具或 schema（`tool_call` 描述中的常量 canonical-name 语法除外），**前缀与连接时序彻底解耦**——连上、断开、接多少 server，前缀逐字节不变。

### 2.5 图片回流与项目信任

- **图片**：`tool_call` 是内置工具、走 `tool_exec` 路径（不经注册表分支），因此其 handler 必须对 MCP 结果跑与 `extract_tool_result_media` 等价的逻辑——**按结果内容里的 `{type:"image", mimeType, data}` 块判定**（而非按工具名），把图片转成 `follow_up_parts` 的 `InputImage`。这与 [v1 §3.2.1 / §4.3](./v1-connector-foundation.md) 完全一致，只是触发点从「注册表 Ok 臂」搬到「`tool_call` 内置分支」。
- **信任**：Global 连接器无需项目批准；未批准项目的 Workspace 连接器返回 `awaiting_project_trust`，不启动也不可 Login。用户批准项目根后，`tool_call` 仍要查当前连接和工具可用性，未连接就返回可行动错误；不再按服务或命令指纹确认。

### 2.6 决策清单 / 什么条件应推翻

- **元工具走应用层（provider 无关）**，而非仅依赖 Anthropic 原生 `defer_loading`：中转站（fcodex）对 beta 头透传不确定，OpenAI 侧也要能用。将来若确认网关稳定透传，可**叠加**原生 `tool_search` 增强命中（不替换）。
- **三件套而非二件套**（search 直接返 schema）：三步让「只有决定要用时才拉 schema」，token 最省，也契合 Hermes/OpenClaw/Speakeasy。
- **推翻条件**：若实测在**极少量**工具（如单 playwright）下，多一次 search/describe 往返显著拖慢或降低成功率 → 该场景留作**未来选项**（§6.1「pin 直连」，本期不做）；若主力场景转为扇出/大数据 → 由 §5 代码执行承担。

### 2.7 伴随的提示词改动：`verify` skill 的 UI 验收章节（必须随 v2 一起改）

**背景**：`verify` skill（`assets/skills/verify/SKILL.md` 的「UI acceptance」节）在 v1 下**直接点名** Playwright MCP 工具——原文有「use the configured Playwright MCP tools」以及「call `browser_take_screenshot` **without** `filename`…」。这在 v1 成立，因为 `browser_*` 工具**直接在工具面里**、模型能直接调。

**为什么必须改**：v2 把 MCP 工具**移出工具面**，模型不能再直接调 `browser_take_screenshot`，必须走 `tool_search(source="playwright") → tool_describe([...]) → tool_call("mcp__playwright__browser_take_screenshot", {...})`。若不改，verify 的 UI 验收指令会指向**已不在工具面的名字**，模型无从下手。

**改动映射（before → after，语义不变，仅换调用方式）**：

| verify 原文（v1） | v2 改法 |
|-------------------|---------|
| 「use the configured Playwright MCP tools when available」 | 「先 `tool_search(source="playwright")` 发现工具 → `tool_describe([...])` 取 schema → `tool_call("mcp__playwright__…", args)` 调用」 |
| 「call `browser_take_screenshot` **without** `filename` and without `fullPage=true`」（为让图片内容回流给视觉循环） | 「`tool_call("mcp__playwright__browser_take_screenshot", { /* 省略 filename、不设 fullPage */ })`」——**视觉回流的注意事项原样保留**（省 filename 才会回图片内容，见 [v1 §5 图片回流](./v1-connector-foundation.md) 与本文 §2.5） |

**落地时序（重要）**：此改动**必须与 v2 实现同一批次落地**，**不能提前改**——否则在 v2 未上线时 `tool_call`/`tool_search` 尚不存在，会破坏当前 v1 的 UI 验收路径。因此本文只登记「要改什么」，实际编辑 `verify/SKILL.md` 排在 §3.4 的 S2（元工具可用）之后。这也正是 [v1 §10 决策日志 P6「跨文档修订」](./v1-connector-foundation.md) 预留的那笔。

---

## 3. 落地选型与实施

### 3.1 选型对比（业界三条路，为什么选「元工具 + skill」）

先把「怎么少放 token」的三条主流路摊开，再说 tomcat 选哪条、为什么。

```text
路线              前缀里放什么           省 token   额外基建           契合 tomcat?
──────────────    ──────────────────    ────────   ──────────────    ────────────────
A 原生 defer      工具名(隐藏 schema)    中          依赖 provider     ✗ 中转站透传不确定
  (Anthropic beta)                                  beta 头
B 元工具/渐进披露  3 个通用元工具         高          search/describe   ✓ 主选(provider无关)
  (本文主选)                                         /call + 目录 API
C 代码执行         工具当代码 API         最高(98.7%) 代码运行环境       ✓ 本期并做(复用插件VM)
  (Anthropic 案例)                                                     (§5)
```

- **A（provider 原生 defer_loading）**：Anthropic 有 beta 能力把工具「延迟加载」，但 tomcat 主要经中转站（fcodex）出网，**beta 头能否稳定透传不可控**，且不跨 provider。→ 不作为地基，仅作**将来增强**（若确认透传稳定，可在 B 之上叠加）。
- **B（元工具 / 渐进式披露）**：应用层实现，**provider 无关**，且与 tomcat 已有的 `ToolRegistry` / `builtin catalog` / `skill` 三套机制天然咬合。→ **主选**。
- **C（代码执行）**：把工具当成可编程 API，模型写代码批量调用、就地过滤，Anthropic 公布过 150k→2k（省 98.7%）的案例。**省得最狠但要有代码运行环境**。→ tomcat 已有 rquickjs 插件 VM，**本期一并做**（§5），无需新沙箱。

**结论**：以 B 为地基（治缓存 + 治 token 的根），C 复用现成 VM 承担扇出/大结果场景，A 留作将来增强。三者叠加而非互斥。

### 3.2 调研佐证（先看别人怎么做，再定架构）

遵循本仓「不可逆架构决定前先看 prior art」规则，查了同级 `~/workspace/` 参考实现，佐证「元工具 + 按需 schema」是主流、且**不需要数据库/向量库**：

| 参考实现 | 证据（文件 / 符号） | 结论 |
|----------|--------------------|------|
| Hermes (`hermes-agent`) | 工具发现按需返回、schema 不全量进前缀 | 元工具/按需 schema 可行 |
| OpenClaw (`openclaw`) | search→describe→call 分层 | 三件套是通用形态 |
| Codex (`codex`) | Rust core 侧不做「草稿/大目录」权威存储，状态留在各自进程内存 | 佐证「易变目录别进前缀/别落盘」 |
| VSCode (`vscode`) | `chatInputStatePersistence.test.ts` 把「图片载荷与频繁更新的输入状态分开存」写成测试 | 「易变 vs 稳定分层」是被测过的不变量 |

- **要不要数据库/向量库？不要**。当前单 server、约 20 工具，且 skill 已提供可浏览地图；检索用**确定性 token 重叠 + 子串**足矣（§4），全内存、零依赖、可单测。BM25 仅作目录规模变大后的升级路（§4.3）。

### 3.3 改动点清单（文件 → 改什么）

| # | 文件 | 改动 |
|---|------|------|
| 1 | `core/connector/mod.rs` | `spawn_connect_all` 移除 `ToolRegistry` 参数；**不再注册/注销** `mcp__{server}__{tool}`，只连接并更新 `McpManager` 目录（切断前缀污染） |
| 2 | `core/connector/mcp/manager.rs` | 新增目录查询 API 支撑 `tool_search` 三态：`list_servers()`（L1）/ `list_tools(server)`（L2）/ `search(query, server, limit, offset)`（关键词）+ `describe_many(names)`（**批量** schema，按入参序、未知名单列错误）；`call_model_tool` 复用既有 `call_tool` |
| 3 | `core/tools/contract/catalog.rs` | `BUILTIN_TOOL_CATALOG` 新增 `tool_search / tool_describe / tool_call / tool_run_code` |
| 4 | `core/agent_loop/tool_exec/**` | 三元工具 handler：经 `global_services.connector_registry` 触达 `McpManager`；`tool_describe` 接受 `names[]` 批量；`tool_call` 反解 `(server, raw_tool)` 直连 `call_tool`，并跑图片回流逻辑 |
| 5 | `assets/skills/connectors/**` + `core/skill/builtin.rs` | **静态** `connectors` skill（embed 资产，`materialize_builtin_skills` 物化，走既有 `discover`/`load_skill_payload` 磁盘链）；正文只教方法。**`core/skill/load.rs`、`handle_load_skill` 零改动**（不引入 `McpManager` 依赖） |
| 6 | `core/llm/system_prompt.rs` | 确认 `prompt_snapshot_signature` 只纳入**写死的**索引条目（静态 skill 天然满足）；gating 由 config 派生 |
| 7 | `core/agent_loop/tool_exec/branches/code.rs` + `ext/runtime/instance.rs`（§5） | `tool_run_code` 走既有 rquickjs VM；`await callTool` 经 async submit/poll hostcall 直连 `McpManager::call_model_tool`，不阻塞 QuickJS 事件循环 |
| 备选 | （仅当采纳 §2.3.4 Option 1）`core/skill/*` | 引入 skill 拥有的 `SkillContentProvider` 端口 + 在装配层注册连接器 adapter；**依赖倒置、`load.rs` 仍不认连接器** |

### 3.4 交付顺序（小步可回滚，每步带测试）

```text
S0 诊断闸门（§1.5）：TOMCAT_PROMPT_PREFIX_FINGERPRINT + prompt_runtime_snapshot 复现，判定 (a)/(b)/(c)
        │
S1 目录 API (manager.list_servers/list_tools/search/describe_many) + 单测(两级发现/批量 describe/确定性打分/分页/byte-stable)
        │
S2 三元工具进 builtin catalog + handler(search 三态 / describe 批量 / call) + gating(config 派生)
        │   ← 此时前缀已只多 3 工具；可先手测 tool_call 直连
S3 切断注册：connector/mod.rs 不再注册具体 mcp__server__tool → 单测「前缀看不到具体 MCP 工具/schema、turn1 起 byte-stable」
        │
S4 静态 connectors skill（verify 物化范式）+ tool_search 两级发现（L1 列 source / L2 列 tool）→ 单测「索引写死、skill 不依赖连接器、两级返回确定性」
        │
S5 tool_call 图片回流：内容型判定 → 集成测「MCP 图片结果 → InputImage」
        │
S6 代码执行(§5)：tool_run_code + callTool hostcall(复用插件 VM) → 集成测「VM 内扇出、同一 trust」
        │
S7 真实 LLM E2E(#[ignore])：不等待即发首条请求也稳定命中；search→describe→call 全链路
        │
S8 伴随/条件项：改 verify skill UI 验收提示词（§2.7，紧随 S2 之后可做）；
   若 S0 判定为 (c) 则处理 Anthropic 运行态尾巴（§1.6，条件性）
   （pin 直连 + grace/latch 为未来选项，见 §6.1，本期不做）
```

**回滚粒度**：S0 只读；S1–S2 纯新增（不碰既有前缀），可独立合入；S3 是「行为切换点」，若线上异常可单独回退 S3 保留元工具；S4–S6 均为增量；S8 各项相互独立、可单独取舍。

### 3.5 验收不变量（必须写成测试）

1. **前缀 byte-stable**：从 turn1 起，无论 MCP 是否连上/断开/接几个 server，工具面与 `<available_skills>` 索引条目逐字节不变（扩展既有 `tool_surface_is_byte_stable_*` 单测，新增「MCP 连接翻转」用例）。
2. **前缀不含具体 MCP 目录**：`observe_tool_surface / list_tools` 输出中不出现任何配置相关的 `mcp__{server}__{tool}` 或 schema；`tool_call` 描述里的固定 canonical-name 格式示例不属于运行态目录。
3. **connectors 索引写死 + skill 不依赖连接器**：`prompt_snapshot_signature` 在 MCP 连接前后不变；动态内容（活地图）只出现在 `tool_search` 结果（消息体），`connectors` skill 正文静态；`core/skill/*` 编译期不引用 `McpManager`（用一条依赖断言/架构测试守住）。
4. **调用等价**：`tool_call` 与 v1 直连 `call_tool` 在结果、trust 拦截、per-server 串行上等价。
5. **图片回流**：MCP 返回 image 块时，内置路径产出 `InputImage`（与 v1 §5 行为一致）。

---

## 4. `tool_search` 打分：到底要不要「子串+分词 / BM25 / 数据库」（回应决策 4）

### 4.1 先解释这几个名词（深入浅出）

- **子串匹配（substring）**：把「工具名+描述」拼成一段文本，看查询词是不是它的**连续片段**。例：查 `click`，`browser_click` 的描述含「click」→ 命中。**说人话：Ctrl+F 找关键词。** 简单、直观，但对同义/多词不敏感。
- **分词 + token 重叠（token overlap）**：把查询与工具文本都**切成词**（小写、去标点、按空格/下划线/驼峰断开），数**两边共有多少个词**，共有越多分越高。例：查 `click a web button` 与 `browser_click`→ 分词后共有 `click`，得 1 分；`browser_type` 共有 0 → 排后面。**说人话：看两句话「撞了几个词」，撞得多的排前面。**
- **BM25**：信息检索里的经典排序公式。在「撞词数」基础上再做两件事：① **稀有词加权**（`the/a` 到处都是不值钱，`rasterize` 少见很值钱——IDF 逆文档频率）；② **长度归一 + 词频饱和**（一个词出现 10 次不等于比出现 2 次值 5 倍，收益递减；长文档不因为字多就占便宜）。**说人话：撞到的词越稀有越加分，且长描述不因为啰嗦而虚高。** 全内存可算，**不需要数据库**——数据库/向量库是「几万条以上要建索引」才需要，我们几十条不必。

### 4.2 本期结论：只做「子串 + token 重叠」，不做 BM25、不引数据库

理由（第一性原理 + 规模）：

```text
当前规模：1 个 server，约 20 个工具，全部在内存 BTreeMap 里
+ 两级发现（L1 列 source → L2 列该 source 工具）已把「浏览」覆盖得很好，
  带词的 tool_search(query) 只是「记得大概名字、想跨 source 直达」时的补充检索
⇒ 20 条候选做一次线性扫描 + 打分，成本可忽略；BM25 的 IDF/长度归一在 20 条上几乎不改变排序
⇒ 结论：确定性「子串命中(权重高) + token 重叠数(权重次) + 名称命中加成」足矣
```

- **确定性**：同一 query 对同一目录，输出顺序**逐字节可复现**（打分相同时按工具名字典序兜底），可写进单测。
- **算法（本期）**：`score = w1·子串是否命中 + w2·(查询词∩工具词 的个数) + w3·(命中落在 name 而非 description 的加成)`；`limit/offset` 做稳定分页。全部纯函数、零依赖、可单测。

### 4.3 什么时候升级到 BM25（推翻条件）

当 ① 接入 server 数上升到**十几个**、工具总量到**数百**，且 ② 观测到「撞词数」把大量泛词工具误排前面（精度掉），再引入 BM25（仍全内存，`term→postings` 的小倒排表即可，**依然不需要数据库**）。两级发现（L1 source → L2 tool）本就把「浏览」拆小了，`query` 检索的边际价值不高，故 BM25 明确推迟。

---

## 5. 代码执行：复用插件 rquickjs VM，本期落地、无沙箱（回应决策 2）

### 5.1 为什么本期就做、且不需要新沙箱

用户主张（采纳）：agent 平时就在写项目代码、跑脚本，`bash` 工具本就能执行任意命令；单独为「调 MCP 工具的胶水代码」再起一套沙箱/IPC，是**多此一举**。第一性原理下更该问的是：**有没有现成的代码运行环境可复用？** —— 有：tomcat 插件系统已经内嵌 **rquickjs JS VM**（`ext/` 下的 dispatcher/ops），温进程、能 hostcall。于是：

```text
不新建沙箱 / 不新建 IPC 桥
        │
复用插件 rquickjs VM：新增 hostcall  callTool(name, args)  ── 直连 ──► McpManager::call_tool
        │                                                     (温连接 + 同一 trust,【非】registry 路径)
新增内置工具 tool_run_code(code)：把一段 JS 函数体交给该 VM 执行，只返回 JSON 终值
```

### 5.2 什么时候写代码、什么时候单发（写进 connectors skill 手册）

```text
                       要连续调用同一工具很多次(扇出) / 要过滤&聚合超大结果 / 多步链式?
                                  │是                              │否
                                  ▼                                ▼
                      tool_run_code：写 JS 循环 callTool，          tool_call：单发一次
                      就地 filter/map/reduce，只把【终值】回给模型   (几步顺序调用足矣)
```

**扇出（fan-out）是什么**：一次意图要对**很多对象**重复同一调用。例：抓 50 个页面标题——若走单发，要 50 轮 `tool_call`、50 份完整结果全灌进上下文（token 爆）；写 JS 在 VM 里循环 `callTool` 50 次、只把「50 个标题的数组」这一个终值回给模型，**上下文只涨一行**。这正是 Anthropic「code execution with MCP」150k→2k 的来源。

### 5.3 就用同一个静态 `connectors` skill 教这套用法（不再新增第二个 skill）

§2.3 已把 `connectors` 定为**静态、只教方法**的 skill；代码执行的教学**并入它**即可，不必再造第二个 skill。其正文讲清：① 两级发现 `tool_search()` 列 source → `tool_search(source=…)` 列 tool（或 `tool_search(query=…)` 直达）→ `tool_describe` → `tool_call`；② `tool_run_code` 何时用、怎么写（附最小 JS 例）；③ 「活地图走 `tool_search` 结果、不在 skill 里」的约定。

**关键边界（示例可以有，但不能被当成事实源）**：skill 正文放一个**具体调用示例没问题、也更好懂**——可以用真实工具名（如 `tool_search(source="playwright")` → `mcp__playwright__browser_click` → `tool_describe([...])` → `tool_call`）。唯一红线：示例必须**显式框定为『示例』**，并写明**真实清单永远以 `tool_search` 运行时结果为准**；**不要把活状态当事实写死**（例如写死「当前接了 playwright、有 21 个工具」），因为连了什么、几个工具是随 config/连接变化的活地图（§2.3 铁律），写死即第二份真相 + drift。一句话：**示例＝教「长什么样、怎么串」，活清单＝`tool_search` 现查**。这样系统内**只有一个 connectors skill**（静态 embed，verify 物化范式），职责单一：教方法。活地图始终由 `tool_search` 结果承载（§2.3）。

### 5.4 信任与边界

- `callTool` hostcall 与 `tool_call` 共用同一 trust 关卡（[v1 §3.1 R7](./v1-connector-foundation.md)）：VM 内调用未信任 server 同样被拦。
- 无沙箱是**有意的工程取舍**（与 `bash` 同源信任模型），非疏漏；若将来要对第三方不可信代码开放，再评估隔离，届时应作为**新决策**记录，而非在此偷偷收紧。
- **两层大小上限（别混，取不同数量级）**：① **VM 堆内存**（`ext/runtime/instance.rs::set_memory_limit`，由配置 `js_heap_mb` 控制，**MiB 级**，已存在）——管 VM 内部能持有多少数据、保护进程；扇出时的大中间结果就靠它兜住而**不外泄**。② **终值文本截断**（**KiB 级**，本期新增）——`tool_run_code` 回给对话的只是那个**终值**，它就是一条普通工具结果，故**对齐本仓既有工具结果上限、不新造魔数**：`MAX_CODE_RESULT_TEXT_BYTES = 64 KiB`，与后台 bash 的 64 KiB 预览同量级。超限时**给可见标记「已截断，省略 N 字节」并提示模型回代码里再 `filter`/聚合**，而非静默丢。截断在此是**护栏**：防「本该 `return 聚合值` 却手滑 `return 原始扇出结果`」把省下的 token 又灌回上下文（正好抵消做代码执行的意义，还可能撑爆上下文/毁缓存）。

#### 图片回流链路：先抽图，再截文本（次序不变量）

`tool_run_code` 的终值可能包含 MCP 的 `{type:"text"}` 与 `{type:"image"}` 块。图片不是文本，也**绝不能**先把包含 base64 的整份 JSON 转成字符串后再截断；必须复用 `tool_exec/media.rs::extract_mcp_tool_result_media`，在结构化结果上先分流：

```text
模型调用 tool_run_code(js)
        │
        ▼
┌────────────────────────────────────────────────────────────┐
│ rquickjs VM                                                 │
│   const shot = await callTool("mcp__playwright__screenshot", args) │
│   return shot;          ← 保留结构化 MCP 结果，不能 stringify  │
└────────────────────────────────────────────────────────────┘
        │ callTool
        ▼
McpManager::call_tool ──► Playwright MCP
        │
        ▼
{ content: [
    { type: "text",  text: "截图完成" },
    { type: "image", mimeType: "image/png", data: "<base64>" }
] }
        │
        ▼
tool_run_code handler：共享「MCP 结果媒体归一化」逻辑
        │
        ├── image 块 ─► 校验 mimeType + 解码 base64
        │                  │
        │                  ├─ < 1 MiB：InputImage（内联 data）
        │                  ├─ 1–10 MiB：优先上传 Files API，成功则 InputImage(file_id)
        │                  └─ ≥ 10 MiB：必须上传；失败则报告“图片省略”
        │
        └── text 块 ──► "截图完成\n[Image returned; see the following user message.]"
                              │
                              └─ 仅文本在这里套终值上限 T
                                 （本期 64 KiB；不碰图片 base64）
        │
        ▼
消息历史（按现有 tool dispatcher 契约）：
  ① tool message：文本 + 图片占位提示
  ② 紧随的 user message：InputImage { inline data | file_id }
        │
        ▼
下一轮 LLM 请求：provider 将 InputImage 编码成自己的视觉输入格式
                 └─► 模型看见截图；视觉 token/费用由 provider、
                      图片尺寸和 detail 等参数决定
```

**因此「图片不吃文本上限」不等于图片免费**：它仍占请求字节、Files API 配额和 provider 的视觉 token。Tomcat 当前只在上下文记账中把每个 `InputImage` 固定估算为 3600 字符（`tool_dispatcher.rs::follow_up_parts_chars`），这**不是**真实视觉 token 的计算结果。实现必须新增集成测试，断言「超 `T` 的文本被标记截断，但同一结果内的图片仍作为 `InputImage` 出现在下一轮消息」。
- **VM 的边界与推翻条件**：rquickjs VM 无宿主文件系统、只跑 JS，对「就地处理工具 JSON」正好够用。若将来确实需要宿主级脚本 / 任意语言 / 文件系统访问，再评估引入**本地 socket 桥**（在 host 侧起一个 unix socket，VM 或子进程经它回调 `callTool`）——只有到那时这套 IPC 才有正当理由；在此之前不预建，避免又一个「先瘦身、其实本不该存在」的基建。

---

## 6. 兜底、测试与调研

### 6.1 未来选项（本期不做）：pin 直连 + grace/latch

> **本期聚焦核心**：一律走渐进式披露（全部 deferred），**不实现 pin，也不实现 grace/latch**。本节只记录「若将来需要」的形态与推翻条件，供后续评估，**不列入本期交付顺序（§3.4）与测试矩阵（§6.2）**。

对「就一个 playwright、且高频」的场景，多一次 `search/describe` 往返可能不划算。**未来**可在 config 里把某 server 标为 `pin`（`expose: direct|deferred`，默认 `deferred`）：被 pin 的工具（**且仅这些**）注册进 `ToolRegistry`、**直接进前缀**（回到 v1 形态）。因为「哪些 server 被 pin」来自 config、不随连接变，**pin 集合对前缀稳定**。这是 §2.6 推翻条件的落地开关。

**一旦引入 pin**，才需要两个稳定器保这批直连工具缓存稳（默认 deferred 路径前缀里本就没有 MCP schema、天然稳，**不需要**它们，这也是本期能安全略过 pin 的原因）：

- **grace（启动宽限）**：首次 `build_prompt_snapshot` 前有界等待 pin 的 server 到终态，避免「turn1 无 pin 工具、turn2 才冒出来」的翻转；`spawn_connect_all` 仍非阻塞。
- **latch（不缩水）**：lifecycle coordinator 对 pin 工具在**瞬时 `NotReady`** 时 no-shrink，只有 deny / reload / 配置移除才真正下架。

二者**仅服务 pin**，本期均不做。

### 6.2 测试矩阵（P0/P1，映射 §3.5 不变量）

| 层级 | 用例 | 断言 |
|------|------|------|
| 单元 P0 | `manager.search` 确定性 | 同 query 同目录逐字节可复现；打分并列按名字典序 |
| 单元 P0 | `search` 分页 | `limit/offset` 稳定、不重不漏 |
| 单元 P0 | 前缀 byte-stable | MCP 连接翻转（未连→Ready→NotReady→Ready）下工具面 + skill 索引不变 |
| 单元 P0 | 前缀不含具体 MCP 目录 | `list_tools` 输出无任一配置 MCP 的 `mcp__{server}__{tool}` 或 schema |
| 单元 P1 | 静态 connectors skill | 索引条目写死；正文不随连接变；`core/skill/*` 不依赖 `McpManager`（架构断言）|
| 单元 P0 | `tool_search` 两级发现 | 无参→source 列表（含 type/tool_count）；`source=`→该 source 工具且不含 schema；均确定性排序、`limit/offset` 稳定 |
| 单元 P1 | `tool_describe` 批量 | 多名一次返回、结果按入参顺序；未知名进 `errors` 且不影响其余；单名 `["name"]` 亦可 |
| 集成 P0 | `tool_call` 等价 | 结果/trust 拦截/per-server 串行与 v1 直连一致 |
| 集成 P0 | 图片回流 | MCP image 块 → `InputImage` |
| 集成 P1 | 代码执行扇出与媒体分流 | VM 内多次 `callTool` 仅回终值；未信任 server 被拦；超 `T` 的文本带截断标记，而同一结果内 image 块仍以 `InputImage` 注入下一轮消息（先抽图、后截文本） |
| E2E `#[ignore]` P1 | 全链路真实 LLM | 不等待即发首条请求仍稳定命中缓存；`tool_search→describe→call` 走通、截图产出 |

### 6.3 为什么现有 E2E 没抓到这个 bug（回应最初排查诉求）

E2E（`tomcat/tests/ui_acceptance_real_llm_e2e.rs`）在发首条请求前 `wait_for_playwright_ready`，**工具集全程冻结**，所以永远命中缓存、测不出「连接时序导致前缀翻转」。整改后新增的 E2E **故意不等待**，复现真实会话的竞态，作为回归防线。

---

## 一句话总结

v2 把 v1「MCP 工具进 `ToolRegistry`、随 `list_tools` 进每轮前缀」这一处**推翻**：前缀里只常驻 `tool_search / tool_describe / tool_call` 三个泛化元工具 + 一条**写死**的 `connectors` skill 索引；「有哪些 MCP、各有哪些工具」这类活地图由**元工具结果**承载并做**两级渐进式披露**（`tool_search()` 列 source → `tool_search(source=…)` 列 tool → `tool_describe` → `tool_call`），随工具结果进消息体，`connectors` skill 保持**静态、只教方法**（verify 物化范式，skill 子系统对连接器**零依赖**——这是相比"虚拟渲染"更优雅的解耦，见 §2.3）；扇出/大结果由**复用现成插件 rquickjs VM**、无沙箱的代码执行承担。前缀恒小恒稳、O(1) 与 server 数无关 —— 缓存失效与 token 爆炸两症状一并根治。v1 其余决策（传输/图片/信任/配置）继续有效。
