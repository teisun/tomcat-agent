# 已知限制与后续决策

本文只记录已确认、但不应在“会话卡死修复”批次中顺带改动的问题。每条都说明当前行为、风险边界和重新排期条件，避免它们在交接时丢失。

## P1

### 旧 `timeout_ms` 配置缺少运行时迁移覆盖

旧的 `ask_question.timeout_ms` 配置会产生迁移提示，但真实运行时组合尚没有端到端测试。当前产品语义仍是“无超时”。

重新排期条件：配置迁移或环境变量加载链重构时，补一条 CLI 真运行测试，断言旧值不会创建 deadline。

### Checkpoint 端到端屏障覆盖不足

Checkpoint 的单元和 serve 测试覆盖了记录、恢复和 busy 拒绝；真实 CLI 子进程在“写入完成后立即退出”的 fsync 屏障仍缺独立 E2E。

重新排期条件：checkpoint 存储格式或同步级别变化时。测试必须走子进程，不可只 mock store。

### Workbench Find 驱动器依赖启发式定位

VS Code E2E 的 `workbenchFindDriver` 依赖当前 workbench DOM 的可访问性结构。它对产品无运行时影响，但 IDE 升级可能令测试定位失败。

重新排期条件：首次因 workbench DOM 演进失败时。修复应提供有界 fallback，并在 fallback 也失败时输出诊断快照。

## P2

### Hangup 命名残留

若干内部变量/日志仍沿用 `hangup`，实际语义已区分用户 interrupt、已确认 host disconnect 和 restart-pending。它不会改变协议，但会增加排障认知负担。

重新排期条件：下一次统一重命名或 Ask Question 协议版本升级；不要在行为修复中混入大范围 rename。

### Plan Mermaid 的 test id 命名不准确

少数 Plan 图相关 test id 沿用早期组件名，测试覆盖本身有效。改名只会制造无功能收益的快照噪音。

重新排期条件：该组件的可访问性标识重构时，一并迁移并保留兼容选择器窗口。

### Windows：旧代码页程序的输出可能乱码

bash 工具按 UTF-8 解码命令输出。Git Bash 里的 GNU 工具和多数现代程序输出 UTF-8，但仍使用 OEM/ANSI 代码页的老程序（如部分 `cmd` 内置命令、`ipconfig`、`icacls`）在中文系统上输出 GBK，显示为乱码。命令本身照常执行，退出码不受影响。

重新排期条件：验收或用户报告中，常用开发工具的输出因此不可读时。修复应按程序实际代码页解码，不能全局改控制台代码页。

### Windows：tomcat.exe 未签名

Release 与 VSIX 里的 `tomcat.exe` 没有 Authenticode 签名。从浏览器下载的压缩包可能触发 SmartScreen 提示；企业环境的应用白名单可能拦截。

重新排期条件：出现 SmartScreen/杀毒软件拦截的用户报告，或有企业部署需求时。需要先取得代码签名证书，再在 release-cli 里加签名步骤。

### Windows：没有 ARM64 原生包

只发布 `x86_64-pc-windows-msvc` / `win32-x64`。Windows ARM64 不在支持承诺内。

重新排期条件：出现 Windows ARM64 用户需求，且 CI 有可用的 ARM64 Windows runner 跑原生验收时。

### Windows：没有一键安装脚本（install.ps1）

Windows 上的终端 CLI 需要手动下载压缩包、解压后运行 `.\tomcat.exe init`；`install.sh` 只用于 Unix。插件用户不受影响（插件包内置 CLI）。

重新排期条件：终端 CLI 的 Windows 用户反馈手动安装成本高时。脚本应复用 release 的 `SHA256SUMS` 校验，与 `install.sh` 保持同一套资产命名。

### Windows：conhost 默认字体显示不了吉祥物

conhost 的默认中文字体（新宋体）不含盲文字符，启动吉祥物显示为方框；Windows Terminal（Cascadia）显示正常。颜色与功能不受影响，可用 `TOMCAT_SPLASH=0` 关闭吉祥物。

重新排期条件：用户反馈 conhost 下的启动画面影响使用时。

### Verifier 目前是 dormant

`verify_gate` 配置、提示词和实现仍存在，但完成路径没有自动派发 verifier；现有测试明确锁住这一现状。把它重新接入会改变执行成本、失败语义和用户交互，应作为单独产品决策评审，而不是默认启用。
