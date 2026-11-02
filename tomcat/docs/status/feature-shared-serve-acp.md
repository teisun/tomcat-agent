| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat Agent | 2026-11-02 13:54 | PENDING_INTEGRATION | feature/shared-serve-acp | - |

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

### 合并审查与验收（本机离线验证完成，待本地合并）

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
- 文档已对齐：根与扩展双语 README 区分共享服务、ACP 与 legacy stdio；共享服务架构补 Native 的编号核对/环境/ready 顺序；技能文档补 Connection 高于基础库存的优先级；测试规范与脚本帮助改为实际 10 个 serial 目标，未改测试分组或并发模型。
- 两份宿主包静态核验通过（`1793587030689-y4z2d0`）：Paperclip 46 个路径/19 个新增文件，Multica 23 个路径/8 个新增文件；清单路径唯一、与 FILES 对应、最终文件和新增载荷/patch/许可证哈希一致、四份 patch 头目标集合准确。分别在干净的精确基线 `2f6ad2226aec330251438c47cbc45fd92e0d3132`、`06835b8f50606ca3f8bebbb5d07ac8901f831e69` 隔离副本执行四次 `git apply --check` 均退出 0；未应用补丁、未在宿主仓跑测试，不用旧包记录代签当前 Tomcat。Multica 30/65 秒取消预算差异保留为未实测风险。
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
- T9 红→绿：同一红批次复现 ready 后把所选旁观会话 B 换回磁盘 A；`bootstrap` 只让仍存在、未退役的宿主选择优先，删除与服务重连仍回原默认路径。实现取刷新列表前的 `state.activeSessionId`，而不是计划写的 `initialized.sessionId`：后者是初始化回执（provider.ts::ensureInitialized），切换页面更新的是 state（::switchSessionView / stateStore.setActiveSession）。`webview_provider_flow` 73 项通过；顺带把 switchSession 替身对齐产品对象回执。
- T10 安装态实测：`1793581140788-2ii5gr` 在隔离 HOME/profile 的真实 VS Code Production 扩展中通过（1 passed，33s）；自动探活确实销毁旧文档并重建，B、草稿、只读与编辑器焦点保留，恢复等待投影不变（旁观会话没有自动重试倒计时），真实 Bash 继续完成。Desktop 1440×900 / narrow 390×844 工作台，实际面板 1381×794 / 331×738；`.agents/acceptance/shared-serve-review/gui-Igyjaf/observer-{desktop,narrow}.{png,aria.txt,console.json}` 全部读回：无空白/重字/横向溢出，模式/模型/发送按钮为禁用；两份面板 console errors=[]（采集上下文范围，并非整段宿主日志）。
- 安装态夹具调试记录：`1793580507670-q1kgz0`、`1793580637484-6y5orr` 发现 ACP 新建 B 自身会更新磁盘默认值，改由原控制路径先恢复 A 再旁观 B；`1793580735922-2ulsn5` 发现销毁旧 iframe 时只读 CDP 探测会失联，限定重试已知连接/上下文消失；`1793580915356-ux1ri2` 的断言误把忙且有草稿的 Send 当 Stop，按现有 Composer 规则核验实际显示的唯一按钮必须禁用，未改 UI 交互。最后一次代码编译和安装态验证通过；这些失败不计产品回归。`1793580422849-y4r84f` launch-env 23 项、harness tsc 与构建通过。
- T14：真实服务夹具停服预算直接引用产品 `STOP_COMMAND_TIMEOUT_MS`，失败保留 HOME 的既有行为不变。CI push/PR 加上 `scripts/dev-install.*`，Windows extension job 接入根目录假 Cargo 的 CMD 测试及两个 tests/ 定向用例。`1793581416440-agmlv7` 本机 20 passed（fixture lifecycle 11 / proxy guard 6 / build selection 3），YAML 解析与根目录执行位置校验通过；`1793581683825-63z3cl` 的 CMD 两例在 macOS 全部 SKIP（0 passed），Windows 实机与 CI 实跑仍须推送前补证，不计为通过。未执行安装或代理修改。
- T8 红→绿：`1793580960772-y1mvlx` 在真实 ACP stdout 停读、Native 独立旁观确认约 1 MiB 已排入输出之后取消，后台 PID 超过原测试期限仍存活；`1793581081555-ak4ezu` 修后同例 3.655s 通过，恢复读取前 PID 已退出且文件不再增长，另一会话持续写入；读取后既有正文完整、Cancelled 终态唯一、其后没有多余帧。早一轮 `1793580713517-6ppqu7` 是串行脚本模型被另一会话的定时等待堵住，非有效缺陷复现；改用既有 pause barrier 后得到上述有效红结果。
- ACP 仅调整 prompt future 的等待：收到本轮取消就并行调用现有 finish_cancelled_turn；受理回执及 agent_idle 都由原 writer 排序，清理完成后才放终态并移除 pending；父 lifetime 取消交原断连清理。`1793581081555-ak4ezu` 的 ACP 库 7 passed、clippy -D warnings、CLI 构建通过，按 B1 跑 acp_stdio_e2e/acp_observer_e2e 共 27 passed / 0 skipped（125s），覆盖立即取消、后续回合、MCP、EOF、转发器崩溃与慢读完整性；未改帧格式或回放分页。
- T17 / IPC ENOENT：`1793582522201-mj3ly8` 先复现连接端点不存在被归为 executable_missing、立即终止重试（该批两项预期失败，另一项为 PATH 目录，分别处置）。`ServeConnectionSupervisor::classifyFailure` 改按系统错误 code=ENOENT 且 syscall=spawn 判断缺可执行文件，不再匹配任意错误文案；临时 IPC 失联沿原策略重试且 allowStart=false，不自动重启服务。`1793582637511-eo7rjd` 中 supervisor 18 项与扩展 TypeScript 检查通过，真实缺 CLI 仍一次失败、版本冲突分类保持。
- T17 / PATH 目录：同一红批次 `1793582522201-mj3ly8` 在真实临时 PATH 的首目录放同名文件夹、次目录放可执行文件，发现 resolver 选中了文件夹。`defaultFileExists` 在原权限检查之前要求 stat.isFile，保留 stat 跟随符号链接的既有安装方式；不更换发现优先级或增加定位进程。`1793582637511-eo7rjd` 的 resolver 13 项通过，含真实文件、POSIX 符号链接、目录跳过和 Windows 无执行位语义；Windows 平台运行仍另行补证。
- T15 / 测试隔离：`1793582133871-2iktgd` 通过仅向子测试进程传入 `TOMCAT__WORKSPACE__PROJECT_RESOURCE_DIR=tools/agents`，稳定复现 package-install 权限用例读取预期 `.agents` 文件失败（原批次 HOME/temp 归因保留为当时观察，不据此认定同一个具体环境变量）。干扰来源有源码证据：`infra/config/tests/load_test.rs` 在进程内修改同名环境。该权限用例改用已有 `load_config_toml_file` 读取自己的固定夹具，不再无关地加载全进程覆盖；未串行化全库、未增加锁。`1793582387881-2mj5wv` 在相同注入下 1 passed / 0 failed。
- T15 / 评审器等待：`1793581958528-mmm2sj` 完整等到单例通过（128.56s），不是已证明的死锁；十轮真实退避造成超 60 秒告警。仅对该测试启用 Tokio 虚拟时钟并新增恰好十次请求的断言，产品期限、重试额度、错误分类原样保留。`1793582387881-2mj5wv` 的 provider-binding 同模块六项共 3.62s 全部通过。原并发全库验证仍由 B2 的 T15 批次完成，不把定向结果冒充全门禁。
- T13：后台 Bash 授权后才解析 shell，解析失败复用 record_spawn_result 记已授权但启动失败。Windows 两条 Git 探测入口共用 5 秒期限；同步入口用短生命周期线程运行已有 Tokio/ProcessTree 能力，超时终止整棵树并等待子进程回收，连同后代占着 stdout 的情形一起覆盖；不改选择规则、控制者环境或增加 Agent 标记。
- T13 红→绿：`1793582387881-2mj5wv` 旧 output 路径直到 8 秒测试保险才退出，期限用例先失败；`1793582756127-qq2hg0` 两个探测用例通过，超时后父子 PID 均已消失、正常/非零退出结果不变。该平台批次为 62 passed / 1 failed：未修改的 local_socket 旧用例在 drop 后一次 connect 仍成功，根因未确认；`1793583022347-6wfzg7` 单独复跑通过，但不据此抹去整批失败，B2 继续核验。
- Bash 原语模块 `1793583022347-6wfzg7` 为 162 passed；随后的 Clippy 指出新增审计代码应使用 inspect_err，已按建议等价修改，`1793583275839-uefi1y` 的 `cargo clippy -p tomcat --lib -- -D warnings` 通过。Windows 专属用例以相对 PATH 的 git.cmd 验证拒绝时不探测、允许后照常执行、探测失败有审计；本机未编译/运行该 cfg(windows) 用例，必须在推送前补齐，不代签授权顺序的 Windows 实测。
- T17 / 新一轮复现：`1793584488404-emb4oz` 的三个库用例均先失败（0 passed / 3 failed）：ack 写失败后 busy 未释放；同一 scope 第二上下文插件 hostcall 返回 `SessionManager not configured`；第二会话执行旧式动态发现时，`INVENTORY_FROM_SECOND_SESSION` 被写入第一会话。前两项按局部修复处理，尚待绿结果；同批 CLI 管理命令测试 1 passed，覆盖 `serve status` 与 `session list`，无效 log.level 已由配置加载器拒绝且未创建运行目录，退出成功的线索未复现，未改 CLI 产品代码。
- T17 / 动态发现归属：对照 develop，`context.rs` 已有 scope 缓存、pin_session、activate_session_plugins 的 bare load_plugin，dispatcher 已有 bare ID 回退首个 SessionManager 的路径；属于旧机制，不因本次复现擅自改变插件初始化/懒加载契约。失败断言原样保存到本地 `.agents/acceptance/shared-serve-review/legacy-dynamic-discovery.repro.patch`，未反转断言或加 ignore 来冒充修复；此诊断不纳入默认回归，问题仍未解决，见 BLOCKED。默认门禁通过也不能代签此问题。
- T17 / 扩展补证：`1793584566135-ymcwib` 的控制往返与隐藏面板恢复两个文件共 81 passed，扩展 TypeScript 检查通过。隐藏期间收到同版本构建冲突，页面挂载和 ready 后仍保留 version_mismatch，未复现普通 Retry 覆盖。空窗口的全局初始化保留 sessionId=null、不猜服务 cwd；不能因此宣布聊天已建 scope，更不能简单让 initialize 拒绝而连带禁用全局设置，界面入口仍继续核对。
- T17 / 空窗口：原断言先失败（`1793585791172-21ker4`），实际 detail 是内部码 `scope_requires_cwd`。provider 只把这个码映射成已本地化的 `bridge.workspaceRequired`，不猜 cwd、不创建会话、不改初始化或设置流程。`1793586042918-2uhx22` 的 provider / recovery / messenger 三文件 175 passed，`npm run lint`（扩展和 GUI）退出 0。
- T3 / 夹具语言串扰：serve 组合批次出现五条英文断言失败；`1793586217650-qnyd85` 用 Native 握手与 foreign-checkpoint 两条测试稳定复现 1 passed / 1 failed。握手用例原来把全进程 locale 留在中文；改为父环境中文、连接环境英文，既证明连接环境覆盖有效，又不污染英文断言。只为它的环境修改使用既有 env_lock，不串行化全库、不改产品语言机制。
- 本机验证前提记录：磁盘剩余 2.8 GiB，先经 `cargo clean -p tomcat --profile dev --dry-run` 预览，再仅清理 debug 包产物，释放后剩余 29 GiB；PATH 的在用 CLI 实际指向 `target/macos-acceptance-8a5b7885f212f4549043a9b26502e6a6e89f5b20/release/tomcat`，未触碰。随后 `1793586354655-a45eup` 在 rustc 编译 lib test 时 SIGSEGV（无 Rust 源码诊断），测试及后续 clippy 未运行，不计通过；正在以单编译任务重试，测试并发不变。
- 格式门禁 `1793586746321-u3lgcz` 指出分支原有 10 个 Rust 文件未按 rustfmt 排版；只修换行与尾逗号，不改逻辑。第一次手工排版复检 `1793587030632-azmmwb` 还差一个 assert_eq 换行，已修；该次因 fmt 失败没有启动编译或测试。版本镜像复检 `1793586880023-meeigw` 通过：CLI 0.1.81、扩展 0.1.97、bundled CLI 0.1.81，未改版本。
- 空窗口视觉补验：使用现有 `gui` 的 `npm run dev` 与已配置 Playwright，真实 React 页面接收与 provider 用例一致的 degraded 状态，正文取英文源 catalog；桌面 1440×900、窄屏 390×844 的 `.agents/shots/shared-serve-review/empty-workspace-{desktop,narrow}.{png,aria.txt,console.json}` 均已读回。提示完整可见、窄屏自然换行、无横向溢出；设置按钮可用，聊天/发送禁用，采集范围内 errors=[]。这是页面呈现补验，不冒充 VS Code 空窗口端到端；T10 的真实安装态另行验收。验后已停止自建 Vite，未操作正式服务。
- T17 / busy 局部修复已转绿：回合任务尚未接管前，rearm/切活动会话/ack 任一准备失败，都取消本轮 token、关闭 steering 并 mark_idle；不启动模型或工具。`1793587162521-afsa6t` 的原失败用例通过，断言没有任务、busy 释放、队列关闭且后来控制者能重新进入；同批 serve/资源库存/platform 为 325 passed / 0 failed / 1 ignored（既有手动项，不计通过），264.34s。编译器 SIGSEGV 后单编译任务重试成功，原崩溃根因未确认；测试仍按原并发运行。Clippy 结果另记。
- T17 / 共享插件绑定局部修复已转绿：SessionRuntime 持有构造时已交给 dispatcher 的同一个 `Arc<SessionManager>`，而不是只保存内部值的 clone 后释放原 Arc；因此弱引用与实际会话同寿命，不改 scope 共享或 VM 初始化。`1793587162521-afsa6t` 中第二会话 getCurrentSession 在构造后和第一会话关闭后均返回第二会话；停止中的真实 JS/fetch 续体也未晚写历史，后者未复现缺陷，仅保留回归。旧式 bare-ID 动态初始化仍按 BLOCKED 单独记录。
- T3 / 语言夹具绿结果：`1793587162521-afsa6t` 中 Native 环境覆盖与所有 serve 断言同批通过，原 foreign-checkpoint 中文串扰未再出现；既有 ignored 项是 `resource_inventory_test::sync_timing_matrix`，不是功能失败。随后 `cargo clippy -j 1 -p tomcat --lib -- -D warnings` 退出 0；编译 jobs=1 仅限制编译调度，没有限制测试并发。
- local_socket 旧失败的跟进：同一 325 项组合中 `local_socket_excludes_duplicate_listeners_and_recovers_stale_endpoint` 通过；原失败保留、根因仍未确认，不改产品或用串行化掩盖，B2 继续观察。
- T17 / 未复现项的保留用例：真实 `run_command_stream` FIFO 中，先排 NewSession 再回复待决审批，原审批仍完成且新会话成功（`1793587162521-afsa6t`）；隐藏面板的版本冲突在挂载/ready 后仍保留分类（`1793586042918-2uhx22`）；无效 log.level 下 `serve status` / `session list` 均非零退出、未创建运行目录（`1793584488404-emb4oz`，CLI 1 passed）。仅保留这些测试，不增加新审批通道、不改隐藏面板恢复或 CLI 管理语义；CLI 同文件附带 rustfmt 排版。
- 源分支实现与逐项提交已收口，开始 B2 正式门禁；仍未合并，不推送、不安装、不重启正式服务。Rust 与扩展验收共用测试 CLI，顺序执行以免编译覆盖正在被测试使用的二进制。

- 合并前全差异复核：相对 develop 共 394 个变更路径，预演树仍等于源树。原始 `git diff --check develop..HEAD` 非零仅来自四份原始 patch 的上下文前缀（合法空格/Tab，不能格式化破坏补丁）与原样内置 workflow 末尾空行；明确排除这五份原始资料后检查通过，未全局关闭空白检查、未改哈希或增加豁免配置。原始资料由前述哈希与 patch apply 核验守护。此前注释写回丢失的门禁脚本执行位已在 `761f6051` 恢复，最终相对原分支没有文件模式变化。
- B2 Rust 首轮（`1793588892574-uour5x`，S=`ce927ef8fdcd5f574a4ba5df4451436890fbde67`，tree=`623f1363a7f54a095bc7ad80e6149f0ed047e026`）exit 1，不能写成全门禁通过。lib 3454 passed / 5 既有 ignored（420.73s），doctest 编译通过但 0 用例，serial 89 passed / 3 skipped（267.667s），含完整 Native/ACP 边界；real-llm 因缺 IDATATLAS key 未运行。T15 两条和 local_socket 原失败在完整库内均通过，历史记录仍保留。
- B2 发现本轮遗漏：全目标 Clippy 拒绝新断言中多余的 `.as_deref()`；parallel 组在编译时发现 `context_management_tests` 的注入 sink 仍要 SessionManager，而调用方 clone 已变成 Arc，未执行该组。沿同一调用形状搜索又找到 `cli_tests` 的对应 sink；三处仅修测试接线/等价惯用法，不改产品或断言含义。修后补全目标 Clippy、该 Native 用例与尚未运行的 parallel 组；源码/依赖/执行环境未变的完整库其余项和 serial 结果继续有效，不机械再跑整轮。

- B2 补验第一步：三处测试修正后的工作树在 `1793591795013-au3690` 通过 `git diff --check`、`cargo fmt --check` 及 `cargo clippy --all-targets -- -D warnings`（exit 0）。这次检查包含测试目标，不是此前只检查产品库的 `--lib`；接下来补 Native 用例和 parallel 组，不预先记成通过。

- Native 断言的 Clippy 修复仅去掉 `Option<&str>` 上无效的 `.as_deref()`，仍精确断言最后一条 Normal 用户消息是 B 的新指令，历史前缀不变；不弱化 T6。

- B2 Rust 补验（`1793592276899-692cph`，S=`c0ab1f428e64ed27babd6f0b88b579a8b70b1061`，tree=`7042eaa72706b4ab445841780f2ef83e1ccbc8bb`）exit 0：Native 新控制者用例 1 passed，parallel 384 passed / 27 skipped（142.131s）；两处注入 sink 回归都执行并通过。结合前轮仍有效的 lib/serial 与全目标 Clippy，完成本机离线 Rust 范围，真实模型仍未运行。
- B2 扩展首轮（`1793593147179-moayq4`，同一 S）exit 1：两份 tsc、host 916 / GUI 888、构建、integration 269 passed / 1 skipped、真实 VS Code 安装态 24 passing / 6 场景条件 pending 均通过；最后 verify:vsix 首场景 4 项超时，服务连接失败，未执行后续场景，不记完整扩展门禁通过。
- verify:vsix 失败已最小复现（`1793594179340-qvpzap`，1 passed / 1 failed）：full-gate 的同步子进程等待堵住同进程的 Native 测试服务，子进程 `serve status` 等不到回复。仅将这个启动器改为异步等待，沿用跨平台 spawnCommand、输出与失败退出约定；不改产品、夹具服务归属或增加期限。新增真实 IPC 及子进程失败传播回归，待绿结果和原门禁路径复验。原失败截图缺失记录与宿主日志保留在 `.agents/acceptance/shared-serve-review/b2-c0ab1f42/`。

- 启动器定向绿结果：`1793594285210-ekctw6` 的真实 IPC / 管理命令 / 非零退出与启动错误共 3 passed；`1793594406711-6pozlu` 的定向 strict tsc 退出 0。脚本不在 extension tsconfig 的 src 范围内，使用与 tsx 混合模块相容的 preserve/bundler、显式 Node 类型；先前两次推断命令分别因 TS6 的 ignoreConfig 要求及错误的模块/类型选项失败，未用改产品代码或放宽 strict 处理。启动器改变了整条门禁的等待方式，因此补跑原 `npm run gate:full` 入口，Rust 源码不变，不重跑 Rust 全门禁。

- B2 扩展完整复验（`1793594442583-2fzg8u`，S=`a4b61eda331c828a26db6bace511d37ff758476f`，tree=`9338e453b089dd59803eda55bb8561c0a6be97fd`）`npm run gate:full` exit 0：两份 tsc、host 916、GUI 888、构建、integration 271 passed / 1 skipped、安装态 24 passing / 6 条件场景 pending、verify:vsix 五场景合计 10 passing。没有放宽期限、跳过原失败场景或改变默认选例；原来超时的四项现为 4 passing。慢握手重启中的 bootstrap 断连日志、VS Code 提案/账号/弃用告警均保留，不称全程零告警。
- B3 最终安装态旁观复验（`1793595245110-u2j0ap`，同一 S）exit 0、1 passing / 26s：当前纯包 + 真实 CLI，自动探活重建页面后仍保留 B、草稿、只读与焦点，后台真实 Bash 完成；跨项目隔离、无重字、切回自有会话可写均通过。`.agents/acceptance/shared-serve-review/b3-a4b61eda/gui-7FxJcN/observer-{desktop,narrow}.{png,aria.txt,console.json}` 已逐项读回，实际面板为 1381×794 / 331×738：内容正常、长命令换行、底部操作不越界，两份捕获范围内 errors=[]。常规门禁的重建/场景替身不冒充此真实服务专项。
- 证据复用核对：`c0ab1f42..a4b61eda` 仅门禁启动器、其已有测试文件、此 status 三个路径；Rust 产品/测试/依赖/分组不变，前述 Rust 分批结果有效。空窗口提示的两组页面 PNG/ARIA/console 对应 GUI/provider 源码也未变，继续有效。随后仅回填此状态文档，不使已测行为失效；版本未升级，正式服务未安装或重启，源分支与 develop 均未推送。

- B4 首次待合并树与 `fe08633bfb180ae83b634112529bc7886a48b23d` 完全相同（tree=`4cd6c2d7f6e9b1db4228dad024084d35b0c6d358`）。`1793595769842-ex75gj` 的 Rust gate-fast exit 1：严格全目标 Clippy、doctest 编译、parallel 384/27 skipped、serial 89/3 skipped 通过；lib 3453 passed / 1 failed / 5 ignored，唯一失败为 `append_test::compaction_skips_a_busy_session_until_a_later_housekeeping_run`（期望 1，实际 0）。串联的扩展快门禁未执行。已核对该测试、压缩函数及配置锁函数与 develop 原基线逐字一致，不是本分支新回归；只撤销本任务尚未提交的合并，回源分支补测试，未改 develop 或他人工作。
- 压缩夹具红→绿：`1793597540910-ed5llm` 保留一个 `File::try_clone` 重复句柄，确定性复现仅 drop 原句柄后仍未解锁（0 passed / 1 failed）。并行 fork 可以产生这种同一文件描述的继承窗口（`infra/platform/process.rs::spawn_service_process` 使用 pre_exec），但原批次未记录具体持有者，不能据此指认某一进程。仅在原测试中显式 `fs2::FileExt::unlock`，重复句柄保留至压缩断言之后；占用时为 0、释放后为 1 的原断言不变。`1793597923702-qgd17f`：fmt、append 模块 24 passed / 3.39s、`cargo clippy -j 1 -p tomcat --lib --tests -- -D warnings` 均 exit 0；未串行化、加等待或修改产品锁/压缩机制。
- 本次补修只涉及上述测试及此状态文档；B4 的其余 3453 库用例、完整集成及此前扩展/UI 证据仍有效，按影响范围复用，不把原 Rust 命令 exit 1 写成 exit 0。最终源码 SHA 与提交树记入 develop status。
- B4 扩展补验：`1793598458874-yr40gj` 在与 `9d48409e` 相同的待合并树 `43ec806ed95ccc8367b671da113072c5c8045972` 上执行 gate:fast；extension tsc 已通过，但 npm GUI lint 进程 SIGSEGV / exit 139，尚未运行单测。检查 Node 23.11.0/npm 10.9.2、磁盘剩余 8.5 GiB 和 swap 使用 7883.5 MiB，未找到源码诊断，崩溃根因未确认；不把资源观察写成已证明的 OOM。源码、依赖和并发不变，`1793598602816-a62zbn` 只补 `npm --prefix gui run lint && npm run test:unit`，exit 0：GUI tsc、host 916 / GUI 888 全通过。两条 gate:fast 原始失败均保留，各有效步骤与定向补验合并记账，不称单次完整入口全绿。回源只订正此文档的进程函数引用并回填结果，不使行为证据失效。
- B2 视觉制品补读：`b2-async-gate/run-yY3gZu/gui-0AFbAi/` 五张原始全帧 PNG 与 `b2-async-gate/gui-eFrGHp/` 两张选择器 PNG，以及各自 ARIA/console 均已读回；进度/折叠/展开/切换/重载、QuickPick 与回填正常。自动 `*-cropped.png` 错裁到左侧 Explorer，不能当聊天视觉证据，保留为调试件并采用原始全帧图；未为此改变产品或重跑无关场景。`1793596077368-22hmlg` 核对七份捕获范围内 errors=[]，保留 VS Code sandbox/local-network-access 警告；不冒充全程监控或系统文件对话框实选。临时图片回看服务已停止。

### BLOCKED（待验收与风险）

| 项目 | 当前边界 | 下一步 |
| :--- | :--- | :--- |
| 旧式动态插件初始化串会话 | 本轮最小用例已失败，第二会话触发的动态发现会向第一会话追加消息；对照 develop 已有相同 bare-ID/default-session 机制，不算本次新增回归，但未修复 | 单独明确初始化应绑定触发会话还是禁止会话能力，再复用现有会话 VM 路径设计；保留失败日志与本地 repro patch。按获准计划，需设计的旧问题记录、不阻塞本地合并，不代签通过 |
| 本轮整改与合并 | 本机离线 Rust、完整扩展门禁及最终真实旁观专项已通过，源分支待本地合并 | 核对合并树与已测树，跑 B4、汇总 develop 状态并本地提交；只改说明文档可复用行为证据 |
| 本轮真实模型验收 | 默认必需 IDATATLAS_OPENAI_API_KEY 未配置，无法执行要求的 gate-full 真实 API 层 | 用户在本机私有环境准备凭据后复查；不收集/输出密钥，不以离线测试代签 |
| 本轮 Windows 专项与 CI | 未提供 Windows 执行环境；本地新增提交未推送，旧 SHA 的 CI 不代签 | 推送前必须补齐 Git 探测授权/回收、根 CMD 安装脚本及 Windows 用例证据；推送后另核对对应 SHA 的 CI，不绕过 runner 保护 |
| write-plans 完整库验收（本轮已补证） | B2 `1793588892574-uour5x` 并发完整库 3454 passed / 5 既有 ignored，两条原失败用例均通过；完整门禁仍因其它已列问题非零 | 保留原始失败和归因，不把库通过代签其它门禁或真实模型层 |
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
