
---

# 技术文档编写规范 (Prompt Spec)

## 1. 目标与定位
本规范旨在统一 `pi_wasm` 项目的技术文档格式，覆盖两类文档：**A. 模块文档**（与 `src/` 一一对应的 README）与 **B. 技术方案文档**（`openspec/specs/architecture/**/*.md`，含跨模块设计、时序、状态机、契约等）。一份合格的文档应具备：
- **可操作性**：开发者看后能立即知道如何对接 API。
- **架构清晰**：阐明模块/方案在全局中的位置及依赖关系。
- **防御性设计说明**：记录并发、错误处理、安全等关键设计决策。
- **一眼能懂**：任何读者——尤其是 12 岁原则下的非本模块作者——都能在 30 秒内看懂"这份方案涉及哪些文件，每个文件分别做了什么"。
- **量化可验收**：目标要能翻成用户可感知的观察指标，验收要能映射到实际测试函数名。
- **与实现一致（No-Stale）**：评审/实现过程中调整了设计，文档对应章节**同步修订**并显式标注偏差，绝不留"原计划/现落地"两份真相。

### 1.1 两类文档的落盘位置与职责分工

**A. 模块文档（Module README）**

- **目录**：与 `src/` 子模块一一对应的技术说明，写在各子目录的 **`README.md`**；顶层索引用仓库根 [`src/README.md`](../../../../src/README.md)（编号规则与 ASCII 总览见该文件）。运行时工作区目录树见 [`openspec/specs/architecture/directory-structure.md`](../../architecture/directory-structure.md)。
- **边界**：用户指南（`docs/user-guide.md`）、组内分享（`docs/sharing/`）、进度碎片（`docs/status/`）**不替代** `src/**/README.md` 中的模块文档；代码变更若影响对外行为或模块边界，应同步更新对应模块 README。**已废弃** `docs/technical/`（勿再新增文件）。
- **结构模版**：见 §2。

**B. 技术方案文档（Architecture Spec）**

- **目录**：跨模块设计、关键状态机、传播/取消/事件/持久化等全链路方案，写在 **`openspec/specs/architecture/**/*.md`**；顶层索引与分层摘要见 [`openspec/specs/Architecture.md`](../../Architecture.md)。
- **触发条件**：任一即需新增一份技术方案文档——
  1. 设计跨 ≥ 2 个 `src/` 一级子目录的行为（如本分支的中断/取消跨 `api/cli`、`api/chat`、`core/agent_loop`、`infra/events`）；
  2. 引入新的事件/协议/wire 契约、或修改既有事件的语义；
  3. 引入新的状态机 / 生命周期 / 取消与传播机制；
  4. 在计划文档（`agents/plan/*`）中被点名"开发前需新增架构文档"（参见 [`PLAN_SPEC.md`](../../../../agents/plan/PLAN_SPEC.md) §2"文档先行与阶段边界"）。
- **边界**：模块 README 描述"本模块做什么"；技术方案文档描述"一件事跨多个模块怎么串起来做"。两者并存、互不替代；若某项设计点在方案文档与模块 README 中都出现，**以 `openspec/specs/architecture/` 下方案文档为权威**，模块 README 以回链指向方案文档。
- **结构模版与硬约束**：见 §2B。

---

## 2. 文档结构模版 (Markdown)

### 2A. 模块文档 (`src/**/README.md`)

#### # [模块名称] 模块说明

##### ## 1. 概述 (Overview)
- **职责**：一句话描述模块解决的核心问题。
- **所在层级**：(如：基础设施层 / 核心逻辑层 / 接入层)。
- **核心文件**：模块对应的核心代码路径。
- **建议**：在概述后增加 **ASCII 架构或数据流图**（标出本模块在栈中的位置及与邻模块的边界），并与 [`src/README.md`](../../../../src/README.md) 中的总览图对照，避免重复画整张系统图。

##### ## 2. 设计方案 (Design Details)
- **设计模式**：使用了哪些模式（如：单例、观察者、装饰器）。
- **关键权衡**：为什么要这么实现（Why vs What）。
- **线程安全/并发**：是否支持 `Send/Sync`，原子性如何保证。

##### ## 3. 核心 API 与数据结构 (API Definitions)
> **要求**：使用代码块列出核心 Trait 或 Struct 定义，并附带关键注释。
- **数据结构**：`pub struct ...`
- **接口声明**：`pub trait ...`
- **错误处理**：描述该模块可能返回的特定错误变体。

##### ## 4. 配置项 (Configuration)
- 列出相关环境变量、配置文件字段及其默认值。

##### ## 5. 交互流程 (Workflow / Sequence)
- 描述典型调用链路（如：初始化 -> 注册监听 -> 事件触发）。

##### ## 6. 示例代码 (Usage Examples)
- 至少包含一个"快速上手"代码示例。

##### ## 7. 验收标准 (Testing & QA)
- 覆盖率要求、关键测试用例、性能指标。

---

### 2B. 技术方案文档 (`openspec/specs/architecture/**/*.md`)

技术方案文档描述"一件事跨多个模块怎么串起来做"，读者画像包括下一个接手的工程师、Review 者、以及关心整体链路的产品/架构师。

**章节总览（推荐顺序）**：

| 章节 | 硬度 | 触发条件 |
|---|---|---|
| §2B.1 目标与非目标 | **MUST** | 所有方案 |
| §2B.2 术语统一 | **MUST** | 所有方案 |
| §2B.3 技术原理（Why & How） | **SHOULD** | 依赖非常识性技术基元时（如 `tokio::select!` / `CancellationToken` / ratio 水位线） |
| §2B.4 文件职责总览（One-Glance Map） | **MUST** | 所有方案 |
| §2B.5 关键改动（按文件） | **MUST** | 所有方案 |
| §2B.6 时序 / 状态机 / 控制流伪码 | **SHOULD** | 有跨模块数据流 / 生命周期 / 状态迁移时 |
| §2B.7 契约（Contracts） | **SHOULD** | 新增或修改 Trait / enum / wire 事件 / 错误变体时 |
| §2B.8 验收（Acceptance） | **MUST** | 所有方案 |
| §2B.9 风险与应对 | **MUST** | 所有方案 |
| §2B.10 设计选型与权衡 | **SHOULD** | 存在 ≥ 2 种可选实现、需要解释"为什么选 A 不选 B"时 |
| §2B.11 边界与 Review 清单 | **SHOULD** | 实施期有 ≥ 5 个易踩的坑、或涉及进程/并发/信号等易错场景时 |
| §2B.12 跨文档修订指引 | **SHOULD** | 本方案会改写相邻方案文档中某一节的语义时 |
| §2B.13 竞品 / 外部调研 | **OPTIONAL** | 引入有成熟开源竞品的能力（多 Agent、上下文管理、插件协议等）时 |
| §2B.14 MVP 降级与实施顺序 | **OPTIONAL** | 方案需要分阶段落地、或存在"最小版 vs 完整版"切分时 |

#### §2B.1 目标与非目标（Goals / Non-Goals，**MUST**）

- **目标**：用**观察指标表**替代散文式陈述。每条目标必须写成"落地后用户**可感知**、**可量化**的指标"——如"2 秒内 child 进程死亡""partial 不丢，走同一条持久化路径"。
- **非目标**：显式列出"本轮**不**解决、留待后续"的项，并标注"推给谁 / 哪个任务号"，防止范围蔓延。
- **模版**（取自 [`interrupt-and-cancellation.md §1.1`](../../architecture/interrupt-and-cancellation.md)）：

  ```markdown
  ### 目标
  | 目标 | 观察指标（落地后用户可感知） |
  |------|------------------------------|
  | G1   | 任何时刻按 Ctrl+C，**2 秒内** 当前子进程死亡 |
  | G2   | 中断时已生成的文本与 tool_result **不丢**，与正常完成走同一持久化路径 |

  ### 非目标
  | 非目标 | 推给 |
  |--------|------|
  | 跨 session 的断点续跑 | T2-P1-001 |
  | 心跳 / 正常超时语义   | T2-P0-003 |
  ```

#### §2B.2 术语统一（Terminology，**MUST**）

- 列出本方案内**所有**易混淆术语的精确定义，钉死同义词歧义（例：`interrupt / cancel / abort / steering / followup` 必须在第一节就分清）。
- **每条术语至少包含**（取自 [`context-management.md §2`](../../architecture/context-management.md)）：
  - **语义**：用一句大白话解释。
  - **数据类型 / 载体**：如 `ChatMessage` 上的 `#[serde(skip)]` 字段 / `Arc<Mutex<CancellationToken>>` / JSONL `type: branch_summary` 行等。
  - **行为约束**：触发/失效条件、唯一性要求、与相邻术语的互斥关系。
- **关键时间点必须显式约定**：像"LLM 回复后""发起下一次 LLM 请求前"这类语义模糊的时间词，**必须**在术语表或首节用一句话钉死所指位置，避免读者各自脑补。参考 [`context-management.md §1.1`](../../architecture/context-management.md) 的"关键时序定义"段。

#### §2B.3 技术原理 / 背景（Why & How，**SHOULD**）

- **何时单开一节**：当方案依赖非常识性技术基元（`tokio::select!`、`CancellationToken`、`Waker`、ratio 水位线、drop-as-cancel 语义等）时，**必须**专开一节说清"为什么要用它、它的语义是什么、不用它会怎样"。
- **形式要求**：
  1. **先反例后正解**：先展示"当前实现为什么不够用"（如 `AtomicBool` 不能 `.await`、导致感知延迟 = 剩余 await 时间），再引出正解。
  2. **辅以小幅 ASCII 图**：把抽象状态机画成时间轴或数据流，12 岁原则。
  3. **一句话总结**：节末用一句话把读者的直觉锚住，例如"**drop = 取消**：在 Rust 异步里，drop 一个 future ≡ '再也不会被 poll' ≡ '取消'"。
- **参考样板**：[`interrupt-and-cancellation.md §4`](../../architecture/interrupt-and-cancellation.md)（从 `AtomicBool` 为什么不够 → `select!` 赛跑语义 → `CancellationToken` 工程化封装 → 一次 chat 回合中的 token 生命周期 → 总图）。

#### §2B.4 文件职责总览（One-Glance Map，**MUST**）
- **位置**：紧邻"关键改动（按文件）"章节之前，或作为其 §x.0 子节开篇。
- **内容**：一张 ASCII 图，按**调用层次或数据流方向**把本方案涉及的**所有业务源文件**（`*.rs`）与**独立单测文件**（`tests.rs`，如按 [`RUST_FILE_LINES_SPEC §A`](../coding/RUST_FILE_LINES_SPEC.md) 分离的测试）全部串起来，每个节点内以**简短要点清单**列出该文件本方案内负责的函数 / 类型 / 常量 / 关键行为。
- **硬约束**：
  1. **串联**：节点之间**必须**用箭头（`│` + `▼` 或 `→`）标明"谁调用谁 / 数据向谁流动"，读者自顶向下一遍即可复现完整链路；
  2. **覆盖完整**：本方案在"关键改动（按文件）"小节中列出的每一个 `*.rs`，图中**必须**有对应节点，缺一不可；
  3. **同时标注独立 tests.rs**：每个业务节点下方（或节点内）注明其配套 `[*/tests.rs]`，并回链 [`RUST_FILE_LINES_SPEC §A`](../coding/RUST_FILE_LINES_SPEC.md)；
  4. **大白话说明**：图之后紧跟一段 2–3 句"阅读顺序建议"，用 12 岁原则复述一遍整条链路；
  5. **与实现一致**：若某节点的设计在 impact-scan 后被调整（如签名未改），图中必须以 **【未改签名 / 依赖 Drop】** 等显式标注方式反映**真实落地**，而不是"原计划"。
- **反例**：
  - ❌ 只画分层框（"基础设施层→核心层→接入层"），不落到文件级；
  - ❌ 节点内只写文件名不写"做了什么"；
  - ❌ 无箭头，只是几个框的并列；
  - ❌ 涉及 5 个文件，图里只出现 3 个。
- **参考样板**：[`interrupt-and-cancellation.md §9.0`](../../architecture/interrupt-and-cancellation.md)（本次 T2-P0-007 定稿）。

#### §2B.5 关键改动（按文件，**MUST**）

- 每个文件单开一小节，描述**本方案内**该文件负责的类型 / 函数 / 行为变更，及其与 §2B.4 图中对应节点的映射。
- **No-Stale 要求（硬）**：若存在"旧设计 vs 最终落地"差异（如 impact-scan 后放弃修改某 trait 签名），在该小节内显式记录决策与理由，并在 One-Glance Map 中该节点打上 `【未改签名 / 依赖 Drop】` `【保留 poll 兼容出口】` 等**显式标签**，确保未来读者不被 stale 设计误导。
- **粒度要求**：列到"签名/常量/关键分支"级别。不允许出现"增加了一些错误处理"这类空洞描述；应写"`execute_bash` 新增 `cancel: CancellationToken` 参数；`select!` 分支命中时调用 `child.start_kill()` 后 `wait()` 再返回 `PrimitiveError::Cancelled`"。
- **影响面回扫**：若签名变更，列出**所有实现方 + 调用方**（可用 `rg "execute_bash" src/` 等命令结果）作为清单，避免漏改。

#### §2B.6 时序 / 状态机 / 控制流伪码（Sequence / State Machine，**SHOULD**）

- **按需组合**以下三种形式：
  - **Mermaid 时序图**：用于跨模块数据流，带 `autonumber`；
  - **ASCII 泳道图**：用于终端友好、方便 diff 的核心路径；
  - **控制流伪代码**：用于复杂循环 / 嵌套状态机（参考 [`agent-loop.md §13.3.2`](../../architecture/agent-loop.md) 三层嵌套循环伪码）。
- **双击 / 重试 / 级联** 等有状态迁移的方案，推荐补一张**状态转移表**（当前状态 | 事件 | 目标状态 | 副作用）。
- **事件发布时序**：若本方案会在特定节点发布 `AgentEvent` / `ExtensionEvent`，需配一段"事件树"（参考 [`agent-loop.md §13.6`](../../architecture/agent-loop.md)），标清"哪些事件条件性发布"。

#### §2B.7 契约（Contracts，**SHOULD**）

- 列出跨模块对外暴露的 Trait / enum / wire 事件 / 错误变体，附 `serde` 序列化约定与向后兼容策略。
- **wire 常量必须显式命名**：如 `WIRE_AGENT_INTERRUPTED = "agent.interrupted"`，指明落在哪个文件（通常 `src/infra/wire.rs`）。
- **向后兼容策略要写死**：是"保留旧事件 + 新增事件"（附加式）还是"替换旧事件字段语义"（破坏式）。优先附加式；若必须破坏式，显式列出订阅方清单与同步修改路径。

#### §2B.8 验收（Acceptance，**MUST**）

- 映射到**实际测试函数名**（单元 / 集成 / E2E 场景库编号），形如：

  | 维度 | 具体断言 | 状态 |
  |---|---|---|
  | 单元测试 | `api::cli::chat_cmd::tests::check_double_tap_*`（4 用例） | ✅ 2026-04-22 |
  | 集成/硬验收 | `src/api/chat/tests.rs::interrupt_persists_transcript_hard_ack` | ✅ |
  | E2E（人工） | `E2E-CLI-062 test_user_interrupt_during_bash`（[`E2E_SCENARIO_LIBRARY.md`](../testing/E2E_SCENARIO_LIBRARY.md) Story 8） | PENDING |
  | 观察指标 | G1–G5 的对应锁死机制 | ✅ |
  | 文档 | 本文定稿 + 相邻方案文档同步 | ✅ |

- **任一可交付能力均有可执行断言**：§2B.1 里的每条目标都要在本表中能找到"锁死它的测试"或"锁死它的机制（如纯函数单测）"。
- **状态标**：`✅ 日期` / `PENDING` / `阻塞于 X`；禁止留空或写"待补"。

#### §2B.9 风险与应对（Risks，**MUST**）

- 用表格列出：**风险** | **影响** | **应对**。
- 应对不能是"注意一下"这种空话，必须落到"加一个 `tokio::time::timeout(2s)` 兜底""transcript `append_message` 返回前必须 fsync"等**具体动作**。
- 至少覆盖：并发 / 信号 / 兼容性 / 回滚 / schema 破坏 / panic 隔离 / 资源泄漏 等横切面中与本方案相关的那几项。

#### §2B.10 设计选型与权衡（Decision Matrix，**SHOULD**）

- 存在 ≥ 2 种可选实现时，**必须**用表格记录：**决策点** | **默认选择** | **替代方案** | **选择理由**。
- 参考样板（取自 [`interrupt-and-cancellation.md §11`](../../architecture/interrupt-and-cancellation.md)）：

  | 决策点 | 默认选择 | 替代方案 | 选择理由 |
  |---|---|---|---|
  | 取消机制 | `tokio_util::sync::CancellationToken` | `AtomicBool + Notify` / 纯 `Notify` | 广播 / clone / 父子 / 幂等齐全；零运行时开销 |
  | 取消深度 | 外层 + 内层双保险 | 仅外层 / 仅内层 | 外层保"不等"，内层保"真杀进程"，互补 |

#### §2B.11 边界与 Review 清单（**SHOULD**）

- 列出"实施期必须逐条核对的坑位"，编号列清单。每条内容 = 标题 + 一句风险 + 回链到本文某小节。
- 目标读者：**代码 Review 者**。看完这张清单他就能按图索骥去 diff 里找"有没有踩坑"。
- 参考样板：[`interrupt-and-cancellation.md §12`](../../architecture/interrupt-and-cancellation.md) 的 10 条清单（token 重建时机 / context_state 一致性 / trait 签名直改 / 进程全局单例 / 双击纯函数抽取 / renderer flush 对称 / ……）。

#### §2B.12 跨文档修订指引（**SHOULD**）

- 若本方案会让相邻方案文档某一节**语义改变**（例如 `interrupt-and-cancellation.md` 会修订 `agent-loop.md §13.2` 的 Abort 语义），必须单开一节：
  - **引用原文**：先抄原文一段；
  - **修订要点**：列出 3–5 条"删哪句 / 加哪句 / 改哪句"；
  - **同步时机**：指明"实际文本改动随实现提交一起发出，本节仅登记修改意图"还是"本 PR 已同步修订"。
- 这样相邻文档的作者与订阅本 repo 的读者都能从一处看到"这次方案让哪些文档需要同步改"。

#### §2B.13 竞品 / 外部调研（Research，**OPTIONAL**）

- **何时需要**：本方案引入的能力在开源界已有成熟竞品（多 Agent 编排、上下文压缩、插件协议、取消语义等）。
- **形式**：一张"调研对象 + 关键结论"表（参考 [`multi-agent.md §14.0.1`](../../architecture/multi-agent.md)），列出项目 / 触发机制 / 上下文隔离 / 完成通知 / 深度限制 / 关键设计亮点。
- **必有"本项目选型理由"段**：编号列 3–5 条理由，说明为什么从对比表里挑了这几家的哪几点，放弃了哪几点。
- **价值**：让 Reviewer 一眼确认"设计不是拍脑袋"，也便于新人快速建立心智模型。

#### §2B.14 MVP 降级与实施顺序（**OPTIONAL**）

- **何时需要**：方案需分阶段落地、或存在"最小版 vs 完整版"的切分。
- **形式**：按 Phase 1 / 2 / 3 列"新增文件 + 修改文件 + 能力"，每阶段可独立上线。参考 [`multi-agent.md §14.8`](../../architecture/multi-agent.md) 与 [`interrupt-and-cancellation.md §14.1`](../../architecture/interrupt-and-cancellation.md) 的"最小版 vs 完整版"表。
- **强调**：每阶段末尾必须能 demo，不允许出现"Phase 2 做一半但没法单独验证"。

---

## 3. 写作风格要求

1. **精准定位**：明确引用代码中的文件名和具体的符号（Struct/Enum/Fn），带行号（如 `run.rs:665`）更佳；跨文件引用用 Markdown 相对路径链接。
2. **术语统一**：严禁混用术语（如：一会儿叫 `Plugin`，一会儿叫 `Extension`）；术语表定义 → 全文只允许用定义里写死的那个词。
3. **关键时间点要钉死**：像"LLM 回复后""发起下一次请求前""读到新输入后"这种模糊词，**必须**在术语表或首次出现处用一句话约定精确位置，否则读者会脑补出不同的行为。
4. **图文并茂**：概述优先用 **ASCII 图**（易 diff、易在终端阅读）；复杂时序或类关系可补充 **Mermaid**。模块文档的图示分工见 `src/README.md`。**技术方案文档必含一张"文件职责总览图（One-Glance Map）"**，参见 §2B.4 的硬约束与样板。
5. **Why 优先于 What**：避免写"实现了一些错误处理"，要写"通过 `AppError` 枚举统一封装，支持从 `io::Error` 自动转换；不使用 `unwrap()`，对不可恢复场景用 `expect("context")`"。遇非常识性技术基元（`tokio::select!`、`CancellationToken`、ratio 水位线等）必须单开一节讲"为什么用它 / 不用它会怎样"，见 §2B.3。
6. **与实现一致（No-Stale）**：当 impact-scan / 评审 / 实现过程中调整了既有设计（如某 trait 签名决定不改），文档**相应小节必须同步修订**并在节点/段落内显式注明 `【未改签名 / 依赖 Drop】` `【保留 poll 兼容出口】` 之类的标记，避免未来读者按 stale 描述行事。
7. **目标可量化、验收可执行**：§2B.1 的每条目标都应对应 §2B.8 表格里的"一行测试或一行机制"；出现"应该""尽量"等非量化措辞时，要求作者重写为"X 秒内""不丢 Y"等可观察的断言。
8. **表格优先于散文**：选型对比、状态转移、风险应对、观察指标、验收映射、术语差异——**能用表格就不用段落**；表格 header 统一、状态列必填。
9. **小结收尾**：篇幅 > 300 行的方案文档，在结尾补一段"一句话总结"把全文收束成一句大白话（参考 [`interrupt-and-cancellation.md §4.6`](../../architecture/interrupt-and-cancellation.md) 末尾的"一句话：……"）。

---

# 示例：《基础设施层说明》

以下是根据上述规范重写的部分示例，你可以对比差异：

# 基础设施层说明 (Infrastructure Layer)

## 1. 概述
- **职责**：为上层模块（Agent, Plugin, Tool）提供统一的错误处理、配置管理、日志记录、跨平台文件操作及解耦的事件总线。
- **设计原则**：最小依赖、强类型约束、零 Panic。

## 2. 核心模块详解

### 2.1 统一错误处理 (AppError)
- **位置**：`src/error.rs`
- **设计**：采用 `thiserror` 实现的枚举，确保错误上下文可追踪。
- **关键变体**：
  - `Primitive(String)`: 原子工具执行异常。
  - `Plugin(String)`: WASM 运行时或插件逻辑错误。
- **最佳实践**：禁止使用 `unwrap()`，对于不可恢复错误使用 `expect("context")`，优先使用 `?` 向上抛出。

### 2.2 配置中心 (AppConfig)
- **加载机制**：
  1. 默认值 (`Default::default()`)
  2. 配置文件 (`pi.config.toml`)
  3. 环境变量 (前缀 `PI_WASM__`)
- **校验逻辑**：`validate_config` 在系统启动阶段执行，若关键路径（如 `sessions_dir`）无写权限，程序将拒绝启动。

### 2.3 异步事件总线 (EventBus)
- **位置**：`src/event_bus.rs`
- **并发模型**：`DefaultEventBus` 基于 `Arc<RwLock<...>>` 实现，支持跨线程安全访问。
- **核心接口**：
```rust
#[async_trait]
pub trait EventBus: Send + Sync {
    // 注册监听，返回 ID 用于后续注销
    fn on(&self, event: &str, callback: EventCallback) -> EventListenerId;
    
    // 异步分发事件，不阻塞调用方
    async fn emit_async(&self, ctx: EventContext);
    
    // 插件卸载时一键清理，防止内存泄漏
    fn remove_plugin_listeners(&self, plugin_id: &str);
}
```

## 3. 交互流程：事件处理
1. **注册**：插件系统启动时，调用 `on` 注册 `extension_loaded` 事件。
2. **触发**：`wasm_plugin` 模块加载新插件，调用 `emit_sync`。
3. **分发**：`EventBus` 根据 `priority` 排序，依次执行回调。
4. **异常处理**：若某个回调 Panic，`EventBus` 会通过 `catch_unwind` 捕获并记录日志，不影响其他监听者。

## 4. 示例：如何添加一个新配置
1. 在 `src/config.rs` 的 `AppConfig` 结构体中添加字段。
2. 在 `validate_config` 函数中增加校验逻辑。
3. 在 `tests/config_test.rs` 中添加环境变量覆盖测试。

---
