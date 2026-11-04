| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat Agent | 2026-11-04 21:47 +0800 | ACTIVE | develop | - |

### DONE：隔离测试自动化整改（本地提交，未发布）

```text
外层 Agent 保留保护标记
  → 已绑定私有 HOME / workspace / profile 的测试子进程净化环境
  → 现有 gate 与真实 VSIX / MCP / connectors / Multica 自动验收
  → 只停自有服务；停服失败保留现场并报错
```

- **范围**：只改测试、夹具、验收脚本、CI、规则说明与文档。产品运行时代码、CLI guard / 拒绝文本、Multica backend 不改；未新增令牌、验收服务、存储格式或依赖。未执行 self-install 或正式后台重启；本次仅提交到本地，不推送或发布。
- **实现**：移除 Rust gate 的整体 unset，在实际需要的 CLI 创建点绑定私有 HOME 并净化；保留通用 `cmd()` 的标记以覆盖负例。Rust / Go 停服失败保留 HOME，补双实例正常停止隔离。TS 复用现有 `serveFixtureEnvironment`，connectors 每个 phase 自行停服，只有收尾成功才删除安装根；Multica 默认测试校验 CLI 与 backend 合并后的环境。扩展三组快速回归复用现有 CI 步骤。
- **工具与证据**：nextest、rg、Python/Pillow 均私有放在 `.agents/acceptance/20261104T110014Z-isolated-tests/`，只在本轮 shell 使用，不安装到全局。命令、退出码、完整门禁、原始失败、VSIX/CLI hash 与资源核对见该目录 `README.md`；背景及实现取舍见 `../reports/isolated-test-automation-background-and-remediation.md` 第 8 节。

**提交阶段核对**：本次代码差异与已验收快照一致，已检查 staged / unstaged / untracked 并将相关文件全部纳入。按用户要求仅检查提交范围、状态与差异格式，不重跑测试、构建或视觉验收；下表保留实施阶段的实际结果，不冒充本次重新运行。Cov% 未测量，保持 `-`，提交信息不编造覆盖率。

| 本轮检查 | 结果 |
|---|---|
| Rust 定向边界 | gate 探针 6、nested guard 4、guard 分类 7、内部 Bash 标记 1、hydration 6、ACP 24、shared service 13 通过；2 个既有 system-proxy ignored 不计通过 |
| Rust 默认 gate-fast | `1793795970608-qgnftz` 整条重跑 exit 0；Clippy、lib 3476、parallel 389 / serial 90 passed；既有跳过不计通过，完整日志与原失败分别保留 |
| 扩展 gate:full | exit 0；宿主 918、GUI 890、harness 279 passed / 1 skipped；fake VSIX 6 个非零测试阶段通过，不代签真实服务 |
| 原生 Release VSIX / MCP | `1793795515456-2bx15w` exit 0；Release 三阶段各 1 passing，真实 MCP 1 passing；外层标记均为 1 |
| 真实 connectors | `1793795319690-id4eow` exit 0；四个 phase 1 / 3 / 1 / 1 passing，覆盖 6 个不同用例，不以画像切换产生的 pending 计通过 |
| Multica | 默认环境回归先红后绿；真实 opt-in `1793794835146-e6mdwd` exit 0，执行 / 取消 / 恢复与 registered backend 均通过 |
| 补充静态与断言检查 | runner / harness 严格 tsc、Rust fmt 通过；保留严格 denied-history 判定的 5 项快测和 Agent-side MCP 入口 5 项快测通过（与其他批次重叠，不累计总数） |

### INTERFACE：隔离测试整改

无显著对外接口变更；生产 CLI guard、配置格式、协议和 Multica backend 均不变，仅调整测试启动与清理边界。

### BLOCKED：隔离测试整改

**失败保留与边界**：Rust 首轮 lib 缺 rg 的失败、两项 CLI 定向失败及原始红测保留，后续通过不解释为偶发根因已经修复。真实 VSIX 改为在重载页面断言历史工具状态，MCP 在页面就绪后导航，connectors 使用 macOS 短临时路径，均为测试侧修正。历史失败根 `/var/folders/81/9fjf_p9n5f90_p5_mlsghtnr0000gp/T/tc-JWOSdE` 按约定保留；最终自有进程和正式后台身份核对见证据。未做 Rust / Go 停服失败注入，perf 仅编译；未验 Windows/Linux/arm64、收费模型及真实账号授权。17 张最终 PNG 的配套 ARIA / console 已核查、图像已逐张读回；checker 自检有且仅有其预期 console.error，普通捕获无错误。窄空窗原有禁用 composer 工具栏裁切沿用下方历史记录，不宣称全界面无裁切。

下列发布与实施记录按当时快照阅读；其中“本机无 nextest / 未跑真实 MCP”等旧环境结论，不代签或覆盖本节本轮结果。

### CURRENT RELEASE：CLI 0.1.85 / EXT 0.2.3（已发布并完成制品核验）

```text
已确认 CI 故障修复 → 91fe2746 标准 CI 五项通过 → develop/main/master 快进同步
  → CLI 0.1.85 四份归档与校验清单下载通过
  → EXT 0.2.3 四平台原生验收通过 → 五份 VSIX 校验与内置 CLI 比对通过
  → Marketplace 四平台公开 Validated；下载字节与 GitHub 已验收包一致
```

- **发布身份与顺序**：两个正式 tag 均指向 `91fe2746008b1c1064b794873149c225cd9bb34f`；EXT 0.2.3 pin CLI 0.1.85。先确认 CLI 公开资产并实际下载核验，再推 EXT tag；三分支仅 fast-forward，没有强推、移动标签或替换旧资产。版本镜像、tag guard 与 CHANGELOG 同提交，0.2.2 及更早日志历史不改。
- **已确认故障及修复**：普通冷启动会预写空 MCP，0.2.1 的目录不存在断言错误，已只修夹具断言并以 0.2.2 续发；本轮修复 MCP 诊断的 needless borrow、并发启动请求过早返回导致迟到子进程在显式 stop 后重新拉起服务，以及 checkpoint 夹具无关完整 init 超时。复用原锁、身份、启动期限和清理机制；保留中断/恢复断言，不加超时、不跳过用例。
- **标准 CI**：发布源码的 develop [41796868754](https://github.com/teisun/tomcat-agent/actions/runs/41796868754) 与 main [41801529625](https://github.com/teisun/tomcat-agent/actions/runs/41801529625) 均已 completed success，各五个 job 全过：三系统 Rust、Windows 扩展、Windows 系统代理/真实 VS Code。三个分支的 version-sync 成功；main 最终状态由 `1793783672048-i0ieuu` exit 0 于 09:14 UTC 单独确认，不再以此前的执行中状态代替最终结果。后续验收记录提交仅改两份状态文档，复用本源码结果，不重复产品门禁。
- **CLI 正式资产**：[release-cli 41801722176](https://github.com/teisun/tomcat-agent/actions/runs/41801722176) 成功；[CLI Release](https://github.com/teisun/tomcat-agent/releases/tag/cli-v0.1.85) 四平台归档与 SHA256SUMS 已公开。`1793782714076-a3r8hh` exit 0，四份归档同时匹配公开 digest 和下载清单，归档内为对应普通二进制；本机 x64 发布二进制 `--version` 为 `tomcat 0.1.85`，没有安装。
- **EXT 正式资产与原生验收**：[release-ext 41803846821](https://github.com/teisun/tomcat-agent/actions/runs/41803846821) 七个 job 全过，含 darwin-arm64、darwin-x64、linux-x64、win32-x64 的 exact VSIX 原生验收与 Marketplace 上传；[EXT Release](https://github.com/teisun/tomcat-agent/releases/tag/ext-v0.2.3) 四个平台包、纯包与 SHA256SUMS 已公开。五份下载 SHA256 通过，`1793783418480-uujglg` exit 0，ZIP/身份/target/版本/pin/入口均正确；包内 changelog 与 tag 源码字节一致，四个内置 CLI 与 CLI 归档逐字节一致，纯包无 CLI。
- **市场最终状态**：OIDC 四平台上传成功；09:16 UTC 公开 Gallery 的 0.2.3 四个平台均为 Validated，`1793783242260-uz64i0` exit 0，四份实际公开下载的 SHA256 同时匹配 Gallery 属性与 GitHub 已验收包。最初仍返回 0.2.2、随后 macOS 尚未验证的阶段记录保留，没有重复上传。OpenVSX 的发布 step 为 skipped（`1793783672048-i0ieuu`），未改发布开关或凭据。
- **原始失败与验收限制**：旧 Windows 专项 run [41787591287](https://github.com/teisun/tomcat-agent/actions/runs/41787591287) 的单次 exit 1 根因仍未查明；修复候选随后两轮同专项通过，用户明确选择 `release-current-green-candidate` 后继续发版，不把后续绿色说成旧根因已修复。第一次一次性包核验器误用大写 `extension/CHANGELOG.md` 而 exit 1；实际包条目为 `extension/changelog.md`，仅修读取路径并补齐检查，未重下或更改制品。本机没有 nextest，使用已完成的定向 cargo test/Clippy 与云端标准 CI；不声称本地跑过 nextest。
- **安全与保留边界**：保留 CLI 0.1.84、失败未发布的 EXT 0.2.1、已发布 EXT 0.2.2 及其 tag/资产。未安装到在用环境、改配置/凭据/PATH 或停止/重启正式共享服务；收到新 CLI 后，用户须先检查活动任务再显式重启，重启会停止全部任务。本轮相对 0.2.2 未改 Webview 结构/样式；未人工下载复阅本轮 CI PNG，不冒称全界面视觉复核或真实收费模型验证。旧动态插件串会话、QuickJS 等历史风险与原始失败继续保留，Cov% 未测量。
- **证据与文档范围**：本地细节及 hash 清单在 `.agents/acceptance/release-0.1.85-0.2.3/{release.md,repair.md,cli/,ext/,marketplace/}`，不强制纳入 Git。本次只回填本权威状态与根状态入口；当前克隆仅本地 `feature/cli-chat`，不运行整表聚合，避免删除看板中其他 feature 的历史碎片。

下方为实施和较早发布的历史记录，按当时阶段阅读；其中“未发布/仅本地提交”的当前交付状态由本节更新，原始失败和独立遗留边界不被覆盖。

### DONE：889b90d7 五项整改（Agent 自动验收完成，未发布）

```text
独立 workspace_required → 绿灯 + 打开文件夹引导
Reload(A) → 目标重连 + 同次变更的其他来源后台连接 → 未变来源保留
定向回归 / gate:fast / Rust clippy → 当前 Debug VSIX → 隔离 A → B → 旧数据升级
```

- **范围与复核**：按计划完成空窗口独立状态、GUI 共享 protocol 类型、迟到 ready 守卫、公开 `vscode.openFolder` 命令、Reload 其他变更来源恢复连接、中英文文档整理，以及冗余 VSIX 自定义 MCP 场景/重复守卫删除。`cwd_lazy` 子进程过滤器由模块路径推导并断言 `1 passed`。对照计划审阅 diff，未新增配置、依赖、存储格式或 wire 版本；默认 MCP 补建/保留、懒换 HTTP Client、诊断脱敏和后端 `scope_requires_cwd` 实现未改。
- **新增回归边界**：Reload 用例既验证新增 `late` 自动连接，也验证目标被删除而返回错误时，配置已变的 `late` 仍能重连；未变 `sibling` 的 PID/generation 保持不变。未启动注册表仍由现有 `started` 条件限制，不扩散到其他工作区。

| 检查（在对应包目录执行） | 本次结果与 task 证据 |
| :--- | :--- |
| `npm ci && npm --prefix gui ci` | `1793763527402-25db8o` exit 0；锁文件未变。npm 对锁定依赖报告宿主 21、GUI 39 项安全告警（含 high/critical），本轮未升级依赖，不将告警说成已修复。 |
| `npx --no-install vitest run src/ui/webview/tests/provider.test.ts`；`npm --prefix gui run test -- src/App.test.tsx` | `1793763611538-i7ouuj` exit 0；95/95、100/100。 |
| `npm run gate:fast` | `1793763780047-6dy9zf` exit 0；宿主与 GUI lint、宿主 918/918、GUI 890/890。定向结果有重叠，不累计总用例数。 |
| `npx --no-install vitest run tests/vscode_launch_env.test.ts tests/playwright_mcp_smoke.test.ts`；harness tsc；脚本严格 tsc | `1793763730685-elecny` exit 0；28/28，Agent MCP 冒烟仍在检查/安装 VSIX 前被拒绝。类型检查分别为 `npx --no-install tsc -p e2e-harness/tsconfig.json` 和 `npx --no-install tsc --ignoreConfig --noEmit --strict --skipLibCheck --esModuleInterop --resolveJsonModule --types node,vscode --target ES2022 --module ESNext --moduleResolution Bundler scripts/run-vscode-verify-vsix.ts scripts/realServeFixture.ts`。 |
| `cargo test --lib reload_connector_rereads_disk_and_preserves_unrelated_ready_source`；`cargo test --lib api::chat::permission::cwd_lazy::tests`；`script -q /dev/null cargo test --lib dismisses_and_forwards_when_stdin_not_tty` | `1793763730621-ef20f6` exit 0；分别 1、16、1 通过，PTY 确实触发父测试及其非 TTY 子进程断言。 |
| `cargo fmt --check`；`cargo clippy --tests -- -D warnings`；`cargo test --test mcp_startup_tests`；`cargo build --bin tomcat` | `1793763805997-urkixd` exit 0；静态检查通过、二进制层保留/写入失败测试 2/2、CLI 构建成功，`file` 为 Mach-O x86_64。 |
| `npm run package:vsix -- --target darwin-x64 --bundle-binary ../tomcat/target/debug/tomcat --out /tmp/tomcat-first-launch.vsix`；`TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR=/tmp/tomcat-first-launch-artifacts npm run verify:vsix -- --release-bundle /tmp/tomcat-first-launch.vsix` | `1793765412329-2889gi` exit 0；VS Code 1.106.0，A 空窗口、B 同 HOME 有项目、旧数据升级三轮各 1 passing / exit 0。Agent 自行执行，未交给用户；外层 Agent 标记保留，沿用临时 HOME/profile/扩展目录与清理机制。 |
| 制品与资料核对 | `1793765620264-ia22n8` exit 0；包内 CLI 与当前构建 SHA256 相同；空窗口 ARIA 有 `Connected` 与 status、无 alert；`git diff --check` 通过，入口文档搜索未命中 `Start Setup`。 |

**真实窗口证据**：产物在 `/tmp/tomcat-first-launch-artifacts`；逐张读回 `gui-GKiuvU/empty-window-{desktop,narrow,settings}.png` 与对应 ARIA、console，以及 `empty-window.json`。桌面 299×808 与窄栏 169×752 的绿灯、引导及唯一打开文件夹按钮均可见，Settings 可用，三个捕获范围 `errors=[]`；窄栏底部原有禁用 composer 工具栏仍裁切，与下方历史记录一致，本次未改变该布局、不声称全界面无裁切。B 的 `gui-nJNGzn` 与升级的 `upgrade-v0.1.90/gui-COXsZR` 保留运行记录及辅助截图；上游模型回复为既有本地脚本，不是真实付费模型验收。本次视觉交付图仅为上述三张空窗口证据。

**制品身份**：`/tmp/tomcat-first-launch.vsix` 是验收用 Debug 包，扩展 0.1.99 / CLI 0.1.83，宿主、Node、Electron、包内 CLI 均为 Intel macOS/x64。VSIX SHA256 `f5b64689caf88d979addffb9982653444b8f0011c701881983d4e572e389ff24`；CLI SHA256 `4dbfb2262865ccfca168d966b7679250990a656383eb0a7cad32c6912c3ee6a7`，与 `release-artifact.json` 一致。

**提交阶段**：按用户要求只核对 staged / unstaged / untracked、差异、空白及提交规范，不重跑测试、构建或视觉验收；上表为此前实施阶段的真实结果。本次范围为五项整改及其测试、文档、状态记录，共 20 个文件，验收包和截图不纳入 Git；仅本地提交，不推送远端。Cov% 未测量，提交信息不填写数值。

### INTERFACE：本次整改

- 扩展内部 `WebviewConnectionStatus` 新增 `workspace_required`，GUI 直接复用该类型；Rust wire 协议不变。
- `ConnectorRegistry::request_reload` 返回契约不变；注册表已启动时，同次配置变更涉及的其他来源通过现有后台连接入口恢复。未新增对外配置或存储格式。

### BLOCKED：本次整改的边界与风险

本次计划范围内无待处理阻塞；既有告警和未覆盖项如下，不把本地定向验收扩大为全平台发布许可。

**失败与未覆盖项**：首轮 fmt（`1793763730747-yoexje`）指出两处本次 Rust 增量的排版问题，修正后上述 fmt/clippy 通过；未出现功能测试失败。构建的 chunk 大小、扩展打包数量和 Electron 启动参数告警原样保留。未运行完整 Rust 全套、Windows/Linux 原生窗口、原生文件选择器实选或真实 Playwright MCP 冒烟；不将定向检查或公开命令的单测冒充这些验收。未升版、推送、self-install、安装到用户在用环境或停止/重启在用共享服务；Cov% 未测量。

以下为上一轮实施的历史记录，保留原始失败和证据；其中自定义 MCP VSIX 子场景已由本轮删除，保留契约由本轮二进制层 2/2 测试覆盖。

### CURRENT WORK：空窗口首启、默认 MCP 与旧连接恢复（本机范围验收完成，未发布）

```text
代码与定向回归 → 用户独立终端真实 VSIX A→B → PNG、ARIA、console 已读回
             └→ Rust 挂起用例修复并补验；parallel 387 / serial 89 → 扩展 Serve 10/10 → 本机验收完成
```

- **范围**：Rust 发出 `scope_requires_cwd`，扩展只显示打开文件夹引导；普通启动锁内补建缺失 MCP、保留已有文件；spawn 建议/JSON 脱敏/Reload 重读；主聊天最终本地 Connect 无状态码时，下次 resolve 换 Client、同 route provider 缓存失效、保留原 Semaphore。历史、工具结果和重试额度不改；未升级 wire 版本。
- **证据边界**：有工作区、跑过 init 或预写空 `mcp.json` 的旧夹具，**不能代签真新用户首启**。`verify:vsix -- --release-bundle <本次VSIX>` 现新增无 cwd 空窗口 A、同 HOME 有文件夹 B、自定义 MCP 保留子场景。普通冷模型夹具仍预写空 MCP 防止测试启动 npx。仅包模式跑该画像；无参数 fake Serve 不算数。
- **定向证据**：`1793722060066-uoq0ff` connection 29；`1793722712195-esetde` transport_recovery 5 / resolver 23 / Anthropic 48 / builtin 6 / serve control 10 通过（有重叠，不累加总数）。子进程 HTTPS_PROXY 红绿只证明 Client 冻结及报告后请求抵达新代理，不证明现场旧会话恢复。
- **审查后的修正**：Reload 收口到 `ConnectorRegistry::request_reload`，用接收方自己的 config；补主请求失败标记，避免前置 compaction 失败误刷新；代理子进程有 30 秒总预算且超时终止；新 `mcp_startup_tests` 已登记门禁。测试复用现有框架，不新增诊断协议、代理 watcher 或重试机制。
- **失败记录不抹掉**：初次 connector 批次 94 通过/1 个 builtin 路径断言失败（macOS `/var` 与 `/private/var`），修正后 builtin 6/6；GUI `openFolder` 类型漏同步、MCP 测试调用私有 i18n、测试 `err().expect()` 的 Clippy 问题均已修正。额外脚本类型检查最初使用错误的 TS6 命令参数，改为下表的 ESNext/Bundler 配置后通过；这不是产品运行失败。一个误写模块过滤器的 platform 检查运行 0 例，不计通过，之后正确路径执行 3 例。

| 检查批次 | 结果与证据 |
| --- | --- |
| 扩展 `npm run gate:fast` | `1793723349554-mbvh3z`：lint、宿主 918/918、GUI 890/890 通过（含 Connectors 23）；随后 harness tsc 通过。整条包装命令后续因额外脚本 tsc 参数失败，不把其总退出码写成 0。 |
| 夹具纯单元部分 | `1793723173176-yvqx64`：`real_serve_fixture_proxy` 按 describe 过滤，7 通过/1 个真实 Serve 用例未执行；runCli 为 mock，不启动/停止服务。 |
| VSIX runner 类型检查（推导检查） | `1793723974878-ac4fsv`：`tsc --ignoreConfig --noEmit --strict --skipLibCheck --esModuleInterop --resolveJsonModule --types node,vscode --target ES2022 --module ESNext --moduleResolution Bundler scripts/run-vscode-verify-vsix.ts scripts/realServeFixture.ts`，exit 0。 |
| MCP 二进制测试 | `1793723200847-q46rdv`：connector 9/9、普通启动补建 2/2；后者保留 Agent 标记，在隔离 HOME 下只运行允许的 session list。 |
| 主聊天及重载边界 | `1793723974811-ihjtrw`：reload 1/1、chat suite 53/53、测试分组登记 1/1；该包装命令后续 Clippy 失败，修正见下项。 |
| 静态检查与补验 | `1793724530920-avcngj`：定向 Clippy、带总预算代理红绿 1/1、platform 通用 3/3、fmt，exit 0。`1793724577993-m02yzy`：`cargo clippy --tests -- -D warnings`、fmt、解析脱敏 1/1，exit 0。 |
| 最终加强的代理配置断言 | `1793724960481-a9zoza`：确认非法代理返回 `config.proxyInvalid` 的英文源错误，而不只判断 is_err；定向测试 1/1、`cargo clippy --tests -- -D warnings` 和 fmt 全部通过，exit 0。 |
| 资源与 diff | `1793724609485-m4f4dk`：中英 JSON 可解析、3 个新诊断 key 均非空、`git diff --check`，exit 0。 |
| 用户独立终端 Rust 门禁 | `rust-gate.log`：all-targets Clippy 通过；doctest exit0（0例，不计用例通过）；parallel 387通过/27跳过、serial 89通过/3跳过。库批次因下述旧用例挂起而 SIGTERM，原 gate-fast 总体失败保留，不称整条门禁一次全绿。 |
| 扩展真实 Serve 补验 | 用户独立终端，2026-11-04 08:13；`serve-integration.log:11-22`：`serve_error_recovery` 2/2（自动Retry/Resume保留原输入与工具结果）、`real_serve_fixture_proxy` 8/8（含真实Native代理请求），共10通过、exit_code=0。构建使用当前CLI 0.1.83。 |
| stdin 挂起修复补验 | `1793749764757-ifrh5s`：所属 `api::chat::permission::cwd_lazy::tests` 16/16、`cargo clippy --tests -- -D warnings`、fmt，exit0；`1793750639339-i7afdz`：显式关闭stdin与真实PTY输入分别1/1、exit0，Agent标记保留。仅改一个测试，复用未受影响成功结果。 |

**真实 VSIX 已验收**：用户独立终端执行了 `.agents/scripts/verify-bootstrap-1793747829.sh`。`vsix.log` 的 A 空窗口、B 同 HOME 配模型并对话、自定义 MCP 保留、旧数据升级四轮均 exit 0；本次包的 SHA256 与准备记录一致。已逐张读回 `visual/` 下 27 份 PNG、配套 ARIA 和 console：A 仅引导与打开文件夹按钮、无警告，设置可用；B 已补建 Playwright 配置并使用包内 CLI 完成对话；自定义 MCP 逐字保留。26 份组件 console 的捕获范围 `errors=[]`；全工作台另有测试 harness 的 Marketplace 查询 404 及 VS Code 警告，不声称全宿主零告警。390px 窗口产生的 169px 侧栏中，原有禁用 composer 工具栏有裁切，引导按钮仍完整可用；本次未改 composer 布局。

**Rust 门禁发现的既有测试问题**：`dismisses_and_forwards_when_stdin_not_tty` 原先假定 stdin 大概率不是终端。外部门禁的 PID14664 实际持有 `/dev/ttys001`，调用栈停在 `cwd_lazy::read_choice_from_stdin → read_line`（`rust-lib-hang.sample.txt`）。对照该二进制 `--list` / `--list --ignored` 的3480例：父批次3474已通过、5忽略，唯一未结束的是此例。首次3475成功/4忽略的统计误把在子进程成功的 ignored 辅助用例 `session_io_worker` 算入父批次，此处已校正。保留原日志并只终止该挂起测试进程后，用户已启动的门禁继续 doctest/integration；原 lib 的 SIGTERM 失败不改写。只修改该用例：终端下以显式空 stdin 的子进程运行相同断言，30秒超时且 kill-on-drop；产品权限逻辑和 Agent 标记不变。所属模块16例、显式空stdin与真实PTY各1例及Clippy/fmt均通过，原挂起已补齐；不重跑不受影响用例，也不改写原 SIGTERM 记录。

**本轮必需验收已齐**：受影响的扩展真实 Serve 两文件已由用户独立终端补跑，10/10且exit0；原包装脚本因Rust库中断而未运行第三段，此次只补缺项，不重跑已通过VSIX或未受影响测试。原计划各功能与不触发边界、离线隔离、真实首启A/B、自定义配置保留及文档均已核对。**未覆盖范围**：Windows PATH 分类及原生首启仍留到发布前补验；不宣称现场旧会话已恢复或新发布已生效。

**用户追加的真实 Playwright MCP 已取证（2026-11-04）**：独立终端运行 `.agents/scripts/verify-playwright-mcp.sh`，同一验收 VSIX 安装进 VS Code 1.106.0 的隔离 HOME/profile，实际使用包内 CLI。`.agents/acceptance/playwright-mcp-9kWtT0/visual/` 留存自动生成的 `@playwright/mcp@0.0.79 --headless` 配置；Settings 显示 Connected / 24 tools；发现、描述、导航、页面快照、截图、console、关闭共7次工具调用均 `isError=false`。页面快照含随机标记，9个本地模型请求的最后一份确含1280×720、28409字节PNG，测试页console为0错误/0警告。只有模型指令使用本地脚本，MCP和浏览器是真实进程，网页为隔离本地页面，不代签公网网站或真实收费模型。

**追加验收的失败与限制保留**：VS Code 内测试1 passing/exit0，但外层原命令exit1：验收收集器误要求导航结果内联正文，实际返回快照链接。已改为校验导航URL、快照随机标记和真实PNG回流；原始请求及证明数据逐字不变，离线重核 `1793753735893-yklvub` exit0，哈希与原失败见 `visual/verified/postcheck.json`；没有将原run改写为全绿或重跑窗口。收集器5例、启动环境23例、打包14例及相应类型检查通过；启动环境旧断言只认一次launch，现逐个检查A/B两次且保留Agent标记。13组PNG/ARIA/console已读回：最终MCP功能图为连接面板与浏览器返回图；其余为准备/调试。窄窗口把Settings挤至2px，聊天侧栏169px有裁切，不能作为窄屏UI通过证据；MCP专项删除无关的整窗390px缩放，不修改产品布局。细节见该目录README。收尾 `npx tsc` 曾SIGSEGV、根因未确定，原 `1793754145164-y5862q` 不计通过；掉线恢复后同源码直接调用本地TypeScript，`1793758113155-iarfj5` 类型/diff通过，`1793758098484-e38nc2` 制品/原始证据哈希与13组三联完整性通过。

**验收包**：`1793747849607-abnzpj` 构建本次 CLI、`1793748045212-igeau7` 构建并打包 darwin-x64 VSIX 均通过。`1793748165278-647pjp` 确认包内 CLI SHA256 与本次构建一致、版本和平台正确。制品位于 `.agents/acceptance/bootstrap-1793747829/`；这是验收用 Debug 包，不是发布包。独立终端入口保留 Agent 保护且已验证会拒绝 Agent 启动。下方保留分步命令供后续复验；此次有效的 VSIX 结果无需重跑。

后续在独立终端复验的命令（本次无需重复；当前计划是 Intel macOS，若设置了 `CARGO_TARGET_DIR`，将下方 CLI 路径改为对应目录）：

```sh
cd /Users/yankeben/workspace/tomcat-agent/tomcat &&
bash scripts/run-integration-tests.sh gate-fast &&
cargo build --bin tomcat
```

```sh
cd /Users/yankeben/workspace/tomcat-agent/tomcat-vscode-ext &&
npm run test:integration -- tests/serve_error_recovery.test.ts tests/real_serve_fixture_proxy.test.ts &&
npm run package:vsix -- --target darwin-x64 --bundle-binary ../tomcat/target/debug/tomcat --out /tmp/tomcat-first-launch.vsix &&
npm run verify:vsix -- --release-bundle /tmp/tomcat-first-launch.vsix
```

真实窗口证据已按 runner 输出目录读回；不得拿无参数 fake Serve、旧包或只有 Ready 状态代签。

- **交付边界**：基于 `37321560` 的本次本地提交包含首启、MCP、旧连接恢复及相关测试/文档，共57个文件；按用户要求，提交阶段只做三类变更区域、差异、空白与提交格式核对，不重跑测试、构建或视觉验收，上方证据均为此前实施验收记录。Cov% 未测量，不填覆盖率。修复前 `.bak` 备份、`.agents/` 临时脚本和验收产物仅本地保留；未推送、升版、安装到在用环境或发布，未停止/重启真实共享服务。旧 `tomcat.path` 仍可能指向未修复程序；PATH 变化需用户显式重启（停止所有任务）。后续 VSIX 必须包含本次 CLI。具体行为见 [MCP 架构](../architecture/mcp-client.md)、[LLM README](../../src/core/llm/README.md) 与中英用户指南。

### WINDOWS TARGETED ACCEPTANCE：共享后台与 ACP 的 Windows 缺口补验（2026-11-02）

```text
f836f03f 的 Windows 已通过项 ──────────────┐
唯一失败：对照任务漏传环境 → 单例修复 4efb1a17 ─┴─> 本轮 Windows 缺口补齐
  产品代码不变；四组已有窗口证据读回通过；修复已择取到 develop，标准 CI 保持不变
```

- **当前结论**：按 Windows 专项计划完成取证、测试夹具局部修复和定向补验；未重跑 Mac/Linux、完整 Rust/扩展套件或真实付费模型。本节更新下方历史记录中 Windows 待验及远端 CI 缺失的状态，不覆盖 real-llm、旧式动态插件串会话或个人 Windows 设备兼容等未验事项，不是发布许可。
- **精确基线**：开工时本地与远端 develop 已为 `f836f03f3d34c84e8c12252b85872bc19d51d291`，tree `4fcc83098a03a5114d082cd6b09045cb1c1189ed`；工作树干净。补验阶段仅推送 `acceptance/windows-f836f03f`，未写入 develop/main、未开 PR、未安装或重启在用服务；随后用户授权将三个已验收提交择取到 develop 并推送，见下方合入记录。
- **根因与修复**：`tomcat/tests/acp_stdio_e2e.rs` 的慢读取消测试为 Native 对照连接传了空环境，Windows 因而找不到 Git Bash，对照写入器没有启动；受测 ACP 任务已拿到 PID。失败发生在准备阶段，不是取消阶段。`289db042` 保留 Native 工具结果和两条 PID 的诊断；`4efb1a17` 仅为该用例接入已有 `open_with_environment`，提供 PATH/PATHEXT/SYSTEMROOT 及隔离 HOME/USERPROFILE，不继承密钥/代理，不改公共夹具默认值或产品代码。

| 批次 | 提交 / 运行 | 实际结果与边界 |
| --- | --- | --- |
| 原 Windows CI，复用成功项 | `f836f03f` / [41397562092](https://github.com/teisun/tomcat-agent/actions/runs/41397562092) | Windows Clippy、库3442通过/5忽略、并行369通过/27跳过；串行85通过/1失败/3跳过。三个 Git 探测用例均通过。扩展host915通过/1跳过、GUI888通过、根CMD包装器2通过/0跳过、代理保护与构建选择9通过。原run仍为失败，不改写。 |
| 加诊断，精确复现 | `289db042` / [41402467311](https://github.com/teisun/tomcat-agent/actions/runs/41402467311) | Windows fmt、目标Clippy及新CLI构建通过；精确1例失败。日志显示 other 的 Bash 为 Git Bash 缺失错误，job PID=4340、other PID=None，确认测试漏传连接环境。 |
| 修复后补验 | `4efb1a17ca41ac4ab45f371c07aa2388dd6209b7` / [41404375858](https://github.com/teisun/tomcat-agent/actions/runs/41404375858) | job `125142979395`，仅Windows，success。fmt、`cargo clippy --test acp_stdio_e2e -- -D warnings`、当前CLI构建通过；T实际执行1例、1通过，耗时2.970s。20例/9个binary被过滤，不计通过。 |

- **断言未放宽**：程序比对确认该用例之外的测试与共享助手不变，取消部分逐字不变。实际覆盖两条活进程和真实背压；目标进程在恢复读取前退出、另一会话继续写入；恢复读取后正文完整、唯一取消终态、至 EOF 无迟到输出。原期限、权限和停止流程不变。仅T内接线/诊断变化，未以同文件修改为由扩大回归。
- **四组真实窗口制品已逐张复核**：原run的 `windows-shared-service-f836f03f…` 制品中，`shared-service-completed`、`restart-menu-desktop`、`restart-menu-narrow`、`restart-reconnected` 均通过 PNG/ARIA/console 读回。宽图1391×808、窄图341×752；完成消息、可读的 `/restart` 菜单、重连后的历史及空可编辑草稿符合既有场景。四份捕获范围内 `errors=[]`，不代表全程宿主零告警。重启由PID4200换为7508；随后关窗前后及禁止启动的Native重连后，PID7508/instance保持不变。系统代理回执 `restored=true`。不重跑窗口，不冒充个人设备或其他窗口场景验收。
- **源码与结果对应**：验收分支除该测试、临时验收CI和本状态文档外，产品、依赖、测试分组及UI与f836f03f相同，原成功项继续有效。原Rust基线镜像为20261030.294.1；本次诊断与修复后两轮均为20261021.278.1，均属windows-2025-vs2026。红→绿发生在相同镜像，其他成功项保留其原环境证据，不声称整个套件在补验镜像上重跑。本次是分批补齐，不声称原始完整门禁一次全绿。GitHub对Node20 action迁移到Node24的告警保留，不在本轮升级CI依赖。
- **本次合入**：按用户后续授权，使用 `cherry-pick -x` 将诊断 `289db042 → bf183325`、修复 `4efb1a17 → 5b153edb`、验收记录 `28327c7e → 3436489b` 无冲突择取到 develop，保留原提交来源；本状态及自动汇总看板随之回填。`baf7cc74` 临时CI提交未合入，`.github/` 与原develop基线逐字一致；测试文件与已通过Windows验收的 `4efb1a17` 逐字一致，复用其有效结果，不重复本机测试。此次按授权正常推送 `origin/develop`，不强推、不改main、不升版或发布；新SHA的常规CI结果须另行查看，不用旧结果冒称新CI全绿。
- **本地证据索引**：项目资源目录 `.agents/acceptance/windows-shared-serve/` 下的 `baseline-evidence.md`、`diagnosis.md`、`acceptance.md`，以及 `baseline/`、`diagnostic/`、`fixed/` 的原始run/job、日志和制品。临时PNG查看器已停止；查看器自身的初次加载失败不计产品故障，交付使用未修改的原CI图片。

下方 14:00 的本地合并结论及更早记录保留原样，按当时状态阅读；本节更新本轮 Windows 补验及择取合入状态。

### CURRENT INTEGRATION：共享后台与 ACP 接入审查后本地合并（2026-11-02）

```text
develop D=86081c31 ──────────────┐
feature/shared-serve-acp S=2b76074a ─┴─> 本地 no-ff 合并
  已测实现 + 分批补验                    ├─ 产品树与 S 完全相同
                                       ├─ 仅回填 develop status / INTEGRATION
                                       └─ 不推送、不安装、不重启正式服务
```

- **本轮结论**：按批准计划完成原 15 提交 / 385 路径的六组审查与有证据的局部修复；本机离线验收范围已由有效批次覆盖，执行本地合并。Windows、真实模型及对应新提交的 CI 仍未验证，见下方“推送前必须补齐”；不是发布许可，也不是“全平台无遗留阻塞”。源分支状态片段记录的是合并前状态，合并结果以本节为准。
- **精确基线**：D=`86081c3121a183ffd1b65997998f88f7fd26f7e0`；最终 S=`2b76074a04bb4063c56d011ac95ca1660e8da92f`。预演与 `git merge --no-ff --no-commit S` 后的树均为 `458479f1bb1d41f54a2566a9f3cabedbe5636e2a`，`git diff --cached S` 为空，无冲突；D..M 为 396 文件 / +29488 −3982。合并提交前只再修改本文件并运行原 aggregate-status 脚本生成 `docs/INTEGRATION.md`，两者为说明性回填，最终树相对 S 的其余路径必须完全相同。
- **修复边界**：Native 先核对服务编号再发送环境，仍直连且管理协议版本为 1；控制者关闭与 ACP 取消复用原停止入口；页面重建保留选择、迟到流式开头不重字、checkpoint 接管自动恢复；Windows Git 探测先授权、限期回收。另修已复现的 busy 遗留、SessionManager 弱引用、IPC/PATH 错误分类与测试设施问题，不加 relay、状态平台或插件初始化生命周期。逐项根因、失败→通过及独立提交见 [源分支审查记录](feature-shared-serve-acp.md#合并审查与验收本机离线验证完成待本地合并)。

| 层 | 本轮有效证据与边界 |
| --- | --- |
| Rust B2/B4 | B2 完整库 3454 passed / 5 ignored；B4 `1793595769842-ex75gj` 的全目标 Clippy、parallel 384 / 27 skipped、serial 89 / 3 skipped 通过，库 3453 passed / 1 failed / 5 ignored。唯一新暴露的旧压缩夹具问题经重复句柄确定性复现，`9d48409e` 仅显式解锁；`1793597923702-qgd17f` 的所属模块 24 项、fmt 与 lib/tests Clippy 通过。原 B4 exit 1 保留，复用其未受影响步骤，不重复累计用例或声称整条命令一次全绿。 |
| 扩展 B2/B4 | B2 `1793594442583-2fzg8u` 完整 gate:full exit 0：host 916、GUI 888、integration 271 / 1 skipped、安装态 24 / 6 条件 pending、verify:vsix 10。B4 `1793598458874-yr40gj` extension tsc 后 npm GUI lint 进程 SIGSEGV / exit 139，根因未确认；同源码、依赖及并发下 `1793598602816-a62zbn` 只补 GUI tsc 与完整单测，host 916 / GUI 888、exit 0。保留原崩溃，不据 swap 使用量断言 OOM。 |
| 真实 VS Code / UI | B3 `1793595245110-u2j0ap`：Production 扩展自动换文档，仍是旁观 B、草稿/只读/焦点保持、真实 Bash 完成，1 passing / 26s。最终桌面/窄屏两组、空窗口两组及 B2 七组 PNG/ARIA/console 已读回，捕获范围 errors=[]；B2 错裁到 Explorer 的 cropped 图仅调试，视觉证据使用原始图。不冒充原生文件对话框实选或全宿主零告警。 |
| 两份宿主集成包 | `1793587030689-y4z2d0`：Paperclip 46、Multica 23 个清单路径与哈希一致；四份 patch 在包中声明的精确隔离基线 `git apply --check` 通过。未在宿主仓运行套件或改部署；旧包的验收 SHA 不代签本次 S。 |

- **复用条件已核对**：`9d48409e` 相对首轮待合并树只改一个测试和源 status；`2b76074a` 只订正文档引用并回填 B4。产品、依赖、分组及 UI 未变，B2/B3 和 B4 未受影响步骤继续有效；测试夹具的变化已补所属模块与 Clippy，不把旧夹具结果当新结果。所有原始失败、环境缺项与解释都保留。
- **资料检查边界**：自动看板逐字汇总当前 develop 加 7 个 feature 分支。`git diff --check` 报告的看板四处行尾空格原样来自 `feature/transcript-rich-render` 的 status 第 264/268/269/278 行；保留原始失败，不修改他人分支或手工改生成结果。四份上游补丁与原样技能附件也保持字节；最终空白检查只精确排除这些保真资料及聚合看板，其余代码/说明仍检查，不关闭全局规则。
- **版本与安全边界**：CLI `0.1.81`、扩展 `0.1.97`、bundled CLI `0.1.81`，未升版。只用隔离 HOME/profile；未安装、发布或停止/重启在用服务，未修改用户配置、代理和凭据，未移除 Agent 保护。保留源分支及其它 worktree；未推送、打标签或清理用户工作。Cov% 未测量。

### 推送前必须补齐 / 需单独决策

| 项目 | 未完成原因与后续动作 |
| --- | --- |
| Windows 真机 | 未提供执行环境。补 Git 探测拒绝/允许/失败审计/超时回收、根 CMD 安装脚本与 Windows 侧用例；Mac 结果不代签。 |
| real-llm | 默认必需 `IDATATLAS_OPENAI_API_KEY` 未配置，真实 API 层未运行。由用户在私有环境准备后补对应层，不输出密钥。 |
| 新 SHA 的 CI | 本轮禁止推送，当前修复没有远端 CI 证据。获准推送后再核对对应提交的三平台和 Windows 系统代理 CI，不使用旧绿色记录或伪造 runner 环境。 |
| 旧式动态插件串会话 | 已复现：第二会话动态发现会向第一会话追加消息；develop 已有同一机制。保留 `.agents/acceptance/shared-serve-review/legacy-dynamic-discovery.repro.patch`，需另定初始化会话归属，不擅自改生命周期；按获准计划不阻塞本地合并，未修复。 |
| 其它保留风险 | local_socket 旧并发失败后在组合与完整库中通过，但根因未确认；Multica 上层 30 秒 / backend 65 秒取消预算差异未实测。原生文件选择、拖拽及既有 Windows 待验事项不由本轮代签。 |

日志在 `~/.tomcat/agents/main/tool-results/bash-<task-id>.log`；本机视觉制品位于 `.agents/acceptance/shared-serve-review/` 与 `.agents/shots/shared-serve-review/`，不纳入 Git。临时回看服务已停止。下方保留 2026-10-22 的历史验收，**旧 SHA 的云端成功不代表本轮通过**。

### 此前接入验收（2026-10-22，仅历史记录）

```text
Paperclip / Multica → 标准 ACP → 共享 Tomcat 后台
VS Code            → 原生连接 → 同项目实时只读旁观
正式 HTTP          → llm.proxy / 系统代理（不读入口 shell 代理）
```

- **最终验收**：代码 `401b6f0cbd8be8478b5453815450f7ce6ada00e7` 的[同一轮 GitHub CI](https://github.com/teisun/tomcat-agent/actions/runs/39765237685)五项必需检查全部成功：macOS/Linux/Windows Rust、Windows扩展、Windows系统代理与真实窗口。下面保留各阶段原始记录；旧的待验/失败描述不是当前结论。文档收口不改产品、测试、依赖或CI配置，复用该代码修订的结果。
- **云端实跑数**：macOS/Linux各为lib3424、并行集成379、串行86 PASS；Windows为lib3411、并行365、串行82 PASS；三平台fmt/严格all-target Clippy均通过。各平台原有lib5 ignored、集成27+3 skipped不计通过；两项系统代理专用用例由专项显式执行，均1 PASS。Windows扩展core803、GUI865 PASS。
- **Windows实际证据**：win25-vs2026 / Node22.23.3 / VSCode1.106.0；13次实际代理请求，含Native/ACP模型、两次MCP tools/call与窗口模型请求，设置已逐值恢复。三条真实Job/受限ACP边界用例通过；真实窗口写文件并退出后，PID7200与instanceId `d1cc9b74-a5a9-4ff8-8b7c-f542c527ba18` 不变，禁止启动的Native重连成功。已读255×808 PNG、ARIA及console，任务/完成结果可见，窄栏省略与横向滚动保留，捕获范围内errors为空；不宣称全程宿主日志零错误，也不代表所有个人Windows环境。
- **历史异常保留**：首轮生命周期用例曾在输出合法JSON后非零退出，原日志缺退出码，根因仍未证实；本轮该原用例在三平台均通过，增强诊断和原失败日志保留，不把补诊断说成已定位修复。不将宿主无关基线失败或未运行项目改记为通过。

- **交付范围**：共享服务生命周期、标准 ACP、显式会话 cwd、原项目隔离、只读旁观、控制断开取消并等待真实子进程停止；ACP 问题放入普通回复，Native 保留提问面板。按复核计划修正慢读完整输出、协议/CLI 版本门槛、准入锁与迁移说明；未新增续跑/WebSocket 或按会话代理机制。
- **首轮本机自动化验收完成（本次 CI 修复的相关复验另记）**：Rust gate-fast exit 0：lib 3422 PASS / 5 ignored，parallel integration 379 PASS / 27 skipped，serial integration 86 PASS / 3 skipped，fmt/严格 Clippy 通过；扩展 core 800、GUI 865、完整默认 devhost 43 PASS / 7 条件场景 pending。仍有效的扩展集成 236 PASS / 1 skipped 与宿主相关测试复用，不把跳过项计为通过。
- **实际操作**：本地真正启动 Paperclip、Multica，以受控模型完成任务、取消、原会话续接及工作树清理；真实 `0.1.80` / 测试构建 `0.1.81` CLI 均跑宿主任务，真实 VS Code 版本提示、重启、恢复以及关窗后同一后台存活已留 PNG/ARIA/console/实例证据。另在 Multica `TOMC-10` 实际完成两轮 DeepSeek 问答，同会话、同评论线程继续；首轮 4 次工具尝试均被隔离权限拒绝，不声称零工具调用。授权两轮已用完，临时真实密钥已清除，测试配置已恢复。
- **代理边界**：正式共享后台统一保存的 `llm.proxy` 或系统代理；仅窗口测试入口可显式写入隔离 `llm.proxy`，ACP 环境不转换，子进程环境原样保留。macOS 系统代理按用户要求仅只读打印 IP/端口，未改全局设置，不将它算作请求链路实测。
- **提交与 CI**：`f6e627f6` 已推送 `feature/shared-serve-acp`，首轮 run `39709693529` 的 Linux Rust、Windows 扩展通过；macOS Rust、Windows Rust与 Windows 专项失败，未关闭门禁。当前补修 HTTP 测试 socket 模式、测试连接环境、Windows 管道关闭观测及 Node 启动链路；检查点清理复用原 CLI 的 30s 预算，ACP/管理关闭等待 60s，Multica 宽限 65s，超时仍失败并保留句柄。并发启动一次非零退出的原因尚待新增退出码/服务日志定位，不能算已解决。
- **本批 CI 修复的本机证据**：Rust 定向库 251 PASS、ACP 两目标 26 PASS、共享服务 10 PASS / 2 ignored，strict all-target Clippy/fmt/diff check PASS；扩展启动器/代理相关 17 PASS及严格 TS 检查；Multica Tomcat 单测与显式当前 CLI 的真实注册 backend 执行/取消/续接通过（本地受控 HTTP 模型）。容量用例初版误读列表权限的失败、首次冷编译期代理用例超时、Go helper 等待不足均留档并定向复验。未再付费调用模型；完整 gate-fast 的同修订结果由下一轮 CI 补齐。
- **第二轮 CI**：`fe1169b0` / run `39743687050` 已取得 macOS、Linux Rust 和 Windows 扩展成功；Windows 专项仍在启动服务时 Access denied，`proxy-requests.json=[]`，不是代理功能已测失败。改为两个 Rust 对照直接交给 nextest（不再包整条 Node 链），并实际查询 Job 标志；删除无效的自制 Node 包装及只测该包装的 4 个用例，保留系统代理/受限宿主/窗口退出断言。不改产品或 Multica Job 策略，下一修订仍需三平台云端验证。
- **宿主本地提交**：Paperclip `1f010b165`、Multica `e12a45e59`，均仅 `feat/tomcat-agent` 本地分支，无远端推送；Multica 隔离 API/web/daemon 已用原环境管理器停止。Paperclip 首次清理因身份不符主动中止，重新确认真正启动器后已停止；四个所属端口均无监听，临时数据与证据保留，正式 Tomcat 后台未动。
- **第二轮 Windows Rust 结论**：失败项缩到 Unix 导入未条件编译、模型 cwd 断言误查 JSON 转义、受限 Job 退出码假设三处；其余并行集成365 PASS、串行80 PASS。当前将导入移回 Unix 用例，cwd 比较解析后的请求文本，真实 Job 显式指定终止码77并精确断言，另一个关闭 Job 句柄用例保持原样。本轮新门禁结果仍待推送后取得，不把未跑的 Windows 分支算作本机通过。
- **第三轮本机预检**：nextest 配置守卫6 PASS、Node→nextest 真实单用例1 PASS、官方 ACP 客户端/模型/cwd 用例1 PASS、两个受影响测试目标严格 Clippy、严格 TS、fmt/diff check、个人机器保护和 CI 接线检查通过；系统代理设置/恢复6 PASS仍有效。曾误用 crate 私有路径归一化入口的编译失败已改用既有公开 `normalize_path`，不为测试扩大产品 API。
- **第三轮云端结果**：`1d9845dd` / run `39750873025`，macOS/Linux Rust和Windows扩展成功；Windows Rust集成365+82 PASS（cwd与真实Job均通过），但Clippy仍因两个Unix专用MCP夹具函数未条件编译而失败，已连同其Path导入收口为 `cfg(unix)`。Windows专项记录Job flags=`0x800`且关闭系统代理对照通过；开启对照被测试的 `NO_PROXY`/`no_proxy` 重名快照拒绝，未发代理请求、未执行GUI，不能计正向通过。
- **第四轮修复与预检**：测试只发送合法环境并检查握手error；模型/cwd断言复用既有非标题请求筛选。扩展也补拒绝既有error的处理（先以真实socket用例证明旧版误报连接成功），Windows环境编码与Node实际子进程一致，保留排序后的首个大小写等价键及undefined删除语义；没有新协议字段、按会话代理或重试机制。Rust定向2 PASS、目标严格Clippy/fmt通过；扩展相关79 PASS，最终所属文件10 PASS/typecheck通过。真实Mac窗口1 PASS，PNG/ARIA/console已读，退出前后及禁止启动重连后同一PID/instanceId；Windows新分支仍待下一轮CI。
- **第四轮云端结果**：`48fd1407` / run `39757922309` 的macOS/Linux Rust和Windows扩展通过。Windows真实系统代理开关对照均通过，Native/ACP模型与远程MCP共10次代理请求（含2次tools/call）；GUI因下载VS Code1.101.0、低于扩展要求的1.106.0而未加载。Windows Rust并行365 PASS；我将父模块Path导入限制为Unix时漏看了技能子模块的glob借用，导致Clippy/串行组编译失败，不能计串行通过。
- **第五轮修复与本机预检**：两个下载入口改读扩展engines.vscode的最低版本，不改兼容性声明或显式程序路径覆盖；技能测试自己导入Path，Unix专属MCP函数仍按平台编译。启动器/系统代理包装29 PASS、严格TS及build通过；Rust所属技能3 PASS、严格Clippy/fmt通过。真正VS Code1.106.0完成任务及关窗后同实例验活1 PASS，299px侧栏PNG/ARIA/console已读；只是测试接线修复，产品、代理来源、协议和断言不变。同修订Windows最终状态与制品仍由CI给出，不以本机结果代签。
- **执行边界**：Windows云端使用受控模型和假ACP宿主/直接协议，不安装Paperclip或Multica；两宿主只在本地实际启动验收及本地分支提交。未升版、合并主干或发布；真实模型授权已用完且未追加调用，本机系统代理未修改，原Tomcat后台未动。临时验收数据保留供复查，Cov%未测量。
- **证据**：架构及迁移见 [shared-serve-and-acp-host-integration.md](../architecture/shared-serve-and-acp-host-integration.md)，本机完整日志索引见 `.agents/acceptance/shared-serve-acp.md`；本地截图、临时脚本与凭据不纳入 Git。

### CURRENT INTEGRATION：Windows 平台适配与 macOS 回归收口（2026-10-19）

- **当前收口**：Windows 适配后的两项 macOS/Unix 回归与四项搜索测试的本地代理排除配置已提交为 `eb1dc2ac`；该提交未重跑测试或构建。用户现已启动标准发版，执行进展以 CURRENT RELEASE 为准。下方保留此前 Windows 交付与发布事实，不作为本轮修复的 Windows 原生验收证据。

- **范围**：按 `windows_平台适配_d3cab8bc` 实施 win32-x64 插件与终端 CLI 一等交付，提交 `80e7cc18`；明细、根因与证据只维护在 [feature-windows-support.md](feature-windows-support.md)。
- **版本**：版本脚本统一递增 CLI `0.1.77 → 0.1.78`、EXT `0.1.93 → 0.1.94`，新 EXT pin CLI `0.1.78`；版本一致性检查通过。
- **本轮边界**：只推送 develop 与 `dry-run-cli-v0.1.78`、`dry-run-ext-v0.1.94` 两个演练标签；工作流的发布步骤只对正式 `cli-v*`/`ext-v*` 标签放行，演练不创建 GitHub Release、不上传 Marketplace/OpenVSX。正式发版需用户另行确认。
- **验证**：本机 Windows（Git Bash）gate-fast 全绿，插件 lint 与集成测试 225 项通过。Cov% 未测量。
- **演练结果（`afbec1ff`）**：release-cli run `39090243675` 成功（四平台归档，含 `x86_64-pc-windows-msvc`）；release-ext run `39090933463` 成功，含 win32-x64 原生安装验收。产物与证据 zip 需登录才能下载，本机未下载复核；同样的检查已用本机 release harness 在真实 VS Code 上跑过。windows-ci run `39090230175`：插件 job 与 version-sync 通过，**rust gate-fast（windows）失败**。原始日志需仓库管理员权限，因此给工作流加了失败注解，据此定位到 9 项只在 runner 上失败的用例：6 项是测试期望值用了 8.3 短名临时路径，2 项暴露私有文件权限缺陷（已改为整体替换 DACL），1 项是 runner 缺 ripgrep。明细见 [feature-windows-support.md](feature-windows-support.md)"CI 上才失败的用例"。修复后 windows-ci run `39097819025`（`bb8c618f`）rust 与插件两个 job 全部通过。
- **正式发布**：develop 已快进同步 main/master。首次 `cli-v0.1.78` 打在带 `[skip ci]` 的文档提交 `800829a7` 上，GitHub 因此跳过了 tag 推送触发的工作流，也没有生成 Release；已删除这个 tag，改打在 `712b09c5` 上。
  - **CLI 0.1.78 已发布**：release-cli run `39101877028` 成功，[GitHub Release](https://github.com/teisun/tomcat-agent/releases/tag/cli-v0.1.78) 含四平台归档（新增 `x86_64-pc-windows-msvc`）与 `SHA256SUMS`。
  - **EXT 0.1.94 暂停**：用户决定先手动验收，`ext-v0.1.94` 未推送，Marketplace/OpenVSX 未上传。手动验收用本机包 `%USERPROFILE%\.tomcat\temp\windows-acceptance\manual\tomcat-vscode-ext-0.1.94-win32-x64.vsix`：同一提交本机编译的 `tomcat.exe`，与包内 `extension/bin/tomcat.exe` 的 SHA256 一致；正式包内置的是 CI 编译的那份。
  - **本次恢复发布**：采用 CLI `0.1.79` / EXT `0.1.95`，不补打 `ext-v0.1.94`；新 EXT 必须等待 CLI 0.1.79 四平台资产就绪，标签提交不能带 `[skip ci]`。
- **验收后续修复**：§13 验收发现的 MCP 退出卡住、路径分隔符混用、验收脚本 CLI 路径，以及 CI 暴露的问题，都在 `afbec1ff` 之后另行提交（`26af661d`、`a8a100f0`、`bb8c618f`），因此正式标签须打在新提交上，并重新跑 release-cli 后再推 EXT 标签。
- **平台层收口**：按 `平台层收口整改_90f59927` 把业务层的平台 `cfg` 与系统 crate 调用清零，`infra/platform` 成为唯一平台差异层，由 `platform/tests/boundary_test.rs` 守护；预检两平台统一为后台安装。工作流 `windows-ci.yml` 改名为 `ci.yml`，rust gate-fast 扩成 Windows / Ubuntu / macOS 三平台矩阵（上文 windows-ci run 均为改名前的记录）。本改动纳入本轮 CLI `0.1.79` / EXT `0.1.95`；`ext-v0.1.94` 保持未发布。明细与证据见 [feature-windows-support.md](feature-windows-support.md)「平台层收口」。

### CURRENT RELEASE：CLI 0.1.83 / EXT 0.1.99（已发布）

```text
e2f9ae6e → CLI 0.1.83 四平台发布 + 下载哈希通过
         → 93a467a7 仅修验收夹具 shell
         → EXT 0.1.99 四平台原生验收 + 五份 VSIX
         → Marketplace 四平台 Validated，下载字节与 GitHub 一致
```

- **最终发布事实**：CLI 标签指向 `e2f9ae6e386bc4f471c3ad5052350b345ad6bf57`，run [41488245125](https://github.com/teisun/tomcat-agent/actions/runs/41488245125) 成功；[CLI Release](https://github.com/teisun/tomcat-agent/releases/tag/cli-v0.1.83) 四份归档及 SHA256SUMS 已公开并下载核验。EXT 标签指向 `93a467a7f9c157541e68f6da25bf8f56bf204cbe`，run [41492236469](https://github.com/teisun/tomcat-agent/actions/runs/41492236469) 七个 job 全过；[EXT Release](https://github.com/teisun/tomcat-agent/releases/tag/ext-v0.1.99) 四个平台包、纯包与 SHA256SUMS 均已公开并通过下载校验。四个 VSIX 的哈希与原生验收记录相同，内置 CLI 与已发 0.1.83 归档逐字节一致，纯包不含 CLI；五份包的 CHANGELOG 与标签源码一致。
- **两个标签为何不同**：复核实际截图发现旧夹具强制所有系统使用 `/bin/zsh`，Linux 宿主因此弹错遮住输入区。`93a467a7` 只删这一赋值并补继承/未设 SHELL 的断言，沿用宿主环境；CLI 源码、版本、依赖、CHANGELOG 和产品 UI 均不变。没有安装额外 shell、隐藏通知、放宽门禁或重建/替换已发 CLI。
- **云端验证**：版本提交的 develop/main CI `41488232063` / `41488231306` 全过；夹具提交的 `41492225198` / `41492225378` 也全过，均含三系统 Rust、Windows 扩展及真实 Windows 系统代理/VS Code。三个 version-sync 通过。正式 EXT 的夹具检查 12 passed、13 个无关打包用例未被过滤器选中；四平台实际安装同一待发布包、冷启动、重启、允许/拒写、停止及重载通过；Unix 三目标另通过原始 0.1.90 数据升级。Windows 不代签该 Unix 旧夹具升级。
- **市场**：OIDC 上传成功，身份检查 [41493675964](https://github.com/teisun/tomcat-agent/actions/runs/41493675964) 成功；01:49 +0800 的公开查询显示四平台 0.1.99 均为 `validated`，下载哈希与 GitHub 四份已验收包一致。最初查询仍返回 0.1.96 的记录保留，不将上传成功冒充立即可见。OpenVSX 按既有条件跳过；发布变量、凭据与保护规则未改。
- **视觉与日志边界**：逐张读回正式包四平台最终聊天及 Unix 升级计划的宽/窄 14 张 PNG、ARIA 与 console；Linux 错误通知已消失，拒写保持 `Write failed`，窄屏保留型号省略和内容滚动。82 份 webview console 的 `errors=[]`；另外四份 workbench 记录各含测试 harness 的 Marketplace 404，以及宿主告警，不能写成“86 份全部零错误”或全程宿主零告警。升级截图中的旧流式失败原本就在 0.1.90 JSONL 第 12–16 行，当前执行已完成，不改写历史。
- **旧资产与本机边界**：旧 `cli-v0.1.82` / `ext-v0.1.98` 标签仍在 `3441b9e8`；旧 CLI 资产标识、大小、时间与校验清单均未变。仅快进同步 develop/main/master。本机不跑产品测试、构建、安装或服务重启；删除干净的指定 Windows worktree，使用 `cargo clean --profile dev` 清理调试产物（报告 17.1GiB），保留 release 目录及在用程序原哈希，清理旧 VSIX，磁盘可用约 43GiB；其它 worktree 和验收证据保留。
- **仍未解决**：旧式动态插件跨会话追加、旧 QuickJS/并行 socket 失败根因、真实付费模型与个人 Windows 设备验证，不因本轮门禁通过而结案。未测 Cov%。完整原始证据与 14 张最终图片清单：`.agents/acceptance/release-0.1.83-0.1.99/{release.md,final-ui-inventory.json,native/}`。下方保留首次失败和整改过程，按当时阶段阅读。

- **首次准备**：用户执行标准 `/release-cli-ext`，源码基线 `12445089`。仅运行版本脚本一次：CLI `0.1.81 → 0.1.82`、EXT `0.1.97 → 0.1.98`，新 EXT pin CLI `0.1.82`；五个版本源/镜像的差异只有业务版本，无依赖变化。起手工作区干净，本地/远端目标 tag 未占用，develop/main/master 可快进同步；版本一致性与两个 tag guard 已通过。
- **CHANGELOG**：以 GitHub 实际已发布的 `ext-v0.1.96` 为基线核对提交与相关差异，将 Unreleased 收入 `0.1.98`，补齐共享后台/ACP、凭据与代理升级提醒、恢复与引用修复、握手/Windows 探测安全边界、规划授权前置及 Unix 安装副本说明。注明 bundled CLI `0.1.82`；已发布历史条目不改，不为未发布的 `0.1.97` 虚构记录。版本与 CHANGELOG 必须同一提交。
- **执行边界**：本地仅做版本镜像、tag guard、CHANGELOG 与差异检查，不额外运行产品测试、构建或安装，不重启在用服务。推送 develop 后仅 fast-forward 同步 main/master；先发布 CLI，确认四平台归档与 SHA256SUMS 就绪后才发布 EXT，沿用既有 GitHub 构建及四平台原生安装验收。
- **首次发布（被拦）**：release 提交 `3441b9e87931b0e83f338a4a76f0e791fd044c0e` 已推送并快进同步 develop/main/master。CLI run `41462040464` 成功，四平台归档与 SHA256SUMS 已公开，实际下载哈希均通过。EXT run `41464467206` 打包与 Windows 原生验收通过，但两种 macOS 和 Linux 的真实 `serve restart` 失败；EXT GitHub/Marketplace 发布均 skipped。两个 tag 保留，不能将 CLI 发布成功写成 Unix 运行验收通过。
- **整改授权与首批修复**：用户已授权修复后以新 patch（CLI `0.1.83` / EXT `0.1.99`）继续，不改旧 tag/资产。先修已确认的 readline 未处理错误与夹具超时常量漏导出，新增真实连接失败回归和 80 秒超时断言；固定 Linux 构建为验收使用的 Ubuntu 22.04，并按 runner/target 隔离缓存。保留隔离 HOME 的服务日志，复用现有两条 workflow_dispatch dry-run 取得证据后再正式发布；本地只做差异检查，不跑测试/构建。本阶段尚未确认启动发现的瞬断处理缺口，后续定位及修复证据见下；不吞掉错误或放宽断言。
- **首轮诊断**：保留远端并行修复 `2a1b6056`，本地未推送提交重放后为 `b9cf1f45`；未强推。EXT dry-run `41470577638` 使用原 CLI 资产：Windows 全过，Linux 冷启动/重启过但旧数据升级失败，macOS 两架构重启仍失败。Linux 已定位为夹具将 `/private/var/...` 替换成 `/private/tmp/...` 的搬迁错误；现只修复制数据，并加入工作目录/会话索引可访问、原件不变的回归。既有 release workflow 增加夹具定向检查；Rust 增加失败阶段日志但不改变返回值、身份校验或清理顺序。下一轮用同一源码提交的 CLI/EXT dry-run 包诊断，不正式发布。`b9cf1f45` 的 Windows 扩展 gate:fast 与真实 VS Code 代理/生命周期门禁已通过，其余 CI 随后亦通过。
- **发现阶段定位与修复**：同提交 `34c23b69` 的 CLI dry-run `41473991662` 构建全过，EXT dry-run `41476369424` 的 Windows/Linux 全过；macOS 日志已将失败定位到 restart 的 start 阶段状态查询 EOF（停止阶段未报错，不据此猜测 None/确认结果或内核原因）。新增统一只读 `probe_service`：仅连接断开在原 connect+hello 10 秒总预算内重试，不将 EOF 当作端点不存在；鉴权/协议/其他错误立即失败，Stop/ACP/Native 不重放，启动锁与清理确认不改。四条真实 IPC 回归覆盖恢复、拒绝、总截止时间及 Stop 正向清理确认；修复提交 `7b842ae7` 的 CI `41481377749` 五个 job 全过，CLI dry-run `41481389686` 四平台构建全过，EXT dry-run `41483492908` 四平台原生包全过（Unix 另含 0.1.90 升级）。夹具聚焦检查 12 passed，13 个打包用例未被该过滤器选择；原生断言没有跳过或放宽。诊断包未发布。
- **正式续发准备**：已按批准范围审查 `3441b9e8..7b842ae7`，保留身份、权限、停止确认、启动锁与退出时任务清理边界。项目脚本仅运行一次，将 CLI `0.1.82 → 0.1.83`、EXT `0.1.98 → 0.1.99` 并 pin CLI `0.1.83`；五个镜像仅版本改动，两个 tag guard 通过。未发布的 EXT 0.1.98 条目合入 0.1.99，补本轮修复与 Linux 基线，0.1.96 及更早已发布历史保留。目标 tag 尚未使用、main/master 可快进；先发 CLI 并验证四份归档及哈希，再发 EXT 并执行正式包原生验收和市场门禁。Marketplace 环境只允许 `ext-v*` tag，没有 reviewer 审批规则。本地仅版本、格式与差异核对，无产品测试/构建/安装。
- **市场与更新**：已查询 `PUBLISH_MARKETPLACE=true`，沿用现有 OIDC 发布；Marketplace 与 OpenVSX 的实际结果以本轮工作流为准，不改仓库变量、凭据或发布门禁。
- **证据边界**：记录与原始失败见 `.agents/acceptance/release-0.1.82-0.1.98/{release.md,repair.md,ext-failed.log,native/}`。原提交三个 version-sync 和 main 普通 CI 已通过；develop 普通 CI `41461835254` 在 Windows 的 `connector_test.rs:96`（`real_tool_entries_preserve_progress_and_session_stop_isolation`）报 agent code execution is closed；同 SHA 的 main CI 通过，根因未定，不称全量全绿。Linux 的 weak `GLIBC_2.39` 提示不是已证实的重启根因；CLI 构建原镜像实际为 Ubuntu 26.04.1。本轮不测 Cov%；既有动态插件串会话、真实模型/个人设备未验等风险继续保留。

### CURRENT INTEGRATION：三项整改（2026-10-16 晚，本次本地提交）

- **提交边界**：本次将已检查的 121 个修改文件与 8 个新增源码/测试文件全部纳入，包含下方供应商分卡改动及两处 status。按用户明确要求，不运行测试、构建、打包或视觉验收；下方结果均为此前验收记录，不冒称本次重新通过。Cov% 未测量，提交信息省略覆盖率；未升级版本、未推送、未打标签或发布。

- **范围**：按 `三项整改计划_6d1637f0` 实施。① 内置技能改由 `build.rs` 生成嵌入表，跳过 `node_modules` 与点文件，删除 `include_dir` 依赖；② 复核供应商分卡实现，修 4 处：混合主机卡以首个用户行为代表（标题、主机顺序、卡内添加地址），编辑表单中文"地址"，用户指南补辅助模型回落说明，续传文档 §4.2.3 去掉旧规则；③ 待答提问面板的选中项改为金色字母块加暖色行底，与悬停明显区分，并补高对比模式样式。验收完成时未提交，现随本次本地提交纳入；未升版本、未推送或发布。
- **验收结论**：本机保留 18M 的 verify `node_modules` 时，嵌入表 19 项（等于 git 跟踪数），release 产物 16,490,436 字节（基线 16,490,556），二进制中无 Playwright 包字符串。Rust lib 3301 / 0 失败、integration（parallel 378 + serial 49）、严格 Clippy、release 构建通过。扩展侧 lint、GUI 854、core 774 通过；提问面板视觉脚本 9 场景通过，新增计算样式断言，且已反向验证（去掉新规则后断言失败）。改前/改后截图与日志见 `.agents/acceptance/remediation-20261016/acceptance.md`。
- **计划外修复**：全量 lib 中，分卡计划新增的 `remote_models_test` 连续两次失败。原因是 `http_client_test` / `web_fetch` 的 4 条代理用例运行期间清空 `NO_PROXY`，把并发的本机 mock 请求截进 mock 代理；两模块同跑可 10/10 复现。现改为这 4 条用例只豁免 `127.0.0.1,localhost`，同跑 10 次 0 失败，生产代码未改。
- **未验证**：提问面板截图来自模拟宿主，未在真实 VS Code 打包安装后复拍；未打包 VSIX；Windows/Linux 未验证。

### CURRENT INTEGRATION：模型设置按供应商管理（2026-10-16 下午，本次本地提交）

- **范围**：按 `供应商分卡模型设置_daf616fb` 实施，五项需求为：供应商卡片（一把 Key 一张卡，每行显示实际协议）、连接供应商对话框与首个配置成功即默认、内置目录刷新为 13 项、删除 Ultrafast（旧值按 Standard 读入，不改写文件）、`llm.title_model` 作为可指向任意已配型号的辅助模型。另含 Anthropic fast、思考请求通用规则、前缀绑定回退与 Chat Completions 只回传当前回合。老用户 models.toml 不迁移。验收完成时未提交，现随本次本地提交纳入；未升版本、未推送或发布。
- **验收结论**：Rust 门禁全绿：严格 Clippy、lib 3300 项（0 失败，5 项 ignored）、doctest、integration（parallel 378 + serial 49）及 release 构建。扩展侧 lint、harness 类型检查、GUI 853、core 774、integration 218 + 打包烟测 13 通过。fake 安装 E2E 23 项、`verify:vsix` 10 项通过。真实 release 包在 macOS x64 / VS Code 1.106.0 下冷启动与 0.1.90 升级各 1 项通过，其中冷启动会在设置页改辅助模型，并断言标题请求只命中所选型号。宽 1440 / 窄 390 视觉 42 张：无禁用词、无越界变量名、无溢出、无 console error。用本机已有 Key 实拉型号列表：DeepSeek 2、MiMo 8、Moonshot 4 个，均 2xx，Key 未打印。完整记录见 `.agents/acceptance/vendor-model-settings/acceptance.md`。
- **本轮回归已修**：删除内置 `utility-flash` 后，执行流夹具的回合标题请求抢占了脚本响应，现只把主模型指向夹具；门禁脚本中 Responses Key 映射仍是旧型号，已同步；设置页窄屏有三处问题已修：连接列表行被压扁、搜索框溢出、卡片菜单右侧被裁。视觉脚本的溢出检查改为同时覆盖对话框内部的横向滚动容器。
- **既有风险**：`include_dir!` 会嵌入本地 gitignored 的 verify 技能 `node_modules`，本地 release 构建会因此膨胀，也会拖慢新工作目录的首次落盘。本轮构建期间暂时移开，之后已恢复，未在本计划内修；已由上方"三项整改"解决（改为 `build.rs` 生成嵌入表）。Windows/Linux 原生安装与真实付费对话未验证。

### CURRENT INTEGRATION (2026-10-16)

- **本次版本与提交**：使用 `scripts/release-version.mjs bump --all patch` 一次升级 CLI `0.1.74 → 0.1.75`、EXT `0.1.90 → 0.1.91`、内置 CLI `0.1.74 → 0.1.75`；前后版本一致性与版本文件空白检查通过，既有依赖修改保留。五项整改、拒写历史修复及相关源码/测试/旧数据夹具/文档随本次本地提交纳入。按用户要求，本次不重跑测试、不构建/打包/安装、不推送/打 tag/发布；下述测试及原生验收属于版本递增前的源码证据，不代表新版安装包已验证。Cov% 未测量，保留空值；QuickJS 未解决项继续保留。暂存检查发现旧版原始夹具 `cli-init/models.toml` 自带 EOF 空行；用夹具目录 `.gitattributes` 仅对该文件设置 `whitespace=-blank-at-eof`，保留原始字节，其余文件与其他空白检查不变。

- **补充修复已验收**：拒写结果新增可选本地 `ChatMessage.is_error` 并在历史重载恢复；新失败显示 `Write failed`，旧记录缺状态显示中性调用文案，不改写历史、不按错误文字猜测、不把该元数据发送给模型。移除安装门禁的版本例外。`ask_question` 描述补 `During plan execution, ask only when necessary.`，生成工具目录已同步。
- **本次补充证据**：Host 分批有效 765、GUI 849、相关集成 8 通过；lint、harness 类型检查、build、wire、严格 all-target Clippy、工具目录 2 项、旧数据兼容 1 项（生成器 1 项按设计 ignored）及 release 构建通过。新真实包冷启动/0.1.90 升级各 1 项通过，拒写前后重载断言、文件未写入与成功写入对照通过；拒写行实际宽窄尺寸为 1391×808 / 341×752。完整记录见 `.agents/acceptance/denied-history/acceptance.md`。Rust 全量本次仍有 QuickJS 失败，详见下表，不能称全绿。该验收完成时尚未提交或升级版本；后续本地提交与版本号以上条记录为准，仍未发布或覆盖本机 CLI。

```text
明确要求实施 -> start_plan -> PLAN 内确认 -> 同回合工作 / 提问 / 回答继续
执行中改另一份计划 -> 保留原执行绑定；Stop -> pending
旧配置 -> 忽略未知键并记录一次启动告警 -> 继续使用旧模型、会话、计划
菜单内部连续管理 -> 保持展开；最终验收截图 -> 逐张交付
```

- **本轮范围**：按 `plan_agent_workflow_cli_agent_cli_serve_c46522fe` 实施五项整改；模型目录与供应商设置属于独立计划，未改。未同步/覆盖本地 CLI；整改与版本号随本次本地提交纳入，尚未发布。
- **实现**：独立 `start_plan({plan_id?})` 复用授权与 build；PLAN 确认复用 review 面板，子 Agent 白名单不放开。执行期问答解除限制，恢复问答接当前回合取消信号；执行中绑定保护落在唯一刷新入口。当时菜单仅框外/Escape/触发器收起，删除弹窗暂停底层监听；2026-10-17 已补齐 Webview 失焦与选择会话后收起，管理操作仍保持展开。
- **配置升级**：`serde_ignored` 收集未知路径，日志初始化后一次输出；删除三处旧键拒绝、init 特殊加载和 9 条专用翻译。已知非法值、工具参数、wire、session/trust 等严格校验保留。两份真实 ext-v0.1.90 数据夹具来自 `d00ebe08` 的 CLI 0.1.74，原字节不手改，仅在测试副本中重定位 HOME 与本地 HTTP 地址。
- **中途证据**：计划开工首批 192 项、恢复取消 1 项、配置/提示/技能批次、SessionBar/App 93 项通过；两份旧目录升级回归、Serve robustness 14 项、门禁配置 6 项、工具目录 2 项通过。真实 Serve 同回合开工→写文件→提问→回答继续、普通/恢复 Stop，以及 Host 新旧四条用例均已通过；Host 原失败来自夹具答案遗漏 pickedRecommended，未放宽协议校验。
- **最终验收进度**：Host 单测 762、GUI 844、wire 和版本一致性通过；分批有效 Rust lib 3266、剩余 integration 367+3 定向修复复验、serial 33，以及严格 Clippy、doc（0 项）、release 构建通过。macOS x64 / VS Code 1.106.0 的真实包冷启动和预置 0.1.90 数据升级行为通过，后者验证同 PID 开工→写文件→执行提问→继续及菜单连续管理。安装 E2E 复用 22 项成功结果，修正菜单持久展开后的测试交互并定向复验 1 项通过；完整 fake `verify:vsix` 10 项通过。20 张最终 PNG 及 ARIA/console 已逐一读取：19 份 console 无错误，完整 workbench 保留 E2E harness 查询 Marketplace 的 404。本机许可数据副本烟测亦通过：36 个模型、3 个近期非当前会话（含空会话与两份真实历史）、35 个引用图片 blob、203 份计划；握手/模型列表/会话切换与读取成功，源模型和抽样历史哈希不变，副本清理完成。受保护原始配置及凭证未读取，使用新隔离配置，不冒称验证了它们或所有历史。详见 `.agents/acceptance/agent-workflow-upgrade/acceptance.md`；不把拆跑记录冒充聚合 gate 退出 0。
- **请求断言校正**：现有 OpenAI/Anthropic 适配器把 runtime tail 合入 wire system；本轮不移动 tail，也不承诺整个 wire 缓存前缀恒定。验证稳定 system 正文、tools、历史字节不变，仅 planner→executor 提醒变化，workspace 权限提醒不变（`core/llm/openai.rs::transport_messages` 及既有 provider 测试）。
- **既有发布**：CLI 0.1.74 / EXT 0.1.90 GitHub Release 已完成，提交 `d00ebe08`；三平台历史验收为 `passed_with_known_issues`，证据见 `.agents/acceptance/release-0.1.74-0.1.90/release.md`。不改旧 tag/资产；该次发版时 Marketplace 关闭，当前联邦身份已就绪且开关已开启，实际自动上传仍待下次发版验证。下方旧日期记录仅属历史。

### ✅ DONE (已完成/进行中)
- [✓] **[P0/P1，2026-11-04，首启/MCP/旧连接恢复，本次本地提交]** 空窗口引导、默认配置补建与保留、MCP诊断/脱敏/重载、仅最终本地连接失败后懒换Client均已实施，相关源码、回归和真实VSIX夹具完整纳入；此前本机验收与原始失败见顶部 CURRENT WORK。本次按用户要求不重跑测试或构建，不将本地提交等同于已发布。
- **[2026-11-03，write-plans 结构整理，本次本地提交]** 将技术设计、文字与图示指导移到独立 `references/writing-standards.md`，入口先读工作流再读写作规范和结构；章节表、局部骨架与小改动示例统一独立标题及层级，问题与需求按适用条件包含根因、方案说明、可选 UI 图、六列决策表和测试章节，删除可选章节模板并精简工作流措辞。提示词契约测试同步覆盖标题/骨架及已移除条款，技能物化测试补新附件加载检查；四个技能文档、两个 Rust 测试及两处 status 全部纳入。仅做差异、空白与提交范围检查；按用户要求不跑测试或构建，不声称新增用例通过，Cov% 未测量；未升版、安装、重启、推送或发布。
- [✓] **[P1，2026-11-02，计划执行就绪]** write-plans 的 Final Check 与计划 reviewer 增加执行就绪检查：先核实关键前提、优先等效自动化，在规划阶段问清可预见的风险操作并记录授权边界，执行时不重复确认已同意事项。同步 review brief、单条提示词契约测试与职责文档；五个实现文件及分支状态/汇总看板纳入本次本地提交。
- [✓] **[执行就绪的提交前验收证据]** 提示词模块与技能物化/发现/加载回归共 33 passed、0 failed（`1793625169364-y9ujje`）；格式与 diff 检查通过（`1793625242644-m4h9li`），五个场景已作条款走查。两次断线仅留编译日志，结果未知；新增测试的一处格式问题已修正，原失败记录保留。按用户要求本次提交不重跑测试或构建，Cov% 未测量；不推送、不安装、不重启。
- [✓] **[P1，2026-11-02]** Unix 命令入口改为独立副本：已有软链（含同目标与悬空链接）迁移为普通文件，内容不同的入口以同目录临时副本、`sync_all`、`rename` 替换；清理构建目录不再破坏入口，运行中的进程继续持有旧文件。新增 `scripts/dev-install.sh`（100755），沿用 Cargo、`CARGO_TARGET_DIR` 与 Agent 保护；脚本测试、非 Windows CI 步骤、双语文档及开发规则一并纳入本次本地提交。Windows 生产行为、插件路径选择与版本号不变。
- [✓] **[本轮提交前的验收证据]** 定向库 13 项、self-install CLI 4 项通过（任务 `1793626820092-6y5212`）；Unix 包装器 2 项通过，Windows 2 项按平台跳过（`1793626881020-yjbd4e`）；`cargo fmt --check`（`1793626952581-m4wypu`）、定向 Clippy `-D warnings`（`1793627274153-2refvy`）及 diff 检查通过。按用户要求本次提交不重跑测试或构建，Cov% 未测量；只提交本次 12 个相关文件与两处 status，先前五个计划技能/提示词及其测试文件仍留在工作区，不推送或发布。
- [✓] **[本机操作，2026-11-02]** NemuDrv 卸载成功，驱动目录和启动项链接已移至 `/Users/Shared/nemu-backup`，没有删除；launchd 原本未注册该任务，bootout 的“进程不存在”如实保留。复查 showloaded 成功且未发现 NemuDrv。用户在独立终端执行安装脚本两次后，已核实 `~/.local/bin/tomcat` 为 0755 普通文件、与 release 产物字节一致、版本 `0.1.81`；服务 PID 12802 及启动时间不变，Agent 保护未移除（安装核对任务 `1793628324840-uvzuia`）。这些机器操作不作为自动卸载机制进入仓库，不据此断言内核崩溃根因或永久治愈。
- [✓] **[P1，2026-10-20，仅设计稿]** 共享 serve 与 Paperclip/Multica 标准 ACP 接入技术方案及架构索引随本次本地提交纳入，见 [shared-serve-and-acp-host-integration.md](../architecture/shared-serve-and-acp-host-integration.md)。已同步最新计划的控制/旁观身份、普通打开与当前消息补发、慢连接不取消任务和背压降级边界；WebSocket 与断线续跑仍为后续规划。文档检查通过：19 个 JSON 样例、74 个本地链接、34 处源码行号引用、表格/围栏/差异格式；28 项功能验收均为 PENDING，未实施、未跑产品测试或构建，Cov% 未测量。后续从本次提交创建 `feature/shared-serve-acp` 开发分支；本次不推送、不升版、不发布。
- [✓] **[P1，2026-10-19]** IP 中转站自动凭证槽修复：GUI `envNameForRelaySlug` 与 Rust `env_name_for_provider` 仅在规范化名称以数字开头时添加 `PROVIDER_`，使报告地址派生为 `PROVIDER_114215206196_API_KEY`；provider、模型 ID、显式槽优先级与冲突编号规则保持不变。5 个生产/测试文件及两处 status 全部纳入本次本地提交。此前 GUI 35/35、Rust auth 7/7、CLI 5/5、隔离环境后的 admin 28/28，以及 GUI 类型检查、目标 Rust 格式检查通过；CLI 新用例实际保存并清缓存重读 `.env`，确认 keyPresent 与 AuthStore 取回 Key。桌面/窄屏共 4 套 PNG/ARIA/console 已读取，真实设置组件使用模拟宿主，不冒充后端持久化或完整 VS Code E2E。按用户要求本次不重跑测试、构建或视觉验收，Cov% 未测量；未升版、未推送或发布。本地记录 `.agents/acceptance/ip-relay.md` 与临时脚本/截图按既有忽略规则保留，不强制纳入 Git。
- [✓] **[P1，2026-10-19]** macOS/Unix 回归修复：`normalize_path` 只剥离本机分隔符，避免把合法反斜杠文件名指向另一个文件；私有原子写入保留创建时的 0600 保护，并在第一字节前按句柄补足被 umask 扣除的属主权限。此前两项用例均完成红→绿，平台模块 54/54、env_file 2/2、OAuth 7/7 通过；平台修复后的严格 Clippy 复验通过。
- [✓] **[P1，2026-10-19]** 四项 hosted web-search 测试不再清空本地代理排除，`NO_PROXY`/`no_proxy` 均设为 `127.0.0.1,localhost,::1`；原第 4–7 项定向复测 4/4、显式 OpenAI 代理对照 1/1 通过，原断言与超时未放宽。上述记录来自本次提交前的实施验收，不代表重新跑过全量门禁；7 个源码/测试文件及两处 status 全部纳入。本地报告 `.agents/acceptance/windows-macos-regression/fixes.md` 与临时验收产物按既有全局忽略规则保留，不强制加入 Git。
- [✓] **[P1，2026-10-17]** 会话菜单收起优化完成并纳入本次本地提交：`SessionBar` 展开期补非捕获 window blur 监听及对称清理，选择会话（含当前会话）立即关闭并归还触发按钮焦点；外部关闭不抢焦点，置顶／展开更多／删除确认与取消的连续管理不变。生产仅新增 5 行，不改 CLI、会话恢复、草稿、协议或样式。此前先在基线复现两处真实红框点击失焦但菜单仍开，修复后 GUI 聚焦 96/96、真实 Dev Host 菜单用例与既有管理用例各 1/1、lint、harness 类型检查和构建通过；两张全景 PNG／ARIA／console 已读取，capture-time 记录无 error/exception。首次宿主取证在状态采集阶段超时，删除测试中多余的双动画帧等待后通过，生产逻辑与断言未放宽。已交付不含 CLI 的 `tomcat-vscode-ext-0.1.92-session-menu-pure.vsix`（1,798,478 字节，SHA256 `27bc0aa4e596e78c0d86128e79a9c2686754b6b41a28d3ef27564888dbc59c93`），构建新鲜度、包完整性及入口文件字节一致性检查通过。上述均为此前实施／打包记录；本次按用户要求不跑测试、不构建，Cov% 未测量；不升版、不推送或发布。本地证据 `.agents/acceptance/session-menu-dismissal/` 与 VSIX 按既有忽略规则保留，不强制加入 Git。
- [✓] **[P1，2026-10-17]** 会话重载恢复修复完成，用户已明确反馈「验收通过」。`SwitchSession` 的 live 分支用目标 scope 的无绑定 SessionManager 先保存磁盘 current，成功后才更新内存 active；未打开分支、运行中会话绑定、扩展与协议不变。源码及两条回归一并纳入本次提交。此前新增单测修复前准确失败、修复后通过；命令模块首轮 97 通过／6 失败，补本机 NO_PROXY 后同一二进制精确复验 6/6 通过，有效结果分批覆盖 103 项；真实多会话／进程重启 8/8 及 debug CLI 构建通过。全仓 fmt 仍有 17 个文件的既有差异，与 HEAD 比较确认无新增格式差异。上述均为此前实施验收记录，本次按用户要求不重跑测试、构建或视觉验收，Cov% 未测量；未升版、未推送或发布。完整本地记录见 `.agents/acceptance/session-reload-current.md`，按现有全局忽略规则不纳入 Git。
- [✓] **[本机，2026-10-17]** 经用户明确授权，已正常卸载旧 `tomcat.tomcat-vscode-ext@0.1.86`，扩展列表仅剩新 `tomcat-agent.tomcat-vscode-ext@0.1.92`；另按用户要求在 `~/.profile` 给 NO_PROXY/no_proxy 补齐本机地址，保留原名单与代理地址，新 Bash 登录验证通过。仅记录本机操作，不将个人 shell 配置或自动卸载机制加入仓库；Agent 未重载用户主窗口。
- [✓] **[P1，2026-10-17]** Models 页默认模型入口集中到顶部，与辅助模型共用 `ModelRolePicker`；移除模型行「设为默认」按钮，保留徽标／详情／编辑／删除。箭头改为可点击且不抢焦点的原生按钮，Tab／Shift+Tab 收起浮层；共享网格对齐标签，中英文词条和组件回归同步。此前设置模块 84 项、词条 4 项、两层 TypeScript 检查、GUI 构建，以及两场景宽窄屏共 10 组 PNG／ARIA／console 验收通过；浏览器使用测试 Bridge + 真实 Serve，不冒充完整扩展宿主 E2E。按用户要求，本次仅提交全部相关源码／测试与两处 status，不重跑测试、构建或视觉验收；Cov% 未测量，提交信息省略覆盖率。此前已生成纯插件 `tomcat-vscode-ext-0.1.92-pure.vsix`，本次不升版、不推送或发布。本地验收脚本、证据和 VSIX 按既有忽略规则保留，持久回归测试随源码提交；详见 `.agents/acceptance/vendor-model-settings/default-picker-acceptance.md`。
- [✓] **[P1，2026-10-16]** 三项整改：内置技能不再嵌入本地 `node_modules`，供应商分卡复核修 4 处，提问选中项与悬停明显区分；另修全量 lib 中代理用例截流本机 mock 请求的竞态。Rust/扩展门禁与 9 场景视觉脚本通过；本次纳入本地提交，尚未推送或发布，详见 `.agents/acceptance/remediation-20261016/acceptance.md`。
- [✓] **[P0/P1，2026-10-16]** 模型设置按供应商管理与内置目录更新：已完成实施、跨层门禁、真实包冷启动/升级验收与 42 张宽窄视觉截图验收，并实拉 3 家型号列表；本次纳入本地提交，尚未推送或发布，详见 `.agents/acceptance/vendor-model-settings/acceptance.md`。
- **[P1，2026-10-16]** 五项整改与拒写历史补充修复收口，CLI/EXT/内置 CLI 版本统一递增并纳入本次本地提交；版本校验通过，按用户要求未重跑测试或构建。
- [✓] **[P0/P1，2026-10-15]** 五项整改、分批门禁补缺、本机真实包安装、许可的个人数据读取烟测与 20 张视觉截图交付完成；已知问题与未验证边界保留如下。当时尚未提交或发布。
- **[P0，代码已实现，原生首配链路已验收]** 模型元数据与凭证解析拆分；缺 Key 不阻断 serve 建会话，配置文件在用户保存时懒创建，运行期只刷新模型选择字段；压缩模型增加当前主模型兜底。@2026-10-14
- **[P1，代码已实现，三平台发布验收带已知问题通过]** 面板内简洁引导、默认模型标记与快捷操作、关联保存回执、单行错误与就地重试已纳入；冷启动、协议、配置、GUI 和安装包测试源码及中英文文档一并提交。@2026-10-14
- [✓] **[P1]** CLI 0.1.74 / EXT 0.1.90 远端发布完成；当时 EXT 三平台原生用例带 `Created` 已知显示问题通过。该缺陷已于 2026-10-16 在源码修复并纳入本次本地提交，旧 tag/资产不修改；历史发布验收不等同于当前完整单测/供应商实测。
- [✓] **[P0/P1]** `tomcat` 多问题优化链路已在 `develop` 侧完成当前轮收口：GUI/webview 的 `max` reasoning 档位、pre-stream loading 指示、全量 shimmer 覆盖与 `Thinking` 文案已修复；`search_files` description 也已瘦身回归。Rust 侧把多模态从“Responses 独占”推进到“三家共用一套 `ChatMessageContentPart`，各 wire 自己翻 JSON”：新增 `FilesApiAdapter` 存储适配层，`OpenAiFilesRuntime` 改持 `Arc<dyn FilesApiAdapter>`，OpenAI Completions 已支持 inline 图片/PDF 与 uploaded `file_id`，Moonshot uploaded 图片走 `ms://...`，Anthropic Messages 已支持 inline / uploaded 图片与 PDF，并在 uploaded 引用时自动补 `anthropic-beta: files-api-2025-04-14`。内置/本地模型配置同步开启 `kimi-k3`、`kimi-k2.7-code` 与 Anthropic 全系 `files=true`。本地验证已补齐：catalog / openai provider / anthropic wire / adapter 单测通过；`serve_stdio_e2e` 新增 Moonshot `ms://` 与 Anthropic `source.type=file + beta header` 断言通过；live 结果为 **Kimi K3 成功**、**fcodex `/v1/files` 返回 404**、**Anthropic 实 key 返回 401 invalid x-api-key**，后两者已按真实结果记录。@2026-07-20
- [✓] **[P0]** 推理档位 catalog-id / wire-name key 不一致已在 `feature/transcript-rich-render` 修复并待并入：`run_loop` 现与存盘/GetState 一样用 catalog `id` 查 `model-thinking.json`，中转站模型选 `xhigh` 不再被回退成默认 `high`；CLI `0.1.15` / 扩展 `0.1.18`。@2026-07-20
- [✓] **[P0/P1]** 推理强度（reasoning effort）链路整改已在 `feature/transcript-rich-render` 收口并待并入 `develop`：这次不再靠“某家模型名字命中后写死映射/全局降级”硬扛，而是把**模型真正支持哪些档位**上收为 catalog 事实源。现在每个模型都可在 `builtin_models.toml` / `models.toml` 声明 `supported_reasoning_levels`；聊天框下拉按当前模型动态渲染（空列表退化为 `Thinking On/Off`），选中的档位写入 `model-thinking.json`，请求前只做一次 clamp，所以 `gpt-5.x` 的 `xhigh`、`glm/deepseek` 的 `max`、以及只支持 thinking 开关的 `mimo/kimi` 都各走各的真实协议，不再被一把梭硬折叠。与此同时，`ProviderCacheKey` 新增 `catalog_generation`，改 `thinking_format / model_name / capabilities` 后无需重启即可生效；保存模型时空 `thinking_format` 会按 API 自动物化成具体值，不再留下黑盒 `Auto`；现代 Claude `4.6/4.7/4.8` 改走 `anthropic-adaptive`（`thinking.type="adaptive" + output_config.effort`）；若 `openai/openai-responses` 线路被配成不会发 effort 的 format，保存后会回传非致命 warning。Rust focused/unit + serve 集成、GUI 单测与 targeted devhost Add Model E2E 已补齐。@2026-07-19
- [✓] **[P0]** plan 预览热更新缺陷已在 `feature/transcript-rich-render` 修正并待并入 `develop`：Rust `update_plan` / `create_plan` 的真实写盘路径是 `std::fs` 临时文件 + `rename()` 原子替换磁盘，但 plan 预览过去只监听 `onDidChangeTextDocument` 且只读 `document.getText()`，所以对 `~/.tomcat/plans/*.plan.md` 这类工作区外文件，Agent 改完盘后 VS Code 缓冲区经常不重载，预览就一直停在旧内容。现已把刷新链改成：**serve `plan.update` / `plan.todos` 事件 → sidebar provider 桥接 → `PlanPreviewEditorProvider.refreshFromServeEvent(planId,pathHint)` 从磁盘重读并回推 state 帧**；`pathHint` 走 canonical path 兜底，`PlanPreviewApp` 刷新时保持滚动阅读位置，host E2E 也改成真实 `fs.writeFile` 写盘 + 注入 `plan.update` 取证，不再依赖会误导人的 `workspace.applyEdit` 缓冲编辑路径。@2026-07-19
- [✓] **[P0]** plan transcript 渲染整改已在 `feature/transcript-rich-render` 收口并待并入 `develop`：这次不是继续打补丁保住旧 `type:"plan"` 卡，而是从第一性原理把职责拆开。最终模型是：**`create_plan` 是唯一卡片来源、`update_plan` 每次都留下独立增量事件行，`plan.*` 只更新 ambient `planState / planTodos / planFile`**。因此“`update_plan` 零足迹”和“plan 卡漂到最新 user 附近”这两个问题被同一刀解决：不再抑制 plan 工具行，也不再维护可变的 transcript plan 卡。随后又按用户体验把旧的 pending 过渡补回：`plan.create`（文件一落盘）会把 `path/planId` 盖到运行中的 `create_plan` 工具上，于是**同一张** `PlanFileCard` 会先显示 `view-plan-pending` 点点点，review 跑完、`tool_execution_end` 到来后再切成正常 `View Plan`；`update_plan` 的运行/增量反馈仍留在事件行。UX 收口同步完成：事件行 `View Plan` 扁平常显淡下划线 + 小 chevron，卡片 footer `View Plan` hover 变白，卡片 `Build` 复用 preview 同款黄底黑字非加粗按钮。对应 GUI/state 单测、reload replay 与 devhost plan UX 场景都已迁到新模型。@2026-07-19
- [✓] **[P0]** TranscriptView 富渲染四次整改已在 `feature/transcript-rich-render` 收口并待并入 `develop`：第三轮虽然把真流式 E2E 和运行时预热补上了，但聊天正文仍是“整条消息一次 `useMemo` 烤 HTML、再异步高亮”的架构，所以代码会先无色后上色（FOUC），而且 streaming 每来一个 token 都可能让整篇旧内容重复参与计算。现已对齐 cline 的低爆炸半径路线：**保留 `marked + DOMPurify + DOM 装饰` 管线，不迁 `react-markdown`，只引入『按 markdown 顶层块切分 + `React.memo` 逐块冻结 + `highlight.js` 静态 import/同步上色』**。落地内容包括：`splitTopLevelBlocks()`、同步 `highlightToHtml()`、`ChatMarkdownBlock` 按 `raw` 单 prop memo、`MarkdownBody` 复用同步高亮/代码卡片/内联路径、`.tc-chat-markdown__block { display: contents; }`、`isStreaming` 富渲染死代码拆线，以及 Vite `manualChunks` 把 `highlight.js` 单独切成 `dist/chunks/highlight.js`（当前构建约 **104.58 kB min / 35.64 kB gzip**）。验证新增/收紧到真实行为：GUI 单测断言“同步即有 `code.hljs`、追加尾块只重算新块”，devhost E2E 明确断言“流式尚未结束时，已完成代码块已经带 `hljs-*`；收尾后 mermaid 才出现 `<svg>`”。@2026-07-19
- [✓] **[P0]** TranscriptView 富渲染二次整改已完成并待并入 develop：聊天里“完全无高亮 / 无 mermaid”的真根因不是配色或 markdown 结构，而是**聊天 webview 的 CSP 比计划预览少了 `'strict-dynamic'` 与 `'unsafe-inline'`**，导致 `ChatMarkdown` / `markdownRuntime` 的 lazy `import()` 分包（`highlight.js` core/语言包、`mermaid`）在 transcript 中被浏览器拦截；卡片结构能出来，但上色和 SVG 永远跑不起来。现已把聊天 CSP 与计划预览对齐，并顺手收敛代码块外观：**不再显示语言标签**；只有带文件路径的围栏才显示头部（文件图标 + basename，hover/title=完整相对路径，点击打开定位行，右侧纯图标 copy），无路径围栏统一为 `bare` 块（无头部，copy 浮在内容区右上角）；正文内联路径显示 basename；`openFile` 失败改为 toast，不再把错误写进 transcript。门禁也补到了真正行为：GUI/provider focused tests 覆盖 basename/title/copy-feedback/toast，host E2E `assertTranscriptRichRenderingFlow` 断言真实 `hljs-*` token span、mermaid `<svg>`、`bare` 块与坏链点击不污染 transcript。@2026-07-18
- [✓] **[P0]** TranscriptView 富渲染整改已收口（**更正上一条 DONE 的口径**）：真机“一直闪 + 没有卡片/可点链接”的根因不是样式，而是 `ChatMarkdown` 先画半成品 HTML、再在 `useEffect` 里改真实 DOM；React 重设 `innerHTML` 时会把代码卡片/path chip 整块抹掉，形成闪帧。现改为 `closeOpenFence -> marked -> DOMPurify -> 离屏 detached div -> decorateCodeCards/linkifyInlineFilePaths -> div.innerHTML`，把**结构**在 `useMemo` 中先烘焙成成品字符串，第一帧就有代码卡片/文件路径/copy 按钮。`highlight.js` 与 `mermaid` 留在 `useEffect` 做纯增强：前者因 core/语言包走懒加载 `import()`，不能进同步 `useMemo`；后者因 `mermaid.render()` 异步且依赖真实 DOM/SVG。与此同时，**thinking 正文已回退为弱化纯文本 `<pre>`**（不再走 `ChatMarkdown`、不再在 thinking 内生成 clickable path），只保留助手正文与工具调用行的“点文件打开”能力。门禁补强：GUI focused tests 覆盖“正文首帧即成品 + thinking 纯文本”，host E2E `assertTranscriptRichRenderingFlow` 新增 copy 按钮、连续两帧 DOM 稳定，以及 thinking 展开后仅正文侧 clickable path 生效。@2026-07-18
- [✓] **[P0]** TranscriptView 富渲染已落地：assistant 正文与 thinking 正文统一走 `ChatMarkdown`（`marked + DOMPurify + mermaid + highlight.js`），多行代码/ASCII 围栏升级为代码卡片，带文件路径的围栏标题可点开文件，正文里的 `` `path:line` `` 会渲染成可点击 path chip；`openFile` intent 新增可选 `line` 并贯穿到 `VsCodeIde.showFile(path, line?)` 的 `revealRange + selection`。Rust system prompt 同步新增 `SystemOutputConventions`，约束模型用 inline-code 路径与 `lang path` 围栏稳定产出可点击格式。配套 GUI/ext focused tests、host E2E `assertTranscriptRichRenderingFlow` 与文档已补齐 @2026-07-18
- [✓] **[P1]** nextest 默认并发已从按 CPU 满开收敛为固定 `4`：`tomcat/.config/nextest.toml` 新增 `profile.default.test-threads = 4`，`integration_gate_config_tests` 加守卫断言，`run-integration-tests.sh` 与 `INTEGRATION_TEST_SPEC` / `INTEGRATION_MERGE_AND_ACCEPTANCE` / `Dispatcher` 文档口径同步改为“默认 4 并发”，避免本机 CPU 满载时放大 I/O、网络 mock 与外部依赖抖动。验证：`cargo test --test integration_gate_config_tests -- --nocapture` 通过 @2026-07-18
- [✓] **[P0]** 验收整改收尾已合入 `develop`：① 失败错误 `detail` 脱敏补全（`Cookie/Set-Cookie`、`x-api-key`、`password/secret`、裸 `sk-*`、`Basic`）并 8KB 截断；② 失败轮恢复边界/幂等加固（append-invariant 不写 Error、同失败尾幂等）；③ 删除 `@tomcat` participant 与单前端后不可达的 `ownership/uiMode/conflict` 死代码，仅保留 sidebar webview chat box；④ View diff 窄窗并排（`renderSideBySideInlineBreakpoint=0`）、live 403→`agent_idle` 摘要回填、Key slot 对齐与 `KeySlotCombobox` 单测/CSS token。门禁：`cargo test --lib`、`npm run lint`、`npm run test:unit`、目标 host E2E（settings/diff/retry/multi-session/switch-restore）全绿；新包 `tomcat-vscode-ext-0.1.16.vsix`。详情见 `docs/status/feature-transcript-ui-and-checkpoints.md` @2026-07-18
- [✓] **[P1]** Add Model Key slot 整改完成：凭证清单现每次从 `~/.tomcat/assets/.env` 热重读且只列合法非空 `*_API_KEY`；Relay 建议名按站点 + API 家族区分；Key slot 收敛为可搜索/可输入的唯一入口；新增输入失焦脱敏、共享 Key 覆盖确认、Refresh 与中英文文档。Rust admin 14/14、serve focused 1/1、GUI 12/12、SettingsPanel 5/5、TypeScript lint 均通过 @2026-07-16
- [✓] **[P1]** reviewer 提示词现统一继承 engineering-standards #6–#8：`code_review.txt` 与 `plan_review.txt` 新增与 `core_identity` / `planner` 一字不差的 first-principles、plain-language+ASCII、UX-first 三句原则；`load_test.rs` 同步扩展 reviewer 双模板逐字守卫。验证：`cargo test --manifest-path tomcat/Cargo.toml --lib standards_6_7_8_are_byte_identical_in_core_identity_planner_and_reviewers` 通过 @2026-07-16
- [✓] **[P0]** `feature/prompt-optimization` 已 fast-forward 合入 `develop`（`9f940b7 -> 46b7ef7`）：系统提示词/工具描述瘦身、`prompt_guidelines` 聚合、工程规范 #6–#10 固化进 `core_identity`/`planner`、`parallel_tools`/`verification` 段与预算门；同提交附带 `cloud-scale-serving-01/` 方案文档与 `commit-with-status` 路径迁移。详情见 `docs/status/feature-prompt-optimization.md` @2026-07-16
- [✓] **[P1]** 云化服务技术方案已落盘：`tomcat/docs/architecture/cloud-scale-serving/` 新增 `01-overview` + Phase A/B/C（`02/03/04`），并在架构 README 登记「扩展性与云化」阅读顺序；纠正「全站单 EventBus」误诊，明确 A 期单机 mailbox/热温冷为先落地、本地 `stdio` 零回退；尚未进入 Phase A 编码 @2026-07-15
- [✓] **[P0]** `feature/transcript-ui-and-checkpoints` 已 fast-forward 合入 `develop`（`866752c -> 62c8811`）；develop-side review 发现并修复跨 session restore 漏洞：`restore_checkpoint` / `/restore` 现拒绝不属于当前会话的 checkpoint，避免 `revertFiles=true` 把共享工作区回滚到别的会话快照；新增 `serve_restore_checkpoint_rejects_foreign_session_checkpoint` 回归测试，`cargo test -p tomcat serve_restore_checkpoint -- --nocapture` 4/4 通过 @2026-07-15
- [✓] **[P0]** develop-side 扩展快门禁与真实宿主高风险复核完成：`npm run gate:fast` 全绿；`TOMCAT_E2E_GREP='renders the \\.plan\\.md custom editor|renders transcript action rows and context groups|keeps sticky user prompts aligned with historical turns in the Tomcat webview|restores plan cards and Ctx after switching sessions' npm run test:e2e:vscode-devhost` 4/4 通过 @2026-07-15
- [✓] **[P0]** develop-side Rust web-search 门禁已收口：把 `tomcat/tests/cli_tests.rs` 与 `tomcat/tests/web_search_tool_tests.rs` 中真实 HTTPS mock + plugin runtime 成功路径的外层 wall-clock budget 与 timeout 失败 teardown 对齐，消除 nextest 满并发下的假红 / slow / hung；focused 复核全绿，`./scripts/run-integration-tests.sh integration-parallel` 364/364 通过（仅 `real_mimo_web_search` 被标 slow 但 PASS）@2026-07-15
- [✓] **[P0]** `feature/add-model-form-redesign` 已快进合入 `develop`（HEAD `68a4f43`）：覆盖 Add Model 双模式 / `ModelView.source` / `ThinkingFormat::Auto` 收口、Composer `@` 上下文搜索，以及 Transcript diff / plan / sticky UX 修复；develop-side review 中顺手补齐 Webview live diff snapshot 复用、`hashline_edit.rs` clippy 收口与对应 E2E 断言对齐 @2026-07-12
- [✓] **[P1]** 新增根目录 Cursor command `/release-cli-ext`：固化 CLI→EXT 发版顺序（patch+1、develop 推送、main/master fast-forward、cli tag 等资产、ext tag 等资产），减少手工漏步 @2026-07-06
- [✓] **[P1]** Add Models 架构方案：新增 `tomcat-vscode-ext/docs/architecture/model-management-add-models.md` 总览 + 5 篇子文档（术语/决策/协议/验收/UI ASCII 基线），对齐 Phase 2 文档组织方式 @2026-07-06
- [✓] **[P1]** Add Models 实现已合入 `develop`：以 `git merge --no-ff` 合并 `feature/add-models`（merge `68ff90a`），并在分支侧与 `develop` 侧完成 Rust/CLI/serve 全量门禁、VS Code 扩展 build/unit/integration/GUI/VSIX install/verify 复跑，验收通过 @2026-07-09
- [✓] **[P1]** 会话标题修复已落地：后端新增 `ChatMessage::first_text()` + `extract_user_text_from_content()`，让 `content` 为 `Parts`/`input_text` 的首条 user 消息也能正确派生标题；`run_loop` 叠加 L0 即时 `session.title_updated` 与 title scene 失败时降级到主模型；扩展状态机/E2E fake host 同步补齐。验证：`cargo test --lib first_text`、`cargo test --lib extract_user_text_from_content`、`cargo test --lib append_user_message_with_structured_parts_derives_title_from_input_text`、`cargo test --lib read_first_user_message_text_supports_structured_input_text_parts`、`cargo test --test transcript_summary_integration_tests session_title_updated_`、`npm run lint`、`npx vitest run src/ui/webview/tests/state.test.ts`、`TOMCAT_E2E_GREP='derives non-placeholder session titles from first webview prompt segments' npm run test:e2e:vscode-devhost` 全绿。

### 🔌 INTERFACE (接口变更)
- **2026-11-04 首启/MCP/连接恢复**：磁盘会话列表在缺项目scope时返回既有 `scope_requires_cwd`；webview新增 `openFolder` intent并使用 `workspace_required` 展示引导。`LlmResolver::report_connect_failure` 提供默认空实现，默认resolver仅对最终主请求本地Connect失败刷新传输缓存；`ConnectorRegistry::request_reload` 先重读配置再预约目标。普通启动补建缺失 `mcp.json`，已有字节不改；无新增配置键、依赖、存储格式或wire版本。
- **2026-11-03 write-plans 结构整理**：仅改变内置计划技能的写作/结构约定及配套测试；新增技能附件 `references/writing-standards.md`，`references/plan-structure.md` 保留章节骨架、决策表与测试设计。未修改工具实现、权限、计划状态机、持久化格式、跨进程协议或依赖。
- **2026-11-02 计划执行就绪**：仅调整写作/评审提示词契约；不改执行器、`ask_question` 实现、工具权限、状态机或审计存储。reviewer 保持建议性输出及原 `<review>` 格式，不代用户决定或授权。
- **2026-11-02 Unix 安装入口**：`install_user_command` 的签名不变；Unix 不再保留内容不同的普通入口，软链一律迁移为副本。`init`/`self-install` 安装当前程序的快照，后续仅编译不会自动刷新，需再次执行安装；macOS 通过根目录 `./scripts/dev-install.sh` 完成编译与安装，不改 PATH、配置、凭据或服务生命周期。
- **2026-10-20 ACP 技术方案**：仅记录拟议接口与实现边界，未修改现有 CLI、serve、扩展或宿主接口；不能将文档中的 access/origin、共享连接和 ACP 示例当作已交付协议。
- **2026-10-19 IP 中转站**：无新增接口、配置键、存储格式或依赖；仅改变数字开头的自动默认槽名，严格校验 `^[A-Z_][A-Z0-9_]*$` 不变。旧数字 provider 若使用默认槽，须重新配置 Key 或把启动器注入变量改为 `PROVIDER_<规范化名称>_API_KEY`；合法显式槽不变，不新增自动迁移或旧名回退。
- **2026-10-19 平台兼容修复**：无新增 API、配置键、存储格式或依赖；Unix 的反斜杠保留为文件名字符，Windows 的两种分隔符行为不变。私有写入沿用创建→设权限→写入→原子发布流程，Windows DACL 与普通文件写入逻辑不变；NO_PROXY 改动仅在四个测试环境夹具内，不修改用户系统代理。
- **2026-10-17 会话菜单收起**：无新增生产接口／协议。新增关闭触发来自用户选择或页面失焦，不随会话快照更新触发；既有框外点击／Escape 等关闭行为保留，会话身份仍等宿主确认。删除弹窗沿用暂停关闭监听，置顶／删除管理和草稿保存流程不变。
- **2026-10-17 会话恢复**：无新增接口或存储格式；仅补齐显式切换已打开会话时的 scope 级 current 持久化。写失败沿用 dispatcher 错误帧且不切内存 active，registry setter 仍只负责内存；不新增前端选中态。
- **2026-10-17 默认模型入口调整**：无新增宿主／CLI／wire 接口。GUI 继续发送原有 `setDefaultModel` / `setUtilityModel`；宿主报告相应角色值才显示选择器，能力仅决定是否可写，已保存值仍以宿主快照为准。首次接入自动设默认、供应商分卡和用户配置存储不变。
- Serve 新增 `list_remote_models`、`upsert_models`、`remove_provider` 与 `set_utility_model` 四个命令，各有对应 capability。`set_default_model` 增加 `onlyIfUnset`，返回 written/skipped，并声明 `set_default_model_if_unset`。`list_models` 增加 `utilityModelId`；模型视图增加 `keySource`，取值 saved / environment / null。wire 镜像与 schema 已同步。
- `Speed` 只剩 Standard/Fast，`"ultrafast"` 作为 Standard 的 serde 别名读入，`model.speedApi` 已删除。Anthropic 选 Fast 时发 `speed: "fast"` 与 `fast-mode-2026-02-01` beta。Chat Completions 的续传名单与 `preserve_history` 已删除，统一只回传当前回合。
- `llm.title_model` 改为字段级默认 `deepseek-v4-flash`；`context.compaction_model` 默认改为空，候选顺序为显式配置 → 会话当前模型 → 默认模型；init 不再写入压缩模型。
- 历史 message 新增可选本地 `is_error`，映射到 Host/GUI 可缺省的 `isError`；缺省代表未知而非成功。发送到模型前剥离，Serve 命令/事件顶层结构不变。
- 新增 `start_plan({plan_id?})`；`ask_question` 支持 executing，描述提示执行期仅必要时提问。`update_plan` 仍只改 todo，权限与 build 状态闸门不变。
- 配置加载忽略未知/已删键并在日志就绪后告警，`load_config_for_init` 删除；旧连接字段不自动迁移。新增 `serde_ignored` 0.1.14。
- 发版链路新增可选仓库变量 `PUBLISH_MARKETPLACE`（未设置时关闭）；GitHub `publish` 仍依赖完整 `build/verify`，市场 job 仅依赖已验收且已发 GitHub 的相同资产。历史显示问题的版本例外现已删除，所有当前安装门禁都必须恢复真实拒写状态。
- 本轮新增 `LlmResolver::resolve_metadata`、`selected_model_id`、`model_ready`；`get_state.modelReady` 为可选布尔值，Host/GUI 仅在显式 `false` 时限制发送，兼容未提供该字段的旧 CLI。早期未提交实现的 `chatConfiguration` 已移除。
- `list_models` 增加 `defaultModelId`；新增 `set_default_model` 命令/能力与 `TomcatMessenger.sendSetDefaultModel`；Settings 新增 `setDefaultModel` intent、创建模型的 `setAsDefault` 标志及关联 `modelSaveReceipt`，wire/schema 夹具随代码提交。
- `DefaultLlmResolver::compaction_candidates` 依次尝试配置的压缩模型、默认模型、当前主模型并去重；`ServeConnectionSupervisor.reconnect()` 删除旧 setup 模式参数，新增 `summarizeServeFailure`；webview 新增 `connectionFailure` 和重试/查看日志 intent。
- 推理强度 / 模型配置链路：Rust `ModelEntry` / `UserModelEntry` / `ModelView` / `ModelEntryInput` 现显式携带 `supported_reasoning_levels`，`UpsertModelResponse` 新增 `warnings`；webview state 新增 `availableModelReasoningLevels`，`Composer` 的 thinking 控件改为按模型动态渲染；`ProviderCacheKey` 新增 `catalog_generation` 以支持模型配置热生效；`thinking_format` 空值保存时会按 API 物化；Anthropic 新增 `anthropic-adaptive` 编码路径（`thinking.type=adaptive` + `output_config.effort`）。
- plan 预览热更新链已从“`onDidChangeTextDocument` + 读旧缓冲”改为 **serve `plan.update` / `plan.todos` → `provider.handleServeEvent()` 桥接 → `PlanPreviewEditorProvider.refreshFromServeEvent(planId, pathHint)` 磁盘重读 → webview `state` 帧重发**；`PlanPreviewDomSnapshot` 新增 `contentScrollTop` / `topVisibleSourceLine`，`PlanPreviewDomAction` 新增 `setContentScrollTop` 供 E2E 锁“刷新不跳顶”。
- transcript rich rendering 四次整改：前端新增 `splitTopLevelBlocks(markdown)` 与同步 `highlightToHtml(code, language)`；聊天富渲染链改为“父组件切顶层块 + 子块 `React.memo(raw)` + 同步 `hljs` + mermaid 按块异步”，`ChatMarkdown` / `MessageBubble` / `TranscriptView` 不再为 assistant 富渲染透传 `isStreaming`。Vite 产物新增独立 `dist/chunks/highlight.js` 分包以控制首屏成本。
- transcript rich rendering：前端新增 `ChatMarkdown` / `markdownRuntime`；**assistant 正文**改为 markdown 富渲染；代码围栏支持 `lang path[:line]`，但**不再显示语言标签**，而是按“有无文件路径”分两形态：有路径 → basename 头部 + 纯图标 copy；无路径 → `bare` 块 + 内容区右上角浮动 copy；正文内联 `` `path:line` `` 显示 basename、hover/title 保留完整相对路径并可点击打开；**thinking 正文已回退为弱化纯文本 `<pre>`**，不再解析 markdown，也不再在 thinking 内生成 clickable path。
- plan transcript / 卡片交互：`WebviewToolCard` 现携带 `planActivity / planId / planPath`；`plan.create` 会把 `path/planId` 提前盖到 running `create_plan` 工具上，因此 `PlanFileCard` 能在 reviewer 运行期间先显示 `view-plan-pending`，完成后再切回 `View Plan`；事件行 `View Plan` 链接改为扁平常显淡下划线 + 小 chevron，卡片 footer `View Plan` 补 hover 变白，卡片 `Build` 复用 `tc-plan-build-button`（黄底 / 黑字 / 非加粗）。
- `MarkdownBody` 现复用 `buildDecoratedHtml(markdown, sourceLineMap?)`，所以 plan 预览和 transcript 共享同步高亮、代码卡片、copy 按钮与内联文件链接语义，同时保留 `data-source-line`。
- webview openFile：`gui/types.ts` / `src/ui/webview/protocol.ts` 的 `openFile.data` 新增可选 `line?: number`；`VsCodeIde.showFile(path, line?)` 会把 1-based 行号映射到原生编辑器 `selection + revealRange`。
- system prompt：`PromptKey::SystemOutputConventions` 加入默认系统提示词 section 链，顺序位于 `tool_instructions` 之后、`parallel_tools` 之前；约束 inline-code 路径、`lang path` 围栏、禁 `file://` / `vscode://` / `【F:...†L...】`。
- 测试门禁调度：`tomcat/.config/nextest.toml` 的 `profile.default.test-threads` 固定为 `4`；默认 integration / E2E 车道不再按 `num-cpus` 满并发运行，`real-llm` 仍保持 `max-threads=2`。
- transcript / history：新增 `TranscriptEntry::Error { summary/detail/... }` 与失败 user `turn_failed:true`；`Error.detail` 脱敏覆盖 `authorization/bearer/api_key/token/cookie/set-cookie/x-api-key/password/secret/basic/sk-*`，超 8KB 截断。
- webview 单前端：删除 `@tomcat` chat participant 与 `ownership/uiMode/conflict` 仲裁层；webview 协议不再携带 `owner/conflictMessage/uiMode`。
- diff：`VsCodeIde.openPreparedDiff` 在用户未显式配置时将 `diffEditor.renderSideBySideInlineBreakpoint` 设为 `0`，保证窄窗左右并排。
- `list_provider_keys` 的 wire 形状保持兼容，但语义调整为 `.env` 唯一清单来源：每次调用热重读，仅返回合法非空 `*_API_KEY` 的名称/状态；不再用模型引用补充名称，也不返回完整或部分 Key。`ModelEntryInput.api_key_env` 与 `set_provider_key.envName` 统一校验 `^[A-Z_][A-Z0-9_]*$`。
- `PromptKey::ReviewerPlan` / `PromptKey::ReviewerCode` 对应模板现内联共享的三条 operating principles（与 `SystemCoreIdentity` / `PlannerReminder` 文案逐字一致）；无对外部客户端的破坏性 API 变更。
- `BuiltinToolCatalogEntry::prompt_guidelines` + `render_tool_guidelines_with_policy`；`PromptKey::SystemParallelTools` / `SystemVerification`（系统提示词 section priority 22/50）。无对外部客户端的破坏性 API 变更。
- 新增架构文档入口：`tomcat/docs/architecture/cloud-scale-serving/` 与 `cloud-scale-serving-01/`；不改变现有 `serve` / `stdio` 对外契约。
- `restore_checkpoint` / `/restore` 现强制 checkpoint 的 `session_id` 必须等于当前会话；跨 session restore 返回错误 `checkpoint 不属于当前会话，不能跨会话 restore`，不再允许误回滚共享工作区。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 后续处理 |
| :--- | :--- | :--- |
| 首启/MCP/旧连接恢复的交付范围（2026-11-04） | 本次提交不重跑测试，复用上方分批实施验收；Windows新分支、公网网站和真实收费模型仍未验证；已有窄侧栏裁切与宿主告警未修，npx检查进程SIGSEGV根因未定但同源码本地TypeScript补验通过 | 本次只本地提交，不升版、推送或发布；发布前补相应平台验证，原失败日志与离线重核证据保留 |
| 旧式动态插件串会话（未修复） | 第二会话发现资源会向第一会话追加消息；复现补丁为 `.agents/acceptance/shared-serve-review/legacy-dynamic-discovery.repro.patch` | 需另定初始化会话归属并修复；本轮 CI/发布通过不代表该遗留问题已关闭 |
| 执行就绪检查的效果边界（2026-11-02） | 已验证条款与加载链路，未进行真实模型行为评测，不承诺 100% 无人值守 | 生效取决于运行包含新资产的 CLI 并在新会话加载，同名技能覆盖规则不变；新风险或超出授权仍需确认 |
| 命令入口整改的跨平台验证（2026-11-03 云端已补齐） | `e2f9ae6e` 与 `93a467a7` 的三系统 Rust、Windows 扩展/命令包装器及真实窗口 CI 均通过 | 仍不冒充个人 Windows 设备验证；本机未重新安装或重启服务 |
| 内核崩溃后续观察（2026-11-02） | 仅确认当前已卸载 MuMu 并备份，尚未重启核对或执行 Apple Diagnostics，崩溃根因未证实 | 用户方便时重启，核对 showloaded/AuxKC；必要时按原计划评估 clear-staging 后再重启，并观察新 panic，不自动重置全部驱动授权 |
| 共享 serve / ACP 验收边界（2026-10-20） | 本次仅完成设计文档，计划尚未实施，T01–T28 全部待执行 | 后续在功能分支实施并验证两个宿主和 VS Code；文档校验不替代真实任务、停止、慢读及 UI 验收 |
| IP 中转站验收边界（2026-10-19） | 此前 admin 首轮 26/28，两个删除 Key 用例继承本机 FCODEX_API_KEY/DEEPSEEK_API_KEY；仅对子进程移除这两个变量后 28/28，未修改凭证文件或断言 | 保留环境失败记录；浏览器使用模拟宿主，未访问报告 IP 或验证真实中转站认证；本次提交按要求不跑测试，不称全仓门禁通过 |
| 会话菜单交付范围 | 已完成真实 Dev Host + fake Serve 的焦点／菜单验收并交付纯 VSIX；安装升级用例同步语义且类型检查通过 | Agent 未安装该 VSIX，不将源码宿主验证或用户请求提交等同于完整安装包／真实 CLI 验收；首次取证超时与后续通过记录均保留 |
| 会话恢复范围 | 保留同 agent／项目 scope 级共享 current；用户已反馈本次修复验收通过 | 不承诺多个窗口各自独立恢复；尚未正式发布，不把用户验收反馈表述为 Agent 已采集主窗口截图／日志 |
| 本轮既有格式与代理环境问题 | 全仓 fmt 的 17 个文件格式差异与 HEAD 一致；命令模块首轮 6 项本地 HTTP 测试受代理环境影响 | 不并入无关格式化；补本机 NO_PROXY 后同一二进制 6/6 复验通过，保留首轮失败，不冒称一次聚合全绿 |
| 本次提交的验证边界（2026-11-03） | 按用户要求不运行测试或构建；本次 write-plans 新增/调整的提示词契约与技能附件物化用例尚未执行，Cov% 未测量 | 本次仅做完整差异、空白与三类变更区域核对；后续补提示词和技能物化相关用例，不以历史通过项代签本次改动 |
| 本机数据验证边界 | 实际 36 模型与近期 3 会话及其引用图片，在隔离 HOME、禁止网络和外部写入条件下通过；原始受保护配置/凭证未读，使用 config_get 非敏感默认模型生成隔离配置 | 不把样本读取说成全目录逐字兼容、所有历史或真实凭证认证通过；先前等待豁免已被安全的数据读取方案替代，没有把未回应当同意；Windows/Linux 新版原生安装未执行 |
| 本轮宿主日志边界 | 真实包完整 workbench 有 E2E harness 的 Marketplace 元数据 404；fake 安装 session-files 用例曾记录 VS Code `Illegal value for lineNumber`，用例本身通过 | 与注入错误边界测试的预期崩溃分开记录；不称全宿主零告警，不归为产品 webview 的运行时错误 |
| Linux CI 宿主 shell 告警（2026-11-03 已修复） | 原夹具强制 `/bin/zsh`，Linux 截图出现 ENOENT 通知；`93a467a7` 改为保留宿主 SHELL 并补断言，正式 0.1.99 宽窄截图已确认通知消失 | 旧证据与已发布资产不修改；harness Marketplace 404 和宿主告警另行保留，不宣称全宿主零错误 |
| QuickJS 时间敏感失败仍未解决（2026-10-19） | 此前两项 20ms 超时保留；本次 main CI 的 `code_vm_polling_and_sibling_progress_cannot_renew_a_wrong_token_call` 再次失败于 `code_test.rs:125` 的 `!vm.is_finished()` 断言，lib 为 3345 passed / 1 failed / 5 ignored | 本次没有输出 20ms 超时，不能据此钉死同一根因；同 SHA 的 develop macOS 已通过，也不证明问题已修。保留原始日志与失败，不重复运行来掩盖结果 |
| 本轮平台验收边界（2026-10-19） | 开发机专项验证为 Intel macOS；release-ext run `39180141401` 已通过 macOS ARM64/x64、Linux x64、Windows x64 四个平台的真实发布包原生安装验收 | 原生安装通过不替代完整 Rust 单测、所有 Linux 发行版或真实供应商验证；Windows Rust 普通 CI 与安装门禁分别记录 |
| 本轮全量门禁记录（2026-10-19） | 平台修复后 gate-fast 原始结果为 lib 3342 passed / 4 failed / 5 ignored、parallel 376 passed / 3 failed、serial 50 passed；后续为定向隔离和复验 | 保留原始失败与后续通过记录，未重新执行完整门禁，不将定向 4/4 与代理对照通过写成全量全绿 |
| 普通分支 CI（截至 2026-10-19 08:27 +0800） | develop `39179229183` 的 macOS/Ubuntu Rust 与 Windows 扩展通过；main `39179237764` 的 Ubuntu Rust/Windows 扩展通过、macOS Rust 失败；两边 Windows Rust 尚在运行 | 与已成功的 release 工作流分开，后续状态以对应 GitHub run 为准，不把尚未完成的作业写成通过 |
| 历史安装包仍携带旧显示缺陷 | CLI 0.1.76 / EXT 0.1.92 正式三平台原生验收已通过拒写与重载后的状态断言；既有 CLI 0.1.74 / EXT 0.1.90 Release 不回写 | 使用新版本接收修复；旧无状态记录中性展示，不改写历史，不再使用旧版本已知问题豁免 |
| 市场更新验收边界 | 0.1.92 真实 OIDC 三平台上传、公开 hash 和保留 profile 的 0.1.89 → 0.1.92 正常市场更新已验证 | 不把 `--update-extensions` 当作无人值守定时更新，也不把 CI 受控模型当作真人首配/付费模型验证；长期总验收边界保留 |
| 既有验证边界 | 历史发布覆盖三目标原生受控模型链路，本轮仅 macOS x64；未覆盖所有 Linux 发行版、远程 OAuth、真实供应商组合；历史 `cargo audit` 的 bincode/smartstring/yaml-rust unmaintained 警告保留 | 不把 CI 的本地 HTTP fixture 当作真实外部模型认证证明 |

- 历史 `real_mimo_web_search` slow 标记仅作性能观察，不把其旧 PASS 作为本轮复验。
- Phase A 编码尚未启动；待评审确认后再按 `02-phase-a-session-mailbox-hot-cold.md` 落地。
- 「同会话关内网后反复 403、新会话可恢复」现象已验收可接受（通网后摘要正常），根因未再钉死到 `previous_response_id`；临时诊断日志已移除，后续若复现再开文件日志排障。

### 集成说明
- 最新补充（2026-07-20 14:25）：`feature/transcript-rich-render` 推理档位 key 修复门禁全绿；产物 `tomcat-vscode-ext-0.1.18.vsix`，CLI `0.1.15`。
- 最新补充（2026-07-19 23:40）：推理强度 v2（列表式）分支侧门禁已全绿：`cargo clippy --all-targets -- -D warnings`、`cargo test`（含 `2153` 个 lib tests、integration tests 与 doc tests）、`npm run check:wire`、`npm run lint`、`npm run test:unit`（core `208/208` + gui `299/299`）、`TOMCAT_E2E_GREP='adds a model through settings and uses it in the webview' npm run test:e2e:vscode-devhost`（`1/1`）以及 `npm run package:vsix`；产物为 `tomcat-vscode-ext/tomcat-vscode-ext-0.1.16.vsix`。
- 最新补充（2026-07-19 18:02）：`feature/transcript-rich-render` 上的 plan transcript 事件行+产物卡模型、pending 呼吸盖章与 View Plan/Build UX 收口已落盘；分支侧门禁为 `npm run lint`、完整 `npm run test:unit`、targeted plan UX / `.plan.md` custom editor devhost E2E、全量 `npm run test:e2e:vscode-devhost` 与 `npm run package:vsix`（`tomcat-vscode-ext-0.1.16.vsix`）全绿。
- 本轮验收整改收尾验证：`cargo test --lib` 全绿；`npm run lint` + `npm run test:unit`（ext+gui）全绿；`TOMCAT_E2E_GREP='adds a model through settings|applies edits from the Tomcat webview|recovers from a failed same-session retry|keeps multiple Tomcat webview sessions isolated|restores plan cards and Ctx after switching sessions' npm run test:e2e:vscode-devhost` 全绿；`npm run package:vsix` 产出 `tomcat-vscode-ext-0.1.16.vsix`。分支侧细节见 `docs/status/feature-transcript-ui-and-checkpoints.md`。
- 本轮 Add Model Key slot 验证：`cargo test --manifest-path tomcat/Cargo.toml --lib admin_test` 14/14、`serve_model_admin_roundtrip_updates_key_presence` 1/1、`npm --prefix tomcat-vscode-ext run lint`、GUI `relayDerive.test.ts + SettingsApp.test.tsx` 12/12、`SettingsPanel.test.ts` 5/5 全绿。
- 本轮 reviewer prompt 对齐已做 focused 验证：`cargo test --manifest-path tomcat/Cargo.toml --lib standards_6_7_8_are_byte_identical_in_core_identity_planner_and_reviewers` 通过；未额外触发更大范围门禁。
- 本轮仅文档：云化技术方案四篇 + 架构 README 地图；无 Rust/扩展代码变更，不触发集成门禁。
- `feature/transcript-ui-and-checkpoints` 已本地 fast-forward 合入 `develop`（HEAD `62c8811`），覆盖 checkpoint restore / transcript UI / `.plan.md` custom editor / bash summary upgrade；develop-side acceptance 现已 **通过**，结论更新为 **GO**（尚未推送远端）。
- Rust develop-side 复核确认：先前 web-search 红点并非产品逻辑回归，而是测试自身的预算/清理脆弱。真实 HTTPS mock + plugin runtime 成功路径在 nextest 满并发下会逼近 5s/10s 外层 timeout；部分用例又在 `end_session` 前 panic，导致大盘表现为 slow/hung。现已在 `tomcat/tests/cli_tests.rs` 与 `tomcat/tests/web_search_tool_tests.rs` 放宽 success-path wall-clock budget，并保证 timeout/result error 时先 teardown 再失败；focused `cargo test --test web_search_tool_tests works_from_public_api -- --nocapture`、`cargo test --test web_search_tool_tests runtime_auto_routes_to_plugin_backends_after_retryable_failures -- --nocapture`、`cargo test --test web_search_tool_tests runtime_session_vm_survives_idle_beyond_call_timeout -- --nocapture`、`cargo test --test cli_tests test_chat_path_executes_web_search_tool_with_mock_server -- --nocapture` 全绿。
- Rust 默认快门禁已重新闭环：`cargo clippy --all-targets -- -D warnings`、`cargo test --lib`、`cargo test --doc` 与 `./scripts/run-integration-tests.sh integration-parallel` 全绿；其中 integration-parallel 为 `364/364 passed`，仅 `real_mimo_web_search` 被 nextest 标 slow 但正常 PASS。
- 此分支无独立 `PENDING_INTEGRATION` 任务卡映射；develop-side 仅记录合并与验收事实，分支侧证据以 `docs/status/feature-transcript-ui-and-checkpoints.md` 为准，不额外伪造 TASK_BOARD 状态迁移。
- `feature/add-model-form-redesign` 已在 `develop` 合并验收：扩展 `npm run check:wire`、`npm run gate:fast`、`npx vitest run src/ui/webview/tests/provider.test.ts src/ide/tests/diff_apply_edit.test.ts` 全绿；Devhost 全量先暴露 `applies edits from the Tomcat webview` 回归，补齐 live prepared snapshot 语义后，`TOMCAT_E2E_GREP='applies edits from the Tomcat webview' npm run test:e2e:vscode-devhost` 复绿。
- 真 LLM 复核本轮未追加 `integration-real-llm`：本机 `DEEPSEEK_API_KEY` / `MIMO_API_KEY` 在 `.env` 中存在，但缺 `OPENAI_API_KEY`，不足以按当前高风险清单完整复跑 OpenAI 直连口径；该空白已显式记录，不冒充“全绿”。
- 面向用户文档已双语化（英文默认 + opencode 式切换栏）：根 README、扩展 README、`user-guide` 各新增 `.zh.md` 中文镜像；`tomcat-vscode-ext/.vscodeignore` 已放行 `README.zh.md` 入 VSIX。
- 根 README 已对齐双组件 monorepo 现状：补充 `tomcat/` + `tomcat-vscode-ext/` 组件索引、Agent Box/CLI 双入口架构图，并修正终端用户前提（`tomcat init` → `~/.tomcat/assets/.env`；Rust 1.70+ 仅源码构建需要）。
- 用户文档已切换主推 **Tomcat Agent Box**：根 README、扩展 README、`user-guide` 与 `package.json` 面板/命令文案同步更新，新增 `assets/tomcat-agent-box.png` 截图；`manifest_contract` 与 GUI 单测已复跑通过。
- 本次 Add Models 按例外流程验收：以 `tomcat-vscode-ext/docs/architecture/model-management-add-models.md` 及其 5 篇子文档作为验收 SSoT，未回填 `TASK_BOARD_002` 任务卡，也未补建 `tomcat/docs/status/feature-add-models.md`；合并与验收口径均按用户确认流程执行。
- 已在 `develop` 上以 `git merge --no-ff` 合入 `feature/add-models`（merge `68ff90a`），并再次复跑 `tomcat/` 的 `./scripts/run-integration-tests.sh all` 与 `tomcat-vscode-ext/` 的 `npm run build`、`npm run test:unit`、`npm run test:integration`、`npm --prefix gui run test`、`npm run test:e2e:vscode-install`、`npm run verify:vsix`，全部通过。
- 已在 `develop` 上以 `git merge --no-ff` 合入 `feature/tomcat-vscode-extension`（merge `2b04acd`），覆盖 `T2-P1-019` 与 `T2-P1-020`。
- Rust develop-side 门禁已通过：`./scripts/run-integration-tests.sh all` 全绿；`integration-openai-responses-wire` 在 LiteLLM 直连网关口径下复跑通过。集成期补修 `tomcat/src/ext/runtime/instance.rs`，把 QuickJS host bootstrap 排除到插件 timeout budget 之外，并补强对应断言，消除 `quickjs_e2e_tests::runaway_plugin_timeout_interrupts_when_budget_disabled` 的假红；同一提交还带入 `tomcat/src/core/tools/primitive/executor/write_edit.rs` 的备份提交路径收敛与回滚失败显式报错，并为 overwrite/rollback 边界补了配套测试。
- VSCode 扩展门禁已通过：`npm run build`、`npm run test:unit`、`npm run test:integration`、`npm --prefix gui run test`、`npm audit`（0 vulnerabilities）、`npm run package:vsix`、`npm run test:e2e:vscode-install`（26 passing）、`npm run verify:vsix`（4 passing，含截图裁剪产物）。
- 为消除 VSIX 安装 E2E 的 develop-side 假红，本轮补稳了 `hostE2eScenario.ts` / `e2eHostFixture.ts`：ownership 场景改为显式挂起 participant 问答后再切 webview，transcript UI 场景在常规安装套件允许直接落到最终折叠态，而 `verify:vsix` 继续强制捕获 docked todo/progress 视觉证据；同一提交也修正了 `App.tsx` / `Composer.tsx` / `provider.ts` / `handler.ts` 的 ownership 释放、interrupt 可用性与 history loading 收口，并补上单测与安装 E2E 覆盖。
- 4 件套 review 已覆盖 `serve`、`summary`、`plan_tool`、`primitive/executor` 与 VSCode 扩展热区；未发现未记录的规范违背。`integration-real-llm` 本轮无新增 target，按 `INTEGRATION_MERGE_AND_ACCEPTANCE.md` §4 跳过，不构成本次合并阻塞。
- 结论：`feature/tomcat-vscode-extension` 已在 `develop` 完成合并、复测与验收，`T2-P1-019` / `T2-P1-020` 可转 `DONE`；对应验收提交现已位于 `origin/develop`。
- 已知残留（本期刻意不补）：插件 `ext/dispatcher/session_ops.rs -> try_append_message_to_session` 的注入路径仍不推送活跃 `session.title_updated`，disk 刷新后的标题正确；本轮仅修复 Prompt → `run_loop` 主链路，即计划中的“放法一”。
