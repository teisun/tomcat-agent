# Skill 系统：声明式技能的发现、渐进式披露与按需装载

本文是 **tomcat Skill 系统** 的技术方案（OpenSpec **架构子系统类**：`docs/architecture/`，与 `plan-runtime.md` / `plugin-system-overview.md` 同级）。Skill 系统跨 `core/skill`（新模块）、`core/llm/system_prompt`、`core/tools/contract/catalog`、`core/agent_loop/tool_exec`、`infra/config` 五处一级落点，触发 `[ARCHITECTURE_SPEC.md](../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)` §1 第 1/2/3 条「跨 ≥2 子目录 + 新增工具/事件契约 + 新生命周期」，故单开一份架构文档。

---

## 先看总图：ASCII 方案总图

```text
┌─ 触发点 / 输入 ─────────────────────────────────────────────────────────────────────────┐
│ • 启动期：tomcat chat 装配（run_loop）→ 后台 spawn 发现任务扫描技能根（不堵启动）          │
│ • 每轮对话：用户输入 + 会话上下文 进入本轮 prompt 构造                                      │
│ • 用户主动（可选）：/skill use <name> "intent..." 直接点名展开某条技能并补充本轮意图；也可 /skill list、/skill reload │
│ 〔说人话〕开机先把技能找一遍；之后每一轮，把「有哪些技能」连同你的问题一起发给模型。         │
└───────┬─────────────────────────────────────────────────────────────────────────────────┘
        │
        ▼  ① 发现 / 编目（启动后台 spawn 扫一次 · 首次披露前 await）
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│ 专业：启动后台 spawn 发现任务；discovery.rs 只扫三层磁盘根 P0>P1>P2、同名 first-wins 去重 →  │
│      catalog.rs 建成 SkillSet；官方内置 skill 文件放 `tomcat/assets/skills/`，编译后由 init   │
│      写入 P2 Managed；首次渲染 <available_skills> 前 await 该任务完成（不堵启动）             │
│      P0 Project(<cwd>/.tomcat/skills) > P1 Agent(~/.tomcat/agents/<id>/skills) >            │
│      P2 Managed(~/.tomcat/skills，含 init 写入的官方内置 skill)；有界：深度1·只读frontmatter │
│ 说人话：开机后台先把技能翻一遍，第一次要用前等它扫完；出厂自带 skill 先由 init 放进全局托管   │
│      目录，再和其他 managed skill 一起被发现；发现期只读「卡片」(frontmatter)，不读正文。    │
│ 伪代码：SkillSet { by_name: Map<name, Skill>, diagnostics, warnings }（内存即缓存，不落盘） │
│        Skill 只存元数据(name/description/file_path/source)，不读正文 → 省 token            │
└───────┬─────────────────────────────────────────────────────────────────────────────────┘
        │ SkillSet（仅元数据，常驻进程；更新=显式 /skill reload，热重载列 P2）
        ▼  ①′ 选择层（可选 · P2 · 本期不做）：技能爆量时先筛 top-n 再披露
┌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┐
╎ 专业：仅当 SkillSet 很大时启用。起一个「筛选子 Agent」——无历史、仅系统提示定义 I/O 格式，   ╎
╎      上下文 = 用户输入(去历史) + 全量技能目录账本；唯一可见工具 filtered_skills(names) 回传 ╎
╎      top-n(n<5)；程序再 ∪ must_include(显式点名/置顶/最近用) 去重，填回 <available_skills>     ╎
╎ 说人话：技能多到塞不下时，先让一个「专职筛选小模型」挑出最相关的几条，再连同必留项交给主模型。╎
╎ 护栏：筛选只决定「展示哪几张卡」，最终用哪条仍由主 LLM 定；must_include 永不被筛掉。          ╎
└╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┘
        │ （未启用时：SkillSet 直接全量进 ②）
        ▼  ② 渐进式披露：把「技能目录」塞进系统提示（正文不进）
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│ 专业：render_available_skills_block() 把 SkillSet 渲成 <available_skills> 注入 system_prompt│
│      过滤 disable_model_invocation（仅用户可用）；预算 = max(2000, 窗口字符×1%)，超限两级降级：│
│      〔A〕每条描述截 max_description_chars(250) →〔D〕整块仍超→全员只留 name（丢描述不丢技能）│
│ 说人话：只把「有哪些技能、各自干嘛」这张目录卡片给模型，技能正文一律不进；技能多到挤爆预算时，│
│        先把描述截短，再不够就只留技能名——宁可描述短，也保证每条技能都还看得见。            │
│ 伪代码：<available_skills><skill><name/><description/></skill>…</available_skills>           │
└───────┬─────────────────────────────────────────────────────────────────────────────────┘
        │ system prompt（含技能目录，无正文）+ 用户输入  ──►  一起发给 LLM
        ▼  ③ LLM 自主决策（关键：由模型挑，不是系统自动匹配）
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│ 专业：LLM 读完 <available_skills> + 用户请求，自行判断这次要不要用、用哪条技能              │
│      • 用不上  ─► 正常回答 / 调别的工具（技能链路到此结束）                                  │
│      • 要用某条 ─► 发起 load_skill(name[, file]) 工具调用                                    │
│ 说人话：模型自己看着办——用不上就不碰；要用就报技能名去调正文。是模型选，系统不替它猜。      │
└───────┬─────────────────────────────────────────────────────────────────────────────────┘
        │ load_skill(name)
        ▼  ④ 按需装载：把「那一条」技能的完整正文拉进上下文（工具调用期）
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│ 专业：tool_exec/branches/load_skill.rs：guard 门控(reviewer/verifier 默认拒) →             │
│      SkillSet.resolve(name) → gate_check_path(Read) → 读 SKILL.md 去 frontmatter →          │
│      包成 <skill>…</skill> 作为 tool 消息回灌 LLM（未知名/越权 → 结构化错误，列出可用技能名）│
│ 说人话：按名字找到这条技能，过一遍权限安检，把「这一条」的完整正文取出来交给模型。          │
└───────┬─────────────────────────────────────────────────────────────────────────────────┘
        │ <skill> 正文进上下文
        ▼  终局：执行（技能零特权）
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│ 专业：LLM 按 SKILL.md 指令执行；正文里要求的 read/write/bash/… 仍逐次过 PermissionGate       │
│ 说人话：技能只是「教程」，真动手做的每一步照样过权限闸门，技能本身没有任何额外特权。        │
└───────────────────────────────────────────────────────────────────────────────────────────┘
```

**看图顺序（说人话）／四步心智模型**：①**发现 / 编目**——技能在启动时按 P0→P2 三层磁盘根（project `.tomcat/skills` → agent `~/.tomcat/agents/<id>/skills` → managed `~/.tomcat/skills`）被「发现」成一张只含名字+描述的目录卡片表（不读正文，省 token），去重后成 `SkillSet`；官方内置 skill 文件不再单列 P3，而是由 `tomcat init` 从 `tomcat/assets/skills/` 写入 P2 Managed；②**渐进式披露**——这张卡片表每轮通过一个系统提示 section 注入给模型，连同用户输入一起发出，让模型知道「有哪些技能、什么时候该用」（正文不进）；③**LLM 自主决策**——模型看完目录和用户请求，自己判断要不要用、用哪条；用不上就不碰，要用就发起 `load_skill(name)`；④**按需装载 → 执行**——`load_skill` 过 guard + 权限闸门，把那一条的 **完整正文** 拉进上下文，模型照着做；正文里要做的事仍逐次过权限闸门，技能零特权。

> **一句话抓住心智模型**：**发现/编目 → 渐进式披露 → LLM 决策 → 按名装载执行**（贯穿一条"技能零特权"安全底座）。注意顺序是**串行**：先把「技能目录」注入 prompt 发给模型，**模型决定**后才 `load_skill` 取正文——不是系统并行地预先装载。这条「全量披露目录 + 由模型自己挑」与主流一致：codex / openclaw / pi 同样把所有技能元数据注入、让模型决策，预算靠**压缩描述 / 降级 / 截断**而非系统侧筛选；唯一在系统侧做自动匹配的是 GenericAgent（服务端**语义检索** over ~10 万技能卡，返回 top-K 再交模型挑，见 §2.2）。tomcat 取同样的「披露给模型、模型决策」路线：更可审计、误命中可控；运行时只认三层磁盘根 P0→P2，官方自带 skill 先由 `tomcat init` 写入 `~/.tomcat/skills/` 再参与发现；**①′ 选择层**（技能爆量时起一个**筛选子 Agent** 挑 top-n 再披露）列为 P2 后续增强、**本期不做**（仅当技能规模爆预算 / 稀释注意力时启用，且 `must_include` 永不被筛掉、由主模型做最终决策，见 §13）。预算超限本期用 **A+D 两级降级**（截描述 → 降级 name-only，丢描述不丢技能）。

---

## 目录

- [1. 术语统一](#1-术语统一)
- [2. 竞品 / 选型对比（调研）](#2-竞品--选型对比调研)
- [3. 目标与设计原则](#3-目标与设计原则)
- [4. 落地选型与实施（已定稿）](#4-落地选型与实施已定稿)
  - [4.1 落地选型决策表（维度取舍）](#41-落地选型决策表维度取舍)
  - [4.2 实施点（路线图）](#42-实施点路线图)
- [5. 协议（SKILL.md / load_skill / available_skills / 配置）](#5-协议skillmd--load_skill--available_skills--配置)
- [6. One-Glance Map（文件职责总览）](#6-one-glance-map文件职责总览)
- [7. 调度时序（运行时图）](#7-调度时序运行时图)
- [8. 状态机（技能解析与发现去重）](#8-状态机技能解析与发现去重)
- [9. 配置与环境变量](#9-配置与环境变量)
- [10. 错误模型 / 截断 / 警告](#10-错误模型--截断--警告)
- [11. 测试矩阵（验收）](#11-测试矩阵验收)
- [12. 风险与应对](#12-风险与应对)
- [13. 历史决策 / 跨文档修订](#13-历史决策--跨文档修订)
- [附录 A：下期官方内置 skill 资产候选（PR-SK-B）](#附录-a下期官方内置-skill-资产候选pr-sk-b)

---

## 1. 术语统一


| 术语                                | 语义（大白话）                                                | 数据载体                                                                                 | 行为约束                                                                                                          | 说人话                                 |
| --------------------------------- | ------------------------------------------------------ | ------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------- | ----------------------------------- |
| **Skill（技能）**                     | 一份「专项任务说明书」：YAML frontmatter + markdown 正文，告诉模型某类任务怎么做 | 磁盘上 `<name>/SKILL.md`；进程内 `core::skill::model::Skill`（**只存元数据，不缓存正文**）               | 单一 `<name>` 目录含一个 `SKILL.md`；正文按需读，不进发现期内存                                                                    | 一个文件夹一个 `SKILL.md`，是给模型看的操作手册。      |
| **frontmatter**                   | `SKILL.md` 顶部 `---` 包裹的 YAML 头                         | `core::skill::frontmatter::SkillFrontmatter`                                         | `name`+`description` 必填；其余字段走 `ALLOWED_SKILL_FRONTMATTER` 白名单，未知键忽略                                           | 文件开头那段元信息，至少要有名字和描述。                |
| **渐进式披露（progressive disclosure）** | 先只把「名字+描述」喂给模型，正文等模型决定用时再读                             | 元数据：system_prompt `<available_skills>`；正文：`load_skill` tool 返回                       | 元数据常驻 prompt（预算封顶）；正文 **只在 `load_skill` / `read` 时** 进上下文                                                     | 先给目录，要看哪章再翻哪章，别一上来塞整本书。             |
| **skill catalog（可用技能清单）**         | 注入系统提示的技能元数据块                                          | `<available_skills>` 文本，由 `core::skill::catalog::render_available_skills_block()` 渲染 | 仅含 `name`/`description`（模型按名调用，目录不暴露路径）；`disable_model_invocation` 的技能不出现；超预算截断 + warning                          | 系统提示里那段「你有这些技能可用」。                  |
| **skill body（技能正文）**              | `SKILL.md` 去掉 frontmatter 后的正文指令                       | 磁盘文件；`load_skill` 读出后作为 tool 消息文本                                                    | 仅在装载时读盘；可引用 `references/*.md` 等同目录附件（`load_skill(name, file=...)`）                                            | 技能的「正文操作步骤」。                        |
| `**load_skill`（工具）**              | 模型按 **名字** 装载某条技能完整正文的内置工具                             | `BUILTIN_TOOL_CATALOG` 新条目；`tool_exec/branches/load_skill.rs::handle_load_skill`     | `scope=Read`、`read_only=true`；按 name 解析（避免模型猜路径）；正文读盘仍过 `PermissionGate`                                      | 模型说「我要用 pdf 技能」，这个工具就把 pdf 技能正文调出来。 |
| **discovery root（发现根）**           | 扫描 `SKILL.md` 的来源目录层级                                  | `core::skill::discovery::skill_roots()`                                              | 三层优先级 P0→P2：Project（`agent_workspace_dir/.tomcat/skills`）> Agent（`~/.tomcat/agents/<agentId>/skills/`）> Managed（`~/.tomcat/skills/`）；Project = 当前 CLI cwd 下的 `.tomcat/skills`，与 `[permission-system.md](./permission-system.md)` 对 `agent_workspace_dir` 的语义一致；官方内置 skill 由 `tomcat init` 先写入 P2，再按普通 Managed skill 被发现 | 技能从哪几个目录找出来，谁优先。                    |
| **source / precedence（来源与优先级）**   | 同名技能的去重裁决依据                                            | `Skill.source: SkillSource{Project,Agent,Managed}`                                   | 同名 **first-wins**（高优先级先入选，低优先级丢弃 + warning）                                                                   | 同名技能撞车时，优先级高的赢。                     |
| `**disable_model_invocation`**    | 该技能是否对模型隐藏（仅用户可用）                                      | frontmatter `disable-model-invocation: true` → `Skill.disable_model_invocation`      | `true` → 不进 `<available_skills>`、`load_skill` 拒绝；用户 `/skill use <name>` 仍可用                                              | 这条技能不让模型自己点，只能用户主动用。                |
| **官方内置 skill 资产**                  | 随 tomcat 发版的官方 skill 文件源                                   | `tomcat/assets/skills/`（编译嵌入 `.rs`），`tomcat init` 写入 `~/.tomcat/skills/`          | 不是独立发现根；写入后按普通 Managed skill 参与发现/披露/装载；用户可见、可删、可被 P0/P1 覆盖                                             | 出厂自带几条 skill，但发现链路里不单独加一档。             |
| **Skill vs Plugin（技能 vs 插件）**     | 技能是「惰性指令文本」，插件是「可执行代码」                                 | Skill：`core/skill`（无 hostcall）；Plugin：`ext/plugin`（Wasm 注册 `Tool`）                   | 技能 **不能** 自己执行代码、不注册工具；要执行的动作交给既有工具 + 权限闸门                                                                    | 技能是说明书，插件才是能跑的程序，两条线。               |


**「LLM 收到 `load_skill` 结果后」**：指 `**tool_exec` 已把技能正文（含 `<skill>` 包裹）序列化为 tool 消息文本**、写入会话历史、**即将进入下一轮模型推理之前**。

---

## 2. 竞品 / 选型对比（调研）

对标 **cc-fork-01 / codex / openclaw / pi-mono / pi_agent_rust / hermes-agent / GenericAgent** 七套 agent 的「技能 / 可复用指令包」实现。下表为 **已写入路线图的调研结论**，已定稿七列取舍见 [§4.1](#41-落地选型决策表维度取舍)。

### 2.1 技能类系统的四类典型关切

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  本地 Skill 类系统通常要同时解决的四类问题                                   │
├──────────────────────┬───────────────────────────────────────────────────┤
│  发现与优先级        │  多来源（项目 / agent / 全局托管 / 内嵌）；同名去重； │
│                      │  哪一层先生效、坏文件是否阻断整批                  │
│  渐进式披露          │  元数据进 prompt（省 token）；正文按需读；预算封顶 │
│  调用机制            │  专用 tool / `read` 路径 / 斜杠命令；按名 or 按路径 │
│  安全与生命周期      │  技能能否执行代码；权限是否随技能放大；用量/老化    │
└──────────────────────┴───────────────────────────────────────────────────┘
```

### 2.2 常见实现横向对比


| 来源 / 形态                             | 磁盘格式                                                                                                                                                                                             | 发现 & 优先级                                                                                                                                                  | 渐进式披露                                                                                                                                                              | 调用机制                                                                                                 | 安全 / 生命周期                                                                                                               | 我们借鉴的点                                                                  | 说人话                          |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------- | ---------------------------- |
| **pi_agent_rust**（Rust，最相关）         | `<name>/SKILL.md` 或 `*.md`；frontmatter 白名单 `name/description/license/compatibility/metadata/allowed-tools/disable-model-invocation`（`pi_agent_rust/src/resources.rs::ALLOWED_SKILL_FRONTMATTER`） | `~/.pi/agent/skills`、`.pi/skills`、包；`merge_resource_paths` 优先级 Temp>Project>Global>...（`pi_agent_rust/src/resources.rs`）                                  | 元数据 `Skill{name,description,file_path,base_dir,source,disable_model_invocation}`，**不缓存正文**；`format_skills_for_prompt` 出 `<available_skills>`                       | `read` 工具 + `/skill:name` 展开（`expand_skill_command`）；门控在 `read` 启用时才注入                               | 技能=资源（无 hostcall），执行交给 extension/工具；`enforce_read_scope_with_roots`                                                     | **Rust 同构样板**：serde 结构 + 发现去重 + prompt XML + read 装载，几乎可直接镜像。           |                              |
| **cc-fork-01**（Claude Code fork，TS） | `<name>/SKILL.md`；`FrontmatterData` 宽松解析 + `parseSkillFrontmatterFields`（`cc-fork-01/src/utils/frontmatterParser.ts`、`src/skills/loadSkillsDir.ts`）                                              | `~/.claude/skills`、项目 `.claude/skills`、managed、plugin、bundled、MCP；`getSkillDirCommands`（`src/skills/loadSkillsDir.ts`）                                    | `skill_listing` 系统提醒注入 name+desc（预算 1% 上下文，`MAX_LISTING_DESC_CHARS=250`，`src/tools/SkillTool/prompt.ts`）                                                           | **专用 `Skill` 工具**（`src/tools/SkillTool/SkillTool.ts`，input `{skill,args}`）+ `/skill-name` 斜杠         | `disable-model-invocation`、`checkPermissions`、`allowed-tools` 临时授权；chokidar 热重载                                         | **专用 `Skill` 工具** + 元数据预算封顶 + 每条描述截断；按名调用。                              |                              |
| **codex**（Rust，`core-skills`）       | `<name>/SKILL.md` + 可选 `agents/openai.yaml` sidecar；`SkillFrontmatter{name,description,metadata}`（`codex/codex-rs/core-skills/src/loader.rs`）                                                    | `.codex/skills`、`.agents/skills`(repo walk)、`$CODEX_HOME/skills`、`$HOME/.agents/skills`、system、admin、plugin；`skill_roots`（`loader.rs`），`MAX_SCAN_DEPTH=6` | `<skills_instructions>` developer 块（name+desc+path，预算 2% token / 8000 char 兜底，`core-skills/src/render.rs`）；`SKILLS_HOW_TO_USE_WITH_ABSOLUTE_PATHS` 指引「打开 SKILL.md」 | 进展披露走 `**read`**；`$skill` 显式 mention → `build_skill_injections` 整文注入（`core-skills/src/injection.rs`） | `SkillScope{User,Repo,System,Admin}`；`allow_implicit_invocation`；`skill_approval`                                       | 多层根 + scope 标签 + 预算 2% + 显式 mention 整文注入；指引文案钉「源路径」。                    |                              |
| **openclaw**（TS）                    | `<name>/SKILL.md`；`SkillEntry`/`OpenClawSkillMetadata`/`SkillInvocationPolicy`（`openclaw/src/skills/types.ts`、`loading/frontmatter.ts`）                                                          | 6 层优先级 extra<bundled<managed<`~/.agents`<workspace `.agents`<workspace `skills`；`loadSkillEntries`（`openclaw/src/skills/loading/workspace.ts`），深度 6 + 预算  | `<available_skills>`（name+desc+location）；`formatSkillsForPrompt`（`loading/skill-contract.ts`）；compact 兜底 + 字符上限                                                    | `read` 工具为主；斜杠改写为指令；`command-dispatch:tool` 路由（`discovery/command-specs.ts`）                         | `requires{bins,env,config}` 运行时门控、`agents.list[].skills` 白名单、per-skill env 注入；`allowed-tools` **不强制**                   | `**<available_skills>` XML 模板** + `requires` 运行时门控 + agent 级 allowlist。 |                              |
| **pi-mono**（TS，pi 上代）               | `<name>/SKILL.md`，`.pi/skills` 根可直接放 `*.md`；`SkillFrontmatter`（`pi-mono/packages/coding-agent/src/core/skills.ts`、`packages/agent/src/harness/skills.ts`）                                        | `~/.pi/agent/skills`、`.pi/skills`、`~/.agents/skills`、祖先、包、settings、CLI；`DefaultPackageManager.addAutoDiscoveredResources`                                 | `formatSkillsForPrompt` 出 XML（name+desc+location），**仅 `read` 在选中工具时注入**（`core/system-prompt.ts`）                                                                   | `read` + `/skill:name` 读文件去 frontmatter 注入（`agent-session._expandSkillCommand`）                      | `disable-model-invocation`；name 冲突 first-wins + 诊断；`allowed-tools` 仅文档                                                  | **「gate 在 read 工具」** + `/skill:` 展开 + 冲突 first-wins 诊断。                 |                              |
| **hermes-agent**（Python）            | `<name>/SKILL.md` + `references/templates/assets/`；`name`+`description` 必填（`hermes-agent/tools/skill_manager_tool.py::_validate_frontmatter`）                                                    | `~/.hermes/skills`，bundled sync（`tools/skills_sync.py`）；`iter_skill_index_files`（`agent/skill_utils.py`）                                                  | 三层：`<available_skills>` 索引 → `skill_view(name)` 整文 → `file_path` 链接文件（`tools/skills_tool.py`）                                                                      | `**skills_list`/`skill_view`/`skill_manage` 三工具**；`skill_view` 计 telemetry                           | **usage/provenance sidecar `.usage.json` + Curator 老化**（`tools/skill_usage.py`、`agent/curator.py`）；hub/bundled/agent 溯源 | **生命周期/用量** 思路（本期列 P4 非目标）；`references/` 附件按需取。                         |                              |
| **GenericAgent**（Python）            | 无统一 `SKILL.md`：技能=`memory/` L3 的 `*_sop.md`+`*.py`；唯一 `skill_search/SKILL.md` 是纯文档（`GenericAgent/memory/skill_search/`）                                                                          | 不扫盘：L1 索引 `global_mem_insight.txt` + `get_global_memory()` 注入（`GenericAgent/ga.py`）                                                                       | 无渐进式披露，`file_read` 读整文；`do_update_working_checkpoint` 钉 SOP                                                                                                        | `file_read` 工具 + 远程语义检索 `skill_search.search`（`engine.py`，105K 技能卡服务端）                               | 无；`file_access_stats.json` 仅计数                                                                                          | **「海量技能时的语义检索」** 思路（本期非目标，先做静态发现）                                       | 把技能当记忆 L3，用「先读索引、按需翻」的记忆式做法。 |


**结论（写入路线图，3–5 条「为什么这么选」）**：

1. **磁盘格式 / 发现 / 渐进式披露 / read 装载** 对齐 **pi_agent_rust**——它是 Rust 同构实现，`Skill` serde 结构、`merge_resource_paths` 去重、`format_skills_for_prompt` XML、`enforce_read_scope_with_roots` 几乎可直接镜像到 tomcat 既有 `catalog.rs`/`system_prompt.rs`/`gate.rs` 模式。
2. **调用机制取 cc-fork-01 / hermes 的「专用工具 + 按名解析」**，而非 codex/pi 的「裸 `read` 路径」——tomcat 的 `tool_exec` match + `catalog` 是干净的单一事实源，新增 `load_skill` 一臂即可统一覆盖 **内嵌（无磁盘路径）/ 托管（在可写集外）/ 工作区** 三类来源，且按名解析免去模型猜路径、天然可审计。
3. `**<available_skills>` 元数据块 + 预算封顶** 取 **openclaw / pi-mono 的 XML 模板 + codex 的 2% 预算**——与 tomcat `render_core_identity_tool_lines()` 注入工具清单的现有做法同构，挂一个 `AvailableSkillsSection` 即可。
4. **技能=惰性资源、执行交给既有工具/插件** 取 **pi_agent_rust 的 skill/extension 二分**——tomcat 已有 `ext/plugin`（Wasm 可执行）承担「可执行扩展」，技能只做指令文本，避免重复造「可执行技能」并放大攻击面。
5. **用量/老化（hermes Curator）、海量语义检索（GenericAgent）列为非目标**——本期先把「静态发现 + 渐进式披露 + 按名装载 + 安全门闩」做扎实（见 §3 非目标），把自进化留给 P4。

### 2.3 跨竞品共识（onboarding 速览）

上表信息密；第一次读可先抓这几条「几乎所有竞品都同意」的结论——A 的取舍即由此收敛：

- **C1 主文件统一 `<name>/SKILL.md` + YAML frontmatter**（除 GenericAgent 外全员）→ A 照做（§5.1）。
- **C2 元数据进 prompt、正文按需载入**（摘要与正文分离）→ A 的渐进式披露（§3 G2）。
- **C3 多来源 + 同名去重 / 优先级是刚需** → A 三层根 P0→P2 first-wins（官方内置 skill 由 `init` 写入 P2 Managed，不单列发现根，§4.1）。
- **C4 frontmatter 至少 `name` + `description`** → A 二者必填、未知键忽略（§5.1）。
- **C5 坏文件 / 重名 / 超预算必须可观测**（绝不静默、绝不 panic）→ A 的 `diagnostics` / `warnings`（§10）。
- **C6 技能 = 提示资产，可执行能力交给插件 / 扩展** → A 技能零特权、可执行推给 `ext/plugin`（§3 G5）。
- **C7（分歧点）触发方式各家不同**：codex/openclaw 自动 selection、cc-fork/hermes 专用工具、codex/pi 裸 `read` → A 取「专用 `load_skill` 按名装载」，自动匹配留 P2 后续（§13）。

## 3. 目标与设计原则

### 3.1 观察指标表（与 §11 验收一一对应）


| 目标         | 观察指标（落地后可核对）                                                                                                                                             | 说人话                         |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------- |
| G1 发现闭环    | 启动期扫描 Project/Agent/Managed 三层根；每个合法 `<name>/SKILL.md` 进 `SkillSet`；坏文件（缺 frontmatter / 缺 `name`/`description`）被跳过 + 记 `diagnostics`，**不阻断**其余技能；官方内置 skill 由 `init` 先写入 P2 Managed | 能从几个目录把技能都找出来，一个坏的不连累其它。    |
| G2 渐进式披露   | `<available_skills>` 仅含 `name`/`description`；**正文不进** system_prompt；正文只在 `load_skill` / `read` 时进上下文                                          | 系统提示只放目录，正文要用才读。            |
| G3 元数据预算封顶 | `<available_skills>` 总字符 ≤ 预算 `max(skills.prompt_budget_floor_chars, 上下文窗口字符 × skills.prompt_budget_pct%)`（默认 1% / floor 2000）；超限**两级降级**：〔A〕单条描述截 `max_description_chars`(250) →〔D〕整块仍超则全员只留 `name`（**丢描述不丢技能**）+ `warnings += "skills_prompt_truncated"` | 技能再多也不撑爆系统提示；宁可描述短，也保证每条技能都看得见。 |
| G4 按名装载    | `load_skill(name)` 按 `name` 解析（非路径）；命中 → 返回 `<skill name=.. location=..>` 包裹正文；歧义/未知 → 结构化错误                                                             | 模型报技能名就能调出正文，不用猜路径。         |
| G5 权限不放大   | `load_skill` 读 `SKILL.md` 仍过 `PermissionGate`；技能正文要求的 read/write/bash 仍逐次过闸门；技能 **不获得** 任何额外授权                                                           | 技能只是说明书，不给它开后门。             |
| G6 同名去重可预测 | 同名技能按 `Project > Agent > Managed`（P0→P2）first-wins；被覆盖者记 `warnings += "skill_shadowed:<name> by <source>"`；官方内置 skill 因先写入 P2，天然按 Managed 规则裁决 | 撞名时优先级高的赢，且能查到谁被盖了。         |
| G7 模式/能力门控 | `disable_model_invocation` 技能不进 `<available_skills>`、`load_skill` 拒绝；reviewer/verifier 子 Agent 默认 **不** 暴露 `load_skill`（除非显式放开）                          | 不该给模型的技能就别给；审查子 Agent 默认禁用。 |


### 3.2 非目标


| 非目标                                       | 推给                                                                                                                                                  | 说人话                                 |
| ----------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------- |
| 技能用量统计 / 老化归档（Curator）                    | P4 自进化（参考 hermes `skill_usage.py` / `curator.py`）                                                                                                   | 用得多/久没用就归档，这期不做。                    |
| 自总结生成 SKILL.md（self-evolution / workshop） | P4（`[Product_Brief.md](../openspec/specs/Product_Brief.md)` P4）；落地时**先 proposal 审核、禁止直接写入 trusted 根**（对齐 openclaw workshop / GenericAgent 自结晶的安全取舍） | 让 agent 自己写技能，先走提案审核、不直接落盘，留给以后。    |
| 系统自动匹配 / 隐式触发选中技能（auto-selection）         | P2 后续（见 §13；届时叠加阈值 + 可解释理由日志）                                                                                                                       | 让系统自动猜该用哪条技能，本期先靠模型显式 `load_skill`。 |
| 来源信任分级（source→trust 细分可见 / 自动性）           | P2 后续（参考 openclaw `SkillTrustLevel`）                                                                                                                | 把技能按来源分信任档，本期先用来源优先级，不做细分信任。        |
| 海量技能语义检索（embedding / 远程检索）                | P2 后续 / P3（参考 GenericAgent `skill_search`）                                                                                                          | 上千条技能再上检索，先做静态清单。                   |
| 可执行技能 / 技能注册自定义工具                         | `ext/plugin`（Wasm 插件，`[plugin-system-overview.md](./plugin-system-overview.md)`）                                                                    | 要跑代码的走插件，不在技能里塞可执行逻辑。               |
| 技能热重载（文件监听）                               | P2 后续（参考 cc-fork chokidar `skillChangeDetector`）                                                                                                    | 改了技能要重启才生效，先不做热加载。                  |
| MCP / plugin 携带技能                         | P2 后续（codex `plugin_skill_roots` 模式）                                                                                                                | 插件捎带技能这期不做，先做本地三类根。                 |
| frontmatter `allowed-tools` 强制白名单         | P2 后续（openclaw/pi 均「解析不强制」）                                                                                                                         | 先解析存下来，强制限权以后再说。                    |


### 3.3 设计原则

1. **单一事实源**：技能元数据契约落在 `core::skill::model::Skill` + `frontmatter::ALLOWED_SKILL_FRONTMATTER`；`load_skill` 工具 schema 落在 `BUILTIN_TOOL_CATALOG`——与既有工具同一张表，`docs/tool-catalog.md` 自动派生。
2. **渐进式披露默认**：元数据常驻、正文惰性——与 codex `SKILLS_HOW_TO_USE` / pi `format_skills_for_prompt` 同向，省 token 是第一性约束。
3. **技能零特权**：技能正文是「文本」，不是「代码」；一切副作用走既有 `PermissionGate`（`[permission-system.md](./permission-system.md)`）。这是与 `ext/plugin` 可执行路线的硬边界。
4. **容错优先**：坏 frontmatter / 重名 / 超预算都归一化为 `diagnostics`/`warnings`，**绝不** panic、**绝不** 阻断其余技能装配——与 pi `merge_skills` 冲突诊断同口径。
5. **复用既有骨架**：发现镜像 `pi_agent_rust` 的 resource loader；prompt 注入镜像 tomcat `SystemPromptSection`；工具分发镜像 `tool_exec` match——不另起炉灶。

## 4. 落地选型与实施（已定稿）

> **§4.0 章节编排**：§4.1 给「维度取舍」七列矩阵（不含落地点/交付物/阶段列），§4.2 给五列实施点总表（吸纳交付物与代码落点）。两节相邻，便于先吃决策再看落地。

### 4.1 落地选型决策表（维度取舍）

**代码落点 / 交付物 / 阶段** 见 [§4.2](#42-实施点路线图)；本表 `**决策`** 列每行一句钉死裁决。


| 维度                     | 关切                               | 决策                                                                               | 取自                                                                                                                                                                                                                                                             | 入选理由                                                                                                                                                                      | 未入选 + 拒因                                                                                                                                                                                                                       | 说人话                        |
| ---------------------- | -------------------------------- | -------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------- |
| **技能 vs 插件边界**         | 技能要不要能执行代码 / 注册工具                | **采用** 技能=惰性指令资源，零 hostcall；执行交给既有工具 + `ext/plugin`。                             | `pi_agent_rust/src/resources.rs`（Skill 无 hostcall）+ `pi_agent_rust/docs/planning/EXTENSIONS.md`（skill pack vs executable）+ 本仓 `[plugin-system-overview.md](./plugin-system-overview.md)`                                                                       | 设计：技能只做文本说明书，副作用全走 `PermissionGate`；理由：复用 `ext/plugin` 既有可执行沙箱，避免造「可执行技能」放大攻击面、且与 P2「核心轻量」一致                                                                              | × cc-fork-01 `SkillTool` 的 inline shell 执行（`src/skills/loadSkillsDir.ts` `executeShellCommandsInPrompt`）——拒因：tomcat 权限模型要求一切副作用经闸门，技能内联跑 shell 绕过审计                                                                            | 技能是说明书不是程序；要跑代码走插件。        |
| **调用机制**               | 模型怎么把技能正文拉进上下文                   | **采用** 专用 `load_skill(name)` 工具按名装载；不暴露裸路径。                                      | cc-fork-01 `src/tools/SkillTool/SkillTool.ts`（专用 `Skill` 工具）+ hermes `tools/skills_tool.py`（`skill_view`）+ 本仓 `src/core/agent_loop/tool_exec/mod.rs:311` match                                                                                                 | 设计：`BUILTIN_TOOL_CATALOG` 加 `load_skill` 条目 + `tool_exec` 增一臂；理由：tomcat 多来源技能（project/agent/managed）虽都在磁盘上，但唯有「按名解析」能统一覆盖、免模型猜路径、天然可审计，也便于 reviewer 白名单门控                        | × codex/pi 的裸 `read(<SKILL.md 路径>)`（`codex-rs/core-skills/src/render.rs` `SKILLS_HOW_TO_USE_WITH_ABSOLUTE_PATHS`、`pi_agent_rust/src/resources.rs` read 装载）——拒因：多来源路径对模型不透明、project 本地技能路径依赖当前 cwd，裸 read 容易猜错且审计弱 | 模型报名字调技能，别让它猜文件路径。         |
| **渐进式披露形态**            | 正文进不进系统提示                        | **采用** 仅元数据（name+desc）进 prompt，正文按需装载。                                  | codex `core-skills/src/render.rs`（`<skills_instructions>` 仅 name+desc+path）+ openclaw `src/skills/loading/skill-contract.ts`（`<available_skills>`）+ pi_agent_rust `src/resources.rs::format_skills_for_prompt`                                                 | 设计：`AvailableSkillsSection` 渲染 `<available_skills>`，`Skill` 结构不缓存正文；理由：技能正文动辄上千字，常驻会爆上下文；元数据常驻足够模型做「该不该用」决策                                                               | × GenericAgent 的「无渐进式披露、`file_read` 读整文」（`GenericAgent/ga.py`）——拒因：技能多时整文常驻不可扩展，且 GenericAgent 靠 L1 索引人工维护，不适合自动发现                                                                                                             | 系统提示只给目录，正文要用再翻。           |
| **发现根与优先级**            | 技能从哪找、谁覆盖谁                       | **采用** 三层 P0→P2：`Project(cwd/.tomcat/skills) > Agent(~/.tomcat/agents/<id>/skills) > Managed(~/.tomcat/skills)`，同名 first-wins。 | openclaw `src/skills/loading/workspace.ts::loadSkillEntries`（多层磁盘根）+ pi_agent_rust `src/resources.rs::merge_resource_paths`（Temp>Project>Global）+ 本仓 `[directory-structure.md](./directory-structure.md)`:9-20,40-41,93-94（`agents/<agentId>/` + `~/.tomcat/skills` + `agent_workspace_dir`） + `[permission-system.md](./permission-system.md)`:10-19 | 设计：`discovery::skill_roots()` 只扫三层磁盘根并 `merge` 去重；理由：project 本地技能最贴近当前仓库；agent 级技能次之；全局 Managed 再次；官方内置 skill 不再单列 P3，而是由 init 先写入 P2 Managed | × 单独 P3 Bundled 根——拒因：把“来源语义”和“实现手段”耦合，发现/优先级/诊断多一层分支，收益小；× codex 的六/七层根——拒因：当前单 agent、无 marketplace/admin 分层 | 当前 project 最优先，其次 agent，再全局托管。        |
| **官方内置 skill 的交付方式** | 官方自带 skill 是独立发现根还是写入 Managed | **采用** skill 文件放 `tomcat/assets/skills/`，编译嵌入 `.rs`；`tomcat init` 解压到 `~/.tomcat/skills/`，作为 **P2 Managed** 参与发现，不保留独立 P3。 | codex `skills/src/lib.rs::install_system_skills`（嵌入后写入 `CODEX_HOME/skills/.system`）+ openclaw `src/skills/loading/workspace.ts`（从 bundled dir 作为磁盘技能源加载） | 设计：运行时发现链路统一成磁盘三层根；官方资产对用户可见、可编辑、可诊断；权限/路径/覆盖规则与 Managed 完全一致；实现上仍可通过 `tomcat/assets/skills` + `include_str!`/`include_dir!` 随版本发版 | × 单独 P3 Bundled 根——拒因：多一层 source/precedence 分支；× 纯内存不落盘——拒因：对用户不可见、难调试、与其他技能路径不一致 | 官方自带 skill 也先落到 `~/.tomcat/skills`，别在发现层再单独加一档。 |
| **发现时机 / 缓存 / 更新**     | 启动扫盘会不会卡死、要不要缓存、怎么更新 | **采用** 启动后台 `tokio::spawn` 扫一次 + 首次渲染 `<available_skills>` 前 `await`；**不落盘**（内存 `SkillSet` 即缓存）；有界扫描（深度 1 / 只读 frontmatter ≤4 KiB / `max_skills` 封顶）；更新 = 显式 `/skill reload` + 配置变更重扫；外层只读 `tomcat skill list` + `tomcat skill reload`。 | codex `app-server/src/skills_watcher.rs`（`clear_cache()` + FileWatcher）+ cc-fork-01 `src/utils/skills/skillChangeDetector.ts`（chokidar/轮询 + 300ms debounce）+ pi_agent_rust `src/resources.rs::load_skills`（同步扫、内存 `Vec<Skill>`，无磁盘缓存）——三家均无磁盘 `.md` 缓存 | 设计：发现与启动初始化重叠、不堵启动；元数据在首轮 prompt 前必然就绪；内存即缓存、进程内有效。理由：`walk`+`stat` 是开销大头，磁盘缓存只省 frontmatter 解析、ROI 低且 staleness 风险高；现实本地技能仅几十到几百条 | × 磁盘缓存 manifest——拒因：省不掉 walk/stat，引入失效/损坏/一致性坑（列 P3 兜底）；× v1 文件监听热重载——拒因：跨平台/死锁/事件风暴复杂度高（列 P2，`notify`+debounce）；× 同步扫在启动关键路径——拒因：pathological N 会加启动延迟 | 开机后台先扫、第一次用前等一下；不存缓存文件，改了就 reload。 |
| **命令面（统一命名）**       | 用户怎么列/重载/手动调技能，斜杠与外层 CLI 叫什么 | **采用** 统一单数 `skill`：聊天内斜杠 `/skill list` \| `/skill reload` \| `/skill use <name> "intent..."`（落地 = `parse_chat_command` 白名单加 `/skill` 一臂 + 子命令）；外层 CLI `tomcat skill list` \| `tomcat skill reload`（只读发现 / 重扫 + 诊断）。**不并存 `/skill` 与 `/skills` 两个名词**。 | 本仓 `src/api/chat/commands/parse.rs`（斜杠白名单 `/path\|/help\|/thinking\|/model\|/ckpt\|/restore\|/plan`，`/model use <id>` 子命令范式）+ `src/api/cli/mod.rs`（外层 clap 单数子命令 `plugin\|session\|config`，仿 `tomcat plugin list`） | 设计：单数 `skill` 与外层既有 `plugin/session/config` 同构；`use <name> "intent..."` 显式动词 + 意图文本，既避免与子命令 `list/reload` / 同名 skill 冲突，也允许用户在点名 skill 的同时补充本轮目标；外层只给只读 `list` + 重扫 `reload`（无缓存，reload≈重扫并打印诊断） | × 仅 `/skill use <name>` 无附加意图——拒因：常常还得再补一句需求，交互割裂；× 同时保留 `/skill <name>` 与 `/skills reload` 两种名词——拒因：一个子系统两个命令名，易混且不统一；× 外层支持 `use`——拒因：外层一次性退出、无「当前轮」可注入 | 点名 skill 时，顺手把这轮要它干嘛也一起说清。 |
| **磁盘格式 / frontmatter** | 用啥结构、字段白名单多大                     | **采用** `<name>/SKILL.md` + YAML frontmatter；`name`/`description` 必填，其余走白名单忽略未知键。 | pi_agent_rust `src/resources.rs::ALLOWED_SKILL_FRONTMATTER` + hermes `tools/skill_manager_tool.py::_validate_frontmatter`（name/description 必填）+ openclaw `src/skills/loading/local-loader.ts`（缺 description 跳过）                                                | 设计：`SkillFrontmatter{name,description,license,compatibility,metadata,allowed_tools,disable_model_invocation}` + 白名单；理由：与七套竞品事实标准一致（Anthropic Agent Skills），白名单忽略未知键保证前向兼容 | × cc-fork-01 的宽松 `FrontmatterData`（十几个字段，`src/utils/frontmatterParser.ts`）——拒因：cc 字段（hooks/paths/agent/context）耦合其 slash-command 体系，tomcat 不需要，过宽 schema 增维护成本                                                                 | 一个文件夹一个 SKILL.md，至少要名字+描述。 |
| **技能正文装载的权限**          | 读 SKILL.md / 执行技能动作走不走闸门         | **采用** 装载读盘过 `PermissionGate`；正文副作用逐次过既有工具闸门，技能零特权。                              | 本仓 `src/core/permission/gate.rs:205`（`DefaultPermissionGate::check`）+ `[permission-system.md](./permission-system.md)`:10-19（cwd 不是默认授权根） + pi_agent_rust `src/tools.rs::enforce_read_scope_with_roots` + cc-fork-01 `SkillTool.checkPermissions`                                                                                    | 设计：`load_skill` 内部调 `gate_check_path(Read, ..)` 再读；理由：项目本地技能位于 `agent_workspace_dir/.tomcat/skills/`，而 cwd 不是默认授权根，因此必须统一过 gate；若 cwd 未授权则返回权限拒绝 / 走 cwd lazy prompt，避免技能形成权限后门；正文让模型做的任何 write/bash 仍各自过闸门 + 审计 | × cc-fork-01 `allowed-tools` 临时授权（技能可临时放开工具，`createSkillCommand.getPromptForCommand`）——拒因：与 tomcat「一切副作用经闸门」冲突，技能临时提权破坏审计可追溯                                                                                                   | 技能不带特权；就算当前项目里有技能，也不能顺手绕过权限。 |
| **子 Agent 门控**         | reviewer/verifier 能否用 load_skill | **采用** 默认 **不** 在 reviewer/verifier 白名单；如需放开显式补名。                                | 本仓 `src/core/agent_loop/tool_exec/guard.rs:3,29`（`is_reviewer_whitelisted_tool`/`is_verifier_whitelisted_tool`）+ web_search.md §2 同款门闩先例                                                                                                                       | 设计：`guard.rs` 两函数默认拒 `load_skill`；理由：审查/验证子 Agent 应聚焦只读核对，引入技能正文会污染其上下文、放大注入面                                                                                             | × 默认全 Agent 可用——拒因：与 web_search 同款裁决，审查场景默认收紧更安全                                                                                                                                                                               | 审查的小弟默认不让用技能，要用得显式开。       |
| **配置位置**               | 开关/路径/预算放哪                       | **采用** 新增 `[skills]` 顶层配置子表（非 `[tools]` 子项）。                                     | 本仓 `src/infra/config/types/tools.rs:10`（`ToolsConfig` 模式）+ `src/infra/config/types/mod.rs:19`（`AppConfig` 顶层）+ codex `config.schema.json::SkillsConfig`                                                                                                        | 设计：`SkillsConfig{enabled,prompt_budget_pct,prompt_budget_floor_chars,disabled,...}` 挂 `AppConfig.skills`；理由：技能是跨工具的独立子系统（不是某个工具的上限），与 codex 顶层 `skills` 一致；env `TOMCAT__SKILLS__`*     | × 塞进 `ToolsConfig.skills`——拒因：技能不属于「工具磁盘上限」语义，顶层更清晰，且便于未来挂发现/预算/老化等多维                                                                                                                                                          | 技能开关单独一张表，别挤在工具表里。         |
| **元数据预算策略**            | 技能多了 prompt 怎么不爆                 | **采用** 预算 = `max(prompt_budget_floor_chars 2000, 上下文窗口字符 × prompt_budget_pct% 1%)`；超限**两级降级**〔A〕单条描述截 250 →〔D〕整块仍超则全员只留 `name`（**丢描述不丢技能**）+ warning。 | codex `core-skills/src/render.rs`（`default_skill_metadata_budget`：2% token，8000 char 兜底；档2 描述截断、档3 才丢）+ openclaw `loading/workspace.ts`（full → compact `name+location` 降级）+ cc-fork-01（1% 上下文 + `MAX_LISTING_DESC_CHARS=250`）                                                                                                                      | 设计：`budget = max(floor, ctx_window_chars × pct/100)`，先截描述再降级 name-only；理由：与 codex 同思路但取 **1%**（目录只是「菜单」，留 99% 给正文与对话；tomcat 已去掉 location，行更短，1% 比 codex 2% 更够用）；按字符算确定性强、floor 兜底小窗口                                                     | × 固定字符预算（8000）——拒因：窗口差 10×，按比例更稳；× codex 的 C 水填(逐字符公平分配)——拒因：实现最重，A+D 已够用；× 直接「按优先级丢技能」(E)——拒因：丢掉的技能模型看不见、无法自救（静默盲区），故先降级 name-only 保全可见性；× 不封顶全量注入——拒因：技能规模上去会挤占对话上下文                                                                                                                                                                                            | 技能太多先把描述压短，再不够就只留名字，别撑爆提示。 |
| **选择层（技能爆量时筛 top-n）** | 技能多到连 name-only 都爆预算 / 稀释注意力怎么办 | **P2 · 本期不做**：起一个**筛选子 Agent**（无历史、仅系统提示定义 I/O），上下文 = 用户输入(去历史) + 全量技能目录账本，唯一可见工具 `filtered_skills(names)` 回传 top-n（n<5）；程序再 `∪ must_include`（显式点名/置顶/最近用）去重，填回主 loop 的 `<available_skills>`，主 LLM 做最终决策。 | GenericAgent `memory/skill_search/skill_search/engine.py`（服务端语义检索 `search(query,top_k)`）——本期形态简化为「LLM 子 Agent 筛选」，不引 embedding/向量库 | 设计：选择层只决定「展示哪几张卡」，不替主模型决定「用哪条」；`must_include` 永不被筛掉（护栏）。理由：本地技能撑死几百条，用一次小模型调用做语义筛选，零索引基建、比纯词法更懂语义 | × BM25/词法预筛——拒因：同义/转述漏召回且静默；× GenericAgent 式 embedding 服务——拒因：要向量库/嵌入模型，本地规模用不上；本期 N 小直接全量注入，**不做**任何选择层 | 技能多到爆了，先让个「筛选小模型」挑几条最相关的，再交给主模型。 |


**硬约束自检**：一格一事；每行可回答「若不采纳本行入选结论会付出什么代价」；证据均落到 `仓库/agent + 文件路径` 级别（如 `cc-fork-01/src/tools/SkillTool/SkillTool.ts`、`pi_agent_rust/src/resources.rs`、本仓 `src/core/agent_loop/tool_exec/guard.rs:3`）。

### 4.2 实施点（路线图）

**实施顺序**：**① PR-SK-A**（model + frontmatter + 配置子表，无运行时）→ **② PR-SK-D**（发现 + 去重 + SkillSet）→ **③ PR-SK-P**（`<available_skills>` prompt section）→ **④ PR-SK-T**（`load_skill` 工具 + tool_exec 分发 + guard 门控）→ **⑤ PR-SK-C**（命令面：`/skill` 斜杠 + `tomcat skill` 外层）→ **⑥ PR-SK-B**（下期：官方 skill 资产嵌入 + `tomcat init` 写入 Managed）。**先定结构再接发现，先发现再注入，再挂工具，最后补命令面；官方内置 skill 资产下期再上**——避免 prompt/工具反复改字面量。


| 实施点                        | 交付范围（含交付物）                                                                                                                                                                                                                      | 主要代码落点（含落地点）                                                                                                                                                                                                                                                                                                                                       | 验收锚点（示例）                                                                                                                                                                                                                                                   | 说人话                             |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------- |
| **PR-SK-A**（类型 + 配置）       | **交付物**：`Skill` / `SkillFrontmatter` / `SkillSource` 类型；`ALLOWED_SKILL_FRONTMATTER` 白名单；`SkillsConfig` 子表 + env。**落地点**：`core/skill` 新模块骨架、`AppConfig.skills`                                                                   | 新 `src/core/skill/{mod,model,frontmatter}.rs`；`src/infra/config/types/skills.rs` + `src/infra/config/types/mod.rs` 的 `AppConfig` 加 `skills` 字段                                                                                                                                                                                                     | `core::skill::frontmatter::tests::parse_requires_name_and_description`、`core::skill::frontmatter::tests::unknown_keys_ignored`、`infra::config::tests::skills_cfg_test::skills_toml_override`（**PENDING**）                                                  | 先把结构和开关放好，后面接发现不改字面量。           |
| **PR-SK-D**（发现 + 去重）       | **交付物**：`skill_roots()` 三层扫描；frontmatter 解析；同名 first-wins + `diagnostics`/`warnings`；`SkillSet` 索引。**落地点**：`core/skill/discovery.rs`、`catalog.rs`                                                                               | 新 `src/core/skill/{discovery,catalog}.rs`；复用 `infra/config/load.rs` 的 `get_work_dir` + 当前 `agentId` 拼 P0 `<cwd>/.tomcat/skills`、P1 `~/.tomcat/agents/<id>/skills`、P2 `~/.tomcat/skills`                                                                                                                                                  | `core::skill::discovery::tests::discovers_three_layer_roots`、`shadowed_by_higher_precedence_records_warning`、`malformed_skill_skipped_not_aborting`（**PENDING**）                                                                                           | 从三层根扫出技能、撞名去重、坏的跳过不崩。           |
| **PR-SK-P**（prompt 注入）     | **交付物**：`AvailableSkillsSection`（priority 15）；`<available_skills>` 渲染 + 字符预算 + 单条 250 字截断；新 `PromptKey::SystemAvailableSkills` + 模板。**落地点**：`system_prompt.rs`、`prompts/mod.rs`                                                 | `[src/core/llm/system_prompt.rs](../../src/core/llm/system_prompt.rs)`（加 section + `build_system_prompt_with_state` 注册）、`[src/core/prompts/mod.rs](../../src/core/prompts/mod.rs)`:8（加 `PromptKey`）+ `templates/system/available_skills.txt`；`catalog.rs::render_available_skills_block`                                                           | `core::llm::tests::system_prompt_test::available_skills_section_renders_metadata_only`、`skills_prompt_truncates_at_budget`、`disabled_skill_absent_from_prompt`（**PENDING**）                                                                                | 把「有哪些技能」按预算注入系统提示，正文不进。         |
| **PR-SK-T**（load_skill 工具） | **交付物**：`BUILTIN_TOOL_CATALOG` 加 `load_skill` 条目（schema `{name, file?}`）；`tool_exec` match 增一臂；按名解析 + 过 gate 读正文 + `<skill>` 包裹；reviewer/verifier guard 默认拒。**落地点**：`contract/catalog.rs`、`tool_exec/{mod,guard}.rs` + 新 branch | `[src/core/tools/contract/catalog.rs](../../src/core/tools/contract/catalog.rs)`:91（加条目 + `load_skill_parameters`）、`[src/core/agent_loop/tool_exec/mod.rs](../../src/core/agent_loop/tool_exec/mod.rs)`:311（match 增臂）+ 新 `tool_exec/branches/load_skill.rs` + `branches/mod.rs` 注册、`[guard.rs](../../src/core/agent_loop/tool_exec/guard.rs)`:3,29 | `core::tools::contract::tests::catalog_test::load_skill_registered`、`core::agent_loop::tests::submodules_test::tool_exec_load_skill_resolves_by_name`、`tool_exec_load_skill_rejected_for_reviewer`、`tool_exec_load_skill_unknown_name_errors`（**PENDING**） | 模型报技能名 → 工具调出正文；审查子 Agent 默认禁用。 |
| **PR-SK-B**（官方资产 + init，下期） | **交付物**：官方 skill 文件放 `tomcat/assets/skills/`，编译嵌入 `.rs`；`tomcat init` 将其写入 `~/.tomcat/skills/`（P2 Managed）并在当前 project 生成 `.tomcat/skills/` 样例。**本期不做，放下期。** **落地点**：`tomcat/assets/skills/`、`core/skill/embedded_assets.rs`、`api/cli` init | 新 `tomcat/assets/skills/**/SKILL.md`；新 `src/core/skill/embedded_assets.rs`（编译期嵌入 / 列表）+ `tomcat init` 资产释放 / 同步到 `~/.tomcat/skills/`                                                                                                                                         | `core::skill::embedded_assets::tests::assets_embedded`、`api::cli::tests::init_test::embedded_skills_written_to_managed_dir`、`core::skill::discovery::tests::project_skill_dir_highest_precedence`（**PENDING**）                                               | 官方内置 skill 方案定了，但实现放下期。        |
| **PR-SK-C**（命令面） | **交付物**：聊天内斜杠 `/skill list\|reload\|use <name> "intent..."`（统一单数 `skill`）；外层 CLI `tomcat skill list\|reload`（只读发现 / 重扫 + 诊断）。`reload` 重跑发现任务原子替换 `SkillSet`；`use <name> "intent..."` 注入当前轮（含 `disable_model_invocation`）。**落地点**：`api/chat/commands`、`api/cli` | `src/api/chat/commands/parse.rs`（`parse_chat_command` 白名单加 `/skill` + 子命令解析，仿 `/model use`）+ 新 `commands/cmd_skill.rs`；`src/api/cli/mod.rs`（`Commands` 加 `Skill{sub: SkillSub{List,Reload}}`，仿 `Plugin`）+ 新 `cli/skill_cmd.rs` | `api::chat::commands::tests::parse_test::skill_use_list_reload_parsed`、`api::cli::tests::parse_cli_test::skill_subcommand_parsed`（**PENDING**） | 列表/重载/手动调技能，斜杠与 `tomcat` 外层同名。 |


下文按实施点展开 **技术要点与示意图**；**交付边界与代码落点仍以表为准**。

#### 4.2.1 PR-SK-A：类型与配置子表

- **交付**：`Skill`（仅元数据，镜像 `pi_agent_rust/src/resources.rs::Skill`）、`SkillFrontmatter`（serde，`#[serde(rename = "disable-model-invocation")]` 等）、`SkillSource{Project,Agent,Managed}`（`Ord` 实现 P0→P2 优先级）；`ALLOWED_SKILL_FRONTMATTER` 常量。`SkillsConfig` 挂 `AppConfig.skills`，env 前缀 `TOMCAT__SKILLS__`*（与 `load.rs` 的 `ENV_PREFIX="TOMCAT"` 一致）。

```text
SKILL.md ──┐
           ▼
   frontmatter.rs::parse  ──► SkillFrontmatter { name, description, allowed_tools?, disable_model_invocation?, .. }
           │  （未知键忽略；缺 name/description → SkillParseError）
           ▼
   model.rs::Skill { name, description, file_path, base_dir, source, disable_model_invocation }
```

**说人话**：先把「技能长什么样、配置怎么写」定下来，不接任何运行时逻辑。

#### 4.2.2 PR-SK-D：发现与去重

- **交付**：`skill_roots()` 返回 `Vec<(PathBuf, SkillSource)>`（仅 Project / Agent / Managed 三层磁盘根）；逐根扫 `<name>/SKILL.md`（深度 1，可选 `*.md` 直挂仅 Project 根，对齐 pi-mono `.pi/skills`）；`merge` 按 `SkillSource` 升序 first-wins；坏文件进 `Vec<SkillDiagnostic>`、重名进 `warnings`。
- **执行时机（不堵启动）**：启动时 `tokio::spawn` 一个发现任务（`discover_async() -> SkillSet`），句柄存 `OnceCell<JoinHandle>`；`AvailableSkillsSection::render()`（即首次构造 `<available_skills>`）前 `await` 该句柄拿到 `SkillSet`。因为技能元数据是构造**首轮 prompt**（用户首条消息之后）才需要，扫描与其余启动初始化天然重叠；扫得快则首轮零等待，扫得慢则只在首轮前阻塞一次。
- **有界扫描（防 pathological N）**：① 目录深度固定 1 层（只看 `<root>/<name>/SKILL.md`，不深度遍历）；② 每文件**只读 frontmatter**（读取上限 ~4 KiB，`name/description/flags` 足够），正文绝不在发现期读；③ 总数封顶 `max_skills`（默认 1000）→ 超出 `warnings += skills_cap_exceeded` 并停扫；④ 跳过符号链接环 / 点目录；⑤ 单文件 IO/解析错 → `diagnostics`，**不阻断整批**。官方内置 skill 已在 init 时写入 P2，因此发现期不再区分额外内嵌根。
- **缓存与更新**：**不落盘**——内存 `SkillSet` 即进程生命周期缓存（同 codex/cc-fork/pi 内存方案，均无磁盘 `.md` 缓存）。更新走**显式重扫**：聊天内 `/skill reload` 命令 + `[skills]` 配置变更 / 新会话时重建；外层 `tomcat skill reload` 同义（重扫 + 打印诊断）。**理由**：目录 `walk`+`stat` 才是开销大头、缓存省不掉它（缓存只省 frontmatter 解析），磁盘缓存 ROI 低且引入 staleness；OS 文件监听热重载（`notify` + ~300ms debounce，镜像 codex `SkillsWatcher`）列 **P2**，磁盘 manifest 列 **P3 兜底**（仅当技能上数千且启动延迟敏感）。

```text
roots (按优先级 P0→P2): [Project, Agent, Managed]
   │  对每个 root
   ▼
 walk <name>/SKILL.md ──► frontmatter.parse
   │            ├─ Ok  ──► 候选 Skill
   │            └─ Err ──► diagnostics += {path, reason}（不阻断）
   ▼
 merge: 按 name 去重，已存在(更高优先级) → 丢弃 + warnings += skill_shadowed
   ▼
 SkillSet { by_name: BTreeMap<String, Skill>, diagnostics, warnings }
```

**说人话**：三层根扫一遍，撞名优先级高的赢，坏文件记一笔但不影响别人。

#### 4.2.3 PR-SK-P：`<available_skills>` 注入

- **交付**：`AvailableSkillsSection: SystemPromptSection`（`priority()=15`，落在 `CoreIdentity(10)` 之后、`ToolInstructions(20)` 之前），仅在「`load_skill` 工具对当前模式可用且 `SkillSet` 非空」时渲染非空；`render_available_skills_block(skills, budget)` 过滤 `disable_model_invocation`、累加字符到预算即停。模板新增 `PromptKey::SystemAvailableSkills`（`prompts/mod.rs` 加枚举臂 + `include_str!`）。

```text
SystemPromptBuilder::default() / build_system_prompt_with_state
   │ register(AvailableSkillsSection{skills})  // priority 15
   ▼
 render(): catalog::render_available_skills_block(&skills, budget=max(floor, ctx_chars×pct%))
   ▼
 <available_skills>
  <skill><name>pdf</name><description>...</description></skill>
   ...   (超 budget → 截断 + warnings)
 </available_skills>
```

**说人话**：挂一个新提示段，把技能目录按预算塞进系统提示，正文一律不进。

#### 4.2.4 PR-SK-T：`load_skill` 工具与分发

- **交付**：`BUILTIN_TOOL_CATALOG` 增 `load_skill` 条目（`scope=Read`、`read_only=true`、`destructive=false`、`category=Filesystem`、`plan_only=false`）；`load_skill_parameters()` 出 `{ name: string(必填), file?: string }` schema。`tool_exec/mod.rs:311` match 加 `"load_skill" => branches::handle_load_skill(ctx, &args).await`；`branches/load_skill.rs` 按 `name` 在 `SkillSet` 解析，命中后对 `file_path`（或 `base_dir/file`）调 `gate_check_path(Read, ..)` + 读盘，去 frontmatter，包 `<skill name=.. location=..>正文</skill>` 返回。`guard.rs` 的 `is_reviewer_whitelisted_tool` / `is_verifier_whitelisted_tool` **默认不含** `load_skill`。

```text
LLM: load_skill(name="pdf")
   │
   ▼ tool_exec/mod.rs match "load_skill"
   ├─ reviewer/verifier? ──► guard 拒（默认）──► is_error
   ▼
 branches::handle_load_skill
   ├─ SkillSet.resolve("pdf")  ├─ 未知/歧义 ──► Err 结构化
   │                           └─ disable_model_invocation ──► Err
   ▼
 gate_check_path(Read, skill.file_path)  ──► 读 SKILL.md ──► strip frontmatter
   ▼
 "<skill name=\"pdf\" location=\"...\">{body}</skill>"  ──► tool 消息文本
```

**说人话**：模型报名字，工具按名找到技能、过安检读正文、包好还给模型；如果命中的是当前 project 下的 `.tomcat/skills` 而 cwd 尚未授权，就按权限系统拒绝 / 提示；审查小弟默认调不动。

#### 4.2.5 PR-SK-B：官方 skill 资产嵌入与 init 写入 Managed

- **交付**：官方 skill 文件放 `tomcat/assets/skills/`，编译时嵌入 `embedded_assets.rs`；`tomcat init` 将其写入 `~/.tomcat/skills/`（即 **P2 Managed**），后续发现期按普通 Managed skill 扫描，无独立 P3 根；`tomcat init` 同时在当前 project 的 `.tomcat/skills/` 释放一条样例技能。

**说人话**：出厂自带的 skill 不再单独占一档，`init` 先把它们放进 `~/.tomcat/skills/`，再像普通 Managed skill 一样被发现；项目里另外给一条样例照着写。

## 5. 协议（SKILL.md / load_skill / available_skills / 配置）

**单一事实源**：

- frontmatter 契约：`core/skill/frontmatter.rs` 的 `SkillFrontmatter` + `ALLOWED_SKILL_FRONTMATTER`。
- 运行时类型：`core/skill/model.rs` 的 `Skill` / `SkillSource` / `SkillSet`。
- `load_skill` JSON Schema（模型可见）：`[catalog.rs::load_skill_parameters](../../src/core/tools/contract/catalog.rs)`（PR-SK-T 添加）→ `[docs/tool-catalog.md](../tool-catalog.md)` 自动派生。
- `<available_skills>` 文本契约：`catalog.rs::render_available_skills_block` + `templates/system/available_skills.txt`。

**核心 Rust 类型（草图，实现以 PR-SK-A 为准；字段表见 §5.1）**：

```rust
// core/skill/model.rs —— 运行时类型（只存元数据，绝不缓存正文）
pub enum SkillSource { Project, Agent, Managed } // Ord 升序 = P0→P2，同名 first-wins

pub struct Skill {
    pub name: String,                  // [a-z0-9-]+，唯一键
    pub description: String,           // 进 <available_skills>
    pub file_path: PathBuf,            // SKILL.md 路径（运行时一律是磁盘路径）
    pub base_dir: PathBuf,             // 技能目录；附件相对此目录解析
    pub source: SkillSource,
    pub disable_model_invocation: bool,
}

pub struct SkillSet {
    pub by_name: BTreeMap<String, Skill>,  // 单一事实源（进程内）
    pub diagnostics: Vec<SkillDiagnostic>, // 坏文件：跳过不阻断
    pub warnings: Vec<String>,             // skill_shadowed / skills_prompt_truncated
}

pub struct SkillDiagnostic { pub path: PathBuf, pub reason: String }

// core/skill/frontmatter.rs —— 磁盘契约（serde；未知键忽略 = 前向兼容）
pub struct SkillFrontmatter {
    pub name: String,                          // 必填
    pub description: String,                   // 必填
    pub license: Option<String>,               // 仅记录
    pub compatibility: Option<String>,         // 仅记录
    #[serde(default)] pub metadata: Map<String, Value>,
    pub allowed_tools: Option<Vec<String>>,    // 本期仅解析不强制（§3.2）
    #[serde(rename = "disable-model-invocation", default)]
    pub disable_model_invocation: bool,
}
// 另：ALLOWED_SKILL_FRONTMATTER 白名单常量；解析缺 name/description → SkillParseError
```

> **刻意不入 v1 类型**：来源信任分级 `SkillTrustLevel`、自动匹配 `SkillSelector` 为 **P2 后续**（见 §3.2 / §13）——v1 用 `SkillSource` 的来源优先级即可，不提前引入信任维度，避免类型与本期非目标自相矛盾。

### 5.1 SKILL.md frontmatter 字段


| 字段                         | YAML 类型           | 必填    | 默认      | 适用场景                   | 说明                                                              | 说人话              |
| -------------------------- | ----------------- | ----- | ------- | ---------------------- | --------------------------------------------------------------- | ---------------- |
| `name`                     | string            | **是** | —       | 技能身份                   | `[a-z0-9-]+`，≤64 字；唯一性见 §8 去重；缺失 → 跳过 + diagnostic              | 技能的唯一名字。         |
| `description`              | string            | **是** | —       | 进 `<available_skills>` | ≤1024 字（prompt 中再按 250 截断）；缺失 → 跳过 + diagnostic                 | 一句话说清这技能干嘛、何时用。  |
| `license`                  | string            | 否     | null    | 元信息                    | 仅记录，不影响装载                                                       | 许可证，存着备查。        |
| `compatibility`            | string            | 否     | null    | 元信息                    | 仅记录                                                             | 兼容性说明。           |
| `metadata`                 | map               | 否     | `{}`    | 扩展位                    | 透传保存，未来扩展（emoji/tags 等）                                         | 自定义附加信息。         |
| `allowed-tools`            | string | string[] | 否     | null    | 解析存储                   | **本期仅解析不强制**（与 openclaw/pi 一致，强制留 P2 后续）                        | 声明用到哪些工具，暂不强制。   |
| `disable-model-invocation` | bool              | 否     | `false` | 可见性门控                  | `true` → 不进 `<available_skills>`、`load_skill` 拒；用户 `/skill` 仍可用 | 设 true 就不让模型自己点。 |
| *（未知键）*                    | any               | 否     | —       | 前向兼容                   | 不在白名单的键 **忽略**，不报错                                              | 多写的字段不会报错。       |


**SKILL.md 样例**：

```markdown
---
name: commit
description: Create a well-formed git commit. Use when the user asks to commit changes; follows the repo's commit-message convention.
allowed-tools: [bash, read]
---

# Commit workflow

1. Run `git status` and `git diff --staged` to review changes.
2. Draft a message: `type(scope): summary` ...
3. Commit via `bash`: `git commit -m "<message>"`.
（正文可引用同目录 references/COMMIT_CONVENTION.md，模型用 load_skill(name="commit", file="references/COMMIT_CONVENTION.md") 取）
```

### 5.2 `load_skill` 工具入参


| 字段     | JSON 类型 | 必填    | 默认   | 说明                                                    | 说人话            |
| ------ | ------- | ----- | ---- | ----------------------------------------------------- | -------------- |
| `name` | string  | **是** | —    | 要装载的技能名（须出现在 `<available_skills>`）；非路径                | 报技能名。          |
| `file` | string  | 否     | null | 装载技能目录下的附件相对路径（如 `references/x.md`）；缺省装 `SKILL.md` 正文 | 想看技能附带的某个参考文件。 |


**三态语义**：缺 `file` / 显式 null → 装 `SKILL.md` 正文；显式相对路径 → 装该附件（仍过 `gate_check_path`，且必须落在技能 `base_dir` 内，越界 → `Err`）。

**调用样例（jsonc）**：

```jsonc
// 装载技能正文
{ "name": "commit" }

// 装载技能附带的参考文件
{ "name": "commit", "file": "references/COMMIT_CONVENTION.md" }
```

**典型出参（tool 消息文本）**：

```text
<skill name="commit" location="~/repo/.tomcat/skills/commit/SKILL.md">
References are relative to ~/repo/.tomcat/skills/commit/.

# Commit workflow
1. Run `git status` ...
</skill>
```

**错误出参**（`is_error=true`，tool 消息为描述文本）：

```text
unknown skill: "commti"; available skills: commit, code-review, pdf
```

### 5.3 `<available_skills>` 注入块（system prompt）

```text
<available_skills>
The following skills provide specialized instructions for specific tasks.
Call load_skill(name=...) to load a skill's full body when the task matches its description.
  <skill>
    <name>commit</name>
    <description>Create a well-formed git commit. Use when ...</description>
  </skill>
  ...
</available_skills>
```

目录块**只给 `name`/`description`**：模型靠 `description` 判断「该不该用」、靠 `name` 发起 `load_skill(name)`，全程不需要路径，所以不注入 `location`（省 token，也避免模型绕过 `load_skill` 去裸读路径）。技能正文里需要的相对路径锚点（`references/`、`scripts/`）由 `load_skill` 的**返回体**给出（见 §5.2），而非目录块。超预算（`max(prompt_budget_floor_chars, 窗口字符 × prompt_budget_pct%)`）时**两级降级**：先把每条描述截到 `max_description_chars`，整块仍超则**全员只留 `name`**（丢描述、不丢技能，保证所有技能名可见）+ `warnings`。

### 5.4 配置（`[skills]` 子表）


| 字段                      | 类型       | 默认      | 说明                                            | 说人话          |
| ----------------------- | -------- | ------- | --------------------------------------------- | ------------ |
| `enabled`               | bool     | `true`  | 总开关；`false` → 不发现、不注入、`load_skill` 报禁用        | 一键关掉整个技能系统。  |
| `prompt_budget_pct`         | u8       | `1`     | `<available_skills>` 占上下文窗口的百分比上限（codex 取 2%，我们取 1%） | 技能目录最多占上下文的 1%。 |
| `prompt_budget_floor_chars` | usize    | `2000`  | 小窗口字符兜底（实际预算 = `max(floor, 窗口字符 × pct%)`）   | 窗口再小也至少给这么多。 |
| `max_description_chars`     | usize    | `250`   | 单条描述截断                                        | 每条描述最多多长。    |
| `max_skills`                | usize    | `1000`  | 发现总数封顶；超出停扫 + `warnings`                      | 防御技能目录爆量，扫描有上界。 |
| `disabled`              | string[] | `[]`    | 按名禁用的技能（即便磁盘存在也不装配）                           | 临时拉黑某几条技能。   |
| `expose_to_reviewer`    | bool     | `false` | 是否给 reviewer/verifier 子 Agent 暴露 `load_skill` | 审查小弟要不要给技能。  |


env：`TOMCAT__SKILLS__ENABLED` / `TOMCAT__SKILLS__PROMPT_BUDGET_PCT` / `TOMCAT__SKILLS__PROMPT_BUDGET_FLOOR_CHARS` / ...（`env > config > 默认`）。

---

## 6. One-Glance Map（文件职责总览）

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  src/infra/config/types/skills.rs                                          │
│  • SkillsConfig { enabled, prompt_budget_pct, prompt_budget_floor_chars,    │
│    max_description_chars, max_skills, disabled, expose_to_reviewer }        │
│  • 挂 AppConfig.skills（types/mod.rs:19 AppConfig 加字段）                  │
└───────────────────────────────┬────────────────────────────────────────────┘
                                │ cfg.skills
                                ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  src/core/skill/  (新模块)                                                  │
│  ├ model.rs        • Skill { name, description, file_path, base_dir,        │
│  │                   source, disable_model_invocation }；SkillSource(Ord)   │
│  │                 • SkillSet { by_name, diagnostics, warnings }            │
│  ├ frontmatter.rs  • SkillFrontmatter(serde)；ALLOWED_SKILL_FRONTMATTER     │
│  │                 • parse(): 缺 name/description → SkillParseError          │
│  ├ discovery.rs    • skill_roots()：Project>Agent>Managed (P0→P2)             │
│  │                 • discover() → walk <name>/SKILL.md → merge(first-wins)  │
│  ├ catalog.rs      • render_available_skills_block(&SkillSet, budget)        │
│  │                 • resolve(name) → &Skill（过滤 disable_model_invocation） │
│  └ embedded_assets.rs • `tomcat/assets/skills/**` 编译嵌入；`tomcat init` 写入 P2 │
└──────────────┬──────────────────────────────────────────┬──────────────────┘
               │ 渐进式披露：元数据                         │ 按需装载：正文
               ▼                                           ▼
┌──────────────────────────────────────┐   ┌──────────────────────────────────────┐
│ src/core/llm/system_prompt.rs        │   │ src/core/tools/contract/catalog.rs    │
│ • AvailableSkillsSection(priority 15)│   │ • BUILTIN_TOOL_CATALOG 加 "load_skill"│
│ • build_system_prompt_with_state 注册│   │ • load_skill_parameters(): {name,file?}│
│ src/core/prompts/mod.rs:8            │   └───────────────────┬──────────────────┘
│ • PromptKey::SystemAvailableSkills   │                       │
│ • templates/system/available_skills  │                       ▼
└──────────────────────────────────────┘   ┌──────────────────────────────────────┐
                                            │ src/core/agent_loop/tool_exec/        │
                                            │ • mod.rs:311 match "load_skill"        │
                                            │ • branches/load_skill.rs               │
                                            │   handle_load_skill → SkillSet.resolve │
                                            │   → gate_check_path(Read) → read 正文  │
                                            │   → <skill> 包裹                       │
                                            │ • branches/mod.rs 注册                 │
                                            │ • guard.rs:3,29 默认拒 reviewer/verifier│
                                            └───────────────────┬──────────────────┘
                                                                │ gate_check_path
                                                                ▼
                                            ┌──────────────────────────────────────┐
                                            │ src/core/permission/gate.rs:205        │
                                            │ • DefaultPermissionGate::check          │
│   （Project 根命中 cwd 权限语义；仍统一过 gate）│
                                            └──────────────────────────────────────┘

  + tests:
    src/core/skill/tests/{frontmatter_test,discovery_test,catalog_test}.rs   (单元)
    src/core/llm/tests/system_prompt_test.rs                                 (扩展：available_skills)
    src/core/agent_loop/tests/submodules_test.rs                             (load_skill 分发/guard)
    src/infra/config/tests/skills_cfg_test.rs                                (配置 TOML + env)
    tests/tool_catalog_doc.rs                                                (load_skill 进派生文档)
```

**阅读顺序（说人话）**：配置 `SkillsConfig` 决定开不开、预算多大；启动时 `core/skill/discovery` 按 P0→P2 三层根扫出技能，`catalog` 去重建成 `SkillSet`；`system_prompt` 的 `AvailableSkillsSection` 把 `SkillSet` 的「名字+描述」按预算注入系统提示（不含正文，也不含路径）；模型要用时调 `load_skill`，`tool_exec` 经 `guard` 门控后到 `branches/load_skill`，按名解析、过 `gate` 读正文、包成 `<skill>` 回给模型；整条链上技能从不获得额外权限，正文里要做的事仍各自过 `permission/gate`。

## 7. 调度时序（运行时图）

### 7.1 启动期：后台发现 → 首轮 await → 注入

```text
run_loop      discovery task(后台)   skill::catalog      system_prompt        LLM
 │                  │                    │                  │                 │
 │ 装配 chat        │                    │                  │                 │
 │ tokio::spawn ───▶│ skill_roots()      │                  │                 │
 │ (存 JoinHandle)  │ walk SKILL.md(深度1)│                  │                 │
 │ ……并行其它初始化  │ frontmatter.parse  │                  │                 │
 │  (不阻塞)        │───────────────────▶│ merge(first-wins)│                 │
 │                  │                    │ → SkillSet       │                 │
 ▼ 用户首条消息 → 构造首轮 prompt        │                  │                 │
 │ await JoinHandle ────────────────────▶│ (未完成则阻塞一次)│                 │
 │◀──────────────── SkillSet ────────────│                  │                 │
 │ build_system_prompt_with_state(.., skills=SkillSet)      │                 │
 │─────────────────────────────────────────────────────────▶│ AvailableSkills │
 │                  │                    │  render_available_skills_block(budget)
 │                  │                    │                  │ <available_skills>
 │◀─────────────────────────────────────────────────────────│ system prompt   │
 │ system message（含技能元数据，无正文） ─────────────────────────────────────▶│
```

> 后续 `/skill reload`（聊天内）或配置变更 → 重跑发现任务、原子替换 `SkillSet`（OS 文件监听热重载列 P2）。

### 7.2 工具调用期：按名装载

```text
LLM            tool_exec/mod          guard            branches/load_skill     permission/gate
 │                  │                   │                    │                     │
 │ load_skill(name) │                   │                    │                     │
 │─────────────────▶│ parse args        │                    │                     │
 │                  │ reviewer/verifier?│                    │                     │
 │                  │──────────────────▶│ whitelisted?       │                     │
 │                  │◀──────────────────│ no → is_error(拒)  │                     │
 │                  │ (allowed) ────────────────────────────▶│ SkillSet.resolve()  │
 │                  │                   │                    │ 未知/歧义/disabled  │
 │                  │                   │                    │   → Err 结构化       │
 │                  │                   │                    │ gate_check_path(Read)│
 │                  │                   │                    │────────────────────▶│ Allow/Deny
 │                  │                   │                    │◀────────────────────│
 │                  │                   │                    │ read SKILL.md        │
 │                  │                   │                    │ strip frontmatter    │
 │                  │◀───────────────────────────────────────│ <skill>..</skill>    │
 │◀─────────────────│ tool 消息文本（正文进上下文）            │                     │
```

**两条路径在 `tool_exec` 出口处归一**：无论装载成功（`<skill>` 文本）还是失败（错误描述），都按 tool 消息文本回灌 LLM；下游不区分技能来源（project / agent / managed）。

---

## 8. 状态机（技能解析与发现去重）

### 8.1 单条技能解析状态

```text
┌──────────┐ read+parse ┌──────────┐  name 唯一  ┌──────────┐
│ on disk  │───────────▶│ parsed   │────────────▶│ admitted │
│ SKILL.md │            └────┬─────┘             └──────────┘
└──────────┘                 │
     │ 缺 frontmatter /        │ 同名已存在(更高优先级)
     │ 缺 name/description     ▼
     ▼                   ┌──────────┐
┌──────────┐            │ shadowed │ warnings += skill_shadowed:<name>
│ rejected │            └──────────┘
└──────────┘ diagnostics += {path, reason}
```


| 当前状态    | 事件                                   | 目标状态     | 副作用                                      | 说人话            |
| ------- | ------------------------------------ | -------- | ---------------------------------------- | -------------- |
| on disk | parse 成功 + name 未占用                  | admitted | 入 `SkillSet.by_name`                     | 解析通过、名字没被占就收下。 |
| on disk | 缺 frontmatter / 缺 name 或 description | rejected | `diagnostics += {path,reason}`，**不阻断**其余 | 坏文件记一笔，跳过。     |
| parsed  | 同名已被更高优先级占用                          | shadowed | `warnings += skill_shadowed`，丢弃低优先级      | 撞名输给优先级高的，记一笔。 |


### 8.2 `load_skill` 调用结局


| 当前状态                  | 事件                                  | 目标状态      | 副作用                                                   | 说人话           |
| --------------------- | ----------------------------------- | --------- | ----------------------------------------------------- | ------------- |
| 收到 `load_skill(name)` | reviewer/verifier 且未放开              | rejected  | `is_error`：`load_skill not available in this context` | 审查小弟默认不让用。    |
| 已过 guard              | name 命中且非 disabled                  | loaded    | gate 读正文 → `<skill>` 文本                               | 找到、过安检、给正文。   |
| 已过 guard              | name 未知                             | unknown   | `is_error`：列出可用技能名                                    | 没这技能，告诉它有哪些。  |
| 已过 guard              | name 命中但 `disable_model_invocation` | forbidden | `is_error`：技能仅用户可用                                    | 这条不让模型点。      |
| 已过 guard              | `file` 越出 `base_dir`                | denied    | `Err`（gate 或路径校验）                                     | 想读技能目录外的文件，拒。 |
| 读正文阶段                 | gate Deny / IO 失败                   | failed    | `Err`（权限或 IO 描述）                                      | 读不了就如实报错；项目根未授权时也走这里。 |


---

## 9. 配置与环境变量

**总则**：`env > config > 默认`。


| 来源                   | 键                                  | 含义                                  | 优先级         | 说人话       |
| -------------------- | ---------------------------------- | ----------------------------------- | ----------- | --------- |
| `tomcat.config.toml` | `[skills] enabled`                 | 技能系统总开关                             | config      | 不写默认开。    |
| env                  | `TOMCAT__SKILLS__ENABLED`          | 同上运行时覆盖                             | env（最高）     | 容器里临时关。   |
| `tomcat.config.toml` | `[skills] prompt_budget_pct`         | `<available_skills>` 占上下文百分比上限 | config      | 默认 1（%）。 |
| `tomcat.config.toml` | `[skills] prompt_budget_floor_chars` | 字符兜底下限                            | config      | 默认 2000。  |
| `tomcat.config.toml` | `[skills] max_description_chars`     | 单条描述截断                            | config      | 默认 250。   |
| `tomcat.config.toml` | `[skills] disabled`                | 按名拉黑技能                              | config      | 临时禁某几条。   |
| `tomcat.config.toml` | `[skills] expose_to_reviewer`      | reviewer/verifier 是否可用 `load_skill` | config      | 默认 false。 |
| CLI                  | `--no-skills`                      | 本次运行禁用技能                            | 运行时         | 一次性关掉。    |
| 磁盘 P0              | `<agent_workspace_dir>/.tomcat/skills/` | 当前 project 本地技能（最高优先级）              | —           | 当前仓库自己写的技能。 |
| 磁盘 P1              | `~/.tomcat/agents/<agentId>/skills/` | 当前 agent 专属技能                         | —           | 绑定 agent 的技能。 |
| 磁盘 P2              | `~/.tomcat/skills/`                | 全局托管技能（Gateway 安装）                  | —           | 集中管理的技能。  |
| 编译期资产源           | `tomcat/assets/skills/`             | 官方内置 skill 文件源（编译期）                  | build/init  | 不是发现根；`tomcat init` 写入 P2 Managed。 |


> **技能不读 env override 正文**：与 `prompts/mod.rs` 同口径，`SKILL.md` 正文从磁盘读但不支持 env 注入；env 仅控制开关 / 预算 / 路径。
> **能否「纯配置接入新技能」**：能——往当前 project 的 `.tomcat/skills/<name>/SKILL.md` 放一个文件即可，无需改码（与「新增工具要改 `catalog.rs`」不同）。这是技能相对内置工具的核心扩展性优势。

## 10. 错误模型 / 截断 / 警告

```text
                        技能子系统
                            │
        ┌───────────────────┼────────────────────────┐
        ▼                   ▼                        ▼
   发现期（启动）       注入期（prompt）          装载期（load_skill）
        │                   │                        │
   ┌────┴─────┐        ┌────┴────┐              ┌─────┴──────┐
   ▼          ▼        ▼         ▼              ▼            ▼
 坏文件    重名      超预算    全空            未知/歧义    gate Deny / IO
 diagnostics warnings warnings (无块)          is_error     Err
 (跳过)   (shadowed) (truncate) (section 不渲染) (列可用名)   (权限/IO 描述)
   │          │        │         │              │            │
   └──────────┴────────┴─────────┴──────────────┴────────────┘
                            │
                            ▼  归一化结局（绝不 panic、绝不阻断其余技能）
              启动继续 / system prompt 照常 / tool 消息回灌 LLM
```

**结局清单**：

- **发现期坏文件**（缺 frontmatter / 缺 `name`/`description` / YAML 非法）→ 进 `SkillSet.diagnostics`，跳过该文件，**不** 阻断其余技能、**不** 阻断启动。
- **同名冲突** → 高优先级 first-wins，低优先级 `warnings += skill_shadowed:<name>`。
- **发现总数超 `max_skills`** → 停扫已扫部分照常入 `SkillSet` + `warnings += skills_cap_exceeded`，**不** 阻断启动。
- **发现任务未完成时首轮已到** → `await` 阻塞至完成（一次性）；发现任务自身 panic / IO 致命错 → 降级为空 `SkillSet` + `warnings`，启动与对话照常。
- **元数据超预算** → **两级降级**：①单条描述 > `max_description_chars` 截断 → ②整块仍超则全员降级 `name-only`（**丢描述不丢技能**）+ `warnings += skills_prompt_truncated`；若连 `name-only` 都超（极端规模）→ 末尾硬截 + warning，并作为启用 ①′ 选择层（P2）的信号。
- `**SkillSet` 全空 / `enabled=false`** → `AvailableSkillsSection` 渲染空串（不出 `<available_skills>` 块）。
- `**load_skill` 未知/歧义/disabled** → `is_error=true` + 友好描述（列出可用技能名）。
- `**load_skill` gate Deny / IO 失败 / `file` 越界** → `Err`（权限或路径描述）。

`**tool_exec` 视角**：`Ok(<skill> 文本)` → tool 消息文本；`Err(_)` → tool 消息为错误描述（`is_error=true`）。§3 G1–G7 的「锁死它的测试」全部位于 §11。

---

## 11. 测试矩阵（验收）

**当前状态（2026-06-06）**：本系统为 **P2 路线图草案**，`src/core/skill/` 尚未落地；下表为 **验收锚点设计**，对应 PR 合入后回写状态列（参考 web_search.md §10 的 PASS/日期回写惯例）。


| 维度                | 用例（计划函数名）                                                                                                                                                      | 状态      | 说人话                             |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------- | ------------------------------- |
| frontmatter 解析    | `core::skill::frontmatter::tests::parse_requires_name_and_description`、`unknown_keys_ignored`、`rejects_missing_frontmatter`                                    | PENDING | name/description 必填、未知键忽略、缺头报错。 |
| 发现去重              | `core::skill::discovery::tests::discovers_three_layer_roots`、`shadowed_by_higher_precedence_records_warning`、`project_skill_dir_highest_precedence`               | PENDING | 三层根扫出、撞名 first-wins、P0 project 最优先。 |
| 容错                | `core::skill::discovery::tests::malformed_skill_skipped_not_aborting`                                                                                          | PENDING | 坏文件跳过不连累其余。                     |
| 官方资产落地            | `core::skill::embedded_assets::tests::assets_embedded`、`api::cli::tests::init_test::embedded_skills_written_to_managed_dir`                                  | PENDING | 官方 skill 资产被嵌入并由 init 写入 P2 Managed。 |
| prompt 注入（G2/G3）  | `core::llm::tests::system_prompt_test::available_skills_section_renders_metadata_only`、`skills_prompt_truncates_at_budget`、`disabled_skill_absent_from_prompt` | PENDING | 只注元数据、超预算截断、disabled 不出现。       |
| load_skill 注册（G4） | `core::tools::contract::tests::catalog_test::load_skill_registered`                                                                                            | PENDING | 工具进 catalog + schema。           |
| load_skill 分发（G4） | `core::agent_loop::tests::submodules_test::tool_exec_load_skill_resolves_by_name`、`tool_exec_load_skill_unknown_name_errors`                                   | PENDING | 按名解析、未知报可用名。                    |
| 权限不放大（G5）         | `core::agent_loop::tests::submodules_test::tool_exec_load_skill_file_escape_denied`、`load_skill_body_load_passes_gate`                                         | PENDING | 越界拒、读正文过闸门。                     |
| 子 Agent 门控（G7）    | `core::agent_loop::tests::submodules_test::tool_exec_load_skill_rejected_for_reviewer`、`..._rejected_for_verifier`                                             | PENDING | reviewer/verifier 默认拒。          |
| 配置                | `infra::config::tests::skills_cfg_test::skills_toml_override`、`skills_env_override_beats_toml`、`disabled_list_filters_skill`                                   | PENDING | TOML/env 覆盖、disabled 过滤。        |
| 派生文档              | `tests::tool_catalog_doc`（含 `load_skill`）                                                                                                                      | PENDING | load_skill 进 tool-catalog.md。   |
| 文档                | 本文定稿 + 相邻 `directory-structure.md`/`permission-system.md` 回链                                                                                                   | PENDING | 字和代码别两张皮。                       |


§3 观察指标 **G1–G7** 与本表逐行对应：G1↔发现去重/容错；G2/G3↔prompt 注入；G4↔load_skill 注册/分发；G5↔权限不放大；G6↔发现去重（shadow）；G7↔子 Agent 门控。

---

## 12. 风险与应对


| 风险                        | 影响                                | 应对（具体动作）                                                                                                                                                                                                                                             | 说人话                          |
| ------------------------- | --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------- |
| **技能正文 prompt 注入 / 越权指令** | 恶意 SKILL.md 诱导模型执行危险操作            | 技能零特权：正文里要求的 read/write/bash 仍各自过 `permission/gate.rs::check`（`[permission-system.md](./permission-system.md)`）；`load_skill` 仅 `scope=Read`；技能不能 inline 跑 shell（拒 cc-fork `executeShellCommandsInPrompt` 路线）；低信任来源（如项目内技能）未来可按来源信任分级降权（P2 后续，见 §3.2） | 技能再怎么写，真动手时照样过安检；来路越不明、越要降权。 |
| **路径穿越（`file` 参数 `../`）** | 读到技能目录外敏感文件                       | `handle_load_skill` 对 `base_dir.join(file)` 先 `normalize_path` 再断言落在 `base_dir` 内，越界 `Err`；并叠加 `gate_check_path(Read)` 二次兜底                                                                                                                          | 技能附件只能在自己文件夹里读，别想跳出去。        |
| **元数据爆上下文**               | 技能多了挤占对话上下文                       | 预算 `max(2000, 窗口字符×1%)`：超限先截描述(250)、再降级 `name-only`（丢描述不丢技能）+ warning；极端规模启用选择层(P2)                                                                                                                                                                   | 技能多也不撑爆系统提示。                 |
| **坏 SKILL.md 阻断启动**       | 一个语法错的技能导致 agent 起不来              | 发现期 per-file 容错：`diagnostics` 收集、跳过、继续；**绝不** `?` 冒泡到启动路径                                                                                                                                                                                            | 一个坏技能不连累整个 agent。            |
| **同名技能静默覆盖**              | 用户不知道托管技能被项目本地同名盖掉               | first-wins + `warnings += skill_shadowed:<name> by <source>`，在发现诊断中可见                                                                                                                                                                                | 撞名覆盖会留痕，可查。                  |
| **审查子 Agent 被技能污染**       | reviewer/verifier 上下文混入技能正文、放大注入面 | `guard.rs` 默认拒 `load_skill`；仅 `expose_to_reviewer=true` 显式放开（与 web_search 同款门闩）                                                                                                                                                                      | 审查小弟默认不碰技能。                  |
| **技能与插件职责混淆**             | 误把可执行逻辑塞进技能                       | 文档与类型双重约束：`Skill` 无 hostcall 字段；可执行扩展明确推给 `ext/plugin`（`[plugin-system-overview.md](./plugin-system-overview.md)`）                                                                                                                                   | 要跑代码走插件，技能只放说明书。             |


---

## 13. 历史决策 / 跨文档修订

- ~~技能正文随元数据一起常驻 system prompt~~ → **否**：技能正文动辄上千字，常驻会爆上下文。**改渐进式披露**：仅 name+description 常驻，正文经 `load_skill` 按需装载（对齐 codex/openclaw/pi）。
- ~~用裸 `read(<SKILL.md 绝对路径>)` 装载（codex/pi 路线）~~ → **否**：内嵌技能无磁盘路径、托管技能在 `agent_definition_dir` 可写集判定之外、project 本地技能路径又依赖当前 cwd，模型易猜错路径。**改专用 `load_skill(name)` 按名装载**，统一覆盖三类来源且可被 guard 门控。
- ~~技能可携带 `allowed-tools` 临时提权 / inline 跑 shell（cc-fork 路线）~~ → **否**：违背 tomcat「一切副作用经 `PermissionGate` + 审计」。**技能零特权**：`allowed-tools` 本期仅解析记录不强制；正文动作逐次过闸门。
- ~~技能=可执行插件的一种（注册自定义工具）~~ → **否**：tomcat 已有 `ext/plugin`（Wasm）承担可执行扩展。**技能=惰性指令资源**，与插件二分（对齐 pi_agent_rust skill/extension 二分）。
- ~~把技能配置塞进 `[tools]` 子表~~ → **否**：技能是跨工具的独立子系统，非「某工具的磁盘上限」。**新增 `[skills]` 顶层子表**（对齐 codex 顶层 `skills`）。
- ~~本期就做用量统计 / Curator 老化 / 语义检索 / 热重载~~ → **否（范围控制）**：先把静态发现 + 渐进式披露 + 按名装载 + 安全门闩做扎实，自进化与海量检索推给 P4 / P2 后续（见 §3.2）。
- ~~系统自动按回合匹配 + 自动注入选中技能正文（auto-selection + per-turn injection，隐式触发路线）~~ → **否（本期）**：自动注入更「魔法」、误命中代价高（要配阈值 / 可解释性 / 缓存失效一整套），且绕开「模型显式决策 + 工具审计」这条 tomcat 主线。**改 `load_skill(name)` 模型显式按名装载**；自动匹配留作 P2 后续增强（届时叠加阈值与理由日志），与显式装载并存、显式优先。
- **元数据超预算的降级策略** → 选定 **A+D 两级降级**（〔A〕单条描述截 250 →〔D〕整块仍超则全员只留 `name`，丢描述不丢技能）。否决了 codex 的 **C 水填**（逐字符公平分配，最优但实现最重）与直接 **E 按优先级丢技能**（丢掉的技能模型看不见、无法自救）。取向同 codex/openclaw：**丢之前先保全「所有技能可见」**。
- **技能爆量时的选择层（≠ 上面的隐式自动调用）** → 选定 **筛选子 Agent** 方案、列 **P2 本期不做**：无历史、仅系统提示定义 I/O 的子 Agent，吃「用户输入(去历史) + 全量目录账本」，经唯一工具 `filtered_skills(names)` 回传 top-n（n<5），程序 `∪ must_include` 去重后填回主 loop 的 `<available_skills>`。注意它只决定**展示哪几张卡**（display selection），与「自动决定用哪条并注入正文」（auto-invoke，上一条）是两回事。否决 BM25/词法预筛（同义漏召回且静默）与 GenericAgent 式 embedding 服务（要向量库，本地规模用不上）。
- **发现的执行时机与缓存** → 选定 **启动后台 `tokio::spawn` 扫一次 + 首次渲染 `<available_skills>` 前 `await`**：扫描与其余启动初始化重叠、不堵进程启动，且元数据在首轮 prompt 前必然就绪。**不做磁盘缓存**（内存 `SkillSet` 即缓存，同 codex/cc-fork/pi）——理由：目录 `walk`+`stat` 是开销大头、缓存只省 frontmatter 解析，ROI 低且引入 staleness。有界扫描兜底 pathological N（深度 1、只读 frontmatter ≤4 KiB、`max_skills` 封顶）。
- **官方内置 skill 的运行时形态** → 选定 **“编译嵌入 + init 写入 P2 Managed”**：官方 skill 文件放 `tomcat/assets/skills/`，编译嵌入 `.rs`，由 `tomcat init` 解压到 `~/.tomcat/skills/` 后按普通 Managed skill 被发现。**否决独立 P3 Bundled 根**——拒因：把“来源语义”和“实现手段”耦合，发现/优先级/诊断多一层分支，收益小；与 Managed 共用磁盘路径更统一、更可见、更好调试。
- **更新机制** → v1 **仅显式重扫**（聊天内 `/skill reload` + 外层 `tomcat skill reload` + 配置变更 / 新会话重建），简单确定零 staleness。**否决 v1 上文件监听热重载**——拒因：OS 监听器有跨平台 / 死锁 / 事件风暴的真实复杂度（参 cc-fork `skillChangeDetector.ts` 的 Bun `fs.watch` 死锁注释与 300ms debounce）；**热重载（`notify`+debounce，镜像 codex `SkillsWatcher`）列 P2，磁盘 manifest（mtime/size 失效）列 P3 兜底**。

**跨文档修订**：

- 本文新增的 `load_skill` catalog 条目触及 `[docs/tool-catalog.md](../tool-catalog.md)`（派生文档，由 `build_function_definitions()` 自动生成）；不需手动改。
- 本文技能发现根与 `[directory-structure.md](./directory-structure.md)` 对齐并扩展：**P0** Project = `agent_workspace_dir/.tomcat/skills/`（不在 `~/.tomcat/` 数据根内）；**P1** Agent = `~/.tomcat/agents/<agentId>/skills/`（实现期须在 `directory-structure.md` 补节点）；**P2** Managed = `~/.tomcat/skills/`（现有 :40-41）。如实现期调整发现根，须同步相关文档。
- 本文复用 `[permission-system.md](./permission-system.md)` 的 `PermissionGate` 语义、`[plugin-system-overview.md](./plugin-system-overview.md)` 的「可执行扩展」边界；不修改其已冻结正文。
- 竞品事实源：`cc-fork-01/src/tools/SkillTool/`、`codex/codex-rs/core-skills/`、`openclaw/src/skills/`、`pi-mono/packages/{agent,coding-agent}/.../skills`、`pi_agent_rust/src/resources.rs`、`hermes-agent/tools/skill_*.py`、`GenericAgent/memory/skill_search/`。

---

## 附录 A：下期官方内置 skill 资产候选（PR-SK-B）

`tomcat/assets/skills/` 中的官方高频 skill 文件会在编译期嵌入，`tomcat init` 时写入 `~/.tomcat/skills/`，因此它们在运行时按 **P2 Managed** 参与发现，而非单独占一个 P3 根。**本期不做，放下期**；下期首批候选（实现时以 PR-SK-B 为准）：


| 技能                          | 用途                                                                                       | 说人话            |
| --------------------------- | ---------------------------------------------------------------------------------------- | -------------- |
| `skill-installer`          | 参考 codex 的 `skill-installer`：安装 / 列出可安装 skill                                               | 安装和列出官方/策展 skill。 |
| `skill-creator`            | 参考 codex 的 `skill-creator`：指导创建或更新一个 skill                                               | 帮你写 skill。 |
| `code-review`              | 结构化代码审查清单                                                                                | 过一遍审查要点。       |
| `docs-architecture-writeup` | 按 `[ARCHITECTURE_SPEC.md](../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)` 写架构文档 | 写方案时格式不跑偏。     |
| `plan_spec`                | 面向 spec / 方案规划的写作与拆解 workflow                                                           | 把需求整理成可执行 spec。 |


> 官方内置 skill 资产随版本一起评审；新增 / 改 `tomcat/assets/skills/**` 等同改代码，走正常 PR + 测试（与 `prompts/` 模板同口径，不走"纯配置接入"路径）。

---

**一句话总结**：tomcat Skill 系统把技能当成「惰性指令资源」——运行时在 `core/skill` 只按 P0→P2 三层磁盘根（project `.tomcat/skills` > `~/.tomcat/agents/<id>/skills` > `~/.tomcat/skills`）发现 `<name>/SKILL.md`、同名 first-wins 去重成 `SkillSet`；官方内置 skill 文件来自 `tomcat/assets/skills/`，编译嵌入后由 `tomcat init` 写入 P2 Managed，再按普通 Managed skill 被发现；系统通过 `AvailableSkillsSection` 只注入 name+description（预算封顶，渐进式披露），正文经 `load_skill(name)` 按名装载、过 `PermissionGate` 读盘后包成 `<skill>` 回灌 LLM；技能零特权、坏文件容错不阻断、reviewer/verifier 默认门控，可执行逻辑明确推给 `ext/plugin`——磁盘格式 / 发现 / 披露镜像 `pi_agent_rust`，调用机制取 `cc-fork-01`/`hermes` 的专用工具按名解析，配置走 `[skills]` 顶层子表。