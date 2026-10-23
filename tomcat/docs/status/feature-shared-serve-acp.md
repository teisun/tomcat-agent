| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat Agent | 2026-10-23 22:43 +0800 | ACTIVE | feature/shared-serve-acp | - |

### DONE（已完成）

- [✓] 修复 Windows/Linux Composer `+` 同时启用文件/文件夹选择导致只显示目录的问题：原生 QuickPick 先选类型，保留多选、分次追加及取消/异常时的草稿和操作收尾；补充英中文案、参数/回填回归与真实宿主菜单交互用例。
- [✓] 已生成包含本次修复的纯插件包 `tomcat-vscode-ext-0.1.97-pure-composer-fix.vsix`（不含 CLI），构建、可解压性及包内代码与当前构建一致性检查通过；未自动安装或重启用户服务。
- [✓] Windows shared serve／Native／ACP 修复与本机验收计划完成 12/12：修复中文 PATH 定位、共享服务子进程弹窗、按连接解析界面语言及回环模型请求误走代理；集成测试在用例前统一构建当前 CLI。
- [✓] Windows 开发更新与重启整改计划完成 6/6：新增 `tomcat self-install`、仓库根 `scripts/dev-install.cmd` 和共享项目规则；主输入框 `/restart` 复用既有宿主重启链，stop／restart 外层期限改为 80／150 秒。
- [✓] 本次一并纳入原已暂存的 HOME 修复：Windows `~\` 路径按控制连接的 HOME 展开；cwd 不因此获得额外权限，缺失 HOME 不回退到服务自身环境。
- [✓] 项目版本脚本已将 CLI 升至 `0.1.81`、扩展升至 `0.1.97`，bundled CLI 同步为 `0.1.81`；当时版本一致性检查及版本脚本 19 项测试通过。
- 更新双语 README、用户指南与共享服务架构文档，明确“更新磁盘文件”和“显式重启后台”是两步操作。

### INTERFACE（接口与行为边界）

- Composer `pickContext` 先显示“添加文件 / 添加文件夹”，`buildAttachmentOpenDialogOptions(kind)` 只开启对应类型；两类需分次选择，继续复用原有附件/引用分类与 `composerWorkResult`。未改拖拽事件处理、只读判定或共享服务。
- `tomcat self-install` 只安装当前可执行文件，复用既有平台安装函数；Windows 复制、macOS 软链接策略不变，不初始化配置、不改 PATH／凭据、不自动重启。保留 `TOMCAT_AGENT_ACTIVE` 嵌套保护。
- `scripts/dev-install.cmd` 仅转交 `cargo run --release --bin tomcat -- self-install`，沿用 `CARGO_TARGET_DIR`；显式 `tomcat.path` 和 VSIX 内置 CLI 选择不受影响。
- 主输入框精确纯文本 `/restart` 先清空并同步草稿，再调用 `restartSharedService`；停止所有共享任务，不调用模型、不入队。参数、引用、附件和同名资源卡沿用普通消息路由；输入禁用条件不变。
- stop／restart 等待上限为 80／150 秒，version／status／start 仍为 35 秒；关窗与 Reload 仍只断开窗口，不停止后台。
- LLM 仅在无显式代理、主地址与可选 fallback 都为回环时绕过自动代理；远端与显式代理语义保留。窗口语言按连接环境解析，服务生成的文本仍使用进程级语言，不承诺多窗口独立重译。
- 未新增 wire 字段或安装／服务生命周期协议。

### 验证记录（此前结果，本次不重跑）

- 按用户本次要求，不运行测试、构建、安装或视觉验收；提交准备仅审查 diff、文件范围、版本声明和 Git 空白检查。Cov% 未测量，不将用例通过率当作代码覆盖率。
- 选择器修复此前验证：`npm run lint` 与相关 22 个测试文件的 470 项用例通过；随后构建/编译及纯 VSIX 内容检查通过。本次提交不重跑。
- 隔离 Windows VS Code 1.143 + 真实 CLI 的 `assertWebviewPickContextFlow` 已验证真实点击 `+`、原生 QuickPick 和附件/引用回填；PNG/ARIA/console 已检查，无 Tomcat Webview 控制台错误。系统文件对话框仍由替身返回 URI 并校验互斥参数，不标作原生选择文件验收通过；测试模型图像能力警告与加载中的缩略图不作为图片预览验收。
- 本地诊断及制品证据：`.agents/acceptance/composer-drag-probe/verification.md`、`gui-ru4M8V/` 和扩展目录下纯 VSIX，均不纳入 Git。原安装版 VS Code 更新锁、Windows fake-Serve 夹具 `spawn EFTYPE` 阻塞保留记录，后改用隔离副本和既有真实 CLI 夹具完成菜单/回填验证。
- 第一项计划此前记录：Windows Rust 影响范围回归 717/717；Native／ACP 冷构建与三次预热每轮 8/8；真实 VS Code 旁观与关窗旅程各 1/1，PNG／ARIA／console 已读回。
- 第二项计划此前记录：薄脚本 2/2；Rust 首批 184/187，修正三处测试缺陷后相关测试及帮助目录检查 7/7；host 807、GUI 877 项经分批检查与复验覆盖。原 gate 失败及 GUI 超时保留，不声称整条 gate 一次全绿。
- 最终真实 VS Code 重启旅程及外层检查退出 0：实例 A 切换到 B，非标题模型请求保持 2 次，关窗后禁止自动拉起的 Native IPC 仍连接 B；四组最终 PNG／ARIA／console 已读回。
- 详细本地证据：`.agents/acceptance/windows-shared-serve-report.md`、`.agents/acceptance/update-restart/report.md`；最终重启截图目录 `.agents/acceptance/update-restart/ui/gui-u7Ujzh/`。这些日志、截图和构建产物不纳入 Git，Mac 拉取代码不会自动获得本机证据文件。

### BLOCKED（待验收与风险）

| 项目 | 当前边界 | 下一步 |
| :--- | :--- | :--- |
| macOS 回归 | 本次两项整改尚未在 Mac 实跑；Windows 结果不能代签 | 拉取同一提交，验证 Native／ACP、PATH、语言、代理、软链接与真实 VS Code 重启／保活；保留 Mac 自己的日志和截图 |
| 新版本交付 | 已生成含选择器修复的纯扩展 0.1.97，但不含 CLI、未安装；不能据此推断用户正在运行的 CLI／扩展／后台版本 | 用户从 VSIX 安装后核对实际版本与路径；本次不自动安装或重启 |
| Windows 原生文件选择 | 已修复互斥参数，宿主菜单与回填通过；系统文件对话框的实际选取仍未验收 | 安装本次纯插件包后验证普通文件可见并可添加，文件夹入口仍可用 |
| 文件树拖拽无提示 | 根因未确认；CDP 鼠标探针未产生源头 dragstart，不能视为有效复现或成功验收；拖拽代码未改 | 在用户故障窗口捕获真实 dragstart、Shift 状态及 iframe dragover/drop，再按事件断点定位 |
| 用户手工验收 | 用户表示 Windows 可能已验收完成，但未提供逐项版本、路径和实例记录 | Mac 交接时补齐真实安装与后台身份确认；Agent 不替用户重启共享服务 |
| CI 与外部宿主 | 本机整改验收不代替本次提交的 GitHub Windows 系统代理 CI；本轮外部宿主为源码契约对照，不是完整宿主重验 | 推送后检查 CI；保留 Multica 上层 30 秒交接与 backend 65 秒清理预算不一致的未实测风险 |
| 提交范围 | 本次仅本地提交，不推送、不打标签、不合并或发布 | 用户确认后推送，再在 Mac 拉取验收 |
