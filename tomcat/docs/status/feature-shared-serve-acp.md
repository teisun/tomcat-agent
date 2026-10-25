| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat Agent | 2026-10-25 10:21 | ACTIVE | feature/shared-serve-acp | - |

### DONE（已完成）

- [✓] 修复临时 LLM 故障分类与重试额度按任务累计：`server_error`、`upstream_http2_stream_error`、`upstream_stream_read_error` 进入统一临时故障判定，账户/内容拒绝保持优先；每次逻辑模型请求独立计数，交互默认 4 次、无人值守临时故障和空答 10 次，均含首次。成功正文及已完成工具不因后续请求重试而重放。
- [✓] 增加宿主拥有的五分钟自动 Retry/Resume，页面只展示会话级倒计时并提供取消；沿用原 RPC、错误卡与手动按钮，同 errorId 连续失败可重新恢复，后台会话不抢焦点。
- [✓] 自动恢复复核 F1～F6 全部完成：补 Build 接管与外部执行取消，移出 Keep，恢复文件夹变化和历史加载原有边界；资格检查仅用于自动到期；单次请求及专用辅助函数逐字集中到 `request_retry.rs`，恢复无关文档。验收中的 `environment.rs` Clippy 修正仅为 `.or_else` → `.or` 等价改写，平台路径规则不变。

- [✓] 修复 VS Code 多根工作区引用丢失所属项目地址的问题：文件、目录及选区保留完整路径，标签仍简短；`@` 搜索独立显示项目名与相对目录，避免同名文件混淆。产品逻辑只改引用构造与搜索展示两处，补齐草稿/发送/历史、Rust 真实读取和双根 Host 回归；Windows 实机验收未完成。

- [✓] 修复 Windows/Linux Composer `+` 同时启用文件/文件夹选择导致只显示目录的问题：原生 QuickPick 先选类型，保留多选、分次追加及取消/异常时的草稿和操作收尾；补充英中文案、参数/回填回归与真实宿主菜单交互用例。
- [✓] 已生成包含本次修复的纯插件包 `tomcat-vscode-ext-0.1.97-pure-composer-fix.vsix`（不含 CLI），构建、可解压性及包内代码与当前构建一致性检查通过；未自动安装或重启用户服务。
- [✓] Windows shared serve／Native／ACP 修复与本机验收计划完成 12/12：修复中文 PATH 定位、共享服务子进程弹窗、按连接解析界面语言及回环模型请求误走代理；集成测试在用例前统一构建当前 CLI。
- [✓] Windows 开发更新与重启整改计划完成 6/6：新增 `tomcat self-install`、仓库根 `scripts/dev-install.cmd` 和共享项目规则；主输入框 `/restart` 复用既有宿主重启链，stop／restart 外层期限改为 80／150 秒。
- [✓] 本次一并纳入原已暂存的 HOME 修复：Windows `~\` 路径按控制连接的 HOME 展开；cwd 不因此获得额外权限，缺失 HOME 不回退到服务自身环境。
- [✓] 项目版本脚本已将 CLI 升至 `0.1.81`、扩展升至 `0.1.97`，bundled CLI 同步为 `0.1.81`；当时版本一致性检查及版本脚本 19 项测试通过。
- 更新双语 README、用户指南与共享服务架构文档，明确“更新磁盘文件”和“显式重启后台”是两步操作。

### INTERFACE（接口与行为边界）

- `AgentLoopConfig.max_attempts` 改为每次逻辑模型请求的额度，`max_tool_rounds` 仍归模型/工具循环；取消、压缩、附件降级、消息提交及工具执行的原有安全边界保留，重试不清零当前请求计数。
- 宿主↔webview 新增会话投影 `errorRecovery: { errorId, dueAt }` 与 `cancelErrorRecovery` 意图；Retry/Resume RPC 不变。取消只停自动恢复，不清除错误卡或手动按钮；切换会话不停止等待，真实断连、关闭环境或宿主销毁会清理；等待不持久化。

- 新文件系统引用的 `path` 保留扩展宿主 `uri.fsPath`（目录尾 `/` 契约不变），`label` 与搜索描述独立显示；会话 cwd、权限、重复 occurrence 及 wire 字段不变。旧相对路径不猜测迁移，旧歧义引用需重加；更新/重载扩展后新添加的引用才按修复后的规则生成。

- Composer `pickContext` 先显示“添加文件 / 添加文件夹”，`buildAttachmentOpenDialogOptions(kind)` 只开启对应类型；两类需分次选择，继续复用原有附件/引用分类与 `composerWorkResult`。未改拖拽事件处理、只读判定或共享服务。
- `tomcat self-install` 只安装当前可执行文件，复用既有平台安装函数；Windows 复制、macOS 软链接策略不变，不初始化配置、不改 PATH／凭据、不自动重启。保留 `TOMCAT_AGENT_ACTIVE` 嵌套保护。
- `scripts/dev-install.cmd` 仅转交 `cargo run --release --bin tomcat -- self-install`，沿用 `CARGO_TARGET_DIR`；显式 `tomcat.path` 和 VSIX 内置 CLI 选择不受影响。
- 主输入框精确纯文本 `/restart` 先清空并同步草稿，再调用 `restartSharedService`；停止所有共享任务，不调用模型、不入队。参数、引用、附件和同名资源卡沿用普通消息路由；输入禁用条件不变。
- stop／restart 等待上限为 80／150 秒，version／status／start 仍为 35 秒；关窗与 Reload 仍只断开窗口，不停止后台。
- LLM 仅在无显式代理、主地址与可选 fallback 都为回环时绕过自动代理；远端与显式代理语义保留。窗口语言按连接环境解析，服务生成的文本仍使用进程级语言，不承诺多窗口独立重译。
- Rust serve wire 及安装／服务生命周期协议未改动；本次协议增量仅限上述宿主↔webview 的恢复投影与取消意图。

### Windows 验证记录（此前结果，Mac 不机械重跑）

- Windows 提交准备阶段按用户当时要求，不运行测试、构建、安装或视觉验收；提交准备仅审查 diff、文件范围、版本声明和 Git 空白检查。Cov% 未测量，不将用例通过率当作代码覆盖率。
- 选择器修复此前验证：`npm run lint` 与相关 22 个测试文件的 470 项用例通过；随后构建/编译及纯 VSIX 内容检查通过。当时提交未重跑。
- 隔离 Windows VS Code 1.143 + 真实 CLI 的 `assertWebviewPickContextFlow` 已验证真实点击 `+`、原生 QuickPick 和附件/引用回填；PNG/ARIA/console 已检查，无 Tomcat Webview 控制台错误。系统文件对话框仍由替身返回 URI 并校验互斥参数，不标作原生选择文件验收通过；测试模型图像能力警告与加载中的缩略图不作为图片预览验收。
- 本地诊断及制品证据：`.agents/acceptance/composer-drag-probe/verification.md`、`gui-ru4M8V/` 和扩展目录下纯 VSIX，均不纳入 Git。原安装版 VS Code 更新锁、Windows fake-Serve 夹具 `spawn EFTYPE` 阻塞保留记录，后改用隔离副本和既有真实 CLI 夹具完成菜单/回填验证。
- 第一项计划此前记录：Windows Rust 影响范围回归 717/717；Native／ACP 冷构建与三次预热每轮 8/8；真实 VS Code 旁观与关窗旅程各 1/1，PNG／ARIA／console 已读回。
- 第二项计划此前记录：薄脚本 2/2；Rust 首批 184/187，修正三处测试缺陷后相关测试及帮助目录检查 7/7；host 807、GUI 877 项经分批检查与复验覆盖。原 gate 失败及 GUI 超时保留，不声称整条 gate 一次全绿。
- 最终真实 VS Code 重启旅程及外层检查退出 0：实例 A 切换到 B，非标题模型请求保持 2 次，关窗后禁止自动拉起的 Native IPC 仍连接 B；四组最终 PNG／ARIA／console 已读回。
- 详细本地证据：`.agents/acceptance/windows-shared-serve-report.md`、`.agents/acceptance/update-restart/report.md`；最终重启截图目录 `.agents/acceptance/update-restart/ui/gui-u7Ujzh/`。这些日志、截图和构建产物不纳入 Git，Mac 拉取代码不会自动获得本机证据文件。

### macOS 联合验收（2026-10-23–25）

- **请求级重试与自动恢复已在提交前完成验证**：最新扩展类型检查、宿主 900 项、GUI 888 项及 Provider/真实 Rust 服务集成 72 项通过；Rust 定向 394 通过 / 1 原有忽略，最终 `gate-fast` 严格 Clippy、库 3437 通过 / 5 忽略，集成 470 通过 / 30 跳过；无新增跳过，定向结果不与全量重复累计。doctest 为 0，不当作行为证据。
- 此前真实 Dev Host 已验证生产五分钟等待、页面重建保留 deadline、取消后手动恢复及后台不抢焦点，中英文桌面/窄屏 PNG、ARIA、console 已读取。整改未改 GUI 渲染或真实计时，按批准计划不重跑五分钟场景；四个搬移函数前后 SHA-256 一致，最终差异检查通过。早期磁盘不足、构建锁等待与截图夹具失败及后续成功结果保留在本地 `.agents/acceptance/error-recovery.md`，截图和制品不纳入 Git；不冒充真实上游或 Windows 验收。本次提交遵照用户要求不再运行测试、构建或覆盖率，Cov% 保持未测量。

- **多根引用修复本机验证完成**：macOS 13.7.8 / VS Code 1.143.1；扩展/GUI/启动器分批有效覆盖 1008 项（15+16+881+73+23），Rust 定向 3 项，真实 Host 6 项通过；构建、类型检查及格式检查通过。初始 Provider 两条旧路径期望、搜索测试断言与 Host 夹具问题修正后仅复跑受影响范围，不声称全仓一次全绿。当时提交按用户要求不重跑测试、构建或覆盖率，Cov% 继续未测量。
- 多根 Host 的 7 组最终 PNG/ARIA/console 已逐项读取，侧栏实际宽度 941/389px；截图、ARIA 与发送数据分别确认项目可区分、短标签和完整目标。Host 使用测试后端，Rust 录制模型与真实 read 独立验证收到 B 完整引用、读取 `ONLY_B` 且 cwd/授权边界不变；不冒充模型自主选文件或原生鼠标拖拽验证。捕获期 console 无 error，宿主警告另列。仓库根本地产物 `.agents/acceptance/multi-root-references.md` 与 `.agents/shots/multi-root-references/gui-1zk4mf/` 不纳入 Git，Windows 不会自动获得这些产物；Windows 复验使用相同 npm Host 入口及 Rust 定向测试。

```text
8a5b7885 → 共用自动化 → 真实 VS Code 旁观 / 重启 / 关窗
                              ↓
                    Mac 制品 → 安装 / Dock / 项目连接 / 语言（已补证）
                              → 正式重启 A→B → Reload / 全退出 / Dock 重开仍 B
```

- **本机自动化验收完成**：macOS 13.7.8 x86_64 / VS Code 1.139.0；Rust 定向 252 passed（3336 skipped 不计通过），扩展 gate:fast 两份 tsc + host 807 / GUI 877，四项真实服务 integration 修复后 11/11。
- 本轮修复的是验收准备：新 CLI 首次启动无输出超过 fixture 15 秒预算；既有构建准备增加有 120 秒上限的只读 `--version`，失败原样抛出，不放大 init/产品超时。新增 3 条回归测试和定向 strict tsc 通过；保留原始 10/11 失败及诊断，不推测系统扫描原因。
- 两条真实 VS Code 场景各 1 passing：同项目 ACP 实时只读旁观、跨项目隔离、Reload；真实写文件任务、菜单只补全、发送后后台 A→B、草稿清空，关窗后禁止自动启动的 Native IPC 仍连接 B。重启前后非标题模型请求为同样两次。
- 6 组 PNG/ARIA/console 均已逐项读取；两条截图脚本先最大化面板，真实图片为桌面 1381×794、窄屏 331×738（workbench 为 1440×900 / 390×844）。捕获窗口无 Tomcat 业务错误；不据此声称 VS Code 全进程零告警。
- 本轮没有改产品逻辑、依赖或版本；隔离自动化未改正式环境，未调用真实供应商。后续经用户明确授权，仅在正常 UI 中信任自建测试目录、切换语言并恢复 auto；未改日常代理，Agent 未执行正式安装或重启（由用户独立完成）。Windows 代码页 936、隐藏控制台、Job Object、EXE 替换在 Mac 为不适用。
- 两份本地报告：`.agents/acceptance/macos/8a5b7885f212f4549043a9b26502e6a6e89f5b20/shared-serve-native-acp.md` 与同目录 `update-restart.md`。目录内保留原始日志、截图、失败、源码快照；这些制品不纳入 Git。
- Mac release CLI 0.1.81 与内置同一 CLI 的 darwin-x64 VSIX 0.1.97 已构建校验，见 `artifacts/artifact-verification.json`。用户实际明确选择“纯插件 0.1.97 + 独立 CLI 0.1.81”，两者安装分别由 `manual-cli-install-20261023-232133.json` 和 `manual-pure-vsix-install-20261024-000233.json` 核对：软链/版本/哈希正确，纯包 251 个载荷文件及 manifest 与安装内容一致；不要求改装内置包。用户已确认 Dock 启动；新日志 process-path 指向独立 CLI 并 ready，后台 0.1.81 / PID 68026 / instanceId 66777c7a… 与退出前一致。
- `manual-ui-acceptance.md` 补充实际生产窗口：中文/空格测试项目连接；auto 跟随英文宿主、保存中文、关闭重开仍中文、恢复 auto；四张原 PNG 已逐张读取。新窗口首次自动启动被 Agent 保护拒绝、随后连接已有后台的日志保留，未移除保护。该普通窗口未取得完整 DOM ARIA / webview console，不将截图补证冒充新的全套 UI 门禁；正式重启及保活证据见下条。

- 用户明确回复“已重启”，`manual-official-restart.json` 记录正式服务 PID 68026 / 66777c7a… → PID 30430 / 5739b8b1…，版本 0.1.81，插件日志恢复 ready。新进程的可执行映射指向本轮交付 release，入口 SHA-256 与交付一致；不冒充手动重启瞬间的菜单/草稿截图。
- `manual-window-liveness.json` 完成正式保活补证：独立 Terminal 在 Reload 后和 Code 进程为零、尚未重开时采到 B/PID 30430；中间用户从启动台打开，随后按指导退出并明确确认“已从 Dock 重开”，07:10:22 新 Code PID 68569、插件 process-path → 独立 CLI → ready，后台仍 B/PID 30430。自动脚本原 Dock 点击 -1728 失败保留，不宣称全程自动；重开前采样来自第一次脚本退出。Terminal 辅助功能按用户要求保持开启。


### Windows 新修复同步与 Mac 合并检查（2026-10-24）

- 已拉取 `e9d8db38`（文件选择器）及 `4cba7ec0`（忽略 Playwright 本地产物），在其上恢复四项 Mac 验收代码改动；仅状态文档存在冲突，双方修复记录与未验收边界均保留。
- 合并后两份 TypeScript 检查通过；host 全部用例 808 + provider 流程 69 + 构建就绪 3，共 57 文件 / 880 项通过。原 Mac 验收的 Rust/共享服务检查未机械重跑，也不将其旧制品当成包含新选择器修复。
- 当前源码构建成功；隔离 VS Code 的 `routes smart picker selections into attachments and context chips` 为 1 passing。两组 PNG/ARIA/console 已读取：原生 QuickPick 的文件/文件夹入口可见，图片及两种引用回填正确，捕获范围内没有运行时 error；VS Code feature/sandbox 警告及测试模型不支持图片的提示保留。
- 本次使用既有 fake-Serve 与系统文件对话框替身，只证明真实扩展菜单交互及回填，不代签 Windows 原生对话框、真实模型或拖拽验收；未安装新插件或重启正式服务。详细证据：`.agents/acceptance/macos/merge-windows-4cba7ec0/report.md`。

### BLOCKED（待验收与风险）

| 项目 | 当前边界 | 下一步 |
| :--- | :--- | :--- |
| 请求级重试与自动恢复交付 | 代码与 macOS 本地受控服务验收完成；未安装到在用环境，未验证 Windows 实机或真实上游可用性 | 安装、共享服务重启及发布须另行授权；源码提交不代表在用版本已更新 |
| 多根引用 Windows 验收 | 开发与 macOS 验证完成；Windows 真实盘符 Host/工具、同事安装版本未核验，UNC 仅有字符串保持用例 | 用户明确保留 Windows 验收并暂停等待产物；收到 6 条 Host、3 条 Rust 及 PNG/ARIA/console 后再关闭计划，UNC 实际共享不作为额外必过项 |
| macOS 正式体验 | 安装、Dock、中文/空格项目连接、当前会话工具读写及语言实际操作已有证据；没有另发新窗口模型任务或全面实测 Mac 系统代理 | 保持上述证据边界，不重复要求已完成的小步骤 |
| Mac 已安装版本 | 纯插件 0.1.97、独立 CLI/serve 0.1.81、process-path 已核对；这是 8a5b7885 基线制品，不含后来选择器修复 | 保留被软链引用的 release 文件；源码合并不等于已安装插件更新 |
| 选择器修复制品 | Windows 已生成含选择器修复的纯扩展 0.1.97，但不含 CLI、未自动安装；同版本号不证明包内容相同 | 安装修复包后核对实际版本、路径与内容；本次合并不自动安装或重启 |
| Windows 原生文件选择 | 已修复互斥参数，宿主菜单与回填通过；系统文件对话框的实际选取仍未验收 | 安装本次纯插件包后验证普通文件可见并可添加，文件夹入口仍可用 |
| 文件树拖拽无提示 | 根因未确认；CDP 鼠标探针未产生源头 dragstart，不能视为有效复现或成功验收；拖拽代码未改 | 在用户故障窗口捕获真实 dragstart、Shift 状态及 iframe dragover/drop，再按事件断点定位 |
| Mac 正式保活（已补证） | 用户正式重启 A→B，Reload、全退出后重开前、最终用户 Dock 重开均仍 B；脚本与用户操作分开记账 | 不再要求重复操作；保留自动点击失败原件与组合回执 |
| CI 与外部宿主 | 本机整改验收不代替本次提交的 GitHub Windows 系统代理 CI；本轮外部宿主为源码契约对照，不是完整宿主重验 | 推送后检查 CI；保留 Multica 上层 30 秒交接与 backend 65 秒清理预算不一致的未实测风险 |
| 提交范围 | 本次本地提交请求级重试、五分钟自动恢复及 F1～F6 整改的全部相关代码、测试、文档和状态；不重跑测试，验收产物保持本地忽略 | 不推送、不打标签、不发布；不安装或重启正式服务；既有 Windows 待验收事项保留 |
