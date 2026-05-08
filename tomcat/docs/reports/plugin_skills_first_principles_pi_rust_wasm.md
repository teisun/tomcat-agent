# 插件系统与 Skills：第一性原理报告（tomcat）

**版本**：1.1（**v1.1**：新增附录「`@openclaw/plugin-sdk` 子路径一览」，与上游 `main` 的 `exports` 对齐）  
**日期**：2026-04-19  
**落盘路径**：`tomcat/docs/reports/plugin_skills_first_principles_pi_rust_wasm.md`  
**范围**：从第一性原理回答「是否必须做插件系统」、与四款 Agent 项目对照、对 **tomcat** 的分阶段建议；**不**替代既有实现细节报告（如 [plugin_systems_openclaw_pi_mono_pi_agent_rust.md](plugin_systems_openclaw_pi_mono_pi_agent_rust.md)）。

---

## 摘要

- **Skills** 主要解决 **「模型/用户如何知道怎么做」**（文档、流程、渐进披露），默认**不**在宿主侧新增「可执行扩展边界」。
- **插件 / 扩展系统** 解决 **「宿主行为如何被第三方或用户代码扩展」**（新工具、钩子、渠道、控制面），**必然**带来加载、信任、策略与运维成本。
- **结论**：对 tomcat 而言，**完整「插件市场」式体系不是 MVP 必要条件**；应先稳住 **会话、工具、权限、流式、记录**，再以 **Skills 等价物 → 窄契约扩展（声明式 / Wasm + hostcall）→ 长生命周期兼容层** 分层推进。
- **常见误区**：Hermes **并非**「只有 Skills」——仓库内同时存在 **Python 插件**（manifest + `register` + hooks）与 **Skills**（`SKILL.md`）；二者双轨并存。
- **与本仓库 TODO 对齐**：已将插件/VM 簇若干条目从 **P1 调整为 P2**（见 [§9](#9-与-tomcatdocstodosmd-的同步修订)），避免与核心体验类 P1 混淆心理压力。

---

## 1. 常见误区：Hermes「没有插件系统」？

**事实**：**有**。

- **插件**（可执行扩展、工具注册、生命周期 hooks）：见上游 [`hermes-agent/hermes_cli/plugins.py`](https://github.com/Dicklesworthstone/hermes-agent/blob/main/hermes_cli/plugins.py)（用户目录 `~/.hermes/plugins/`、`plugin.yaml`、`register(ctx)`、`hermes_agent.plugins` entry-point 等）；用户文档 **[Plugins](https://github.com/Dicklesworthstone/hermes-agent/blob/main/website/docs/user-guide/features/plugins.md)**。
- **Skills**（渐进披露、`SKILL.md`、清单/查看工具）：[`hermes-agent/tools/skills_tool.py`](https://github.com/Dicklesworthstone/hermes-agent/blob/main/tools/skills_tool.py)，目录约定 `~/.hermes/skills/`。

因此：**「只有 Skills、没有插件」不成立**；对比 OpenClaw / **pi-mono** 等生态时应区分「文档技能轨」与「可执行扩展轨」，Hermes 是 **双轨** 产品。

---

## 2. 第一性原理：两类机制各解决什么问题？

### 2.1 根本区分

| 维度 | Skills（及同类） | 插件 / 扩展系统 |
|------|------------------|----------------|
| **核心对象** | 知识、流程、约束如何进入 **模型上下文** | **宿主进程**如何执行 **新代码路径**或与外部系统 **接线** |
| **典型载体** | `SKILL.md`、提示片段、渐进加载的文档 | Manifest、SDK、`register*`、WASM 组件、hostcall |
| **默认信任压力** | 偏低（多是文本；若触发工具则走已有工具策略） | 偏高（执行任意扩展逻辑 = 新攻击面） |
| **工程必然产物** | 扫描、索引、token 预算、引用解析 | 发现、版本、隔离、审计、冲突与降级 |

### 2.2 一句话

- **Skills**：让模型 **「会做题」**（方法论、步骤、何时调用已有工具）。
- **插件**：让系统 **「长出新手脚」**（新工具实现、新渠道、新钩子——**脚长在别人写的代码里**）。

### 2.3 是否「必须」上插件系统？

从第一性原理：**不是「要做 Agent 就必须做插件」**。

- 若产品目标仅是 **coding agent + 内置工具 + 良好提示与规程**，**Skills（或等价机制）+ 宿主内建工具** 即可闭环。
- 若目标包含 **不可内置的能力**（例如必须与某 IM 深度集成、替换模型管线、用户可分发「真逻辑」扩展），才需要 **插件式扩展**；且应优先 **窄契约**（声明式描述、Wasm、hostcall 策略），而非一上来全量复制某生态的 Node 扩展面。

---

## 3. 四项目对照（极简）

| 项目 | Skills / 文档轨 | 可执行扩展轨 | 备注 |
|------|------------------|-------------|------|
| **OpenClaw** | `SKILL.md`、skills 扫描（「菜谱」） | `openclaw.plugin.json` + Plugin SDK（「插头」） | 产品与文档明确 **分流**（参见 Tomcat 内 `openclaw_docs/11-Skills与Tools.md`、`12-Plugins.md`） |
| **Hermes** | `SKILL.md` + skills 工具 | `plugin.yaml` + Python `register` + hooks + pip 插件 | **双轨**（见 §1） |
| **pi-mono** | prompts/skills 等资源管线（与扩展并列） | `pi.extensions` + **ExtensionAPI**（Node/Bun） | 扩展 **表达力强**，宿主信任模型接近完整 Node |
| **pi_agent_rust** | 同类资源发现子集 | JS（QuickJS）/ Wasm / native 描述符等多运行时 | **统一协议**下的分层执行；侧效应强约束 + hostcall |

对 **tomcat** 的启示：**不必**在一条线上同时复制「OpenClaw 插件 + pi-mono 扩展 + pi_agent_rust WASM」的全部面积；应按 **目标风险与兼容需求** 选层。

---

## 4. Skills 与插件系统的本质区别（对照表）

以下 **Skills** 取 OpenClaw / Hermes / Anthropic 系常见语义；**插件**取可执行扩展的一般语义。

| | **Skills** | **插件系统** |
|---|-----------|-------------|
| **载体** | 主要是 ** markdown / 文档 ** + 元数据 | **代码或带执行语义的制品**（JS、Python、WASM 等）+ manifest |
| **谁执行** | **模型阅读**或经 **既有工具**（如 `skill_view`）加载；执行工具时仍走宿主工具实现 | **扩展代码**在宿主加载的运行时中执行（或 Wasm 引擎） |
| **边界** | 多数是 **数据面**（上下文、指令） | **控制面 + 侧效应**（文件、网络、子进程、通道） |
| **典型用途** | 规范流程、降 token、团队共享「怎么做」 | 新增工具、接线、替换某子系统、生命周期自动化 |
| **比喻（OpenClaw 文档）** | **菜谱夹** | **可插拔插头**（仍须配合策略与沙箱） |

**边界情况**：Skill 文档里若只写「请调用某某工具」，则执行时仍落在 **插件/内置工具** 上——**Skills 与插件在此衔接，但不互相替代**。

---

## 5. 对 tomcat 的分阶段建议（忽略当前实现细节）

### 5.1 阶段 0 — MVP / 核心可用

- **优先**：会话完整性、工具与权限、流式与超时、TUI 与中断、会话记录等与 **P0** 一致的能力（参见 [`docs/TODOS.md`](../TODOS.md)）。
- **Skills 等价物**：静态或半静态 **技能库**（文档 + 渐进加载 + 与系统提示词/模板结合）——**低侵入**，不强制先上「第二套动态执行面」。

### 5.2 阶段 1 — 扩展能力「第一跳」

- **优先**：**宿主内建工具**完善 + **声明式策略**（权限、目录、hostcall 分类）；若需用户逻辑，倾向 **Wasm + 窄 ABI + hostcall 闸门**（与 tomcat 已有 Wasm/VM 方向一致）。
- **审慎**：**全量 pi-mono SDK shim**（`#T-062` 类）——生态价值高，但**不应**阻塞核心体验；详见 TODO 优先级调整。

### 5.3 阶段 2 — 生态与兼容

- **长生命周期 VM**、LRU、初始化资产搬迁（`#T-063`/`#T-064`/`#T-065`/`#T-066`）：在 **确有扩展驻留需求** 后加码。
- **WAPM / 预热 / 沙箱**（`#T-067`～`#T-070-wasm`）：归为 **生态与加固**，在单 Agent 体验稳定后推进（与 [`TODOS.md`](../TODOS.md) §十 多 Agent 备注一致）。

### 5.4 研究项

- **对标 Skills**（如 `#T-114`、`#T-115`）：保持 **P3 研究**，为阶段 0–1 提供输入，但不提前锁死插件形态。

---

## 6. 架构与技术选型原则（建议）

1. **契约先于功能面**：先固定 **manifest、能力声明、hostcall 类别**，再扩注册种类。
2. **分层**：声明式描述（最低功率）→ Wasm（可审计）→ 长生命周期脚本 VM（兼容性成本最高）。
3. **与核心隔离**：扩展默认 **不可**等价于无限 OS 权限；默认走 **策略 + 审计**（对齐 pi_agent_rust / OpenClaw 文档中的治理叙事）。
4. **不复制「最多功能的单一_RUNTIME」**：按场景选择 Skills **或** 插件 **或** Wasm，而不是一条线上堆满。

---

## 7. 结论（给 tomcat 产品与设计）

| 问题 | 建议 |
|------|------|
| **插件系统是否必须有？** | **不是**做出可用 coding agent 的必要条件。 |
| **要什么再来做插件？** | 明确出现「宿主侧必须加载第三方/用户可验证扩展」的产品承诺时，再以 **窄契约** 引入。 |
| **Skills 要不要？** | **更值得早做**（或以轻量文档+模板先替代），因其对 **上下文质量** 提升大、工程风险相对可控。 |
| **插件与 Skills 关系？** | **互补**：Skills 教模型 **用现有脚和工具**；插件 **提供新脚**（新执行能力）。 |

---

## 8. 相关仓库内文档

- 三端插件/扩展对比：[plugin_systems_openclaw_pi_mono_pi_agent_rust.md](plugin_systems_openclaw_pi_mono_pi_agent_rust.md)
- 长生命周期 VM 与异步：[async-handler-in-long-lived-vm.md](async-handler-in-long-lived-vm.md)
- OpenClaw 概念分册（Tomcat 摘引）：`openclaw_docs/11-Skills与Tools.md`、`openclaw_docs/12-Plugins.md`

---

## 附录：`@openclaw/plugin-sdk` 子路径一览（上游 OpenClaw）

便于对照 **「窄契约」**：OpenClaw 将扩展可见能力拆成 **`packages/plugin-sdk/package.json` → `exports`** 的多子路径；下表为 **类别 / 模块（npm 子路径）/ 简述**，依据上游 **`main`** 分支 `src/plugin-sdk/*.ts` 的注释与导出摘要整理；**以前端 manifest 与源文件为准**，本表仅作导读。

| 类别 | 模块（`@openclaw/plugin-sdk/…`） | 描述 |
|------|-----------------------------------|------|
| 标识 | `account-id` | 账户 ID 归一化（与路由一致）。 |
| ACP | `acp-runtime` | ACP 会话/控制面运行期类型与注册。 |
| 插件入口 | `plugin-entry` | `OpenClawPluginApi` 等类型、`definePluginEntry`、插件配置 schema。 |
| 插件运行期 | `plugin-runtime` | 命令/钩子/HTTP 注册/交互/lazy-service；`PluginRuntime` 类型。 |
| Provider 样板 | `provider-entry` | 单 Provider（API Key 等）catalog 与 `definePluginEntry` 组合样板。 |
| Provider 鉴权 | `provider-auth` | Profile store、OAuth/CLI 凭据、marker（偏静态配置）。 |
| Provider 鉴权 | `provider-auth-runtime` | 执行期 key 轮换、`requireApiKey`、运行时 auth 解析。 |
| Provider 鉴权 | `provider-env-vars` | Provider 相关环境变量列举与省略。 |
| Provider 传输 | `provider-http` | Provider HTTP：`fetchWithTimeout`、`postJsonRequest` 等。 |
| Provider 模型 | `provider-model-types` | `ModelApi` / `ModelDefinitionConfig` 等类型再导出。 |
| Provider 模型 | `provider-model-shared` | Replay/catalog 共用 helper，减循环依赖。 |
| Provider 模型 | `provider-onboard` | 轻量 onboarding（默认模型、fallback）。 |
| Provider 流式 | `provider-stream-shared` | pi-mono 流包装与 `tool_stream` 参数合成。 |
| Provider 工具 | `provider-tools` | 工具 schema 兼容性（Gemini/XAI 等）。 |
| Provider 用量 | `provider-usage` | 多云用量抓取与快照。 |
| Provider 搜索 | `provider-web-search` | Web 搜索 Provider 注册与搜索辅助。 |
| Provider 搜索 | `provider-web-search-contract` | 契约化注册 + 配置启用。 |
| Provider 搜索 | `provider-web-search-config-contract` | 仅配置侧凭证/合并契约。 |
| Provider 视频 | `video-generation` | 视频生成 Provider 类型门面。 |
| 渠道 | `core` | **Channel 插件**主 SDK（`ChannelPlugin`、适配器工厂等）。 |
| 渠道 | `channel-secret-runtime` | 渠道密钥与 Secret 输入运行期收集。 |
| 渠道 | `channel-streaming` | 渠道流式与块式输出配置类型。 |
| 浏览器 | `browser-config-runtime` | 浏览器向配置快照、插件 enable 状态、端口默认。 |
| 浏览器 | `browser-node-runtime` | `callGatewayFromCli`、Gateway RPC、懒服务、`runExec`。 |
| 浏览器 | `browser-setup-tools` | `callGatewayTool`、节点/媒体/CLI 共用工具。 |
| 浏览器 | `browser-security-runtime` | SSRF、安全 FS、代理、脱敏等窄安全工具。 |
| 配置 | `config-runtime` | 完整配置读写、群策略、上下文可见性等。 |
| 运行时 | `runtime-env` | 默认 runtime、日志 verbosity、退避计时。 |
| 诊断 | `runtime-doctor` | 诊断/卸载/遗留配置别名检测。 |
| 安全 | `security-runtime` | 密钥与 DM/可见性策略聚合。 |
| Secret | `secret-ref-runtime` | `SecretRef` 窄导出。 |
| Secret | `secret-input` | `SecretInput` + zod schema。 |
| 安全 | `ssrf-runtime` | Pinned dispatcher、guard fetch、私网策略。 |
| CLI | `cli-runtime` | CLI 共用解析与版本输出。 |
| 错误 | `error-runtime` | 域错误类与错误图格式化。 |
| 文本 | `text-runtime` | Markdown/日志/文本处理大块导出。 |
| 测试 | `testing` | 渠道与插件运行期的受支持测试桩。 |
| 依赖 | `zod` | 再导出 `zod`。 |

**Memory**：使用 **`@openclaw/memory-host-sdk`** 各子路径（与 Plugin SDK 分列 npm 包）。

---

## 9. 与 `tomcat/docs/TODOS.md` 的同步修订

依据本报告 **§5–§7**（扩展面不应与核心 P0/P1 **体验类**条目同等「紧迫」心理排序），已对 `TODOS.md` 作如下修订（具体以文件 diff 为准）：

- 将 **T-062、T-063、T-064、T-066、T-133** 自 **P1** 调至 **P2**，并在条文与 §八 清单中增加 **指向本报告** 的备注。
- 更新文首 **「优先级速查」** 表中 P1/P2 条目列表与统计数说明。
- **T-067～T-070-wasm、T-114～T-116、T-095-mem** 等保持 **P3** 研究与生态位；不另抬级。

修订目的：**区分「核心体验尽快修」与「扩展平台逐步建」**，避免插件/VM 工程量在排期上与流式、权限、会话等 **真正 P1 体验债** 混排。

---

*本报告随产品目标与仓库 TODO 迭代；若与源码实现冲突，以源码与 TASK_BOARD 为准。*
