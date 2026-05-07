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

## 阶段性理念调整（2026-04-22 起）

项目经过一期 MVP 落地，已验证 Rust+WasmEdge+QuickJS 架构可行性与 pi-mono 兼容能力。当前阶段的战略调整：

- **先把单 Agent 做到极致**，再扩展到多 Agent、Skill、插件生态、UI 等周边能力；
- **插件系统冻结**：保留已完成的沙箱、4 原语、长生命周期 VM、pi-mono 兼容层，不再新增插件特性；待单 Agent 体验与自进化能力成熟，再回到插件生态做第二轮投资；
- **新路线图使用 P0-P9 十档**：P0-P9 不再是「紧急度」，而是**执行编排顺序**（上一档完成后再投入下一档），具体见下文。

## 当前状态（001-mvp 已关闭）
一期核心交付已完成并冻结：Rust 宿主核心、WasmEdge+QuickJS 沙箱、4 原语执行引擎、pi-mono 兼容层、异步 Hostcall、长生命周期 VM（VM Actor）、CLI 对话模式、Agent Loop 三层循环、基础审计日志。`001-mvp` 时期归档文档已从仓库移除；若需溯源可查 **Git 历史**（曾位于 `openspec/specs/archive/001-mvp/`）。

## 当前迭代：002-single-agent-complete（单 Agent 完善期）
完成 P0 + P1 两档，目标：让单 Agent 在 macOS/Linux 上的基础体验、状态管理、任务循环达到「无 P0 bug、可稳定长时间运行」。执行看板：[`agents/TASK_BOARD_002/README.md`](../../agents/TASK_BOARD_002/README.md)。

核心方向（对应 P0-P1 十六个顶层任务）：
1. **基础体验**：bug 修复（VMActor shutdown / 三套管道 / stream timeout / tool loop detection）、工作目录权限分级、工具系统整改、TUI 体验强化、中断/恢复 transcript 完整性、长任务后台化；
2. **思考与展示**：Thinking API 接入 + TUI 可折叠展示；
3. **摘要优化**：对齐 [`docs/reports/compaction-prompt-cc-vs-pi.md`](../../docs/reports/compaction-prompt-cc-vs-pi.md) §5.3/§5.4，升级 Compaction prompt 为 9 节模板，禁止 tools 调用；
4. **Agent Loop 模块化**：拆 `run.rs`（832 行）为 dispatcher / tool_exec / stream_handler / error_classifier；
5. **状态管理**：Checkpoint + 断点续跑、PLAN 模式增强、提问/应答机制、结果验证 review 子流程、Feedback 回路、集成测试规范。

## 不做（当前阶段 Out of Scope）
1. **多 Agent / Agent 编排 / 安全体系 / 多会话**（P5 才启动）；
2. **插件系统新特性**（冻结区，P6；仅保留 T-001 VMActor shutdown 这类维护性修复）；
3. **Skill 系统**（调度、工作流、内置 Skill）—— P2；
4. **记忆系统 / USER.md / MEMORY.md**—— P3；
5. **自进化 / 学习回路**—— P4；
6. **跨平台（WasmEdge 下载脚本、Android、openclaw 兼容）**—— P7；
7. **多 LLM 适配 / 多 IM 网关（Telegram/Slack/企微）**—— P8；
8. **UI（Tauri+React、Android 端、插件可视化）**—— P9；
9. **插件自举 / AI 自主生成插件闭环** —— 随 P6 插件系统解冻时再评估；
10. **容器化运行 / 插件市场 / 多模态** —— 长期规划，不在 P0-P9 范围内。

## P0-P9 路线图（十档执行顺序）

| 档位 | 核心主题 | 核心方向 | 关联模块/报告 |
|------|----------|----------|---------------|
| **P0** 单 Agent 基础体验 | bug 修复 / 工作目录权限 / 思考与工具展示 / 工具系统 / 摘要优化 / Agent Loop 模块化 | T2-P0-001~010（16 项中 10 项） | `src/core/agent_loop/`、`src/core/compaction/`、`src/ext/dispatcher/`、`src/api/chat/`、TUI |
| **P1** 状态管理 | Checkpoint / PLAN 模式 / 任务断点续跑 / 结果验证 / Review / Feedback | T2-P1-001~006 | `src/core/session/`、`src/api/render/`、PLAN 子流程 |
| **P2** Skill 系统 | Skill 声明/注册/发现、调度器、工作流引擎、内置高频 Skill | 新建 `src/core/skill/` | [plugin_skills_first_principles_pi_rust_wasm.md](../../docs/reports/plugin_skills_first_principles_pi_rust_wasm.md) §4-5、T-114/T-115 |
| **P3** 系统提示词 + 记忆 | USER.md / MEMORY.md 加载注入、系统提示词文件化/模板化、会话记忆总结 | 扩展 `src/core/system_prompt.rs`、新建 `src/core/memory/` | T-030/T-031/T-045/T-093/T-094/T-097/T-098 |
| **P4** 自进化 / 学习 | 自总结生成 Skill、学习回路（从 Feedback 生成 SKILL/MEMORY 增量）、自举 AI 编程 Agent | 参考 [hermes-agent](../../../hermes-agent/) | T-095/T-103 + 新增 T-142 |
| **P5** 多 Agent + 安全 + 多会话 | Agent 编排器、Agent 邮箱、独立 VM 运行、安全体系 9 条、多会话管理 | 多 Agent 基础设施 | T-022/T-025~T-029/T-052~T-061/T-073~T-084 |
| **P6** 插件系统（冻结区，仅维护） | 插件管线收尾、WAPM/预热/关闭 AOT、自举闭环、VMActor shutdown 修复 | `src/ext/`、`src/ext/plugin/` | T-001/T-062~T-070/T-133/T-134 |
| **P7** 跨平台 | WasmEdge standalone 下载与链接、install 脚本、Android、openclaw 兼容 | `scripts/`、`build.rs` | T-104/T-105/T-106/T-107 |
| **P8** 多 IM / 多 LLM 适配 | 多 LLM 适配层（Anthropic/Gemini/local-llm）、IM 网关（Telegram/Slack/企微/邮件/Webhook）、商米场景 | 新建 `src/core/llm/providers/`、`src/gateway/` | T-128/T-129/T-130 + 新增 T-143/T-144 |
| **P9** UI | Tauri+React Web 桌面端、Android 端、插件/Skill/Agent 管理可视化 | 新仓库或 `ui/` | - |

> **档位语义**：P0-P9 是**执行编排顺序**，上一档核心完成后才投入下一档；`docs/TODOS.md` 中并行维护「紧急度标签」`[BUG]/[UX]/[REF]` 用于同档位内排序。
>
> **档位 vs 旧十一期**：旧「一期（MVP）/ 二期（插件自举）/ 三期（多 Agent）/ ... / 十一期（资源改造）」在新架构下重新分配到 P0-P9，不再按时间期次编号。

## 长期愿景
打造一款轻量、安全、全兼容 pi 生态的 AI Agent 运行时引擎：从单 Agent 做到极致，让用户通过自然语言完成复杂任务，带出可审查的产出（日志、diff、测试、review）；再扩展到多 Agent 协作、Skill 复用、插件生态、跨平台，兼顾生态开放性与系统安全性，成为 pi-mono 生态的高性能、高可靠 Rust 实现，推动 AI Agent 全民化、安全化落地。