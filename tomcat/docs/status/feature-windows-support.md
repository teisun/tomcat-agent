| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| 汤姆猫 | 2026-10-18 | IN PROGRESS | develop | — |

### CURRENT STATUS (2026-10-18)

```text
Windows 平台适配（win32-x64：VS Code 插件 + 终端 CLI）
  ├─ CLI 产品代码 ........ 完成（§1-§8）
  ├─ 插件宿主修复 ........ 完成（§9）
  ├─ 测试基线修复 ........ 完成：本机 Rust gate-fast、插件 lint 与集成测试全绿（§10）
  ├─ CI / 发布 ........... 两个 dry-run 成功；windows-ci 全绿（`bb8c618f`，run 39097819025）（§11、§12）
  ├─ 验收 ................ 场景 1-12、14 通过或由自动化覆盖；场景 13 待正式发布（§13）
  └─ 验收与 CI 中发现的问题 3 + 3 个已修复，见下方两节
```

- **已完成**：Git Bash 作为 bash 工具；路径身份（dunce、大小写、`~\`）；权限规则与危险命令；安装包安全快照；外部程序查找（PATHEXT、npx.cmd、MCP 环境白名单）；CRLF 计划文件、`.env`/OAuth 私有权限、shadow-git 长路径；可取消的控制台/管道输入与 ConPTY 端到端测试；`tomcat init` 写用户 PATH 与副本刷新；控制台 VT 颜色；插件 `windowsHide` 与 `@` 目录 Uri。
- **验证**（本机，版本已升到 CLI 0.1.78 / EXT 0.1.94）：Git Bash 下 `./scripts/run-integration-tests.sh gate-fast` 全绿（`26af661d`）——严格 Clippy、lib 3306 项（5 ignored）、doctest、integration 并行组 365 项、串行组 49 项；之后的私有权限修复另跑 lib 全量 3307 项通过与严格 Clippy。GitHub windows-ci 在 `bb8c618f` 上全绿；插件 `npm run lint` 通过，`npm run test:integration` 225 项通过（9 项为仅 Unix 的用例，按设计跳过）。基线失败的修复见下方"测试基线修复"。

### 基线（改动前，2026-10-17）

本机环境：Windows 11 家庭版 26100，用户目录 `C:\Users\汤姆猫`（含中文）；rustc 1.99.1（x86_64-pc-windows-msvc）、cargo-nextest 0.9.148、Node 22.23.3 / npm 10.9.9、Git for Windows 2.55（`C:\Program Files\Git`）、VS Build Tools；12 逻辑核、15.7 GB 内存。

**Rust lib**：`cargo nextest run --lib --no-fail-fast`，3272 个用例中 120 个失败、1 个卡死，按计划章节归类：

| 归类 | 失败数 | 典型报错 / 代表用例 |
| :--- | ---: | :--- |
| §2 bash 走了 cmd | 29 | `'i' is not recognized`、`taskkill … exit 128`；bash_task_test、primitive suite_test、task_output |
| §3 路径带 `\\?\` 前缀 / `~/` 拼反斜杠 | 13 | `git init failed … work_tree=\\?\C:\…`（cmd_restore 9 个）、`~/AppData\Local\…` |
| §4 权限规则 | 1 | spawn_denied_by_path_preflight（Windows 路径没被识别） |
| §5 包安装快照未实现 | 23 | `no safe no-follow installation snapshot`；package_manager、cmd_install、config_tool |
| §6 MCP stdio 假服务起不来 | 24 | `initialize: transport request failed`；mcp manager、code VM、connector |
| 计划外：Windows 文件语义 | 17 | `拒绝访问 (os error 5)`、`os error 33`、`session_in_use` 未触发；checkpoint/session/usage_guard |
| 测试自身写法 | 3 | JSON/TOML 里直接拼了反斜杠路径；load_test 在 `C:\Windows` 下建临时文件 |
| 跑法问题（非产品） | 5 + 1 卡死 | 本机开了系统代理 `127.0.0.1:12450`，直接跑 nextest 没带门禁脚本导出的 `NO_PROXY`；门禁本来就会设置 |
| 待查 | 5 | transcript_migration、serve_connector_commands、search_files 上下文行、project_instructions ×2 |

那个卡死用例：本地桩服务器只接一次请求，请求被系统代理截走后测试永远等不到连接。

- **Rust clippy**：Windows 上 8 条（只在 Unix 用到的项触发 `unused_imports`/`dead_code`，`bash_task.rs` 一处多余借用），已修。
- **Rust 集成测试**：12 路并发链接耗尽内存（`memory allocation … failed`），后面一串"找不到 crate"都是连锁反应；本机统一用 4 路（`CARGO_BUILD_JOBS=4`）。
- **插件**：`npm run gate:fast` 的 lint 通过；core 单测 13 个失败（路径分隔符、`os.homedir()`、junction 等测试写法）已修复，现 779/779；GUI 860/860。

### 测试基线修复（§10）

基线里的失败分三类：真实的产品缺陷、测试夹具的平台假设、测试环境副作用。

```text
基线失败
  ├─ 产品缺陷（改产品代码）
  │    ├─ serve 子进程卡在 stdin ....... 修复：serve 独占协议 stdin，子进程继承 NUL
  │    ├─ 从 Git Bash 启动 Ctrl+C 失效 .. 修复：CLI 启动时恢复 Ctrl+C 输入
  │    ├─ 后台任务没结束，CLI 退不出 .... 修复：运行时关闭时终止任务整棵进程树
  │    ├─ 子进程握住父进程给的管道 ..... 修复：CLI 启动时去掉 std 句柄的继承标记
  │    ├─ 附件租约续期不更新时间 ....... 修复：续期显式 set_modified
  │    └─ 锁冲突错误码不同 ............. 修复：platform::is_lock_contended（见下方偏差）
  ├─ 夹具假设（只改测试）
  │    └─ /tmp、sh、反斜杠路径、JSON 转义、/private 前缀、HOME 不一致 …
  └─ 环境副作用（改门禁脚本）
       └─ chat 启动时自动 winget 安装 rg/fd ... 门禁设 PI_SKIP_SEARCH_TOOLS_PREFLIGHT=1
```

#### serve 的子进程卡在 stdin（产品缺陷）

- **问题现象**：`serve_rewind_and_resend` 3 项在 Windows 上稳定失败，回退请求返回 `stop_timeout`；serve 日志写着 `checkpoint writes did not finish before timeout`（3 秒）。
- **根因与证据**：
  ```text
  插件 ──协议消息──▶ serve stdin（同步管道）
                         │  读线程一直阻塞在 ReadFile 上，等下一条消息
                         ▼
             git / rg / fd … 用 inherit 继承了同一个管道
                         │  子进程启动时查询这个句柄 → 排在那次 ReadFile 后面
                         ▼
             卡住，直到插件发来下一条消息
  ```
  打个比方：只有一个窗口的柜台，serve 的读线程站在窗口前一直等人来；子进程启动时也要到这个窗口问一句话，只能排在它后面。
  - `GIT_TRACE_PERFORMANCE` 显示 `git config core.worktree` 自身只用了 1.5ms，但 serve 一直等不到这个 git 进程退出；影子仓初始化停在第 2 条 git 命令上。
  - 只给影子仓的 git 加 `stdin(Stdio::null())` 后测试立即通过（3.5s），证实是 stdin 继承。
  - `src` 里 14 处起子进程，12 处继承 stdin：checkpoint、会话 scope、代码审查的 git 调用，`search_files` 的 rg/fd，OAuth 打开浏览器，icacls。在插件里，这些都可能卡到用户下一次操作。
- **解决方案**：在源头保证"serve 的协议输入不交给任何子进程"，而不是逐个调用点加 `stdin(null)`。
  - serve 启动时取走原 stdin 句柄自己读，把进程 stdin 换成 `NUL` <sub>`infra/platform/stdin.rs::take_process_stdin`、`api/serve/stdin.rs::run_stdio_loop`；仅 Windows，Unix 仍用 `tokio::io::stdin()`</sub>
  - 之后所有 `Stdio::inherit()` 子进程（现有的和以后新增的）拿到的都是 NUL，也不会再抢读协议字节。
- **测试用例清单**：`tests/serve_rewind_and_resend.rs` 的 3 项即回归用例（修复前 3/3 失败，修复后通过，回退往返约 1.7s）。

#### 从 Git Bash 启动时 Ctrl+C 失效（产品缺陷）

- **问题现象**：`windows_conpty_cli_tests` 在门禁里失败（提问时按 Ctrl+C，2 秒内没有回到主输入），从 PowerShell 单跑却 3/3 通过。
- **根因与证据**：门禁经 Git Bash 运行。Windows 的"忽略 Ctrl+C"属性会被子进程继承（父进程以新进程组启动时就带上它），于是 tomcat 收不到 Ctrl+C。同一条用例在 Git Bash 下单独跑也 0/2 失败，排除了机器负载。用户从 Git Bash 运行 `tomcat chat` 会遇到同样的问题：Ctrl+C 既打断不了生成，也打断不了提问。
- **解决方案**：CLI 启动时（与开启 VT 颜色同一处）调用 `SetConsoleCtrlHandler(NULL, FALSE)` 恢复 Ctrl+C <sub>`infra/platform/mod.rs::restore_ctrl_c_input`、`api/cli/mod.rs::run_cli`；仅 Windows</sub>。
- **测试用例清单**：`tests/windows_conpty_cli_tests.rs` 即回归用例；修复后在 Git Bash 下 2/2 通过。

#### 后台任务没结束时 CLI 退不出、管道不关（产品缺陷，插件集成测试发现）

- **问题现象**：插件集成测试 `cli_autofeed`（`EOF=false`）失败：stdin 关闭时后台 bash 还在跑，`tomcat code` 打印 `Goodbye!` 后迟迟不退，父进程也一直收不到管道关闭（`close` 事件）。脚本里 `tomcat code | tee`、插件关闭 Serve 都会这样等下去。
- **根因与证据**：其实是两个独立的问题，叠在一起：
  ```text
  父进程（插件/脚本）──管道──▶ tomcat.exe ──▶ bin\bash.exe（Git 启动器）──▶ usr\bin\bash.exe（真正干活）
                                │                     ▲ kill_on_drop 只杀到这里
                                │
     问题 1：运行时关闭时要等所有"阻塞读"结束；Windows 上 tokio 读子进程输出用的是阻塞读，
             真正的 bash 还活着 → 输出管道不关 → 读不结束 → tomcat 永远退不出
     问题 2：tomcat 从父进程继承来的 std 句柄本身"可继承"，每个子进程都会顺手带走一份；
             比 tomcat 活得久的进程（如 `npm run dev &`）一直握着父进程的管道 → 父进程等不到 EOF
  ```
  - 证据 1：卡住时进程列表里 `tomcat.exe code` 仍在，`usr\bin\bash.exe` 的父进程（启动器）已不存在；去掉修复 1 后新用例报"后台任务拖住 CLI 退出"。
  - 证据 2：杀掉遗留的 bash 后，整条 PowerShell → nextest → 测试 → tomcat 链才一起返回；去掉修复 2 后新用例报"脱离出去的进程握着 stdout"。
  - Unix 不受影响：子进程不会保留父进程的 0/1/2，子进程管道也不占阻塞线程。
- **解决方案**：
  - 修复 1：后台任务的 wait 协程没跑完就被丢弃（只在运行时关闭时发生）时，终止该任务的 Job（整棵进程树）；正常结束时不动，用户用 `&` 脱离出去的进程照常保留 <sub>`core/tools/primitive/bash_task.rs::TerminateTreeIfAbandoned`；仅 Windows</sub>。
  - 修复 2：CLI 启动时去掉本进程 stdin/stdout/stderr 的继承标记，相当于 Unix 的 CLOEXEC；`Stdio::inherit()` 创建子进程时会另外复制一份可继承句柄，不受影响（已核对 Rust 1.99.1 标准库 `sys/process/windows.rs`） <sub>`infra/platform/mod.rs::stop_std_handle_inheritance`、`api/cli/mod.rs::run_cli`；仅 Windows，覆盖 serve 在内的全部子命令</sub>。
- **测试用例清单**：`tests/cli_tests.rs::bash_processes_do_not_keep_a_piped_cli_or_its_pipes_alive`（跨平台）：脚本模型先让 bash 用 `&` 脱离出一个循环，再起一个不会结束的后台任务，关闭 stdin 后要求 30 秒内退出、5 秒内读到 EOF，最后确认脱离出去的循环确实活过了 CLI。分别去掉两个修复各跑一次，都按预期报出对应的失败；两个修复都在时 3.5 秒通过。插件侧 `cli_autofeed` 即端到端回归。

#### 附件租约续期不生效（产品缺陷）

- **问题现象**：附件租约"续期"后仍按旧时间过期。
- **根因与证据**：续期原来靠 `fs::write(path, [])` 重写空文件来刷新修改时间；Windows 截断一个本来就空的文件时不更新修改时间。
- **解决方案**：续期时显式 `set_modified(now)` <sub>`core/session/attachments.rs`</sub>。
- **测试用例清单**：沿用已有的附件租约单测（`core/session/manager/tests/attachment_test.rs`）。

#### chat 启动时自动安装 rg/fd 拖住测试（环境副作用）

- **问题现象**：`checkpoint_cli_e2e` 等 CLI 测试挂到超时；进程列表里有 `cmd /C winget install BurntSushi.ripgrep.MSVC … && winget install sharkdp.fd`。
- **根因与证据**：`tomcat code` 启动前检查 rg/fd，缺失就自动安装（`api/chat/preflight.rs`）。Windows 上安装进程继承了 CLI 的 stdout 管道，CLI 退出后测试仍在等管道关闭；同时它真的在改本机软件。
- **解决方案**：门禁脚本统一 `export PI_SKIP_SEARCH_TOOLS_PREFLIGHT=1` <sub>`scripts/run-integration-tests.sh`；文档本来就建议 CI 设置此变量</sub>。产品行为不变。
- **测试用例清单**：无新增；`checkpoint_cli_e2e` 3 项恢复通过。

#### 测试夹具的平台假设（只改测试）

| 测试 | Windows 上的问题 | 修法 |
| :--- | :--- | :--- |
| `bash_assignment_deny` | 不加引号时 bash 吃掉路径里的 `\` | Windows 下给赋值右侧加单引号 |
| `data_dir_forward_compat_tests` | 旧数据里的 home 路径放进 JSON 要转义 `\\`；macOS 夹具还带 `/private` 规范前缀 | 替换时先 JSON 转义，并单独替换 `/private<home>` |
| `resume_hydration_cli_e2e` | 进程内写入的 `~/…` 按真实 HOME 缩写，CLI 子进程按测试 HOME 展开；断言拿 JSON 序列化后的文本比路径 | 写入期间临时把 HOME 指向测试 HOME；断言改比消息原文 |
| 其余 CLI/工具测试 | 直接调 `sh`、写 Unix 脚本当假程序、`/tmp` | `tests/common::write_fake_executable`（Unix 脚本 / Windows `.cmd`）、`platform::posix_shell`、系统临时目录 |
| `stream_handler_test` 的日志捕获用例 | 只有一个 tracing 订阅者时，并行的兄弟用例先触发同一条 `warn!`，会把它缓存成"所有线程都不记录"，本用例捕获不到形状日志（单跑通过，整模块跑 1/1 失败） | 用例期间多保留一个空订阅者，让注册走包含本用例订阅者的共享列表（修复后整模块连跑 3/3 通过） |
| 插件 `tests/plugin_sdk_types.test.ts` | TypeScript 把文件名规范成 `D:/…`，与 `path.resolve` 的 `D:\…` 直接比较对不上，内存里的夹具源码没被用上 | 比较前先 `path.resolve` 传入的文件名 |
| `bash_task_test::lifecycle_waits_for_final_preview_barrier` | 固定睡 100ms 就假定 `printf` 已退出；Git Bash 启动比 `sh` 慢，满载时仍是 Running | 改为轮询到进程退出再断言（"生命周期事件不能抢在确认之前"这条断言因此真正在进程退出后检查） |

### CI 与发布（§11、§12）

```text
windows-ci.yml（新增）              release-cli.yml                    release-ext.yml
  rust：gate-fast（Git Bash）        矩阵 + windows-latest              打包 win32-x64 VSIX（带 tomcat.exe）
  extension：npm run gate:fast       打包 tomcat.exe                    verify 矩阵 + windows-latest
                                     guards 要求 4 个 CLI 资产           MARKETPLACE_TARGETS + win32-x64
```

- `windows-ci.yml`：develop/main 推送、PR（按 `tomcat/**`、`tomcat-vscode-ext/**` 过滤）与手动触发；YAML 已本地校验，首次推送后看实际运行结果。
- `run-vscode-verify-vsix.ts`：`vscodeTargetFor` 支持 `win32-x64`；Windows 用 PE 头的 Machine 字段（0x8664）校验架构，替代 `file`/`uname`；临时目录用 `os.tmpdir()`；VSIX 内容用 `readVsixEntry` 读取，不再依赖 `unzip`。
- 插件脚本里的 `npm`/`npx`/`node_modules/.bin` 统一走 `scripts/runCommand.ts`（cross-spawn），Windows 上正确调用 `.cmd`，不需要 `shell: true`。
- 新增 `tests/verify_vsix_target.test.ts`（平台映射、PE 头解析、本机 node.exe 为 AMD64）；`package_vsix_smoke` 增加 win32-x64 打包断言（内含 `bin/tomcat.exe`、不含 `bin/tomcat`）；`release_guard_scripts` 更新为四平台。

### 与计划的偏差（实现时的取舍）

- **插件测试的假 CLI**：计划写"通过 `process.execPath` + `ELECTRON_RUN_AS_NODE` 启动"。宿主夹具测试跑在 Node 里而不是 Electron 里，所以改为传入 `spawnFactory: spawnFakeServe`（`scripts/e2eHostFixture.ts`），用当前 Node 运行 `.js` 假 CLI，不需要 `ELECTRON_RUN_AS_NODE`。
- **npm script-shell**：按计划正文不配置（脚本已改用 cross-spawn，不依赖 shell）。
- **`release_guard_scripts` 的工作流脚本用例**：这些 `run:` 块在 Ubuntu runner 上执行；Windows 上裸 `bash` 是 WSL，在 win32 上跳过，由 Linux CI 覆盖。`install_script_release_selection` 同理（`install.sh` 只给 Unix）。
- **verify-vsix 的升级场景**：0.1.90 从未发布过 Windows 版，Windows 上跳过"从 0.1.90 升级"场景。
- **Unix 临时目录**：verify-vsix 在 Unix 上保持原来的 `/tmp`，只有 Windows（没有 `/tmp`）用 `os.tmpdir()`。

- **进程树用 Job Object（触发了 §14 的反转条件）**：Git Bash 执行命令时会经过一个随即退出的 fork 中转进程，`taskkill /T` 依赖的父子 PID 链在这里断开，后台 `sleep` 之类的孙子进程成了孤儿。改为每个 bash 任务一个 Job（`infra/platform/process_job.rs`），以挂起方式创建进程、加入 Job 后再恢复；停止和超时用 `TerminateJobObject`。Unix 进程组逻辑不变。
- **安装包 registry 读取**：父路径是普通文件时，Unix 报 ENOTDIR，Windows 只报 NotFound。`read_registry_text` 统一按 NotADirectory 拒绝，保证在复制资源前就失败（`core/package/manager/registry.rs`）。
- **测试里的输出生成器**：用 `awk` 代替 `python3`（Git Bash 自带 awk，Windows 默认没有 python3）。
- **锁冲突判断**：Windows 上 fs2 的锁冲突返回 `ERROR_LOCK_VIOLATION`(33)，不是 `WouldBlock`。新增 `platform::is_lock_contended`，用于配置锁、MCP 配置锁、会话占用锁、模型管理锁四处；基线里 MCP 并发添加的失败由此修复。计划未列出此项。
- **私有权限**：起初沿用 icacls（`/inheritance:r /grant:r USERDOMAIN\USERNAME:F`），CI 暴露它只去掉继承权限、不动显式授权（见下方"CI 上才失败的用例"），已改为 Win32 API 整体替换 DACL（`infra/platform/private_acl.rs`）。i18n 键从 `oauth.acl*` 改为通用的 `platform.privateAclFailed`，`.env` 头注释改为"仅当前用户可读写"。测试用 `icacls /save`（UTF-16 SDDL，避开 OEM 代码页乱码）比对 `whoami /user` 的 SID。
- **ConPTY 测试**：除计划中的 Ctrl+C 与中文答案外，还要回复终端的光标位置查询（`ESC[6n`）并回答项目信任提示 `[y/N]`，否则 chat 卡在启动阶段。已用旧的 std `read_line` 反向验证：测试会在第二轮输入处失败。
- **stdin 的 Unix 分支**：原样搬入 `infra/platform/stdin.rs`，但本机无法编译 Unix 代码，依赖 CI 的 macOS/Linux 任务验证。
- **VT 开启位置**：计划写在 `main.rs`，实际放在 `run_cli()` 开头。`main.rs` 是独立的二进制 crate，访问不到 `pub(crate)` 的 platform 模块；为此把 platform 改成公开 API 不值得。效果相同（`main` 的第一件事就是调用 `run_cli`）。
- **副本刷新测试位置**：计划写在 `run_basic_test.rs`。内容不同/相同、清理 `*.old` 三项在该文件；"旧副本正在运行时仍可替换"需要真实的 tomcat.exe，单测拿不到二进制，因此放在 `tests/cli_tests.rs`（`test_init_replaces_a_running_windows_command_copy`）。
- **临时注册表键**：测试用 `HKCU\Software\TomcatTest-<uuid>`，而不是 `Software\TomcatTest\<uuid>`，删除后不会留下空的父键。

### 验收中发现并修复的问题（§13）

#### MCP 服务（经 `npx.cmd` 启动）拖住 CLI/Serve 退出（产品缺陷）

- **问题现象**：Playwright MCP 正在执行工具调用时关闭 Serve 的 stdin，tomcat 过了 16.3 秒才退出；插件关闭 Serve 的 5 秒期限因此超时（`timed out closing tomcat serve after 5000ms`），只能强杀。服务器忙时完全不理会 EOF 的话，就会一直等下去。
- **根因与证据**：与上面"后台任务没结束时 CLI 退不出"是同一类问题，发生在 MCP 子进程上：
  ```text
  tomcat.exe ──▶ cmd.exe（npx.cmd 外壳）──▶ node（playwright MCP）──▶ chrome × 8
                    ▲ kill_on_drop 只杀到这里        │
                                                     └─ 仍握着 MCP 的 stdout 管道
  Windows 上 tokio 读子进程管道用阻塞线程 → 运行时关闭等这条读线程拿到 EOF → 等 node 自己退出
  ```
  - 证据：脚本模拟模型发起 `browser_navigate`，确认 node 与 8 个 chrome 在跑后关闭 stdin，测得进程退出 16337ms、管道关闭 16337ms，随后整棵树才消失。
  - 空闲时 Playwright 收到 EOF 会自己退出（实测 23ms），所以只在"服务正忙或清理很慢"时出现。Unix 不受影响：子进程管道不占阻塞线程，运行时关闭不等它。
- **解决方案**：MCP stdio 子进程在 Windows 上放进关闭即终止（`KILL_ON_JOB_CLOSE`）的 Job：传输被丢弃或 tomcat 退出时，整棵进程树一起结束 <sub>`core/connector/mcp/transport.rs::owned_process_tree`；用 rmcp 自带的 `process-wrap` 的 `JobObject` + `KillOnDrop`（已是传递依赖，`Cargo.toml` 只在 Windows 下显式声明）；Unix 不变</sub>。
  - 为什么不复用 bash 的 `ProcessJob`：rmcp 只接受 `CommandWrap` 并在内部创建进程，拿不到"挂起创建 → 加入 Job → 恢复"的时机；`process-wrap` 的 `JobObject` 正是在这一处做同样的事。
- **测试用例清单**：`tests/serve_robustness_tests.rs::serve_eof_exits_promptly_when_a_cmd_wrapped_mcp_server_outlives_its_client`（仅 Windows）：用 `.cmd` 启动假 MCP 服务（与 `npx.cmd` 同样是 cmd.exe → node），服务在 EOF 后还会再活 60 秒；要求 Serve 10 秒内退出且服务进程不残留。去掉修复时报 `timed out waiting for serve exit`（nextest 同时报 LEAK），修复后 0.8 秒通过。夹具 `fake_stdio_server.mjs` 为此新增 `--linger-after-eof-ms`、`--pid-file` 两个可选参数。
  - 写测试时踩到的坑（已在测试里规避并注明）：cmd.exe 按 OEM 代码页解码批处理文件，路径里的中文会乱码；`.cmd` 里只写 ASCII，路径经 `%~dp0` 和环境变量传入。

#### 路径分隔符混用（展示缺陷）

- **问题现象**：真实 `tomcat init` 输出 `配置文件已写入: C:\Users\汤姆猫\.tomcat/tomcat.config.toml`，doctor 显示 `Git Bash：C:/Program Files/Git\bin\bash.exe`。能用，但看起来像出错。
- **根因与证据**：
  - 配置路径由 `~/.tomcat/tomcat.config.toml` 展开；文件已存在时 `canonicalize` 会顺带统一分隔符，init 时文件还不存在，原样保留了 `/`。
  - `git --exec-path` 输出用 `/`，向上取 3 级后再 `join("bin")`。
- **解决方案**：两处都在源头按路径组件重建，得到本机分隔符 <sub>`infra/platform/mod.rs::normalize_path`（仅 Windows，路径不存在时）、`infra/platform/shell.rs::locate_git_bash`</sub>。
- **测试用例清单**：`platform/tests/suite_test.rs::normalize_path_displays_missing_home_paths_with_native_separators`、`platform/tests/shell_test.rs::derived_bash_path_is_displayed_with_native_separators`（仅 Windows，断言展示串本身；原有用例按组件比较路径，发现不了这个问题）。

#### 验收脚本只设 `VSCODE_EXECUTABLE_PATH` 时用错 VS Code CLI（脚本缺陷）

- **问题现象**：本机用已安装的 VS Code 跑 `run-vscode-verify-vsix.ts --release-bundle`，安装扩展时报 `spawnSync /Applications/Visual Studio Code.app/... ENOENT`。
- **根因与证据**：只有"自动下载 VS Code"分支会同时设置 `VSCODE_CLI_PATH`；指定了可执行文件时，CLI 回落到 macOS 默认路径。CI 走下载分支，所以没暴露。
- **解决方案**：未设置 `VSCODE_CLI_PATH` 时，一律由 `VSCODE_EXECUTABLE_PATH` 推导（与下载分支同一个函数） <sub>`tomcat-vscode-ext/scripts/run-vscode-verify-vsix.ts`</sub>。
- **测试用例清单**：本机 release-bundle 验收即覆盖（见下方场景 1）。

### CI 上才失败的用例（windows-ci）

本机 gate-fast 全绿，GitHub 的 Windows runner 上 lib 8 项、integration 1 项失败。原始日志要管理员权限，所以给工作流加了失败注解（含 panic 信息与 left/right），再用本机能造出来的等价环境复现。

```text
runner 与本机的差别                     影响到的用例
  ├─ TEMP 是 8.3 短名 RUNNER~1 ........ 6 项：期望值用了字面临时路径
  ├─ 临时目录的权限不同 ............... 2 项：私有文件仍带别的显式授权（产品缺陷）
  └─ 没装 ripgrep ..................... 1 项：search_files 默认上下文行
```

#### 临时目录是 8.3 短名，期望值与规范路径对不上（测试缺陷）

- **问题现象**：报错信息、会话目录里出现的是 `C:\Users\runneradmin\...`，测试拿来比的是 `C:\Users\RUNNER~1\...`。
- **根因与证据**：产品按规范路径显示和比较（`canonicalize` 会展开短名）；测试直接用 `tempdir().path()` 作期望值。Unix 的 cwd 总是内核解析过的真实路径，Windows 的 `current_dir` 则原样返回设置时的写法。macOS 上没暴露，是因为 `/private/var/...` 恰好包含 `/var/...` 这个子串。本机不生成短名，改用 junction 作 TEMP，同样复现 6 项。
  - 权限判断不受影响：`gate.rs::canonicalize_with_existing_ancestor` 两边都先规范化再比较，所以这是测试问题，不改产品。
- **解决方案**：期望值改用 `canonicalize` 后的路径；`runtime_split_test` 直接把进程 cwd 设成规范路径 <sub>`run_basic_test`、`admin_test`、`config_tool_test`、`cmd_uninstall_test`、`runtime_split_test`、`tests/resume_hydration_cli_e2e.rs` 的夹具</sub>。
- **测试用例清单**：上述 6 项在 TEMP 为 junction 与普通目录时都通过。

#### 私有文件（`.env`、OAuth 令牌）仍带别的显式授权（产品缺陷）

- **问题现象**：`assert_private_to_current_user` 在 runner 上失败。
- **根因与证据**：旧实现 `icacls /inheritance:r /grant:r 用户:F` 只去掉继承来的权限，文件上已有的显式授权原样保留。父目录没有可继承权限时，新文件按令牌默认 DACL 带上显式授权。本机造了这样的目录作 TEMP，复现结果是 `D:PAI(A;;FA;;;<用户>)(A;;FA;;;SY)(A;;0x1200a9;;;<登录会话>)`。用户手工给 `.env` 加过 "Everyone 读" 时同样会留下（`(A;;FR;;;WD)`）。这与 Unix 0600 "只剩自己"的承诺不符。
- **解决方案**：改用 Win32 API，把 DACL 整体换成只含当前令牌用户一条完全控制的受保护 DACL <sub>新增 `infra/platform/private_acl.rs`（`OpenProcessToken` 取 SID → `InitializeAcl`/`AddAccessAllowedAce` → `SetNamedSecurityInfoW` + `PROTECTED_DACL_SECURITY_INFORMATION`）；不再依赖 `USERNAME`/`USERDOMAIN` 与 icacls，删掉 `platform.privateAclUser`；`Cargo.toml` 的 windows-sys 加 `Win32_Security_Authorization`</sub>。
- **测试用例清单**：新增 `platform/tests/suite_test.rs::private_file_permissions_drop_explicit_grants_to_other_accounts`：先显式授予 Everyone 读，调用后 SDDL 只剩当前用户（按旧做法手工执行会留下 `(A;;FR;;;WD)`）。原有 `env_file_test`、`oauth_test` 在两种 TEMP 下都通过。
  - runner 注解还暴露了断言本身的一个问题：runner 账户 `runneradmin` 是内置 Administrator（RID 500），SDDL 把它写成别名 `LA` 而不是 SID。`test_support::assert_private_to_current_user` 对 RID 500/501 改用 `LA`/`LG` 比对。

#### `search_files` 默认上下文行缺失（CI 环境缺 ripgrep）

- **问题现象**：`search_files_content_mode_defaults_to_three_context_lines` 报"缺少 line 3"。
- **根因与证据**：没有 `rg` 时工具走兜底实现（tier2），它本来就不带上下文行，只给出警告（`executor/search.rs`，既有设计）。本机的 `rg` 来自 Cursor 安装目录里自带的那份，所以本机一直走 ripgrep；runner 上没有 rg。从 PATH 去掉 rg 后本机同样复现。
- **解决方案**：windows-ci 在跑门禁前用 choco 装 ripgrep；用户指南的 Windows 开发环境表加上 ripgrep。产品不改：普通用户缺 rg 时，启动时的 preflight 本来就会提示安装。
- **测试用例清单**：无新增；该用例在装了 rg 的环境里通过。

### 验收证据

截图与脚本只保存在本机，不入库：`%USERPROFILE%\.tomcat\temp\windows-acceptance\`。按用户要求不用真实模型：模型回复由脚本模拟，其余（VS Code、`tomcat.exe`、Git Bash、Playwright、Chrome）都是真实的。本机用户目录 `C:\Users\汤姆猫` 含中文，临时目录也在其下，因此全程覆盖非 ASCII 路径。

```text
验收方式
  ├─ A 真实 VS Code + 安装 release VSIX（run-vscode-verify-vsix.ts --release-bundle）....... 场景 1
  ├─ B 脚本探针：真实 TomcatMessenger（插件同一套 spawn 代码）启动 release tomcat.exe .... 场景 1、2、5、7
  ├─ C 真实用户目录跑 init + 经资源管理器打开新终端 ......................................... 场景 8
  ├─ D 隐藏 Git 的临时环境 + dumpbin 查依赖 ................................................... 场景 9
  └─ E 已在 Windows 上跑过的自动化用例（gate-fast 全绿）...................................... 其余场景
```

| 场景 | 结果 | 证据 |
| :--- | :--- | :--- |
| 1 安装 VSIX、连接、无黑窗 | 通过 | A：1 passing（17s），截图 `vsix-local/gui-*/release-workbench.png` 等；B：窗口监视器全程未发现新窗口（先用手动打开的控制台做过正向对照，0.1 秒内能捕获） |
| 2 Git Bash 跑 `ls`/`git status`，中文输出 | 通过 | B：回传给模型的工具结果含 `中文文件.txt`、`你好，Windows` 与 git 状态 |
| 3 CRLF 文件编辑 / Files 面板 / Undo | 自动化覆盖 | `primitive/tests/suite_test.rs::edit_preserves_crlf_line_endings`；Files 面板还原：插件 `serve_session_files.test.ts`（真实 Serve） |
| 4 检查点恢复 CRLF | 自动化覆盖 | `checkpoint/tests/shadow_git_test.rs::windows_long_paths_and_line_endings_survive_checkpoint_restore` |
| 5 会话记录只读 | 通过 | B：写 `~/.tomcat/agents/main/sessions/probe.jsonl` 被拒绝、文件未生成；另有 `gate_test.rs::windows_builtin_sessions_rule_blocks_backslash_write` |
| 6 `/install` 安装 skill | 自动化覆盖 | `serve_package_install_integration`、`package_manager_test`（含 junction 拒绝） |
| 7 Stop 后整棵进程树消失 | 通过 | B：前台 `sleep 300` 发 interrupt、后台任务 `task_stop`，两次 `tasklist` 都确认 sleep 与新起的 bash 全部消失；探针退出后无残留 |
| 8 init → 新终端 → chat；Ctrl+C；颜色 | 通过 | C：见下方；Ctrl+C 由 `windows_conpty_cli_tests` 覆盖；颜色见下方截图说明 |
| 9 无 Git / 无 VC++ 运行库 | 通过（替代环境） | D：本机 Home 版没有 Windows Sandbox；见下方 |
| 10 CRLF `.plan.md` | 自动化覆盖 | `file_store_frontmatter_test.rs::crlf_plan_checked_out_by_git_parses_and_writes_back_as_lf`、`safety_test.rs::reviewer_body_diff_guard_still_protects_crlf_plans` |
| 11 `d:\` 与 `D:\` 同一会话列表 | 自动化覆盖 | `session/tests/scope_test.rs::code_key_ignores_path_case_and_has_no_verbatim_prefix_on_windows` |
| 12 Playwright MCP 连接并截图 | 通过 | B（含 Job 修复的 release exe）：经 `npx.cmd` 连上，`browser_navigate` 与 `browser_take_screenshot` 均成功，输出目录生成 PNG（`mcp-playwright-screenshot.png`，中文正常）；Chrome 在 Job 内工作正常，Serve 关闭 514ms，之后无 Playwright/Chrome 残留进程 |
| 13 从商店安装 | 未做 | 需要正式发布后才能执行 |
| 14 超过 260 字符的路径 | 自动化覆盖 | 同场景 4 的用例（`core.longpaths`） |

- 场景 8（init 与新终端）：用 release `tomcat.exe` 在真实用户目录运行 `init`：写入配置、`models.toml`、`mcp.json`，复制 `~/.local/bin/tomcat.exe`，把 `C:\Users\汤姆猫\.local\bin` 加入用户 PATH，并提示"请打开新的终端窗口"。随后经资源管理器启动检查脚本（等同用户新开终端，环境来自刷新后的注册表）：`where tomcat` → `C:\Users\汤姆猫\.local\bin\tomcat.exe`，`tomcat --version` → `0.1.78`，`tomcat code` 正常进入对话模式（`new-terminal-tomcat-code.png`，Windows 11 默认用 Windows Terminal 打开）。
- 场景 8（退出）：真实默认配置（含 Playwright MCP）下 `tomcat code` 收到 EOF 后 199ms 退出；`serve --stdio` 连上 Playwright 后关闭 stdin，23ms 退出，8 个 cmd/node 进程全部清理。工具调用进行中退出的情况见上方"MCP 服务拖住退出"。
- 场景 9：PATH 只留 `C:\Windows\System32;C:\Windows`、ProgramFiles/LOCALAPPDATA 指向空目录来隐藏 Git：`doctor` 显示"✗ Git Bash：未找到"及 winget 安装提示，`serve --stdio` 正常完成 initialize 并以 0 退出。`dumpbin /dependents` 显示 release `tomcat.exe` 只依赖系统 DLL（CRT 静态链接），不需要 VC++ 运行库。自动化用例 `test_doctor_reports_git_bash_on_windows` 同样覆盖。
- 场景 12 的附带发现：把 `USERPROFILE` 指向一个空的临时目录时（测试夹具隔离 HOME 的做法），Playwright 直接调用也会卡在 `browser_navigate`（同样的白名单环境、真实 `USERPROFILE` 则 0.4 秒完成）。这是夹具隔离带来的，不是产品问题；真实用户的 `USERPROFILE` 总是真实目录。
- 场景 8（颜色）：`HKCU\Console\VirtualTerminalLevel` 未设置的前提下，在 conhost 中运行 `tomcat chat`，吉祥物显示为橙色，没有 `←[` 乱码（`conhost-vt.png`）；Windows Terminal 同样正常（`wt-vt.png`）。
  - 观察：conhost 的默认中文字体（新宋体）不含盲文字符，吉祥物显示为方框轮廓；Windows Terminal（Cascadia）显示正常。这是字体问题，与 VT 无关，本轮不处理；用户可设置 `TOMCAT_SPLASH=0` 关闭吉祥物。
  - 注意：Cursor 的终端会注入 `NO_COLOR=1` 和 `TERM=dumb`，从那里启动的进程会去掉颜色，人工验收时要先清掉这两个变量。

### BLOCKED (当前风险)

- 两个 dry-run 打在 `afbec1ff` 上，之后的修复提交没有重跑 dry-run；正式发版时 release 工作流会在新提交上重新构建并验收。
- 两个 dry-run 的产物与证据 zip 需登录下载，本机未复核；同样的检查已在本机真实 VS Code 上跑过。
- stdin 的 Unix 分支与 serve 的 Unix 读取路径未在本机编译，依赖 macOS/Linux CI。
- 场景 13（从商店安装）只能在正式发布后执行；正式发布需用户确认。
- 观察（非本轮改动）：跑 lib 测试期间真实 `~/.tomcat/plans` 的修改时间变了（目录为空），疑似既有测试隔离不严，本轮未处理。
