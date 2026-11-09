# Multica 对技术方案质量的提示词与上下文影响

> TOMC-2 源码调研报告 · 复核修订版
>
> 初稿由「全栈攻城狮」通过 Multica → Tomcat 执行并交付；本会话复核源码后，修正了负向结论的范围、质量归因、PLAN 状态判定与评审权限。首版附件保留在任务历史中，阅读时请以本修订版为准。

## 1. 结论先行

**Multica 确实会给 Tomcat 增加提示词和上下文，但本次在本地运行时 brief、内置技能及默认工作流路径中，未发现专门教模型「怎样写技术方案」的内置模板。Tomcat 自身反而有专门的 `write-plans` 技能。**

这不等于「Multica 的提示词对质量没有影响」。它提供的角色、任务阅读流程、协作约束和交付要求，都可能改变模型的行为。**目前不能证明你觉得更好的那次方案，是由哪一项因素造成的，也不能排除这些通用提示词的作用。**

```text
Multica 增加的内容                       Tomcat 自带的内容
------------------                     ------------------
智能体角色 / 工作区背景                 证据优先、图解、面向读者等常驻要求
读 issue、读评论、按任务边界工作          工具、仓库规则和技能发现
项目资源 / 小队协议 / 技能指针            PLAN 提醒 + write-plans + 可选评审
必须通过评论和附件交付结果                         |
             |                                    |
             +----------------+-------------------+
                              v
                 实际模型请求与技术方案产物
                              |
                 质量还受模型、输入、历史、
                 有效参数和单次生成差异影响
```

对你最关心的三个问题：

1. **是不是 Multica 有一个特别强的「写技术方案」提示词？** 本次检查的内置路径中未发现，不能据此断言所有用户配置、插件或其他版本都没有。
2. **Multica 有没有额外引导？** 有，主要是角色、任务上下文、协作与交付纪律，而不是固定技术方案章节。
3. **怎样在单独使用 Tomcat 时复用？** 先明确交付物、读者、源码范围和证据要求；需要正式可执行计划时使用 PLAN。不要把「更多模板」直接等同于「更好方案」。

## 2. 范围与版本

| 对象 | 本次核对范围 |
|---|---|
| Multica | `/Users/sm0007/AndroidStudioProjects/multica`，HEAD `de2c11c57af49b4b94c90dd49de130ee3a876e46`，分支 `feat/tomcat-agent` |
| Tomcat | `/Users/sm0007/AndroidStudioProjects/Tomcat`，HEAD `e341c2f004d371e4387eb4fdc8b8c85416acb856`，分支 `feature/multica` |
| 当前工作区 | `tomcat-agent`，ID `b0def236-dc75-4b20-ad38-6751c7ad1cfb` |
| 案例 | TOMC-1「写一份技术方案」；本次调研任务为 TOMC-2 |
| 修改边界 | 不修改两仓产品代码或配置；新增并复核这一份报告，不提交、推送、安装或重启服务 |

源码路径以下均以 **Tomcat 仓库根目录**为基准；`../multica/` 指同级 Multica 仓库。

开始核查时，Tomcat 已有五个版本文件的未提交改动：`release-versions.json`、扩展的 `package.json` / `package-lock.json`、Rust 的 `Cargo.toml` / `Cargo.lock`；Multica 已有 `apps/web/next-env.d.ts` 改动。它们不是本报告的变更。任务执行还使用了 Multica 管理的 `AGENTS.md` / `.multica/` 上下文，不将其当作手写产品功能。

**证据层次必须分开：** 当前源码说明当前实现；当前配置说明现在的设置；历史会话头和索引说明其记录的事实。三者不能自动互相替代，尤其不能把当前配置冒充历史完整请求快照。

## 3. Multica 实际增加了什么

### 3.1 运行时 brief：有引导，但本次未发现技术方案专用模板

主要拼装入口是 `../multica/server/internal/daemon/execenv/runtime_config_sections.go:914` 的 `buildMetaSkillContentSlim`，**运行在本机 daemon 的执行环境准备路径中，不是在服务端把最终 brief 拼好后发下来**。

它会组合以下内容：

| 内容 | 源码证据 | 与方案质量的关系 |
|---|---|---|
| 智能体名称、ID 与 `instructions` | `../multica/server/internal/daemon/execenv/runtime_config_sections.go:66-85` | 设定角色；若用户写入设计方法，也能带来专用引导 |
| 用户配置的工作区背景 | `../multica/server/internal/daemon/execenv/runtime_config_sections.go:134-143` | 任意用户配置不能用默认模板搜索排除 |
| 项目描述与资源 | `../multica/server/internal/daemon/execenv/runtime_config_sections.go:945` | 提供上下文、代码入口，不等于方案章节模板 |
| 按任务类型选择工作流 | `../multica/server/internal/daemon/execenv/runtime_config_sections.go:951-961` | 决定读任务、读评论、交付和状态维护流程 |
| 技能指针、平台 CLI 和输出契约 | `../multica/server/internal/daemon/execenv/runtime_config_sections.go:970-978` | 影响操作方式、成果可见性和交付形式 |

对这个 981 行文件做不区分大小写的 `plan|design|方案|计划` 检索，命中两行：

- `:613`：`research, design, planning, review` 用于说明活动类型不决定 issue 状态。
- `:881`：`Do NOT post progress updates or plans along the way` 用于限制中途评论。

这两处不是技术方案写作方法。**UNVERIFIED：本次在该 brief 的静态正文中未找到技术方案专用写作模板；这项检索不能排除动态注入的 `instructions`、工作区背景或技能正文。**

### 3.2 内置技能与工作流的边界

本地 `../multica/server/internal/service/builtin_skills/` 下有 `multica-platform` 和 `multica-onboarding`；前者是平台契约与操作手册，后者面向 Mika 的上手流程。技能还可叠加用户配置，见 `../multica/server/internal/service/builtin_skills.go:24-50`、`:68-100`。旧技能重定向另由 `builtin_skills_legacy` 处理。

默认 issue 状态清单是 backlog / todo / in_progress / in_review / blocked / done / cancelled，见 `../multica/server/cmd/multica/cmd_issue.go:511-517`。**默认状态名称不是方案写作方法，也不能据此排除用户自建工作流或技能。**

因此，本次范围内的合理结论是「未发现内置技术方案专用模板」，不是「Multica 不可能指导写方案」。

## 4. 从任务到 Tomcat 的真实调用链

```text
Multica Server
  保存 issue、评论、agent 配置、项目、技能与小队数据
                         |
                         v 任务 claim 数据
本机 Multica daemon
  buildMetaSkillContentSlim 组合运行时 brief
    + 智能体 instructions / 工作区背景 / 项目 / 工作流 / 技能指针
                         |
                         +--> 写任务目录 AGENTS.md
                         +--> 准备平台技能及项目资源 sidecar
                         |
                         v
Tomcat adapter 启动 tomcat acp
  initialize
    -> session/new（恢复时为 session/load）
    -> session/set_config_option：model（配置了模型时）
    -> session/prompt：本轮任务消息
                         |
                         v
Tomcat 发现 AGENTS.md，纳入 User Custom Instructions
  + Tomcat 常驻提示 + 其他实际适用规则 / 技能
  + 当前运行模式对应的提醒
                         |
                         v
模型读取 issue 和必要评论，调查代码，生成并交付方案
```

关键接缝与证据：

| 接缝 | 源码 | 需要注意的事实 |
|---|---|---|
| 生成、写入 brief | `../multica/server/internal/daemon/execenv/runtime_config.go:185-192`、`:221-222`、`:254` | Tomcat provider 的文件目标是任务目录 `AGENTS.md`；已有用户正文按受管区块机制保留 |
| 启动 ACP | `../multica/server/pkg/agent/tomcat.go:53-56` | 显式传入工作目录，启动 `tomcat acp` |
| 会话与模型 | `../multica/server/pkg/agent/tomcat.go:197-247` | 支持新建或恢复会话；当前代码显式设置的是模型 |
| 本轮任务消息 | `../multica/server/pkg/agent/tomcat.go:249-253` | 注释明确：instructions / skills 已在文件中，不把 SystemPrompt 再重复发送成用户消息 |
| 任务正文获取 | `../multica/server/internal/daemon/prompt.go:317-335` | assignment 消息给 issue 指针，并要求读取 issue / 评论；不是把全部需求正文直接复制进 ACP 消息 |
| Tomcat 发现文件 | `tomcat/src/core/project_agents.rs:13-67` | 沿仓库内祖先目录发现适用 `AGENTS.md`，受读取权限检查约束 |
| 渲染规则 | `tomcat/src/core/project_instructions.rs:397-447` | 纳入 `## User Custom Instructions`，带来源标识；读取失败会产生诊断 |

**文件注入与 ACP 文本是两条配合的路径。** 不能说 Multica 什么都没传，也不能把完整 brief 当成 ACP 的独立 system-message 参数。文字要求与运行时权限守卫是不同层面，不能凭拼接顺序推断谁取得了安全权限。

## 5. Tomcat 自己的方案引导

### 5.1 常驻要求

Tomcat 的常驻提示本来就包括：

- 证据优先、诚实、第一性原理、先整体图解、让零背景读者读懂：`tomcat/src/core/prompts/templates/system/core_identity.txt:3-14`。
- 调查结论需要证据，负向发现不能裸写为全局不存在：`tomcat/src/core/prompts/templates/system/verification.txt:1-7`。
- 收尾验证及分层发现测试命令：同文件 `:9-54`。

这些是两个调用路径都可能继承的基础能力。**方案里出现图解或源码引用，与这些提示要求一致，但文本相似不等于已经证明了唯一成因。**

### 5.2 PLAN 默认引导与 `write-plans`

`tomcat/assets/skills/write-plans/SKILL.md:8-11` 要求先读计划工作流、写作标准与结构，写完再做 final check。它的附件提供章节组织、设计取舍、测试与验收等专门方法。

`tomcat/src/api/chat/run_loop/workspace_state.rs:25-35` 只在模式为 Plan 时追加 planner 提醒；技能名从常驻清单中过滤，见 `tomcat/src/core/skill/catalog.rs:104-110`。

**但这不是「技能被锁在 PLAN 中」。** `catalog.rs:6-8` 明确说明过滤不限制发现、显式加载或用户清单；`tomcat/src/api/chat/commands/cmd_skill.rs:70-103` 支持按名使用技能。

也要区分两件事：

```text
加载一段写作技能文本 != 切换运行时模式
切入 PLAN           != 已经创建了计划文件
```

`write-plans` 的工作流附件以 PLAN 为前提，见 `tomcat/assets/skills/write-plans/references/tomcat-plan-workflow.md:3-14`。因此不建议在 Chat 中手动加载它以后，就假定 `create_plan`、计划写入权限或提问能力也自动就绪。需要正式 PlanFile 时，使用正常 PLAN 入口。

### 5.3 可选评审不是「完全不能改稿」

Tomcat 有独立方案评审机制。其工具面包括 `read` / `search_files` / `list_dir`，也包括 `edit` / `update_plan`，且生产改稿开关启用：`tomcat/src/core/plan_runtime/plan_reviewer.rs:9-19`。

评审提示明确：源码调查只读，但可在守卫限制下修改**目标计划正文和 todos**，并非允许任意改产品代码；见 `tomcat/src/core/prompts/templates/reviewer/plan_review.txt:67-80`。因此应称「受限评审、允许计划改稿」，而不是「绝对只读评审代理」。

## 6. TOMC-1：能证明什么，不能证明什么

### 6.1 已核对的案例事实

| 项目 | 证据与结果 |
|---|---|
| 原需求 | TOMC-1：「写一份技术方案，目标是在 multica 可以切换 tomcat 的模式（Chat 或 Plan）」 |
| 指派与执行 | issue 指派给小队；运行 `01a1b18e-06cf-7069-8cd8-533d7027247c` 由「全栈攻城狮」执行，状态 completed |
| 会话 ID | `1793980238107_27f197c8e8db6c4d`，来自该运行的 `result.session_id` |
| 调用来源 | 会话头 `origin.kind=acp`、`clientName=multica-agent-sdk` |
| 当时记录的模型 | 会话第二条记录 `model_change.modelId=deepseek/deepseek-flash` |
| 当时项目关联 | TOMC-1 的 `project_id` 为 null，不能把本次 TOMC-2 新项目的上下文当作它当时已拥有的上下文 |
| 当前索引记录 | schema 5；`agent_mode`、`active_plan_path`、`active_plan_id`、`latest_plan_event` 均为 null；索引记录的 transcript 大小与实际文件均为 676297 字节 |

历史记录文件：`/Users/sm0007/.tomcat/agents/main/sessions/1793980238107_27f197c8e8db6c4d.jsonl` 及同名 `.resume-index.json`。核对只提取来源、模型与控制态字段，没有把隐藏思考内容作为报告交付。

当前 CLI 配置另行核对为：智能体 instructions 是「你是全知全能全栈工程师」，模型为 `deepseek/deepseek-flash`，`thinking_level` 为空，用户分配的 `skills=[]`；工作区 `context=null`。**这些是当前配置，不是 TOMC-1 的历史配置快照；空思考等级也不能自动代表两条路径的实际模型参数相同。**

### 6.2 当时是否进入了 PLAN？

**现有证据与默认 Chat 路径一致，未取得它实际启用 PLAN 专属工作流的证据。**

依据是：当前 adapter 的完整请求路径（`../multica/server/pkg/agent/tomcat.go:184-258`）设置模型并发送任务，没有在这里发起模式切换；Tomcat 新运行时默认 Chat（`tomcat/src/core/plan_runtime/mod.rs:365-397`）；该案例的索引也没有记录模式或活动计划绑定。

但不能过度解读：

- `ResumeControlState.mode=None` 的源码说明包括「旧版本 / 全量扫描路径中没有这项信息」，见 `tomcat/src/core/session/manager/types.rs:277-284`。不能只看 null 就宣布历史上绝对没有模式切换。
- 同文件 `:242-245` 明确说 `plan.enter` 时计划文件往往还不存在。因此「没有计划文件」不能独立证明「没有进入 PLAN」。
- 交付物是普通评论也不是独立的模式证明；PLAN 草稿可以被引用到其他交付界面。
- `session/load` 是恢复路径，不能把所有恢复会话都一概视为新建 Chat。
- 技能未被常驻清单展示，不等于历史上不可能显式加载过它；本次未逐请求证明 TOMC-1 的所有有效提示文本。

所以，应区分「当前 adapter 没有接入显式模式切换」这一实现事实，与「所有历史会话从未进过 PLAN」这一更强、未验证的断言。

## 7. 为什么感觉更好：候选解释，而不是已定根因

| 候选因素 | 有什么依据 | 还缺什么 |
|---|---|---|
| 任务和评论形成较清楚的上下文 | Multica 提示要求先读 issue 和必要评论 | 两次任务输入、历史上下文的匹配对照 |
| 读者与交付约束不同 | Multica 要求通过评论、附件交付可读成果 | 不能仅凭这条要求证明实际质量改善 |
| 角色、小队、技能与工作区设置 | 有明确的注入机制，TOMC-1 还是小队指派路径 | 当时完整配置快照；当前名册不能代替历史成员状态 |
| 模型、参数与运行环境 | TOMC-1 的模型记录已核对 | 用户所指 standalone 会话的模型、有效思考参数、版本和工具权限 |
| 写作骨架的适配程度 | Tomcat 确有详细计划写作技能 | 尚未定位 standalone 那次，不能认定模板刚性造成了它的不足 |
| 单次生成差异 | 本次没有做重复对照实验 | 不能估算波动幅度，也不能给这些原因排可信度顺序 |

**两种相反的归因都不成立：** 既不能说「已证明 Multica 的提示词让它更好」，也不能说「已证明这些好特征只来自 Tomcat，Multica 提示词没有贡献」。本次确认的是机制与适用边界，不是质量的因果实验。

用户所指的 standalone 方案会话尚未确认；完整历史模型请求快照也未获取。没有这些材料，就不宜改造任一产品的提示词来“修复”尚未证实的根因。

## 8. 可以立即复用的最小做法

### 8.1 只是要一份给人看的技术方案

无需改代码、配置或产品提示词，先把任务写清楚：

```text
请基于 <仓库路径 / revision> 写一份面向零背景读者的技术方案。
目标：<一句话目标>。

先只读调查代码；给出结论、整体调用链图、关键证据和最小方案。
关键判断引用 file:line，区分已证实、推断和未验证。
说明取舍、风险与验收方法；深度与需求规模匹配，不强行堆章节。
本次不实现、不提交；将方案交付到 <回复 / 指定文档>。
```

这段建议复用了两边可观察的纪律，但**尚未验证能稳定复现 TOMC-1 的主观质量**。

### 8.2 要正式、后续可执行的计划

通过 Tomcat 正常入口进入 PLAN，使用 `write-plans` 的完整工作流，保存后按需要选择独立评审。只有用户明确要求执行时，再开始实施。

如果只想参考其中的写作方法，可以显式阅读技能文本；不要把“加载文本”当成运行时已经切模式，也不要为了写一份普通报告自动启动计划执行。

### 8.3 可选的最小对照，本任务未运行

先比较两条 **Chat 路径**，不要同时把 PLAN 模式也混进来：

1. 固定同一模型、有效参数、源码版本、需求文本和读权限；记录两边实际适用的角色、技能、规则与历史。空配置值不等于有效参数已经对齐。
2. A：单独 Tomcat Chat；B：Multica → Tomcat Chat，需求正文相同，将 Multica 注入的差异记录下来。
3. 先各一份检查证据准确性、目标覆盖、可理解性、取舍和可验收性。单对样本只作探索；需要判断稳定差异时再重复，不直接宣称因果成立。

本任务没有为此额外调用模型，也没有自动改变任何设置。

## 9. 复核与交付说明

本修订版特别修正了初稿的四类问题：

- 将「Multica 全局没有」改为本地已检索路径中的有界负向发现，保留动态配置和外部技能边界。
- 将「write-plans 只在 PLAN 能用」改为默认 PLAN 提醒与显式加载的区别。
- 不再用空索引字段、无计划文件或评论形式，分别独立证明全部模式历史。
- 修正 brief 的 daemon 组装位置、评审代理可受限改稿的权限，并移除未经对照的唯一质量归因。

验证范围为报告内容、关键源码引用、案例元数据、变更边界与附件交付；这是文档调研，不将源码阅读冒称为代码测试或新版本行为验收。未运行产品编译、全量测试或新的质量对照模型实验。
