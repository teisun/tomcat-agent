| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat Agent | 2026-11-21 08:50 +0800 | ACTIVE | feat/web-search-backends | - |

### ✅ DONE (已完成/进行中)
- CLI 和共享 Serve 启动时补齐内置网页搜索插件，VS Code 用户不必先执行 `tomcat init`。
- 内置插件负责 DeepSeek 官方 Search API 和 OpenAI 官方 Responses 搜索。默认顺序为 DeepSeek → Tavily → Brave → Serper → MiMo → OpenAI；缺依赖则跳过，不覆盖用户已保存的名单。
- 在 VS Code「设置 → 工具 → 网页搜索」管理后端顺序、参与开关和 Key。默认选中「网页搜索」二级标签；「去配置」行内填写，「已配置 ▾」查看来源或管理 Key。
- OpenAI 搜索固定使用 `gpt-6.2-sol`，只复用地址为官方域名的模型 Key，不改变聊天模型或网关。旧模型 `capabilities.web_search` 会被忽略。
- 计划完成后保留绑定；重启按保存的当前计划恢复，不因编辑另一个计划而跳转。摘要记录压缩开始时的状态和相关文件的路径、更新时间；不再列已完成计划的清单，只有会话 Todos 文件比它新才列 Todos。
- 修复共享 Serve 下图片只显示占位框的问题；缩略图失败时明确提示「预览不可用」，且不再被验收探针计为成功。精简 Tools 页文案，去掉多余滚动条，保留滚动、键盘操作和输入换行。
- 已交付的 `darwin-x64` VSIX 是扩展 `0.2.19` / CLI `0.2.1`。本次是合并验收，不覆盖原包、不发布。合并基线：远端 `develop` `c75239bc` → 本地 Todos 前置 `ed1f49f8` → 功能分支原提交 `1b1360e5` → 待验收整改。
- Rust 定向两批 20/20（`1795183782094-eyxr1w`）、81/81（`1795186078138-qt8nbv`）：覆盖同一插件内的搜索回退、被覆盖插件不得调用、目录发现与权限、Stop/恢复、冷建、压缩、上下文行及工具目录一致性。两批早于本次最后的 PDF／模式恢复整改。扩展 lint 及 965 个宿主单测、920 个 GUI 单测通过；不等于已通过宿主集成或安装态。
- 扩展 `gate:full`（`1795186626383-is4z47`）通过 lint、单测和构建，在集成阶段退出 1（285 通过、2 失败、1 跳过），因此没有进入后续 Host 安装步骤。README 的跨目录相对链接在打包后生成 `HEAD/..`，改为仓库绝对链接，原 smoke 断言不变，定向复跑通过（`1795187509330-yrxhu9`）。会话删除保留现有 `session_in_use` 冲突响应和拒绝、持有者后续操作断言，修正过时夹具预期（`session_delete.rs:83-85`），待复跑。
- PDF 测试在独立 HOME、stdin 关闭时复现；采样进入本地 Responses HTTP 及重试路径（`1795187412690-qgvesk`），不是停在命令行输入口。原用例改用 Wiremock，隔离标题请求，验证 PDF 消息通过能力检查后确实发出一次 Responses 请求，且转录保留 PDF；未改产品重试逻辑。修后定向 `cargo nextest run --lib` 83/83（`1795215667058-63rzjg`），含 PDF、冷索引、会话交接、摘要快照和插件适配；另有扩展设置/会话状态定向 308/308 与真包 harness 类型检查（`1795217994644-mtxqmt`）。

### 🔍 编码规范评审（阶段结论）
- **架构与编码总纲（§1/§3/§4/§6）**：Core 决定搜索顺序与缓存，Ext 调用生效插件，Serve 管理设置。仍从 `scope > agent > global` 发现资源，按 `point` 取高层、同层 first-wins 并警告；`unsupported_backend` 不跨插件回退。修复缓存遗漏插件与模型代次、设置写回执被刷新吞掉、旧会话 Stop 保留他人执行绑定。`session/manager/types.rs:285-315` 区分模式与文件状态：完成另一 Plan 不能把当前会话从 PLAN 改到 CHAT；旧转录保留回退。Serve 的 Rust/TS 声明相符，后续仍需全门禁与安装态结果。
- **Rust 文件行数（L-2/L-3）**：搜索设置入口 148 行，路由 220 行，插件适配 196 行；边界独立。`serve/commands.rs`（4149）和 `plan_runtime/mod.rs`（1985）是已有长文件：本次保留前者的命令派发和后者的绑定职责，搜索设置、文件持久化、会话 Todos 分别留在子模块；不为行数搬动无关代码。业务与测试继续分文件。
- **Rust 惯用法（I-1/I-4/I-6）**：沿用 `Result`、所有权检查和已有锁；冷建复用单行缓冲，搜索预检不再复制全部函数声明。`cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings` 通过（`1795219466777-iyincy`，Clippy 用时 6m14s；其余阶段独立记录）。
- **注释（§1/§4）**：写清提前回复附件根、显式空绑定、冷索引和同一文件版本取时间与清单的理由；`User_Stories.md` 和 `E2E_SCENARIO_LIBRARY.md` 跟进搜索设置、Plan/Todos 恢复与图片验证。`T2-P1-012` 的 DONE 及旧 `auto` 策略是历史交付记录，不作为本次证据。相对 `origin/develop` 的 18 个有改动的集成测试目标均已有并行、串行或显式真模型归属；未增测试目标，不改分组。Rust doctest 阶段正常退出（当前无示例用例，`1795219466777-iyincy`）。

### 🔌 INTERFACE (接口变更)
- `tools.web_search.backends` 接受有序后端标识名单；单项为指定后端，多项按序回退。兼容旧 `backend`，旧 `*_base_url` 忽略并告警。
- 插件 `web_search.backend` 的 `metadata.backends` 声明可用后端、依赖和默认顺序。新占位符 `{{host_key}}` 按 HTTPS 目的主机查 Key，仅在请求头替换；项目插件和显式路径插件不能借用已保存的 Key。
- Serve 新增 `web_search_settings`、`set_web_search_routing`、`clear_provider_key`，保存 Key 复用 `set_provider_key`。设置读取按窗口工作区解析生效插件，路由和凭据更新对共享 Serve 生效。
- 会话记录新增内部 `plan.current` 事件，恢复索引升级到 schema 7；无该事件的旧会话仍按最后一条带计划 ID 和路径的事件恢复。共享 Serve 在 `initialize` 阶段就返回 `attachmentRoot`，不再等待项目选定。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| Rust 全门禁未通过 | `gate-full`（`1795219466777-iyincy`）中 Clippy 和 PDF 用例通过；lib 阶段多条用例长时间排队，采样显示等待 `serial(env_lock)`，持锁用例的 Tokio 收尾仍在等实际 Git checkpoint（`.agents/acceptance/plan-progress/rust-lib-gate-sample.txt`）。采样后部分用例陆续通过；lib 进程被终止，不能计作通过 | 请求/转录测试使用已有 `NoopStore`，保留专门 checkpoint 用例的断言，不改产品 Git 路径；9/9 定向复跑通过（`1795221142336-ii83hm`），全目标 Clippy 和 lib 复验（`1795222137759-6140uq`），集成仍待结果 |
| 扩展全门禁未通过 | README 失败已复跑闭环；会话删除需用新源码构建后复跑。安装阶段尚未执行 | 扩展全门禁复跑（`1795221275165-uhqbwq`）：lint、宿主 965/965、GUI 920/920 已通过；构建与后续阶段单独判定 |
| 真实模型门禁未运行 | 缺 `IDATATLAS_OPENAI_API_KEY`，`integration-real-llm` 退出 2 | 配齐获授权的测试凭据后运行；不计通过 |
| 候选 VSIX 安装态未执行 | 原交付包只核过内容与版本且 SHA 未变；新增安装态场景已过 TypeScript 检查（`1795187913090-6b2bdh`），不是运行证据 | 前置门禁完成后另产候选包，在临时 HOME / VS Code profile 验证设置和实际图片解码 |
| DeepSeek 真账号未验证 | 未验证开放平台 Search API 的授权、额度与计费 | 使用明确授权的账户单独验证；本地替身不计供应商实测 |
