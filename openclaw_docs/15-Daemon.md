# Daemon

## 零、先用大白话

Daemon 像 **把塔台注册成小区保安亭**：开机要在、倒了要起、还要看台账（日志）。  
macOS 用 **launchd**，Linux 用 **systemd**，Windows 用 **计划任务**——表格不同，心情一样。

**这一节你会学到**：`daemon install` 写哪些文件；日常更推荐啥启动方式。

---

**设计思想**：Daemon 负责 Gateway 服务的安装与生命周期管理，支持 launchd（macOS）、systemd（Linux）、schtasks（Windows）。

---

## ASCII 核心四图

### 1) 结构图

```text
daemon-cli
        |
        +--> launchd plist（macOS）
        +--> systemd unit（Linux）
        +--> schtasks（Windows）
```

### 2) 调用流图

```text
openclaw daemon install
  -> 生成服务描述文件
      -> register 到 OS 调度器
          -> start / status / logs 子命令走平台 API
```

### 3) 时序图

```text
Admin       daemon-cli        OS service mgr      Gateway 进程
  |              |                  |                |
  | install      |                  |                |
  |------------->| write unit       |                |
  |              |----------------->| enable/start  |
  |              |                  |--------------->|
```

### 4) 数据闭环图

```text
开机自启
        |
        v
崩溃重启策略
        |
        v
日志轮转与健康检查
        |
        v
升级 openclaw 后重装 unit -> 再观察
```

---

## 一、入口与 CLI

- **daemon-cli**：**`src/cli/daemon-cli.ts`**，register、install、lifecycle、status。
- **daemon-cli/register.ts**：注册 daemon 子命令。
- **daemon-cli/install.ts**：安装服务。
- **daemon-cli/lifecycle.ts**：启动、停止、重启。

---

## 二、平台实现

- **macos/gateway-daemon.ts**：macOS launchd 集成。
- **daemon/systemd.ts**：Linux systemd。
- **daemon/schtasks.ts**：Windows 计划任务。
- **daemon/runtime-paths.ts**：运行时路径。
- **daemon-install-helpers**：**`src/commands/daemon-install-helpers.ts`**，跨平台安装逻辑。

### 2.1 macOS 特别提示（与上游 AGENTS 对齐）

日常使用中，**Gateway 常与 macOS 菜单栏应用同生命周期**；不要默认假设存在某个固定 LaunchAgent label。调试启动/停止请优先跟随 **OpenClaw Mac 应用** 或上游脚本（如 `scripts/restart-mac.sh`），详见上游 `AGENTS.md` 与 `docs/gateway/doctor.md`。

---

## 常见误会

- **误会**：`daemon install` 一次管一辈子。**正解**：升级大版本有时要 **重装 unit** 或跟发行说明走。  
- **误会**：systemd 和 launchd 配置能互拷。**正解**：字段名、路径、权限模型都不同。  
- **误会**：服务在跑就等于 Gateway 健康。**正解**：进程在 ≠ **渠道连上**；用 `channels status` / `doctor` 交叉看。
