| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat Agent | 2026-11-21 11:03 +0800 | ACTIVE | feat/web-search-backends | - |

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

### 本轮门禁复验
- Rust lib 复验（`1795222137759-6140uq`）：strict Clippy 通过；3555 通过、8 失败、7 忽略。5 条模型目录测试在独立 HOME 仍失败（`1795224098202-69i811`），去掉从会话目录回退读取的分支，列表和会话夹具都用 Serve 自己刷新的共享目录。另 3 条 Plan 交还测试在独立 HOME 仍失败（`1795224296165-y2stxl`）；运行时夹具补齐与文件相同的 session ID，未放宽所有权检查或事件断言。修后定向复验 27/27（`1795224377348-ms4q2z`，退出 0），含上述 8 条、插件替换及搜索上下文行。整改后全目标 strict Clippy 和 `cargo test --doc` 退出 0（`1795226330713-uo1p2l`，无 doctest 用例）。
- Rust 默认集成（`1795219466777-iyincy`）：386 通过、11 失败、28 跳过。两条 prompt 体积超出原上限 131 字符，删去 Todos tool 描述与参数说明之间的重复，不调门槛。八条 QuickJS 用例的手工实例没有规范化临时目录路径（macOS `/var` 与 `/private/var`）；统一使用真实目录注册接口，不改插件路径校验。空 glob 超时在独立环境原断言通过（`1795224421605-us08ik`）。修后从正式并行组筛选复跑 23/23（`1795226143128-yoldoi`，退出 0）：全部 QuickJS、prompt 体积、生成目录一致性及空 glob。同一门禁的集成串行组 96/96（3 跳过）通过，run `df335c06-1c0b-4bfa-a5b6-c6ed520e8987`；门禁总体退出 1。
- 搜索证据：同一集成批次的 17 条 `web_search_tool_tests` 都通过（真实 QuickJS + 本地 HTTPS，不是真供应商）。覆盖普通缓存命中、Key 轮换/清除、模型地址冲突导致旧缓存不可复用、工作区覆盖隔离；lib 中插件适配器拒绝旧选择，未启动被覆盖插件，下一次查询选中新提供方（`1795222137759-6140uq`）。
- 扩展复验（`1795221275165-uhqbwq`，退出 1）：lint、宿主 965/965、GUI 920/920、构建通过，README 打包 smoke 14/14、会话删除 3/3（含观察者被拒绝后原持有者继续控制）通过。集成共 284 通过、3 失败、1 跳过：两个真实 CLI 初始化超时和一个 SDK 类型检查超时。SDK 原样隔离复跑 1/1（`1795224367120-eqtord`，退出 0）；两个 CLI 场景原样隔离复跑 2/2（`1795224760290-azu40j`，退出 0）。没有放宽时间预算；正式安装阶段未到达。使用缓存 VS Code 1.106.0 的全门禁复验（`1795226915515-yve1om`，退出 1）通过单测和全部扩展集成（287 通过、1 跳过），安装套件 23 通过、6 跳过、1 失败：模型设置宿主断言还写死了 CLI `0.1.x`。保留版本存在、与当前扩展声明一致以及界面显示正确的断言，只删除固定次版本号。原安装用例复跑 1/1（`1795227678648-mra1kx`）；随后补跑 `npm run verify:vsix` 的五组宿主场景 10/10（`1795227790388-a0in14`）。这些使用替身 CLI，不计为候选整包验收。
- `git diff --check`、Rust 格式、发布版本与 Serve Rust/TS wire 声明比对通过（`1795230038700-u99od7`）；候选 CLI 的 `init`、`doctor`、`config`、`session`、`plugin`、`audit` 帮助均正常（`1795230132888-enjrxb`）。
- 按授权补跑 NickMjiang 真实 Responses：隔离 HOME 下，新构建的 CLI `serve --stdio` 使用 `nickmjiang/gpt-6-astra`，接收请求、流式返回预期文本、完成回合并正常退出（`1795225632901-iis3d2`）。Key 仅由该子进程从环境读取，没有输出或写入夹具；这条证据不代替固定 IDATATLAS 的全量真实模型门禁。

### Intel 候选包：真实 VS Code 安装验收
- 用干净的 `f0959fd5`（树 `c0bd59894459e2fa684d4bdec052bc9e8f7b9394`）构建 CLI（`1795228160916-2bn7xu`），另产候选 VSIX。候选 SHA-256 `2f64d4e06c108abf638565baed24b378cfb45749b7e1eb754c31f4e4b0ada69d`；包内 `extension/bin/tomcat` 为 `0755`、Mach-O x86_64、CLI `0.2.1`，其 SHA-256 `b5e3099c000d65106734789ad3e93661b3e055cf9edf4c1c51989494e1626558` 与安装及实际运行的副本一致。原交付包 SHA-256 仍为 `e87291fd72e6afe3fb301dd340b029dbfb1283b0504d035251f7e604653c92f5`。
- 同包三次独立安装：首轮夹具误将 macOS `/tmp` 与 `/private/tmp` 当不同根（`1795229238682-ados9a`），次轮读取了刚销毁的 Webview 上下文（`1795229361882-i8w3td`）。只修测试：对临时 HOME 取实际路径；重载后等待新文档；只读探针跳过已销毁上下文，不重试点击或吞其他协议错误。产品代码、包内文件和功能断言不变。
- `TOMCAT_RELEASE_WEB_SEARCH=1 npm run verify:vsix -- --release-bundle <候选>`（`1795229502317-mzk2hm`，退出 0）：临时 HOME、独立 VS Code 1.106.0 profile、真共享 Serve。冷启动铺好内置搜索插件；合成 Brave Key 保存、移除及服务重启后状态、顺序和启用项正确，Key 不进共享设置或 Serve 日志。原图解码 256px、缩略图 192px，重载后历史图可解码，破损缩略图显示「预览不可用」。回环模型收到一条带图片请求，不计外部模型或搜索供应商实测。6 组最终 PNG、ARIA 和 console（捕获范围内无错误）在 `.agents/acceptance/plan-progress/intel-f0959fd5-3OXDKG/reload-Viwh40/gui-VEegMo`（`1795229598472-6cwwze`）。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| Rust 全门禁未通过 | 首次 lib 阶段因测试夹具等待实际 Git checkpoint 被中止，`NoopStore` 复跑 9/9（`1795221142336-ii83hm`），之后 full lib 暴露 8 条失败。默认集成结果见上 | 不重复已通过的检查；精准复跑整改和超时用例，补齐未通过或未执行阶段 |
| 真实模型门禁未运行 | `integration-real-llm` 退出 2（`1795225101990-mzyk2o`）；当前环境和项目 `tomcat/.env` 均未配置 `IDATATLAS_OPENAI_API_KEY`（`1795225119124-6gs817`，只检查存在性） | 配齐获授权的测试凭据后运行；不计通过，不合入 `develop` |
| DeepSeek 真账号未验证 | 未验证开放平台 Search API 的授权、额度与计费 | 使用明确授权的账户单独验证；本地替身不计供应商实测 |
