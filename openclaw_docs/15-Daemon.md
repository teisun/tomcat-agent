# Daemon

**设计思想**：Daemon 负责 Gateway 服务的安装与生命周期管理，支持 launchd（macOS）、systemd（Linux）、schtasks（Windows）。

---

## 一、入口与 CLI

- **daemon-cli**：`openclaw/src/cli/daemon-cli.ts`，register、install、lifecycle、status。
- **daemon-cli/register.ts**：注册 daemon 子命令。
- **daemon-cli/install.ts**：安装服务。
- **daemon-cli/lifecycle.ts**：启动、停止、重启。

---

## 二、平台实现

- **macos/gateway-daemon.ts**：macOS launchd。
- **daemon/systemd.ts**：Linux systemd。
- **daemon/schtasks.ts**：Windows 计划任务。
- **daemon/runtime-paths.ts**：运行时路径。
- **daemon-install-helpers**：`openclaw/src/commands/daemon-install-helpers.ts`，跨平台安装逻辑。
