# 连接器 / MCP 架构总纲（v1 → v2 导航）

> 定位：本文是**连接器(Connector)模块**的**总纲与导航**——用最短篇幅讲清「这个子系统是什么、经历了哪两代设计、现在以哪份文档为准」，并把细节分流到两份版本文档。
> 关联规范：上位 [`ARCHITECTURE_SPEC.md`](../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)；相邻方案 [`plugin-system-overview.md`](./plugin-system-overview.md)、[`skill-system.md`](./skill-system.md)、[`context-management.md`](./context-management.md)。
> 研发计划：`~/.cursor/plans/mcp缓存前缀失稳整改_66bd7b9d.plan.md`（承载 todos / 交付顺序 / 测试清单，**设计以本目录文档为单一真相**）。

---

## 一、这个子系统解决什么问题（一句话）

把外部能力（MCP / CLI / A2A server）安全地接入 Tomcat。当前 MCP 连接器同时支持 stdio 与 Streamable HTTP；HTTP 可使用无认证、Bearer/custom headers 或标准 OAuth/PKCE。工具目录仍通过 v2 的 `tool_search` → `tool_describe` → `tool_call` 渐进式发现。

```text
   外部 server (MCP: @playwright/mcp …)          Tomcat Agent
   ┌───────────────────────┐    连接/发现    ┌────────────────────────┐
   │ tools: browser_click  │◄──────────────►│  连接器模块              │
   │        browser_type   │    调用/结果    │  (连接·发现·接入·回流)   │──► LLM
   │        screenshot …   │◄──────────────►│                        │
   └───────────────────────┘   含图片回流    └────────────────────────┘
```

---

## 二、两代设计 & 当前以谁为准

```text
v1 连接器基座（Option B）                    v2 渐进式披露（最新·权威）
────────────────────────                    ──────────────────────────
把 MCP 工具【像插件一样注册进                 前缀只放 3 个泛化元工具
ToolRegistry】、随 list_tools                tool_search / tool_describe / tool_call
【进每轮前缀】                       ──────►  + 1 条【写死】的 connectors skill 索引
                                    修订      MCP 完整目录不进前缀，按需拉进消息体
✔ 传输/图片/信任/配置 仍权威                  ✔ 治好「缓存失稳 + token 爆炸」
✘ 「进前缀」导致缓存失稳 + token 爆炸
```

- **v1 仍权威的部分**：传输（R2，stdio + Streamable HTTP + OAuth）、图片回流（R5）、信任模型（R7）、配置形状（R10）。
- **v2 修订的部分**：v1 §3.1 **R4「MCP 工具进 `ToolRegistry` / 进前缀」+ R9「Ready 即注册」** 被推翻，改走渐进式披露。
- **冲突时**：以 v2 为准。

---

## 三、v2 为什么必须来：一根病根，两个症状

```text
     病根：完整且【易变】的 MCP 工具目录，被放进了「每次都要重发的前缀」
                            │
           ┌────────────────┴────────────────┐
     症状①缓存失效                        症状②token 爆炸
 (工具集随异步连接/断开漂移，前缀反复变)   (server 越接越多，前缀越堆越大)
           └────────────────┬────────────────┘
                            ▼
   v2 正解：让 MCP schema 根本不进前缀 —— 渐进式披露
```

> 大模型服务商靠「请求**前缀逐字节相同**」命中缓存；`tools` 数组排在最前面，它一变、后面 system + messages 的缓存全废。把「易变的完整工具目录」放进这里，等于每轮都在自毁缓存，还随 server 增多不断膨胀。详见 [v2 §1 病根复盘](./connector-mcp/v2-progressive-disclosure.md)。

---

## 四、文档导航（读哪份、看什么）

| 文档 | 内容 | 何时读 |
|------|------|--------|
| **本文** `mcp-client.md` | 总纲 / 导航 / 两代演进 / 病根 | 先读，建立全局 |
| [`connector-mcp/v1-connector-foundation.md`](./connector-mcp/v1-connector-foundation.md) | v1 基座：MCP stdio/Streamable HTTP、OAuth/PKCE、信任模型、图片回流与配置形状 | 要了解**传输/信任/图片/配置**细节 |
| [`connector-mcp/v2-progressive-disclosure.md`](./connector-mcp/v2-progressive-disclosure.md) | v2 渐进式披露：抽象/具体总图、三元工具、**静态** connectors skill（只教方法）、`tool_search` 两级发现与打分、代码执行（复用插件 VM）、落地选型与实施、测试矩阵 | 要了解**当前工具暴露方式 / 缓存整改 / 落地步骤** |

---

## 五、v2 一图速览（细节见 v2 文档）

```text
【常驻前缀·恒稳·O(1)】          【按需·进消息体·不入前缀】           【运行时唯一事实源】
tool_search                    tool_search()         → 列 source   ┌──────────────────┐
tool_describe                  tool_search(source=…) → 列 tool     │  McpManager 目录  │
tool_call                      tool_describe([名])   → schema(批量)◄│ (连/断只改这里,   │
+ <available_skills> 里一条     tool_call(名,参数)    → 结果        │  从不动前缀)      │
  【写死】connectors 索引       写 JS(插件VM) 扇出     → 终值        └──────────────────┘
（活地图=tool_search 结果；load connectors skill 只学方法·静态·不出活数据）
```

连接/断开只更新 `McpManager` 目录、从不改前缀 → 前缀逐字节恒稳、大小与 server 数无关。

---

## 一句话总结

连接器模块把外部 server 的工具接入 Tomcat；**v1** 打好基座（stdio 传输 / config 即信任 / 图片回流 / 配置），但把「易变的完整工具目录放进缓存前缀」，招致缓存失稳 + token 爆炸；**v2** 以渐进式披露根治——前缀只留 3 个泛化元工具 + 1 条写死的 skill 索引，MCP 目录按需披露、复用插件 VM 做代码执行扇出。**当前以 v2 为权威，传输/信任/图片/配置仍看 v1。**
