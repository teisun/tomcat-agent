# pi-rust-wasm 用户故事
## 故事分级规则
- P0：一期MVP必须实现，核心路径能力，无替代方案
- P1：二期必须实现，核心竞争力能力
- P2：三期及以后实现，核心扩展能力
- P3：长期规划能力，非核心路径

## P0 一期MVP核心用户故事
### Story 1: 宿主引擎初始化与基础配置
**作为用户**，我希望能快速安装、初始化pi-rust-wasm引擎，完成基础LLM配置，正常启动运行。
**验收标准**：
- [ ] 支持单文件二进制安装，无需额外依赖配置
- [ ] `pi-wasm init`命令可完成初始化配置，引导用户配置LLM API密钥、基础安全策略
- [ ] 配置文件可正确加载并生效（敏感信息加密存储 TODO 后续考虑）
- [ ] `pi-wasm doctor`命令可检测运行环境、WasmEdge依赖、配置合法性，给出修复建议
- [ ] 引擎可正常启动、关闭，无崩溃、无致命错误日志，跨Windows/macOS/Linux三大平台正常运行

### Story 2: 4原语核心能力与安全管控
**作为用户**，我希望Agent能通过4原语安全地操作文件、执行命令，所有操作可控、可追溯。
**验收标准**：
- [ ] 完全对齐pi-mono的read/write/edit/bash API规范，功能完整
- [ ] 实现路径白名单机制，仅允许访问用户授权的目录
- [ ] 实现bash命令三级管控：白名单自动执行、审批类需用户确认、禁止类直接拦截
- [ ] write/edit操作前自动备份原文件，支持回滚，操作前显示diff预览与用户确认
- [ ] 所有4原语操作完整记录审计日志，包含操作内容、用户确认状态、执行结果、时间戳
- [ ] 支持全局配置自动确认策略，可设置白名单操作无需重复确认
- [ ] `pi audit list` 可列出历史审计记录；`pi audit show <id>` 查看单条详情；`pi audit export <file>` 导出完整日志文件

### Story 3: WasmEdge+QuickJS沙箱插件系统
**作为用户**，我希望能加载、运行pi-mono插件，插件在隔离沙箱内运行，不影响宿主系统安全。
**验收标准**：
- [ ] 基于WasmEdge实现插件沙箱运行时，每个插件独立Wasm实例隔离
- [ ] 内置WasmEdge官方QuickJS运行时，支持原生JS/TS插件执行，无需手动编译
- [ ] 实现插件全生命周期管理：加载、初始化、启用、禁用、卸载，资源完全释放
- [ ] 100%兼容pi-mono ExtensionAPI，社区标准插件零修改即可正常加载运行
- [ ] 插件级权限管控，加载插件时需用户确认授权的权限范围，未授权API无法调用
- [ ] 插件执行错误完全隔离，单个插件崩溃不会影响宿主引擎与其他插件运行
- [ ] `pi-wasm plugin`系列命令可实现插件的加载、卸载、列表查看、启用/禁用

### Story 4: Node.js兼容层与pi生态适配
**作为用户**，我希望使用的pi-mono插件能正常调用Node.js API，无需修改代码。
**验收标准**：
- [ ] 基于WasmEdge原生实现Node.js核心兼容层，覆盖pi插件高频使用的fs/path/process/console等全局对象与内置模块
- [ ] 支持CommonJS模块规范，require/import正常工作，插件内相对路径模块加载正常
- [ ] 实现事件循环机制，基于WASI Preview2异步IO，setTimeout/setInterval/Promise异步行为与Node.js完全对齐
- [ ] 支持http/https模块，网络请求受插件权限管控，可正常调用第三方API
- [ ] 绝大多数纯JS npm包可在插件内正常加载使用，无兼容性问题

### Story 5: 宿主核心API与工具注册
**作为插件开发者**，我希望能通过宿主API注册自定义工具，扩展Agent能力，被对话与其他插件调用。
**验收标准**：
- [ ] 实现完整的工具注册API，支持registerTool/unregisterTool/getToolList，完全对齐pi-mono规范
- [ ] 注册的工具可在对话中被Agent自动调用，也可被其他插件调用
- [ ] 工具调用经过权限校验、审计日志记录，调用参数与结果完整可追溯
- [ ] 支持工具的描述、输入Schema定义，符合LLM函数调用规范
- [ ] 插件卸载时，自动注销该插件注册的所有工具，无残留

### Story 6: 事件系统完整实现
**作为插件开发者**，我希望能监听宿主核心事件，在关键节点执行自定义逻辑，扩展Agent行为。
**验收标准**：
- [ ] 实现全局事件总线，完全对齐pi-mono的事件API，支持on/emit/off/once方法
- [ ] 覆盖会话、对话、4原语、插件、工具全生命周期核心内置事件
- [ ] 支持插件监听事件，注册同步/异步回调函数，事件触发时正常执行
- [ ] 单个回调函数执行错误被完整捕获，不影响其他回调与宿主主流程
- [ ] 支持插件发布自定义事件，实现插件间通信
- [ ] 插件卸载时，自动注销该插件注册的所有事件监听，无内存泄漏

### Story 7: LLM统一接入与调用
**作为用户**，我希望能自由配置、切换不同的大模型，插件可通过统一API调用LLM能力。
**验收标准**：
- [ ] 实现统一LLM Provider Trait，兼容所有OpenAI API格式的大模型
- [ ] 支持配置模型的温度、最大Token、上下文窗口等参数，会话级模型配置隔离
- [ ] 实现流式与非流式LLM调用API，完全对齐pi-mono规范，插件可正常调用
- [ ] 支持Token消耗统计与记录，每次对话显示Token消耗
- [ ] API密钥可配置并生效，调用支持限流与指数退避重试（加密存储 TODO 后续考虑）

### Story 8a: 异步 Hostcall 与 JS API 对齐
**作为插件开发者**，我希望在插件中使用 `async/await` 调用宿主 API（如 `await pi.exec("ls")`），耗时操作不阻塞插件执行，与 pi-mono 的异步编程模型一致。
**验收标准**：
- [ ] `pi.exec()`、`pi.createChatCompletion()` 等耗时 API 返回 Promise，插件可用 `await` 消费
- [ ] LLM 调用、命令执行等耗时 Hostcall 不阻塞 Wasm 实例，宿主后台异步处理
- [ ] 返回值格式与 pi-mono 一致（`ExecResult: {stdout, stderr, exitCode}`、`CompletionResult: {message, usage}`）
- [ ] `pi.on`/`pi.off`/`pi.emit` 无重复定义 bug，`pi.once` 可用，注册一次后多次 emit 仅触发 1 次
- [ ] 插件内多个并发异步调用（如同时 `await pi.exec()` + `await pi.createChatCompletion()`）可正确运行
- [ ] 异步操作超时后返回清晰错误，插件可通过 `try/catch` 捕获
- [ ] 同步 API（`pi.log`、`pi.registerTool`、`pi.on` 等）行为不变，不受异步改造影响
- [ ] `pi.registerTool` 注册工具后宿主可通过 host_call 感知（registerTool 触发 ≥1 次 host_call）；`pi.unregisterTool` 可正常反注册
- [ ] 以上异步 API 行为均可通过 `tests/wasmedge_e2e_tests.rs` 中 Wasm 真实运行时集成测试验证（E2E-WASM-011/022/023）

### Story 8: CLI工具基础对话与会话管理
**作为用户**，我希望能通过CLI工具与Agent对话，管理会话历史，正常使用插件能力。
**验收标准**：
- [ ] `pi chat` 命令启动对话模式，支持自然语言对话，流式响应渲染
- [ ] `pi chat --resume` 可恢复上次会话，历史上下文从持久化 JSONL 文件加载并注入 LLM
- [ ] 支持多轮对话上下文关联，Agent 可正常调用 4 原语、注册的工具、加载的插件能力；重启后从 JSONL 恢复消息历史，不丢失上下文
- [ ] 实现会话管理功能，支持创建、切换、归档、删除、搜索会话，历史持久化不丢失
- [ ] 对话中Agent调用4原语/工具时，清晰展示操作内容，等待用户确认后执行
- [ ] 支持Markdown/代码块高亮渲染，快捷键支持（Ctrl+C中断、Ctrl+D退出、↑↓历史导航）
- [ ] `pi session` 系列命令（list/new/switch/delete/archive/search）可完整管理会话生命周期
- [ ] Agent 执行工具期间，用户发送新消息可触发 Steering——完成当前工具后跳过剩余工具，注入新指令并重新调用 LLM（中途换方向不需要重新创建会话）
- [ ] Ctrl+C 可触发 Abort——当前工具执行完毕后立即终止 Agent，发布 agent_end(interrupted)
- [ ] Agent 回答完毕后用户继续追加消息（FollowUp），在同一会话上下文中无缝继续，无需重新初始化
- [ ] LLM API Rate Limit 或网络超时时，Agent 自动指数退避重试，对用户透明；致命错误（API Key 无效、模型不存在等）给出清晰提示并终止
- [ ] 工具执行进度通过事件实时反馈（agent_start/turn_start/tool_execution_start/end/agent_end），CLI 据此渲染执行状态
- [ ] 单条工具结果超过 `[context].layer0_single_result_max_chars`（默认 **50_000** chars，与 [context-management.md §4.4](architecture/context-management.md) 一致）时 Layer 0 落盘 + preview，不撑爆单次请求；可观测事件见 `tool_result_truncated` / 压缩相关事件（以代码与 [events.md](architecture/plugin-system/events.md) 为准）
- [ ] 长对话 token 超预算时按 [context-management.md](architecture/context-management.md) **现行**链路：**Layer 0**（同步：落盘 / compactable 区占位）→ **Layer 1**（异步预热摘要，时机 ⑤ 不阻塞）→ **Layer 2**（Boundary 延迟应用，时机 ②）→ **Layer 3**（仅 API **Context Overflow** 后 `force_drop_oldest_to_target` 兜底）；保护最近若干 turns 与水位线见文档 §4.2；压缩后继续正常对话
- [ ] Session 重载时正确识别 `BranchSummaryEntry`（含 `is_boundary=false` 跳过 / `true` 折叠、`S::E` 锚点，§5.7），恢复 `CompactionSummary` 消息（`MessageKind::CompactionSummary`）/ `Preheat` 状态与运行时一致，不重复摘要

> **Story 8 — 自动化 / 集成索引（上下文与 JSONL）**  
> 逐条对照见 [docs/reports/traceability-story8-context.md](../../docs/reports/traceability-story8-context.md)；E2E 场景编号见 [guides/testing/E2E_SCENARIO_LIBRARY.md](guides/testing/E2E_SCENARIO_LIBRARY.md) 表中 **E2E-CLI-081～091、092、093**（Story 9 小节，覆盖 AgentLoop + 上下文管理）。  
> **Transcript 格式（开发阶段）**：压缩摘要行仅支持 JSONL **`type: branch_summary`**；**不**提供读盘时将历史 `type: compaction` 映射为 `branch_summary`。无法反序列化的行在 `read_entries_tail` 中 **warn + skip**（见 `src/core/session/transcript.rs`）。  
> **§5.7.5.1 陈旧 `CompactionResult`**：单测 `check_after_reply_stale_apply_removes_branch_summary_and_keeps_preheat_idle`（`src/core/compaction/tests.rs`），与 [context-management.md §5.7.5.1](architecture/context-management.md) 一致。

## P1 二期核心用户故事
### Story 8b: 长生命周期 VM 与有状态插件支持
**作为插件开发者**，我希望插件的全局变量、事件监听器、定时器能在整个会话期间保持，不因事件触发而重置，与 pi-mono 的插件运行模型一致。
**验收标准**：
- [ ] 插件的全局变量跨多次事件调用保持（如 `let counter = 0` 在多次 `tool_call` 事件间累加）
- [ ] `pi.on()` 注册的 handler 只需注册一次，后续事件直接触发，无需每次重新执行插件脚本
- [ ] 周期性定时器在会话期间持续运行（`setInterval` 或等价的 `setTimeout` 链；后者为 wasmedge_quickjs 兼容实现，见 E2E-WASM-033）
- [ ] `session_start` 初始化的数据可在后续 `before_agent_start`、`tool_call` 等事件中读取
- [ ] 会话结束时（`session_shutdown` 或用户退出）VM 正常关闭，资源完全释放
- [ ] pi-mono 核心有状态插件（git-checkpoint、todo、plan-mode、ssh 等）可零修改正确运行

### Story 9: 插件自举全闭环
**作为用户**，我希望Agent能根据我的自然语言需求，自主生成、编译、加载插件，无需人工干预。
**验收标准**：
- [ ] Agent可从自然语言需求中提取插件功能点，生成符合pi-mono规范的JS/TS插件代码
- [ ] 支持运行时动态加载生成的插件代码到WasmEdge沙箱中，无需重启引擎
- [ ] 插件加载/运行失败时，自动提取错误信息，反馈给LLM自动修复代码，形成闭环
- [ ] 插件生成前清晰告知用户功能、权限需求，获得用户确认后方可生成加载
- [ ] 实现插件模板库，内置常用场景插件模板，提升生成成功率
- [ ] CLI新增插件生成相关命令，支持手动触发插件生成、预览、安装

### Story 10: 插件管理与模板库完善
**作为用户**，我希望能方便地管理本地插件，导入导出插件模板，复用社区资产。
**验收标准**：
- [ ] 实现插件本地仓库管理，支持插件的分类、标签、版本管理
- [ ] 支持插件的导入导出，可分享给其他用户使用
- [ ] 实现插件安全扫描门禁，导入插件时自动检测安全风险
- [ ] 支持第三方插件源配置，可在线检索、安装社区插件
- [ ] 完善插件详情查看、配置管理、日志查看能力

## P2 三期及以后核心用户故事
### Story 11: 多Agent体系完整落地
**作为用户**，我希望能创建自定义Agent，每个Agent有独立的人格、权限、插件列表，完全隔离互不干扰。
**验收标准**：
- [ ] 支持创建、编辑、删除、启用/禁用自定义Agent
- [ ] 每个Agent可独立配置人格设定、System Prompt、权限等级、启用的插件列表、LLM配置
- [ ] 每个会话可绑定指定Agent，切换Agent后上下文与配置自动同步
- [ ] 内置核心系统Agent，不可修改删除，负责项目核心流程
- [ ] Agent配置自动持久化，重启后不丢失

### Story 12: 长程记忆系统落地
**作为用户**，我希望每个Agent有独立的记忆空间，对话内容自动记忆，零上下文遗忘，越用越贴合我的需求。
**验收标准**：
- [ ] 每个Agent有完全独立的记忆空间，不同Agent的记忆完全隔离，无串扰
- [ ] 对话全流程自动提取核心信息、用户偏好、专业知识，向量化存入长期记忆库
- [ ] 对话时自动检索相关记忆，注入Prompt上下文，保证历史信息不丢失
- [ ] 支持记忆的手动编辑、分类、标签、删除、导入导出
- [ ] 支持每个Agent独立配置记忆检索规则、自动提取/注入开关

### Story 13: Skills技能系统落地
**作为用户**，我希望能创建、保存、复用标准化的工作流模板，不用每次重复写提示词。
**验收标准**：
- [ ] 支持创建、编辑、执行、删除Skill，每个Skill有完整的工作流定义
- [ ] 实现Skill工作流引擎，支持条件分支、循环执行、工具调用、4原语调用
- [ ] 支持Skill的导入导出、版本管理、Agent专属绑定
- [ ] 用户输入时，自动检索匹配相关的Skill，推荐给用户选择
- [ ] 内置常用Skill模板，覆盖代码开发、数据分析、文档写作等高频场景

## P3 长期规划用户故事
### Story 14: Web前端与Android跨平台适配
**作为用户**，我希望能通过Web界面和Android手机使用pi-rust-wasm，核心能力与CLI完全对齐，数据可跨端同步。
**验收标准**：
- [ ] 基于Tauri+React实现Web桌面端界面，核心功能与CLI100%对齐
- [ ] Android端APK可正常安装、启动，核心功能与桌面端完全对齐
- [ ] 界面适配移动端竖屏与触控操作，无错乱、无误触
- [ ] 支持桌面端与Android端的数据导出导入，无数据丢失（加密 TODO 后续考虑）
- [ ] 全平台体验一致性优化，操作逻辑统一

### Story 15: 独立应用生成能力
**作为用户**，我希望Agent能根据我的需求，生成完整的独立可执行应用，无需依附主程序运行。
**验收标准**：
- [ ] Agent可根据自然语言需求，拆解技术方案，生成完整的多文件项目代码
- [ ] 在隔离容器环境中完成项目编译、打包，生成对应平台的可执行安装包
- [ ] 支持生成桌面端、CLI工具、前端项目等常见类型的应用
- [ ] 复杂应用可通过多Agent协作并行开发，提升效率与质量
- [ ] 生成的应用完全独立，可直接运行、分发、二次开发，不依赖主程序

### Story 16: 插件市场与生态建设
**作为用户**，我希望能在插件市场中检索、安装、分享优质插件、Agent模板、Skill模板，完善社区生态。
**验收标准**：
- [ ] 实现官方插件市场，支持插件、Agent、Skill的检索、预览、一键安装
- [ ] 实现开发者上传、发布、更新插件的完整流程
- [ ] 实现插件评分、评论、使用量统计，优质内容推荐
- [ ] 完善插件安全扫描、合规审核门禁，保证市场内容安全
- [ ] 实现开发者文档、最佳实践、插件开发模板，降低开发门槛

### 注：以下 Agent Loop 能力延迟到二期
- **工具循环自动检测**：Generic Repeat / Ping Pong / Global Circuit Breaker 三道防线