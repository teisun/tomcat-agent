# pi-rust-wasm - 安全自进化AI Agent运行时
## 项目定位
pi-rust-wasm是一款参考pi-agent-rust设计、基于Rust+WasmEdge构建的轻量、高安全、可自进化的AI Agent核心运行时。通过WasmEdge内置的QuickJS引擎与Node.js兼容层，实现pi-mono生态100%兼容，提供沙箱隔离的插件系统、原子化4原语能力、可自举的插件生成闭环，打造「人人可扩展、安全可管控」的AI Agent底层引擎。

## 问题陈述
1.  pi-mono原生实现存在无限制系统权限风险，插件可直接访问宿主系统，无沙箱隔离，存在严重安全隐患
2.  现有AI Agent插件体系要么能力边界僵化，要么开放裸系统权限，无法兼顾灵活性、生态兼容性与安全性
3.  pi-mono插件生态与JS/TS强绑定，缺少Rust实现的高性能、高可靠的宿主运行时，自举能力受限
4.  多Agent协作与Skill工作流体系与核心插件系统耦合度过高，导致核心引擎臃肿，新手上手门槛高
5.  现有Wasm插件方案开发成本高，对JS/TS生态兼容性差，无法复用pi-mono社区成熟插件资产
6.  长程记忆、跨平台能力与核心引擎强绑定，无法实现按需扩展，导致资源占用高、适配难度大

## 理念
1. 极简、可读、轻量
2. 自举、自闭环进化

## 核心价值
1.  **WasmEdge+QuickJS沙箱插件体系**：基于WasmEdge官方优化的QuickJS运行时，每个插件独立Wasm实例硬隔离，100%兼容pi-mono插件API与Node.js生态，兼顾灵活性与安全性
2.  **pi-mono原生4原语能力**：完全对齐pi-mono的read/write/edit/bash原子化操作，作为宿主核心可信API，全程权限管控、审计可追溯
3.  **安全自举全闭环**：Agent可基于自然语言需求，自主完成「插件代码生成→沙箱内编译→热加载→错误修复」全流程，全程隔离运行，无宿主系统污染
4.  **全兼容pi生态**：原生对齐pi-mono ExtensionAPI、事件系统、工具注册规范，社区pi-mono插件零修改即可运行，无缝复用社区资产
5.  **极简分层架构**：核心引擎仅保留「宿主可信层+Wasm沙箱插件层」，多Agent、Skill、长程记忆等能力均通过插件实现，核心轻量稳定、易维护
6.  **全平台原生支持**：基于Rust+WasmEdge实现跨平台兼容，Windows/macOS/Linux/Android全平台覆盖，核心能力100%对齐
7.  **精细化权限管控**：插件级细粒度权限配置，默认最小权限，仅开放授权的4原语、网络、文件访问能力，从根源规避安全风险

## MVP范围（一期）
1.  核心宿主引擎：Rust+Tokio异步核心架构，全局单例WasmEdge运行时引擎初始化与生命周期管理
2.  核心4原语能力：完全对齐pi-mono的read/write/edit/bash宿主API，实现权限管控、审计日志、用户确认机制
3.  沙箱插件系统：基于WasmEdge+官方QuickJS运行时，实现插件全生命周期管理，独立Wasm实例隔离，pi-mono API兼容
4.  Node.js核心兼容层：基于WasmEdge原生实现，覆盖fs/path/process/console等pi插件高频核心模块
5.  宿主核心API：LLM统一接入、工具注册、事件系统、配置管理核心能力落地
6.  极简CLI工具：实现会话管理、对话交互、插件加载/卸载、配置管理核心命令
7.  Agent Loop 核心运行时：三层嵌套循环（对话管理/容错重试/思考-行动），Steering/FollowUp/Abort 用户中断机制，Rate Limit 指数退避自动重试，完整 AgentEvent/ExtensionEvent 生命周期发布，消息类型边界隔离
8.  基础安全体系：插件级权限管控、沙箱隔离、4原语操作全链路审计（敏感数据加密 TODO 后续考虑）
9.  异步 Hostcall 与 JS API 对齐：复用 `__pi_host_call` 的 submit/poll 机制实现异步非阻塞 Hostcall（LLM/exec 等耗时调用），pi_bridge.js 核心 API 返回 Promise 对齐 pi-mono async/await 编程模型。技术方案见 [异步 Hostcall 与事件循环设计](architecture/async-hostcall-event-loop.md)、[JS API 对齐设计](architecture/js-api-alignment.md)

## 不做（一期Out of Scope）
1.  多Agent完整体系、自定义Agent能力（三期及以后实现）
2.  Skills技能工作流系统（五期及以后实现）
3.  长程记忆系统（四期实现）
4.  Web/Android前端界面（六期实现）
5.  插件自举全闭环、AI自主生成插件（二期实现）
6.  容器化运行环境、多平台交叉编译（三期实现）
7.  插件市场、多模态能力、团队协作（长期规划）
8.  内存模式多档位与运行时动态切换（一期仅预留设计，十一期实现）
9.  工具调用循环检测（ToolLoopGuard）：三道防线检测（一期仅有 MAX_TOOL_ROUNDS 硬限制，二期实现）
10. 上下文自动压缩（Compaction）：Context Overflow 时 LLM 摘要（一期仅做简单截断兜底，二期实现）
11. 长生命周期 VM（插件跨事件调用状态保持）：一期短生命周期 VM 足以支持无状态 async/await 插件；二期通过 waitForEvent 模式实现 VM 会话级存活，支持 pi-mono 有状态插件（git-checkpoint、todo、plan-mode 等）。方案见 [Phase 2 长生命周期 VM 设计](architecture/phase2-long-lived-vm.md)

## 十期迭代路线图
| 期数 | 核心主题 | 核心交付内容 | 预计周期 |
|------|----------|--------------|----------|
| 一期（MVP） | 核心引擎与插件系统落地 | 1. Rust宿主核心架构；2. WasmEdge+QuickJS沙箱运行时；3. 4原语宿主API；4. pi-mono API兼容层；5. 基础CLI工具；6. LLM统一接入 | 2周 |
| 二期 | 插件自举闭环 | 1. AI自主生成插件全流程；2. 运行时动态编译与热加载；3. 错误自动修复闭环；4. 插件模板库；5. CLI自举相关命令 | 2周 |
| 三期 | 多Agent基础能力 | 1. 自定义Agent生命周期管理；2. Agent级独立权限与插件隔离；3. 容器化安全执行环境；4. 跨平台交叉编译适配 | 3周 |
| 四期 | 长程记忆系统 | 1. Agent级独立记忆空间；2. 对话自动记忆提取与注入；3. 向量存储引擎；4. 记忆管理CLI与API | 2周 |
| 五期 | Skills技能系统 | 1. 标准化工作流模板；2. Skill工作流引擎；3. 插件与Skill联动；4. 内置高频场景Skill模板 | 3周 |
| 六期 | 全平台前端界面 | 1. Tauri+React Web桌面端界面；2. Android端基础适配；3. 全平台核心能力对齐；4. 插件管理可视化界面 | 4周 |
| 七期 | 多Agent协作体系 | 1. 多Agent异步协作；2. 串行/并行/评审协作模式；3. 协作状态同步与依赖协调；4. 协作模板化 | 3周 |
| 八期 | 独立应用生成能力 | 1. 基于4原语的完整应用生成；2. 容器化隔离编译打包；3. 多平台安装包生成；4. 二次开发引导 | 4周 |
| 九期 | 插件市场与生态建设 | 1. 本地插件市场基础能力；2. 插件/Agent/Skill模板分享；3. 第三方模板源支持；4. 安全扫描门禁 | 3周 |
| 十期 | 生产级稳定与体验闭环 | 1. 全量性能优化与稳定性提升；2. 新手引导与全流程体验优化；3. 企业级安全审计与权限管控；4. 完整用户文档与最佳实践 | 4周 |
| 十一期 | 资源改造（内存模式与资源伸缩） | 1. MemoryProfile/配置观测；2. 按 profile 限制 Wasm/QuickJS；3. 运行时动态切换；4. 惰性加载与 LRU、Auto、mimalloc 可选等 | 待定 |

## 长期愿景
打造一款轻量、安全、全兼容pi生态的AI Agent运行时引擎，让用户可以通过自然语言轻松扩展Agent能力，自主生成、安装、运行插件，兼顾生态开放性与系统安全性，成为pi-mono生态的高性能、高可靠Rust实现，推动AI Agent全民化、安全化落地。