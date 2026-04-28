# pi_agent_rust 插件运行时与生态对比报告

**版本**：1.1  
**日期**：2026-04-28（v1.1：增补故障扩散、多运行时、OpenClaw API/SDK 辨析）  
**范围**：汇总 pi_agent_rust 与 pi-mono 扩展兼容性、QuickJS/PiJS 嵌入方式、与 pi-rust-wasm（WasmEdge）及 OpenClaw Plugin SDK 的差异与优劣；供内部设计与评审引用。

**相关材料**：

- `pi_agent_rust/docs/extension-architecture.md`
- `pi_agent_rust/EXTENSIONS.md`、`pi_agent_rust/CONFORMANCE.md`
- Tomcat：`pi-rust-wasm/docs/reports/plugin_systems_openclaw_pi_mono_pi_agent_rust.md`
- Tomcat：`pi_agent_rust-docs/插件与-pi-mono-扩展系统兼容说明.md`
- Tomcat：`pi-rust-wasm/openspec/specs/architecture/plugin-system/wasmedge-runtime-layer.md`
- OpenClaw（若已克隆）：`openclaw/src/plugins/types.ts`、`openclaw/docs/plugins/sdk-overview.md`、`openclaw/src/plugins/sdk-alias.ts`

---

## 1. QuickJS（PiJS）是如何跑起来的？是否需要单独安装「运行时」？

**结论：不需要 Node/Bun 等外加进程。** pi_agent_rust 通过 **`rquickjs`** 将 **QuickJS 嵌入 Rust 二进制**（例如 `extensions_js.rs` 中的异步 QuickJS runtime/context、`PiJS` 桥）。用户安装的是 **单个可执行文件**，其中已包含嵌入式 JS 解释器。

表述上应注意：

- **不是没有「运行时」**：嵌入式引擎本身就是 JS 运行时。
- **是没有「独立安装的 Node/Bun」**：扩展执行不依赖用户机器上的 Node 发行版。

详细架构见 `extension-architecture.md`（JS/TS 入口在嵌入式 QuickJS 中执行）。

---

## 2. pi_agent_rust 如何兼容 pi-mono 插件系统？

兼容主要集中在 **契约与加载语义**，而非 **一比一复制 Node 运行时**：

- **清单侧**：与 pi-mono 对齐的 **`package.json` / `pi.extensions`**、入口发现、`extension.json` 等与扩展相关的约定（详见仓库内兼容性说明与对比报告）。
- **执行侧**：扩展在 **PiJS（QuickJS）** 中加载，通过宿主注入的 **`pi`**（Extension API）注册能力；加载完成后收敛为 **`RegisterPayload`**（JSON），由 Rust 侧统一管理。

因此：**语义上对齐 pi-mono 的扩展模型**；**执行环境并非 Node**，受 `EXTENSIONS.md` / `CONFORMANCE.md` 约束，不能直接假设「任意 npm 包与完整 Node API 均可用」。

---

## 3. 扩展是否跑在 QuickJS 上？共用环境还是各自隔离？

### 3.1 pi_agent_rust（JS 扩展）

- **是**：与 pi-mono 兼容路径下的 **JS/TS 扩展**主要在 **嵌入式 QuickJS（PiJS）** 中运行，敏感能力通过 **hostcall**（如 `pi.tool`、`pi.exec`、`pi.http` 等）进入 Rust，再由 **能力与策略（capability / policy）** 门控。

### 3.2 是否为「一个扩展一个 QuickJS」？

**默认模型是：多个 JS 扩展共用同一套 `PiJsRuntime`（同一嵌入式 QuickJS 宿主）**，按加载规格依次装入；不是「每个扩展单独进程」或「每个扩展独立 Wasm 实例」级别的隔离。

隔离与安全依赖：

- **裁剪的 JS 环境**（非完整 Node；无任意 `node_modules` 解析等，以合约为准）。
- **hostcall + 策略 + 审计**。
- **加载期/静态侧的模块与 builtin 约束**（具体规则见 `EXTENSIONS.md`、`CONFORMANCE.md` 与源码中的解析与校验逻辑）。

### 3.3 安全性（概括）

| 手段 | 说明 |
|------|------|
| API 面收敛 | 敏感 OS 能力不直接等同于 Node 的 `fs`/`child_process` 全集，而走 `pi.*` 与宿主实现 |
| 策略与审计 | hostcall 路径可配置、可记录，便于合规与排查 |
| 合约与一致性测试 | `CONFORMANCE.md`、扩展合规模块等 |

### 3.4 单 QuickJS 运行时加载多插件时，如何防止「故障扩散」？

**先界定预期**：多个扩展共享 **同一 PiJS 进程内 VM**，**不是** Wasm/多进程那种硬内存隔离，因此「一个坏插件拖垮整个 QJS VM」在理论上仍可能发生；工程上靠 **多层软隔离 + 宿主兜底** 控制影响面。

| 机制 | 作用（源码/文档锚点） |
|------|----------------------|
| **生命周期钩子 fail-open** | `startup` / `session_start` 等扩展事件派发失败时 **只打日志，不阻止 Agent 继续跑**（`agent.rs` 注释 *Fail-open: extension errors must not prevent the agent from running*）。 |
| **单次调用超时** | 扩展工具、命令等在宿主侧带 **超时**（如工具执行路径上的 `timeout_ms`），避免单次调用永久挂死宿主线程逻辑。 |
| **错误映射与可恢复路径** | QuickJS 异常经 `map_quickjs_error` 等收敛为 `Error::extension(...)`；扩展加载管线对 **多入口** 场景：主入口失败则整扩展失败，**非主入口**失败可 **跳过并告警**（`ext.load.multi_entry.skipped`），避免一条入口堵死整条包。 |
| **中断预算（interrupt budget）** | PiJS 配置可对解释执行设 **`interrupt_budget`**，超限视为扩展执行超支并映射为扩展错误（见 `extensions_js.rs` 中 `InterruptBudget` 与 `PiJS execution budget exceeded`）。 |
| **扩展预算 / 结构化并发** | `ExtensionBudget*` 等与扩展侧并发、超时Envelope 相关的控制（详见 `extensions.rs` 中预算分层），限制「拖垮整场会话」的概率。 |
| **热重置与冷重建** | 扩展运行时线程内存在 **warm reset** 失败后的 **cold fallback**：必要时 **重建 `PiJsRuntime`**，尽量从「坏状态 VM」恢复（`extension_runtime.warm_reset.error` → 回退冷启动）。 |

**小结**：防扩散的手段是 **不让单次扩展错误阻断主会话**、**限时与预算**、**必要时换一台 VM（重建运行时）**；若产品承诺「插件 A 永远不能破坏插件 B 的内存」，则需要 **更强隔离载体**（见下一节），而不是仅靠同一 QJS。

---

## 4. 能否做成「多个 QuickJS 运行时」？有没有必要？

**能做吗？** 从架构上讲 **可以**：每插件（或每组插件）持有独立 `PiJsRuntime` / 独立线程与 channel，宿主侧仍通过 hostcall 统一向外。代价是 **内存与初始化次数上升**、**跨扩展的 JS 互操作消失**（若 mono 语义曾假设共享全局或同进程副作用，需重新核对）。

**当前为何多为单运行时？**

- 与现有 **加载管线、RegisterPayload 聚合、调度器与 hostcall 队列** 一体化设计相匹配。
- 降低 **进程内实例数量** 与 **启动延迟**。
- 兼容与一致性测试集中在 **单 PiJS 形象**上更易收敛。

**何时值得考虑多运行时？**

- 明确需要 **插件间内存级隔离**（恶意或崩溃隔离），又不走 **Wasm/native** 路径时。
- 运维上愿意为 **按插件限额**（CPU/内存）付复杂度。

**与替代方案的关系**：需要强隔离时，仓库内更自然的方向往往是 **Wasm 扩展路径**（`wasm-host`）或 **pi-rust-wasm 式每实例 Wasm+QJS**，而不是无限堆叠 QJS 进程——需按 **威胁模型与成本** 选型。

---

## 5. pi-rust-wasm（WasmEdge）：是否「一个扩展 = 一个 Wasm + QuickJS」？

**与设计文档一致**：pi-rust-wasm 侧描述为 **全局共享 WasmEdge Engine**，**每个插件对应独立 Store/Instance**，实例内包含 **独立的线性内存与 QuickJS 上下文**（见 `wasmedge-runtime-layer.md`）。会话侧还可按 **`session_id` + `plugin_id`** 管理长生命周期 VM（actor 模型）。

**与 pi_agent_rust 的对比（简要）**：

| 维度 | pi_agent_rust（共享 PiJS） | pi-rust-wasm（每插件独立 Wasm 实例 + QJS） |
|------|---------------------------|--------------------------------------------|
| 隔离强度 | 同进程、共享嵌入式 QJS；靠策略与 API 边界 | Wasm 线性内存与实例隔离更强，故障不易扩散 |
| 资源开销 | 相对轻，一次运行时承载多扩展 | 每插件实例成本更高 |
| 与 pi-mono 的亲缘 | 入口/manifest 语义对齐，运行时非 Node | 意图在 Wasm 内借助 Node 兼容层贴近 pi-mono，但仍受宿主绑定与 WASI 等约束 |

**优劣不是绝对**：更强隔离通常伴随更高开销；更易审计的窄宿主也可能要求扩展侧更多适配。

---

## 6. OpenClaw：`OpenClawPluginApi` 与 Plugin SDK 是不是同一件东西？

**不是同一个东西，是「类型契约」与「/npm 包与入口」的关系。**

| 概念 | 是什么 | 典型用途 |
|------|--------|----------|
| **`OpenClawPluginApi`** | OpenClaw 源码里定义的 **TypeScript 类型/接口**：插件入口 `register(api)` 回调参数 **`api` 的形状**（有哪些 `registerTool` / `registerHook` / `registerChannel` / …）。 | 写插件时声明「宿主会递给我什么能力」；以 **`openclaw/src/plugins/types.ts`** 为准（Tomcat 外仓路径，见对比报告锚点）。 |
| **Plugin SDK（`@openclaw/plugin-sdk`）** | 发布到 npm 的 **多子路径包**（如 `plugin-entry`、`plugin-runtime`、channel 相关子路径等），包含 **类型、辅助函数、各子系统入口**，供插件 **import** 使用。 | 实现插件时从这里拉类型与运行时工具； **`openclaw.plugin.json` 指向的入口** 最终仍要在 Node 里被执行。 |
| **加载器与别名** | `loader.ts` + `sdk-alias.ts` 等将 `openclaw/plugin-sdk/*` **解析到宿主内置真实文件**，减少插件去 `import` 宿主仓库任意内部路径。 | **工程约束**，不是虚拟机级别的能力边界。 |

**插件作者侧「定义了哪些 API」**（摘要，完整列表以 `types.ts` 与 `openclaw/docs/plugins/sdk-overview.md` 为准；下表与 `plugin_systems_openclaw_pi_mono_pi_agent_rust.md` §4.4.2 一致）：

- **与 pi / pi_agent_rust 可对齐的一类**：`registerTool`、`registerCommand`、`registerHook`、以及类型化的 `on(...)` 等。
- **OpenClaw 更宽的产品面**：`registerChannel`，Gateway 相关（如 `registerGatewayMethod`、`registerHttpRoute`），`registerCli` / `registerService` / `registerCliBackend`，多类 Provider 槽位（语音、媒体、生成类等），内存/上下文（如 `registerMemoryCapability`、`registerContextEngine`），以及安全/探针等扩展点。

**扩展是否「只能」用 SDK？能否像 pi-mono 一样用大量 Node 原生 API？**

- **契约上**：应通过 **`OpenClawPluginApi` 暴露的那套 `register*`** 与 SDK 子路径接入，这是 **官方支持面**。
- **运行时上**：仍是 **Node + jiti**，与 **「禁用所有 Node 内置模块」** 不是一回事；**纵深防御**靠文档与产品里的 **沙箱、审批、策略** 等（见 OpenClaw 侧 Sandbox/Approvals 文档），而不是像 pi_agent_rust 那样在嵌入式 JS 里 **没有** `require('fs')`。
- **与 pi-mono 对比**：pi-mono 在 **终端 + ExtensionAPI** 上 **`exec` 等可很直接**；OpenClaw 在 **Gateway/多渠道** 上 **登记面更广**，但 **运行时仍是完整 Node 生态量级**（见对比报告 §4.4.3）。

**三者对照（信任边界）**：

- **pi-mono**：接近完整 Node/Bun，`exec` 等可在 ExtensionAPI 面直达宿主绑定能力（具体以 mono 源码为准）。
- **OpenClaw**：**大号 Node 插件** + **`OpenClawPluginApi` 注册面**；SDK 为官方入口； OS 侧仍靠治理而非嵌入式裁剪。
- **pi_agent_rust**：**小号 PiJS + hostcall**，默认 OS 面刻意收小，便于审计。

---

## 7. 延伸阅读

- 三端插件体系统览：`pi-rust-wasm/docs/reports/plugin_systems_openclaw_pi_mono_pi_agent_rust.md`
- 与 pi-mono 字段级兼容：`插件与-pi-mono-扩展系统兼容说明.md`
- PiJS / hostcall 细节：`pi_agent_rust/docs/extension-architecture.md`、`pi_agent_rust/EXTENSIONS.md`

---

*本报告由对话结论整理，若与源码或上游文档冲突，以源码与 `EXTENSIONS.md` 为准。*
