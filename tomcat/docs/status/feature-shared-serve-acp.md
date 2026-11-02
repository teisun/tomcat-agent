| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat Agent | 2026-11-02 08:30 | IN_PROGRESS | feature/shared-serve-acp | - |

### DONE（已完成）

- [✓] 将 write-plans 三文件原样纳入正式内置资产并显式登记；复用现有物化、发现、同名覆盖、禁用及权限机制，不依赖试验目录或归档评测工具。
- [✓] planner 只保留 PLAN 身份和一句加载指引；常驻技能清单静态排除 write-plans，同轮开始执行后的下一请求不再推荐；迁走的提示词契约改由正式资产测试守护。
- [✓] 计划评审器的重复写作模板换成读者检查，保留 UX、设计挑战、反过度设计及独立评审职责；同步技能和 planner 架构文档。

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

- write-plans 仅通过 PLAN 临时提醒自动推荐，显式 `load_skill`、用户 `/skill` 清单和历史正文不变；不增加模式配置、加载状态、协议或评审器技能权限。包含此改动的新二进制在创建聊天上下文时物化内置技能；源码提交不代表当前运行环境已更新。

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

### write-plans 内置验证（2026-11-01，提交前已有结果）

- 聚焦库测试 104 项、离线集成 9 项通过；期间删掉模板开头空行导致 3 项模式切换回归，恢复原空行后仅重跑受影响的 6 项库测试和 3 项端到端测试，均通过，不重复累计。
- `cargo clippy --all-targets -- -D warnings`、受影响 Rust 格式及文档模板一致性检查通过；正式资产与原包逐字节一致，reviewer 仅替换原 11–26 行。doctest 命令成功但为 0 个用例，不作为行为证据。
- 其余库测试批次为 3333 passed / 1 failed / 5 ignored；失败的 `package_install_manifest_authorizes_consumed_tree` 在加载配置时遇到其它测试的临时 HOME/temp 路径无效，未改源码单独重跑通过，保留原批次失败记录。
- 既有 `prod_code_reviewer_keepalive_only_provider_surfaces_idle_timeout` 隔离运行超过 60 秒仍未完成，停止后记为未验证；完整库门禁不记全绿。详细任务退出码记录在本地 write-plans 内置计划的验收 evidence 中。
- 本次提交遵照用户要求不重跑测试、构建或覆盖率，Cov% 保持未测量；归档 bench、真实模型评测、release 构建、安装和服务重启均不执行。技能工作流附件的原文末尾空行保留，不为 Git 空白提示改写已验收资产。

### 合并审查与验收（修订获准，整改进行中）

| 审查组 | 规范 / 调用链与结论 | 测试落点 |
| --- | --- | --- |
| ① serve / IPC / 所有权 | 架构 §4/9：CLI 预检检查对端系统账号（`infra/platform/local_socket.rs::connect`），Node 第二条连接尚未保护发送环境；`service.rs:468` 仅比产品版本，本轮不升版本，不能挡住同版本旧构建。补握手识别是计划修订阻塞。CloseSession、晚注册 Bash 按已有计划复现；writer 失败的 busy 遗留另待复现。 | `sharedConnection.test.ts`、`serve/tests/{service,commands,connection,session_job}_test.rs`、`serve_shared_e2e.rs` |
| ② ACP / MCP / 规则 / 插件 | 架构 §1/4/9：ACP 复用 serve，取消清理受 writer 顺序影响按计划修；MCP 私有层及 AGENTS 规则权限已有对应回归。插件执行停止、共享 scope 第二上下文的 SessionManager 弱引用、动态初始化 bare ID 与会话绑定的关系需先构造复现，不直接引入新生命周期。 | `acp_stdio_e2e.rs`、`serve/tests/connection_test.rs`、`chat/tests/{context,resource_inventory}_test.rs` |
| ③ 重试 / 技能 / 提示 | 架构 §4.1/注释规范：`empty_turn` 吞错误是从 develop 原样迁移的旧问题，本轮补警告但保持尽力写入；请求预算、取消、禁用和同名覆盖按已有契约。技能文档缺 Connection 优先级说明待文档任务修正。 | `agent_loop/tests/run_basic_test.rs`、`request_retry_test.rs`；现有 skill/prompts 测试 |
| ④ 扩展 / GUI / 引用 | 架构 §4/单一状态源：页面重建和迟到开头按计划修；新增线索为空窗口未建立 scope、隐藏面板版本冲突仍给普通 Retry、IPC ENOENT 被归为缺 CLI；`webview_provider_flow.test.ts` 的 switchSession 字符串替身已与对象回执漂移，进入 UI 任务核对。多根路径与标签分离保持原实现。 | `webview_provider_flow.test.ts`、`dual_channel.test.ts`、`provider_error_recovery.test.ts`、`ServeConnectionSupervisor.test.ts`、真实安装态 harness |
| ⑤ Windows / 安装 / 代理 / CI | 架构 §5/12：`shell.rs::posix_shell_for` 由连接 PATH 定位 Git；`environment.rs:96-113` 将相对 PATH 拼到任务 cwd，`bash_task.rs:756-770` 先执行探测才授权。这是反驳计划“Git 不受项目控制”的新代码证据，修订后才能按最小边界调整。探测期限、夹具预算、CI 接线仍保留；代理快照不扩围。管理命令校验失败退出成功和 PATH 目录误判另待复现。 | `platform/tests/shell_test.rs`、`primitive/tests/suite_test.rs`、`cli_tests.rs`、`resolveTomcatExecutable.test.ts`、现有 fixture/CI 测试 |
| ⑥ 宿主包 / 版本 / 文档 | 架构 §6/文档对齐：包清单、路径与哈希只读核验对应，等待精确基线 apply 检查；CLI/扩展/bundled 镜像一致。Multica 上层 30 秒与适配器 65 秒边界仍是明确未实测风险，不把本次 Tomcat 侧通过等同宿主通过。 | 包内 `VALIDATION.md` 检查、隔离基线 `git apply --check`、`release-version.mjs check` |

- 基线：源 `970b6bb53bfdcf718341aee7ce2e0843933a33c7`，fetch 后 develop 由 `383a7b97b19f0471048c10a30cab68255503bcf0` 快进到 `86081c3121a183ffd1b65997998f88f7fd26f7e0`；merge-base 为前者。预演合并树与源树均为 `61b5b670fe0eb336d2bebe5713bcf009b7b49265`，无需回合并。开始时工作区干净，develop 无其它 worktree 占用；不清理已有工作树。
- 环境：Darwin x86_64，rustc 1.96.0、nextest 0.9.140、Node 23.11.0；扩展依赖与隔离 VS Code harness 存在，初始可用磁盘 16GiB。默认 real-llm 必需的 `IDATATLAS_OPENAI_API_KEY` 未配置，仅核对存在性；Windows 环境未确认。真实模型与平台验收不能记为通过。
- 审查覆盖：按 `383a7b97..970b6bb5` 的 385 个变更路径分六组检查 serve/IPC/所有权、ACP/MCP/插件、重试/技能/提示、扩展/GUI、多平台/CI、宿主包/版本/文档；以架构分层、错误传播、测试布局、Rust 惯用法和注释规范为依据，长文件不机械拆分。
- 已知修复仍按计划推进。额外审查线索须由主 Agent 复核或用例复现后归因：开始回合前 writer 失败导致 busy 遗留、审批回复被入队命令阻塞、插件停止与第二会话的弱引用存活、空窗口初始化及 IPC ENOENT 分类、Git 探测早于授权、CLI 管理命令失败返回成功、可执行路径误选目录。未复现项不冒充运行时结论，不因报告建议直接扩大设计。
- 握手兼容需特别核对：精确包版本不能区分同版本号的不同构建；fake-Serve 安装态夹具也是 Native 握手消费者。保持现有直连，不恢复已撤销的 relay。
- 技能文档仍称 Project 最高优先级，但实现有 Connection 覆盖；在文档任务中修正。服务端文本采用进程级语言是既有边界，不扩成逐连接翻译。关窗清理失败通知、一次性 CI 代理原值修复不因审查报告自动恢复为新需求。
- 两份宿主包只读审查的清单、27 个新增文件、69 个最终文件和四份 patch 哈希对应；精确基线隔离副本已找到，patch apply 检查待执行。Multica 宿主 30 秒与适配器 65 秒取消预算差异沿用既有风险记录，不能把 backend 单测当成宿主工作目录安全收尾证明。
- 本轮审查小修：先补 Story 8c/8d 与 E2E-CLI-103～105、E2E-VSCEXT-036～037 的契约（待后续实现验收）；更正 QueuedCommand 注释；empty_turn 写盘失败增加含回合/尝试/分类的 warn，不打印正文、不改变重试额度或结果。
- T16 红→绿：任务 `1793572679889-arqsiq`（日志 `~/.tomcat/agents/main/tool-results/bash-1793572679889-arqsiq.log`）先得到 0 passed / 1 failed，失败原因是两次空答诊断写失败没有 warn。修后任务 `1793572948891-ydxnc6`（同目录对应 `.log`）执行 `cargo test -p tomcat --lib empty_turn -- --nocapture`：6 passed / 0 failed / 0 ignored；校验正常与诊断失败都恰好请求三次、最终正文 recovered、不泄露思考文本/响应 ID/载荷哨兵。紧接 `cargo clippy -p tomcat --lib -- -D warnings` 退出 0。仅定向结果，未执行全库/全门禁。
- `node scripts/release-version.mjs check` 与本轮 `git diff --check` 通过；版本未修改。测试后只更新状态说明，未改变行为或夹具。
- 用户已批准修订后的执行计划：Native 改用握手形状识别同版本旧构建，保持管理协议版本 1；Git 探测移到授权之后。恢复实施，不恢复 relay 或额外 Agent 标记。
- session-cleanup 复现：`1793578183918-6esdvy` 四项库测试均先失败，分别暴露 CloseSession 超时仍成功、维护任务旧关闭路径直接 abort、断连追加消息留存、晚注册 Bash 漏停；真实进程用例 `1793578454531-aml9z0` 在收到 closed 后仍查到活动 shell。测试自建进程已回收，没有操作正式服务。
- 修复仍收敛到原停止入口：CloseSession 先 stop_slot_work；停止结束清内存追加队列（不动历史），回合退出后补一次 Bash 扫描。`1793578796044-u1fb2m` 的 serve 模块批次为 247 passed / 1 failed；失败是新增模型请求断言把 EphemeralTail 工作区提醒误认成最后一条用户消息，已按 Normal 用户消息类型修正。`1793579618198-eio5wr` 定向复验全部四项关闭/断连/晚注册用例通过；同批服务握手用例共 11 passed / 0 failed。`cargo clippy -p tomcat --lib -- -D warnings`、当前 CLI 构建均退出 0，`cargo nextest run --test serve_shared_e2e` 为 12 passed / 2 skipped（原有受控系统代理专项，未运行、不计通过），含关闭和 EOF 之后本会话 shell 已退出、文件不再增长、另一会话仍继续工作的真实进程断言。产品未修改维护任务、FIFO 或 steering 的既有停止规则。
- Native 扩展侧红→绿：`1793578883207-6o6hpp` 复现先发环境和同版本误导文案，3 failed / 1 passed；`1793578927379-eibfes` 新握手、失败分类与关闭回归 46 passed，两份 TypeScript 检查通过。`1793579245175-epovi3` 安装态假服务 transport/history/questions 4 passed；仅夹具协议验证，不代替真实 VS Code 视觉验收。Rust `1793579213705-y4249a` 先得到 4 failed / 3 passed；新代码在 `1793579618198-eio5wr` 转为 7 passed：第二阶段之前不建连接、非法/旧形状拒绝、原总握手期限、ACP/status/stop 格式不变。原 `service_test.rs` 移为 service 的私有子测试模块，未提高生产符号可见性。
- 安全前提复核：instanceId 只由 `service.rs::run_service` 用 UUID v4 生成；常驻服务 stdout 为 null，日志 0600。Windows `local_socket.rs::private_pipe` 使用 `private_acl::current_user_acl` 的当前用户单条 ACE 并拒绝远程客户端；Unix accept 检查 peer UID。CLI `open_management` 先经 `local_socket::connect` 核对服务端账号才读取编号。Node 首条 hello 不含编号或环境，编号不匹配时环境 0 字节发送的断言覆盖全部接收字节。此证据不等于双账号攻击实测，也不防同账号恶意进程。
- T11 红→绿：`1793579754418-6c7wi4` 复现 history 后迟到的 message_start 会把正文和 thinking 再追加一遍；`state.ts` 在开头复用 hasPersistedAssistant 跳过已落盘消息，不新增序号或完成表。`1793579798278-25d6ex` 的 dual_channel 17 项通过（同批三个文件共 163 项），新消息仍正常流式显示，断言在第二次 hydrate 之前检查；扩展与 GUI TypeScript 检查通过。
- T12 红→绿：同一红批次复现 restoreCheckpoint 等待期间旧计时器仍对 s1 发 Retry；只把该意图加入现有接管集合。绿批次 provider_error_recovery 73 项通过：恢复进行中与失败后旧等待都不重试，s2 的计时仍按时执行，原计时/持久化机制不变。
- 本轮尚未运行正式门禁、未完成合并；保持不推送、不安装、不重启正式服务。

### BLOCKED（待验收与风险）

| 项目 | 当前边界 | 下一步 |
| :--- | :--- | :--- |
| 本轮整改与合并 | 计划修订已获准，正在逐项复现、修复与定向验证，尚未执行最终全门禁或合并 | 按 session-cleanup、Native、ACP、界面与平台边界等任务推进；未完成项不记为通过 |
| 本轮真实模型验收 | 默认必需 IDATATLAS_OPENAI_API_KEY 未配置，无法执行要求的 gate-full 真实 API 层 | 用户在本机私有环境准备凭据后复查；不收集/输出密钥，不以离线测试代签 |
| write-plans 完整库验收 | 相关离线用例通过；全库批次有 1 项临时目录错误（单独重跑通过）、5 项忽略，另 1 项既有评审器测试挂起未验证 | 保留失败与未验证记录；后续单独处理测试隔离与挂起，不将本次提交标成全库全绿 |
| 请求级重试与自动恢复交付 | 代码与 macOS 本地受控服务验收完成；未安装到在用环境，未验证 Windows 实机或真实上游可用性 | 安装、共享服务重启及发布须另行授权；源码提交不代表在用版本已更新 |
| 多根引用 Windows 验收 | 开发与 macOS 验证完成；Windows 真实盘符 Host/工具、同事安装版本未核验，UNC 仅有字符串保持用例 | 用户明确保留 Windows 验收并暂停等待产物；收到 6 条 Host、3 条 Rust 及 PNG/ARIA/console 后再关闭计划，UNC 实际共享不作为额外必过项 |
| macOS 正式体验 | 安装、Dock、中文/空格项目连接、当前会话工具读写及语言实际操作已有证据；没有另发新窗口模型任务或全面实测 Mac 系统代理 | 保持上述证据边界，不重复要求已完成的小步骤 |
| Mac 已安装版本 | 纯插件 0.1.97、独立 CLI/serve 0.1.81、process-path 已核对；这是 8a5b7885 基线制品，不含后来选择器修复 | 保留被软链引用的 release 文件；源码合并不等于已安装插件更新 |
| 选择器修复制品 | Windows 已生成含选择器修复的纯扩展 0.1.97，但不含 CLI、未自动安装；同版本号不证明包内容相同 | 安装修复包后核对实际版本、路径与内容；本次合并不自动安装或重启 |
| Windows 原生文件选择 | 已修复互斥参数，宿主菜单与回填通过；系统文件对话框的实际选取仍未验收 | 安装本次纯插件包后验证普通文件可见并可添加，文件夹入口仍可用 |
| 文件树拖拽无提示 | 根因未确认；CDP 鼠标探针未产生源头 dragstart，不能视为有效复现或成功验收；拖拽代码未改 | 在用户故障窗口捕获真实 dragstart、Shift 状态及 iframe dragover/drop，再按事件断点定位 |
| Mac 正式保活（已补证） | 用户正式重启 A→B，Reload、全退出后重开前、最终用户 Dock 重开均仍 B；脚本与用户操作分开记账 | 不再要求重复操作；保留自动点击失败原件与组合回执 |
| CI 与外部宿主 | 本机整改验收不代替本次提交的 GitHub Windows 系统代理 CI；本轮外部宿主为源码契约对照，不是完整宿主重验 | 推送后检查 CI；保留 Multica 上层 30 秒交接与 backend 65 秒清理预算不一致的未实测风险 |
| 提交范围 | 本次本地提交 write-plans 默认内置、PLAN 短入口、常驻清单过滤及评审职责分离的全部相关代码、测试、文档和状态；不重跑测试，试验包与归档评测产物不纳入 | 不推送、不打标签、不发布；不安装或重启正式服务；既有 Windows 待验收事项保留 |
