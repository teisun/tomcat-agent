# PI Agent TODOS

> 个人灵感 / 想法 / 待办收集池。成熟条目可晋升为 [TASK_BOARD_002 索引](docs/agents/TASK_BOARD_002/README.md) 正式任务。
>
> 组织方式：**先按领域分类，再在每条上标注档位（P0-P9）**；同档位内用「紧急度标签」`[BUG]/[UX]/[REF]/[DOC]` 做二次排序。
>
> 最近更新：2026-05-25（恢复误删的 T-142/T-146 Feedback 相关 backlog）

---

## 档位定义（P0-P9 执行编排顺序）

> **档位不再是紧急度，而是执行顺序**。上一档的核心工作完成后再投入下一档。紧急度用 `[BUG]/[UX]/[REF]/[DOC]` 等标签在同档位内排序。

| 档位 | 主题 | 核心内容 | 对应迭代 |
|------|------|----------|----------|
| **P0** | 单 Agent 基础体验 | bug 修复 / 工作目录权限 / 思考与工具展示 / 工具系统 / 摘要优化 / Agent Loop 模块化 / 中断恢复 / TUI 强化 / 长任务后台化 | `002` T2-P0-001~010 |
| **P1** | 状态管理 | Checkpoint / PLAN 模式增强 / 任务断点续跑 / 结果验证 / Review / Feedback / 集成测试规范 | `002` T2-P1-001~006 |
| **P2** | Skill 系统 | Skill 声明/注册/发现、调度器、工作流引擎、内置 Skill；claude-code / openclaw skill 对比 | 未启动 |
| **P3** | 系统提示词 + 记忆 | 系统提示词文件化 / 模板化 / USER.md / MEMORY.md / 跨会话记忆、Token 节省 | 未启动 |
| **P4** | 自进化 / 学习 | 自总结生成 Skill、学习回路（Feedback → SKILL/MEMORY）、自举 AI 编程 Agent；业界学习（Codex / Harmony / Candle / llm-chain / swarms / coworker 等） | 未启动 |
| **P5** | 多 Agent + 安全 + 多会话 | 多 Agent 编排 / 邮箱 / 独立 VM / 安全体系 9 条 / 多会话管理 | 未启动 |
| **P6** | 插件系统（冻结区） | 插件管线收尾、VMActor 修复、WAPM / 预热 / 关闭 AOT、插件自举闭环（仅维护） | 冻结 |
| **P7** | 跨平台 | 平台适配、Android、openclaw 兼容与安装体验收口 | 未启动 |
| **P8** | 多 IM / 多 LLM 适配 | 多 LLM 适配（Anthropic/Gemini/local-llm）、IM 网关（Telegram/Slack/企微/邮件/Webhook）、商米场景 | 未启动 |
| **P9** | UI | Tauri+React Web 桌面端、Android 端、插件/Skill/Agent 管理可视化 | 未启动 |

### 紧急度标签（同档位内）

- `[BUG]` 明确 bug，应优先于增强
- `[UX]` 体验类
- `[REF]` 代码/文档重构
- `[DOC]` 文档 / 规范 / 报告
- `[RES]` 研究 / 对比 / 阅读

### 代码核对约定（tomcat 仓库）

- **已实现**：本文件中带 `[x]` 的条目表示在 **当前工作区 `tomcat/` 源码** 中已有对应实现或行为；核对日期写在条目内。
- **仍开放**：`[ ]` 表示未完成或未核对通过；若你认为条目表述过时，应先改代码再改文档。

---

## 优先级速查（P0-P9）

### P0 — 单 Agent 基础体验（~19 条）

| 编号 | 分类 | 条目 | 说明/备注 | T2 映射 |
|------|------|------|-----------|---------|
| T-002 | Bug | 三套管道混乱需重构 | 技术债，功能未损坏 | T2-P0-009 |
| T-008 | 交互/TUI | workspace 没切换到当前目录 | shell `cwd` 未生效 | T2-P0-005 / T2-P0-004 |
| T-009 | 交互/TUI | user turn list 显示 | TUI 信息展示 | T2-P0-008 |
| T-010 | 交互/TUI | 状态总览 | TUI 信息展示 | T2-P0-008 |
| T-011 | 交互/TUI | 时间戳、时间显示 | 辅助信息 | T2-P0-008 |
| T-012 | 交互/TUI | 编辑模式美化 | 视觉/美观 | T2-P0-008 |
| T-013 | 交互/TUI | user content 可重新输入 | 编辑/重发 | T2-P0-008 |
| T-014 | 交互/TUI | diff 视图 | 增删行数可见 | T2-P0-008 |
| T-035 | 工具 | 默认不用子进程工具创建目录 | 可通过 prompt 调优缓解 | T2-P0-005 |
| T-036 | 工具 | Chat 不访问当前目录也不申请授权 | 不尝试访问也不弹授权 | T2-P0-005 |
| T-037 | 工具 | 无法在规划中执行 tomcat CLI 命令 | 规划模式约束 | T2-P0-005 |
| T-039 | 工具 | 拦截删除换成归档 | 安全增强 | T2-P0-005 |
| T-153 | 工具/Web | 按架构文档实现 `web_search` + `web_fetch` | 契约与路线图见 [`docs/architecture/tools/web_search.md`](architecture/tools/web_search.md)、[`docs/architecture/tools/web_fetch.md`](architecture/tools/web_fetch.md)（含 PR-WS-* / PR-WF-*、`openai-responses` 门闩、§2.4.2.1 HTTP 上游字段、cc-fork/hermes/openclaw 对标）；认领时把文档验收矩阵与 `src/core/tools/web_*` 单测对齐；晋升正式卡可走 **T2-P1-007** 或另拆 T2 子任务 | T2-P1-007（占位） |
| T-046 | 权限 | 工作目录读写授权分级缺失 | 读/写权限未分级 | T2-P0-004 |
| T-050 | 权限 | Bash 访问目录限制和授权 | 解析命令限制 | T2-P0-004 |
| T-051 | 权限 | 工作目录说话就可以改配置 | 便捷性 | T2-P0-004 |
| T-148 | 权限 | `tomcat pathrules remove` CLI | T2-P0-004 follow-up | T2-P0-004 |
| T-149 | 权限 | chat `/reload` 配置热加载 | T2-P0-004 follow-up | T2-P0-004 |
| T-150 | 权限 | path_rules 双层存储（builtin + TOML） | T2-P0-004 follow-up | T2-P0-004 |

### P1 — 状态管理（~0 条）

| 编号 | 分类 | 条目 | 说明/备注 | T2 映射 |
|------|------|------|-----------|---------|
| — | — | （无开放条目） | Plan / Checkpoint / 集成规范 / Review 相关 T2 均已 DONE | — |

> `T-146 Feedback 回路` 见新增条目区与 **四、会话管理**；对应 T2-P1-005（002 迭代看板已取消，TODOS 仍保留为 P3/P4 铺垫）。

### P2 — Skill 系统（~5 条）

| 编号 | 分类 | 条目 | 说明/备注 |
|------|------|------|-----------|
| T-114 | 研究/Skill | 对比 claude code 的 skill 系统 | 升档自 P3 |
| T-115 | 研究/Skill | 对比 openclaw 的 skill 系统 | 升档自 P3 |
| T-138 | Skill | Skill 注册/发现机制 | 新增 |
| T-139 | Skill | Skill 工作流引擎 | 新增 |
| T-147 | 研究/安全 | 自我攻击/自我安全进化机制设计 | P2 研究；不阻塞 002 |

### P3 — 系统提示词 + 记忆（~10 条）

| 编号 | 分类 | 条目 | 说明/备注 |
|------|------|------|-----------|
| T-016 | 交互/TUI | 上下文管理视图 | 依赖上下文核心 |
| T-030 | 会话 | 每个会话维护记忆总结文件 | |
| T-031 | 会话 | 三级会话上下文可选 | 当前/父会话/智能体记忆 |
| T-045 | 上下文 | Token 节省机制 | 2 轮后落盘 |
| T-093-mem | 记忆 | 记住喜好/过往/方法论 | |
| T-094 | 记忆 | 是否读取记忆可配 | |
| T-098 | 系统提示词 | 系统提示词文件化 | |
| T-099 | 系统提示词 | 通才描述 | |
| T-140 | 记忆 | USER.md 加载注入 | 新增 |
| T-141 | 记忆 | MEMORY.md 加载注入 | 新增 |

### P4 — 自进化 / 学习（~12 条）

| 编号 | 分类 | 条目 | 说明/备注 |
|------|------|------|-----------|
| T-095-mem | 记忆/自进化 | 自总结生成 skill | |
| T-103 | 规范/自进化 | 自举的 AI 编程 Agent 设计 | 长期架构 |
| T-108 | 研究 | 拉 Codex / Harmony 代码 | 业界学习 |
| T-109-res | 研究 | Beads / Gas Town / Anthropic Tasks / Dolt | |
| T-110 | 研究 | Candle 推理库 | |
| T-111 | 研究 | llm-chain 编排库 | |
| T-112 | 研究 | swarms | |
| T-113 | 研究 | coworker | |
| T-116 | 研究 | 对比 CC 对 MCP 调用的优化 | |
| T-118 | 愿景 | 每天 surprise me、每周 big surprise idea | 学习灵感 |
| T-142 | 自进化 | 学习回路（Feedback → SKILL/MEMORY 增量） | 新增 |
| T-146 | 反馈 | Feedback 回路落盘（为 P3/P4 铺垫） | 新增，目前在 P1 实作 |

### P5 — 多 Agent + 安全 + 多会话（~30 条）

| 编号 | 分类 | 条目 | 说明/备注 |
|------|------|------|-----------|
| T-021 | Agent Loop | 任务前信息搜集和评估 | |
| T-022 | Agent Loop | Agent loop 安全设计 | |
| T-023 | Agent Loop | 一心二用（子 Agent 干活） | |
| T-025 | 会话 | 多 session 支持 | |
| T-026 | 会话 | 父会话 ID + 对话序号 | 依赖 T-025 |
| T-027 | 会话 | 可配置上下文 | |
| T-028 | 会话 | 只看问题模式 | |
| T-029 | 会话 | 无限上下文/会话 | |
| T-052 | 安全 | 提示注入攻击检测 | |
| T-053 | 安全 | 行为意图审查策略系统 | |
| T-054 | 安全 | 数据加密（规格中多处 TODO） | |
| T-055 | 安全 | 仅在网络请求边界注入凭证 | |
| T-056 | 安全 | CLI 访问依赖系统钥匙串解密 | |
| T-057 | 安全 | 6 位临时验证码换取 token | |
| T-058 | 安全 | 绑定本地回环地址 | |
| T-059 | 安全 | 参考 IRONClaw 安全方案 | |
| T-060 | 安全 | 安全参考文章 | |
| T-061-sec | 安全 | 攻击指令拦截/用例收集 | |
| T-151 | 安全 | Bash 动态路径访问与提示词注入防御 | 当前 `bash_parser` 只能静态、尽力解析命令文本中的显式路径；`eval $X`、脚本内部访问、复杂 shell 展开、运行时拼接路径等动态访问不一定能被预检发现。后续结合小模型安全审查 / 提示词注入攻击防御 / 命令意图判定处理，可作为 T-052、T-053、T-061-sec 的细化后继。 |
| T-073 | 多Agent | Agent 插拔机制 | |
| T-074 | 多Agent | 多 Agent 共享记忆/任务进度 | |
| T-075 | 多Agent | 每个 Agent 配一个子 Agent 助手 | |
| T-076 | 多Agent | 工具发现能力 | |
| T-077 | 多Agent | Defer-loading 扩展能力 | |
| T-078 | 多Agent | 非主线任务让子 Agent 干 | |
| T-079-agent | 多Agent | 会话管理参考多 Agent 思路 | |
| T-080 | 多Agent | 智能体邮箱设计 | |
| T-081 | 多Agent | 智能体编排器设计 | 类似 Gas Town |
| T-082 | 多Agent | 多 Agent 开发协作流程 | spec → plan → dev → test |
| T-083 | 多Agent | Agent 独立 VM 运行 | |
| T-084 | 多Agent | 给模型多线程的能力 | |
| T-088 | 计划 | 计划里耗时任务可并行 | 依赖多 Agent |
| T-117 | 愿景 | 群体智能 | |
| T-120 | 愿景 | Agent 公司 | |
| T-121-vis | 愿景 | 发散-收敛讨论收集需求 | |

### P6 — 插件系统（冻结区，仅维护）

| 编号 | 分类 | 条目 | 说明/备注 |
|------|------|------|-----------|
| T-001 | Bug/维护 | VMActor shutdown 命令无效 | 维护性 bug fix |
| T-062 | 插件/VM | 实现所有 shim（pimono SDK） | 冻结 |
| T-063 | 插件/VM | 长生命周期插件注册 | 冻结 |
| T-064 | 插件/VM | 长生命周期 VM 清理机制 | 冻结 |
| T-065 | 插件/VM | VM LRU 清理策略 | 冻结 |
| T-066 | 插件/VM | 初始化搬放 js 配置文件 | 冻结 |
| T-067 | 插件/VM | WAPM .wasm 包加载 | 冻结 |
| T-068 | 插件/VM | 简易沙箱 | 冻结 |
| T-069 | 插件/VM | Wasm 预热 | 冻结 |
| T-070-wasm | 插件/VM | 关闭 LLVM/AOT 预编译 | 冻结 |
| T-133 | 插件/VM | 测试迁移到长生命周期 VM（11 处） | 冻结 |
| T-134 | 插件/VM | 清理弃用 `dispatch_event` | 冻结 |

### P7 — 跨平台（~4 条）

| 编号 | 分类 | 条目 | 说明/备注 |
|------|------|------|-----------|
| T-104-plat | 平台 | 兼容 openclaw 生态 | |
| T-105 | 平台 | 历史 WasmEdge standalone 下载/链接链路清理（已废弃） | 迁移到 `rquickjs` 后不再作为现行能力 |
| T-106 | 平台 | 历史 install-wasmedge.sh 安装脚本清理（已废弃） | 迁移到 `rquickjs` 后不再作为现行能力 |
| T-107 | 平台 | Android 支持 | |

### P8 — 多 IM / 多 LLM 适配（~5 条）

| 编号 | 分类 | 条目 | 说明/备注 |
|------|------|------|-----------|
| T-128 | 商业/IM | 云动态生成操作脚本下发端执行 | 商米 |
| T-129 | 商业/IM | 端接收语音指令 | 商米 |
| T-130 | 商业/IM | 自学习生成软件技能包 | 商米 |
| T-143 | LLM | 多 LLM 适配层（Anthropic/Gemini/local-llm） | 新增 |
| T-144 | IM/网关 | IM 网关（Telegram/Slack/企微/邮件/Webhook） | 新增 |

### P9 — UI / 远期（~12 条）

| 编号 | 分类 | 条目 | 说明/备注 |
|------|------|------|-----------|
| T-119 | 愿景 | 用户每天做了什么功能，反馈回来 | |
| T-122 | 愿景 | 企业级支持 | |
| T-123 | 愿景 | 消除幻觉 | |
| T-124 | 愿景 | Token 管家 | |
| T-125 | 愿景 | Agent 评测平台 | |
| T-126 | 阅读 | 《Accelerando》 | |
| T-127 | 阅读 | 《Functional Programming in Scala》 | |
| T-096-mem | 记忆/愿景 | 用户兴趣推送 | 长期 |
| T-100-dev | 规范 | 编码规范增加面向对象思想 | 与 UI 无关但远期打磨 |
| T-131 | Agent Loop | 可选 ToolLoopGuard / tool-loop-detection | 本期不做；依赖 `max_tool_rounds` + 上下文预算兜底；规格 TODO 见 `docs/architecture/context-management.md` 6.7 段 | 无 T2 映射（远期） |

---

## 一、Bug / 缺陷

- [ ] **[P6] `[BUG]`** `#T-001` VMActor shutdown 命令无效
  - **档位说明**：插件系统冻结区，仅作维护性 bug fix 保留。原报告结论：会话结束靠 `__shutdown` 事件通道，非 `cmd_rx` 上 `Shutdown`；属管道语义/可维护性
  - 已有分析报告：[vm-actor-shutdown-dead-code-analysis.md](reports/vm-actor-shutdown-dead-code-analysis.md)
  - 关联模块：`src/ext/vm_actor.rs`

- [ ] **[P0] `[REF]`** `#T-002` 三套管道混乱，需要重构
  - 档位：并入 T2-P0-009 pipeline-refactor
  - 关联模块：`src/ext/`

---

## 二、交互体验 / TUI

- [ ] **[P0] `[BUG]`** `#T-008` tomcat chat 模式下 workspace 没有切换到当前目录
  - 关联模块：`src/api/chat/`
  - 档位：随 T2-P0-004 / T2-P0-005 落地

- [ ] **[P0] `[UX]`** `#T-009` 进入 chat 模式应把 user turn list 显示在 TUI 上
  - 档位升档自 P2：并入 T2-P0-008
  - 关联模块：`src/api/render/`

- [ ] **[P0] `[UX]`** `#T-010` 状态总览
  - 任务和分文档、其他运行状态
  - 档位：T2-P0-008

- [ ] **[P0] `[UX]`** `#T-011` 时间戳、时间显示
  - 档位：T2-P0-008

- [ ] **[P0] `[UX]`** `#T-012` 编辑模式很难看，需美化
  - 档位：T2-P0-008

- [ ] **[P0] `[UX]`** `#T-013` user content 可以重新输入（编辑/重发）
  - 档位：T2-P0-008

- [ ] **[P0] `[UX]`** `#T-014` 每个文件增加和删减的行数和内容可见（diff 视图）
  - 档位：T2-P0-008

- [ ] **[P3] `[UX]`** `#T-016` 上下文管理视图
  - 目前的上下文每个实体情况
  - 每个 userturn 的 toolresult 总量和被调阅次数
  - 档位：与 P3 记忆 + 系统提示词一起考虑

---

## 三、Agent Loop 与核心循环

- [ ] **[P5] `[REF]`** `#T-021` 任务开始前先做好信息搜集和评估
  - Agent 行为优化，依赖多 Agent 子流程

- [ ] **[P5] `[REF]`** `#T-022` Agent loop 安全设计
  - 档位：并入 P5 安全体系

- [ ] **[P5] `[REF]`** `#T-023` 一心二用：开子 agent 干活，自己继续思考
  - 依赖多 Agent 基础设施

- [ ] **[P9] `[REF]`** `#T-131` 可选 ToolLoopGuard / tool-loop-detection（**本期不做**）
  - 当前：`max_tool_rounds` + 上下文预算兜底；规格侧说明见 `docs/architecture/context-management.md` §6.7
  - 远期若实施：连续同名 tool 阈值、近 N 轮输出相似度、总轮数可配上限等，再评估是否恢复正式 T2 任务卡

---

## 四、会话管理

- [ ] **[P5] `[REF]`** `#T-025` 无法多 session
  - 档位：多 Agent 同档落地

- [ ] **[P5] `[REF]`** `#T-026` 父会话 ID + 对话序号
  - 依赖 T-025

- [ ] **[P5] `[REF]`** `#T-027` 可配置上下文
  - 加入或不加入父会话和记忆

- [ ] **[P5] `[REF]`** `#T-028` 只看问题模式
  - 辅助浏览

- [ ] **[P5] `[REF]`** `#T-029` 无限上下文/会话
  - 长期挑战

- [ ] **[P3] `[REF]`** `#T-030` 每个会话维护一个记忆总结文件

- [ ] **[P3] `[REF]`** `#T-031` 会话上下文可配置
  - 当前会话、父会话、智能体记忆三级可选

- [ ] **[P1] `[UX]`** `#T-146` Feedback 回路落盘（`session.feedback.jsonl`）
  - **档位说明**：T2-P1-005；002 迭代看板已取消，本条目仍保留为 P3 记忆 / P4 自进化（T-142）的原料采集 backlog
  - 目标：捕获用户反馈（👍/👎 + 文字），沉淀到会话 feedback 日志
  - 子项：CLI `/feedback good|bad <text>` 或快捷键；append-only `session.feedback.jsonl`；schema `{turn_id, rating, text, tags?}`；TUI 消息快捷按钮（与 T2-P0-008 一起做）；为 `USER.md` / `MEMORY.md` 预留字段
  - 依赖：T2-P0-008（TUI 强化）
  - 关联模块：`session`、`render`；提供 `FeedbackStore`

---

## 五、工具系统

- [ ] **[P0] `[BUG]`** `#T-035` 默认不会用 tomcat 子进程工具创建目录
  - T2-P0-005

- [ ] **[P0] `[BUG]`** `#T-036` 当前 chat 应默认尝试访问当前目录
  - 如无权限应申请授权；赋权后再申请 4 原语操作权限
  - 档位：T2-P0-005
  - 关联模块：`src/core/primitives.rs`

- [ ] **[P0] `[UX]`** `#T-037` 无法在规划中执行 tomcat CLI 命令
  - 档位：T2-P0-005

- [ ] **[P0] `[BUG]`** `#T-039` 拦截删除操作，换成归档操作
  - 档位：T2-P0-005

- [ ] **[P0] `[REF]`** `#T-153` 按架构文档实现 `web_search` + `web_fetch`
  - 契约与路线图见 [`web_search.md`](architecture/tools/web_search.md)、[`web_fetch.md`](architecture/tools/web_fetch.md)（含 PR-WS-* / PR-WF-*、`openai-responses` 门闩、§2.4.2.1 HTTP 上游字段、cc-fork/hermes/openclaw 对标）
  - 认领时把文档验收矩阵与 `src/core/tools/web_*` 单测对齐
  - 档位：T2-P1-007（占位）；晋升正式卡可走 T2-P1-007 或另拆 T2 子任务
  - 关联模块：`src/core/tools/`（待建 `web_*`）

---

## 六、上下文管理与压缩

- [ ] **[P3] `[REF]`** `#T-045` Token 节省机制
  - 工具结果用完 2 轮后落盘/删除

---

## 七、权限与安全

### 工作目录与授权

- [ ] **[P0] `[BUG]`** `#T-046` 工作目录下读文件不需要授权，写文件可以授权（always / 单次）
  - 档位：T2-P0-004

- [ ] **[P5] `[REF]`** `#T-049` 额外的工作目录需要区分 agent
  - 依赖多 Agent

- [ ] **[P0] `[BUG]`** `#T-050` Bash 访问目录限制和授权
  - 档位升档自 P1：T2-P0-004
  - 通过解析命令来限制

- [ ] **[P0] `[UX]`** `#T-051` 工作目录调整——说话就可以改配置
  - 档位：T2-P0-004

- [ ] **[P0] `[UX]`** `#T-148` `tomcat pathrules remove` CLI 子命令
  - 档位：T2-P0-004 follow-up（PR-10 暂不实施）
  - 当前 path_rules **没有** remove 入口（`config_set` 工具仅 append；CLI 仅 add/list）
  - 设计：`tomcat pathrules remove <path>` 按 path 字符串精确匹配；模糊匹配多条时报错让用户精确指定；命中 builtin 路径时报错"builtin rules cannot be removed"
  - 复用 `with_config_lock` + `remove_path_rule_from_disk()`
  - 关联：[workspace_permission_tiers_design plan §5.x 删除路径](../../../.cursor/plans/workspace_permission_tiers_design_1ad7681a.plan.md)

- [ ] **[P0] `[UX]`** `#T-149` chat `/reload` 配置热加载
  - 档位：T2-P0-004 follow-up（原 PR-11 暂不实施）
  - 用户在另一终端 `tomcat config edit` / `tomcat pathrules add` 改了配置 → 当前 chat 进程用 `/reload` 即时生效，无需重启
  - 改动点：`src/api/chat/mod.rs` chat_loop 增 slash command 解析；`DefaultPermissionGate::reload(new_cfg)` 原子替换 user_config + 重编译 path_rules glob / bash_* regex
  - reload 失败 → 保留旧配置 + 渲染错误提示
  - system prompt **不**重新拼接（避免 LLM context 漂移）；如需 Agent 看新配置应让它调 `config_get`

- [ ] **[P0] `[REF]`** `#T-150` path_rules 双层存储策略（builtin 常量 + TOML 可见性）
  - 档位：T2-P0-004 follow-up（当前实现走"仅代码常量"方案）
  - 现状（PR-1/PR-5 实施时）：`BUILTIN_DEFAULT_PATH_RULES` 仅在代码常量中，TOML 不写入；Agent 通过 system prompt 渲染才能看到 builtin 列表
  - 目标：保留代码常量作安全兜底（不可弱化），**额外**把 builtin 渲染成带 `# managed-by: builtin-defaults` 注释的 TOML 段写入用户 `tomcat.config.toml`，提升用户/Agent 可见性
  - 合并时去重（builtin 路径 + user 自加 → 不重复生效）；用户手编 TOML 删除 managed-by 段不影响代码常量生效（启动时 stderr 提示该段缺失，可选自动补回）
  - 关联：[workspace_permission_tiers_design plan §5 默认 path_rules](../../../.cursor/plans/workspace_permission_tiers_design_1ad7681a.plan.md)

### 安全体系（P5 统一推进）

- [ ] **[P5]** `#T-052` 提示注入攻击检测
- [ ] **[P5]** `#T-053` 行为意图审查策略系统
- [ ] **[P5]** `#T-054` 数据加密（规格中多处 TODO）
  - `docs/openspec/specs/Constitution.md:16` — 敏感数据加密
  - `docs/openspec/specs/User_Stories.md:14,75,179` — 加密存储相关
  - `docs/user-guide.md:586` — 审计日志加密
- [ ] **[P5]** `#T-055` 仅在网络请求边界注入凭证
- [ ] **[P5]** `#T-056` CLI 访问依赖系统钥匙串解密
- [ ] **[P5]** `#T-057` 6 位临时验证码换取 token
- [ ] **[P5]** `#T-058` 绑定本地回环地址
- [ ] **[P5]** `#T-059` 参考 IRONClaw 安全方案
- [ ] **[P5]** `#T-060` 安全参考文章（https://mp.weixin.qq.com/s/tDkwZLkgljozUTAIAVgrcA）
- [ ] **[P5]** `#T-061-sec` 攻击指令拦截 / 攻击用例收集

- [ ] **[P2] `[RES]`** `#T-147` 自我攻击/自我安全进化机制设计
  - 思路：让 Agent 在隔离沙盒里**主动尝试**越权（绕 path_rules / bash_forbidden / config_set 白名单 / sudo 提权 / 凭据读取等），把成功路径自动写回 `bash_forbidden` / `path_rules` 形成"攻防闭环"
  - 启发：红队思路 + Anthropic constitutional AI；与 T-052（提示注入检测）/ T-053（行为意图审查）形成"防御 + 主动检测"对照
  - 设计要点（待研究）：
    - 沙盒环境（Docker / Wasm / chroot）使其攻击不影响真实系统
    - 攻击 prompt 库（已知 jailbreak / 工具滥用 / SSRF / RCE 模板）
    - 成功率统计 + 失败模式聚类
    - 自动生成新规则的安全门：人类 review 必经，避免 Agent 自己加规则把自己锁死
  - 风险：Agent 可能"学会"绕过自己的规则（构造比当前 regex 更绕的命令）；需配合 LLM 行为日志审计
  - 关联：[workspace_permission_tiers_design plan §11 风险表](../../../.cursor/plans/workspace_permission_tiers_design_1ad7681a.plan.md)
  - 优先级释义：**P2 档位**（用户 2026-04-27 标注"优先级不高"），不阻塞 002 迭代

---

## 八、插件 / Wasm / VM（P6 冻结区）

> **档位说明（2026-04-22）**：插件系统整体冻结到 **P6**，仅保留维护性 bug fix（如 T-001）。不在 002 迭代开工。参考 [plugin_skills_first_principles_pi_rust_wasm.md](reports/plugin_skills_first_principles_pi_rust_wasm.md)。

- [ ] **[P6]** `#T-062` 实现所有 shim（pimono 插件 SDK）
- [ ] **[P6]** `#T-063` 加载插件要启动长生命周期
  - 关联报告：[async-handler-in-long-lived-vm.md](reports/async-handler-in-long-lived-vm.md)
- [ ] **[P6]** `#T-064` 长生命周期 VM 的清理机制
  - 关联模块：`src/ext/vm_actor.rs`
- [ ] **[P6]** `#T-065` 维护 10 个 VM 的 LRU 算法清理策略
  - 关联模块：`src/ext/runtime_manager.rs`
- [ ] **[P6]** `#T-066` 初始化时搬放 js 配置文件和其他文件
- [ ] **[P6]** `#T-067` 接受 JS/TS 插件包的加载运行
- [ ] **[P6]** `#T-068` 简易沙箱
- [ ] **[P6]** `#T-069` 插件 VM 预热
- [x] **[P6]** `#T-070` 清理历史 AOT / `.wasm` 预编译链路
  - 迁移后现行运行时为 `rquickjs`，不再维护 LLVM/AOT 预编译链路
- [x] **[P6]** `#T-133` 插件运行时 / JS API 测试迁移到当前入口
  - 真实锚点：`tests/quickjs_e2e_tests.rs`、`tests/long_lived_vm_tests.rs`、`src/ext/plugin/tests/suite_test.rs`
- [ ] **[P6]** `#T-134` 清理已弃用的 `dispatch_event`
  - 来源：`src/ext/instance_rquickjs.rs`

---

## 九、LLM 接入与 Thinking

> T-071（Thinking API + TUI 展示）已随 **T2-P0-006** 落地（`develop.md` 2026-05-08）；本区新增一个多 LLM 产品化收尾条目。

- [ ] **[P8] `[REF]`** `#T-154` 多 LLM 产品化收尾：切 model 不重置 CLI `[ctx]` 统计
  - `/model use` 后，当前 session 的 `[ctx]` token / 占用 / 压缩水位应继续沿用同一份 `ContextState`，不因 provider/model 切换被清零。
  - W2-3 可观测/计量（`scene/provider/api/model/latency/retry` tracing + 按 model 聚合 usage）只允许做“旁路新增”，不得替换 `ContextMetricsUpdate` 或 CLI `stderr` 的 `[ctx]` 现有口径。
  - 若未来要引入按 model 的 tokenizer / context-window 差异，需单独设计 UI 与迁移说明，不能在观测补齐时静默改变当前统计语义。
  - 测试与集成验收参考 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](agents/INTEGRATION_MERGE_AND_ACCEPTANCE.md)；尤其遵循脚本入口、日志留存、后台执行与轮询诊断要求。

---

## 十、多 Agent 协作（P5 统一推进）

> 整体档位：**P5** — 先把单 Agent 做到极致再投入。002 迭代不动。

- [ ] **[P5]** `#T-073` Agent 插拔机制
- [ ] **[P5]** `#T-074` 多 Agent 共享记忆，共享任务进度
- [ ] **[P5]** `#T-075` 每个 Agent 配一个子 Agent 助手
- [ ] **[P5]** `#T-076` 工具发现能力（工具搜索子 agent）
- [ ] **[P5]** `#T-077` Defer-loading 扩展能力
- [ ] **[P5]** `#T-078` 非主线任务让子 agent 干
- [ ] **[P5]** `#T-079-agent` 会话管理也可参考类似的多 Agent 思路设计
- [ ] **[P5]** `#T-080` 智能体邮箱设计（ID-收件箱）
- [ ] **[P5]** `#T-081` 智能体编排器设计（类似 Gas Town）
- [ ] **[P5]** `#T-082` 多智能体的开发协作流程
  - spec → plan → review → dev → test → code review
- [ ] **[P5]** `#T-083` Agent 独立 VM 运行
- [ ] **[P5]** `#T-084` 给模型多线程的能力

---

## 十一、计划与任务管理

- [ ] **[P5] `[REF]`** `#T-088` 计划里的耗时任务可并行
  - 依赖多 Agent / 异步执行基础设施

---

## 十二、记忆系统

- [ ] **[P3] `[REF]`** `#T-093-mem` 记住喜好、记住过往
  - 更好用，方法论总结

- [ ] **[P3] `[REF]`** `#T-094` 是否读取记忆可配

- [ ] **[P4] `[REF]`** `#T-095-mem` 自总结生成 skill
  - 档位：P4 自进化；关联 [plugin_skills_first_principles_pi_rust_wasm.md](reports/plugin_skills_first_principles_pi_rust_wasm.md) §5

- [ ] **[P4] `[REF]`** `#T-142` 自进化学习回路（从 Feedback 生成 SKILL/MEMORY 增量）
  - 参考 [hermes-agent](../../hermes-agent/)
  - 档位：P4 自进化；依赖 `#T-146` Feedback 落盘作为原料
  - 目标：将用户反馈与会话信号增量写入 SKILL / MEMORY，而非整文件重写

- [ ] **[P9] `[UX]`** `#T-096-mem` 记录用户感兴趣的事物，定时推送

---

## 十三、系统提示词与模板

- [ ] **[P3] `[REF]`** `#T-098` 系统提示词文件化
  - 可供大模型修改

- [ ] **[P3] `[REF]`** `#T-099` 系统提示词增加通才描述

---

## 十四、编码规范与开发流程

- [ ] **[P9] `[DOC]`** `#T-100-dev` 编码规范增加面向对象思想优先
  - 档位：远期规范打磨

- [ ] **[P4] `[REF]`** `#T-103` 自举的 AI 编程智能体设计
  - 档位：P4 自进化愿景
  - 极简可读自主进化，少即是多；会话上下文可配置；主要功能插件、用户功能插件、会话级插件

---

## 十五、平台与部署

- [ ] **[P7] `[REF]`** `#T-104-plat` 兼容 openclaw 生态

- [x] **[P7] `[REF]`** `#T-105` 归档历史 WasmEdge 下载与链接方案
  - `rquickjs` 迁移后不再需要 standalone WasmEdge 安装链路

- [x] **[P7] `[REF]`** `#T-106` 归档历史 install-wasmedge.sh 安装脚本
  - 脚本已删除；保留历史背景仅供溯源

- [ ] **[P7] `[REF]`** `#T-107` Android 支持
  - 学操作 SAAS 的 skill；通过 adb 或 Android API 操作

---

## 十六、学习研究

- [ ] **[P4] `[RES]`** `#T-108` 拉 Codex、Harmony 代码
  - 业界最佳实践

- [ ] **[P4] `[RES]`** `#T-109-res` 了解 Beads、Gas Town、anthropic Tasks、Dolt

- [ ] **[P4] `[RES]`** `#T-110` 了解 Candle 推理库

- [ ] **[P4] `[RES]`** `#T-111` 了解 llm-chain 编排库

- [ ] **[P4] `[RES]`** `#T-112` 了解 swarms

- [ ] **[P4] `[RES]`** `#T-113` 了解 coworker

- [ ] **[P2] `[RES]`** `#T-114` 对比 claude code 的 skill 系统
  - skill creator、evals
  - 对齐本仓库结论：[plugin_skills_first_principles_pi_rust_wasm.md](reports/plugin_skills_first_principles_pi_rust_wasm.md) §4–§5

- [ ] **[P2] `[RES]`** `#T-115` 对比 openclaw 的 skill 系统
  - 同上报告 §4–§5

- [ ] **[P4] `[RES]`** `#T-116` 对比 claude code 对 MCP 调用的优化

---

## 十七、远期愿景与商业场景

### 远期愿景

- [ ] **[P5]** `#T-117` 群体智能

- [ ] **[P4] `[RES]`** `#T-118` 每天 surprise me，每周 big surprise idea

- [ ] **[P9]** `#T-119` 用户每天做了什么功能，反馈回来

- [ ] **[P5]** `#T-120` Agent 公司（https://mp.weixin.qq.com/s/XBqK3SQLzX8th6SBMefexA）

- [ ] **[P5]** `#T-121-vis` 发散-收敛讨论收集需求（每轮 1-10 个问题）

- [ ] **[P9]** `#T-122` 企业级支持

- [ ] **[P9]** `#T-123` 消除幻觉

- [ ] **[P9]** `#T-124` Token 管家

- [ ] **[P9]** `#T-125` Agent 评测平台

- [ ] **[P9] `[RES]`** `#T-126` 《Accelerando》阅读

- [ ] **[P9] `[RES]`** `#T-127` 《Functional Programming in Scala》阅读

### 商米场景（P8 IM 统一推进）

- [ ] **[P8]** `#T-128` 云动态生成操作脚本下发端执行
  - 端自动生成脚本自执行

- [ ] **[P8]** `#T-129` 端接收语音指令
  - 操作对应软件，每个软件有自己的操作技能包

- [ ] **[P8]** `#T-130` 自学习生成软件技能包
  - 页面/功能路径地图；操作点：名称、类型、ID、描述等

---

## 十八、本次新增条目（2026-04-22 随 P0-P9 改造）

| 编号 | 档位 | 条目 | 来源 / 关联 |
|------|------|------|-------------|
| T-138 | P2 | Skill 注册/发现机制 | [plugin_skills_first_principles_pi_rust_wasm.md](reports/plugin_skills_first_principles_pi_rust_wasm.md) §4 |
| T-139 | P2 | Skill 工作流引擎（串/并/评审） | 同上 §5 |
| T-140 | P3 | USER.md 加载注入 | 参考 claude-code；个性化规则 |
| T-141 | P3 | MEMORY.md 加载注入 | 跨会话长程记忆 |
| T-142 | P4 | 自进化学习回路（从 Feedback 生成 SKILL/MEMORY 增量） | 参考 [hermes-agent](../../hermes-agent/) |
| T-143 | P8 | 多 LLM 适配层（Anthropic / Gemini / local-llm） | `src/core/llm/` 解耦 |
| T-144 | P8 | IM 网关（Telegram / Slack / 企微 / 邮件 / Webhook） | 新增 |
| T-146 | P1 | Feedback 回路（session.feedback.jsonl） | T2-P1-005；为 P3/P4 铺垫 |

### 同步变更（本次改造一并完成）

- ~~建 `docs/openspec/specs/archive/`~~（目录已删除；历史见 Git）
- 建 `docs/agents/TASK_BOARD_002/` 目录化看板（吸收迭代立项；索引 `README.md` + `tasks/`）
- `docs/openspec/specs/Product_Brief.md` 重写为 P0-P9 路线图
- T-135（Product_Brief 产品级 TODO）关闭

---

## 十九、本次新增条目（2026-04-27 随 T2-P0-004 plan review）

| 编号 | 档位 | 条目 | 来源 / 关联 |
|------|------|------|-------------|
| T-147 | P2 | 自我攻击/自我安全进化机制设计（红队思路） | T2-P0-004 plan review；与 T-052/T-053/T-061-sec 形成防御+主动检测对照 |
| T-148 | P0 | `tomcat pathrules remove` CLI 子命令 | T2-P0-004 PR-10 follow-up；首版仅 add/list |
| T-149 | P0 | chat `/reload` 配置热加载 | T2-P0-004 原 PR-11；暂不实施 |
| T-150 | P0 | path_rules 双层存储（builtin 常量 + TOML 可见性段） | T2-P0-004 PR-1/PR-5 follow-up；当前仅常量 |
| T-151 | P5 | Bash 动态路径访问与提示词注入防御 | gate-root-remediation follow-up；`bash_parser` 对运行时拼接路径只能尽力解析 |
| T-153 | P0 | `web_search` + `web_fetch` 按 [`web_search.md`](architecture/tools/web_search.md) / [`web_fetch.md`](architecture/tools/web_fetch.md) 落地 | T2-P1-007 占位；HTTP 字段见 web_search §2.4.2.1 |

---

## 附录：Agent 犯的错（经验教训）

> 记录 Agent 在开发过程中犯过的典型错误，作为未来改进和规范制定的参考。

1. **重复的代码，要抽取复用** — 应在 code review 阶段检测
2. **没有按研发流程来** — 应强制 spec → plan → review → dev → test 流程
3. **事件没有用常量来表示** — 应在编码规范中要求事件名用常量定义
4. **任务少做，验收也漏了** — 应强化验收 checklist 和完整性检查

---

## 统计（P0-P9 档位分布）

| 档位 | 条目数（估） | 说明 |
|------|--------------|------|
| **P0** | ~19 | 单 Agent 基础体验；开放 T2：T2-P0-008/009；含 T-148/T-149/T-150 follow-up、T-153（web 工具） |
| **P1** | ~0 | 状态管理；Plan/Checkpoint/集成规范相关 T2 均已 DONE |
| **P2** | ~5 | Skill 系统（T-114/T-115/T-138/T-139）+ 安全研究（T-147） |
| **P3** | ~10 | 系统提示词 + 记忆 |
| **P4** | ~12 | 自进化 / 学习 / 业界研究（含 T-142/T-146 Feedback 回路） |
| **P5** | ~35 | 多 Agent + 安全 + 多会话 |
| **P6** | ~12 | 插件系统（冻结，仅维护） |
| **P7** | ~4 | 跨平台 |
| **P8** | ~5 | 多 IM / 多 LLM 适配 |
| **P9** | ~11 | UI / 远期愿景 / 阅读 |
| **合计** | **~106** | 详细清单 `[ ]` 条目数（含 T-142/T-146 Feedback 恢复） |

### 与前一版（P0-P5 紧急度档）变更一览

| 前一版档位 | 新档位 | 备注 |
|-----------|--------|------|
| P0（破损/不可用） | → P0（单 Agent 基础体验） | 范围扩大：吸收 Agent Loop 拆分 / Thinking / TUI 等 |
| P1（高价值） | → P0 或 P1 | 多数与基础体验有关的升到 P0；状态管理类留 P1 |
| P2（增强） | → P0~P6 | 按主题重新分派；TUI 升 P0，插件冻结到 P6 |
| P3（远期） | → P3~P5 | 安全/多 Agent 到 P5；记忆到 P3 |
| P4（探索） | → P4~P9 | 学习研究到 P4；愿景到 P9 |
| P5（灵感/阅读） | → P4/P9 | 研究类到 P4；阅读到 P9 |
