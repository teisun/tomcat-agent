# CLI

## 零、先用大白话

CLI 像 **遥控器面板**。  
`openclaw gateway` 像 **开关机**；`openclaw doctor` 像 **自检**。  
多数钮背后只是：**读配置**、**连一下 Gateway**、**改几个本地文件**；真正重的活在 Gateway / Agent。

**这一节你会学到**：程序入口在哪；子命令怎么懒加载。

---

**设计思想**：CLI 通过 Commander 构建，子命令按需懒加载（register.subclis），与 Gateway 通过 call/health/probe 通信；config-guard 与 preaction 保证配置与前置条件。

---

## ASCII 核心四图

### 1) 结构图

```text
openclaw（bin）
        |
        v
buildProgram + registerProgramCommands
        |
        v
子 CLI 懒加载（gateway/agent/doctor/...）
        |
        v
本地文件 或 远程 Gateway WS
```

### 2) 调用流图

```text
argv
  -> preaction / config-guard
      -> 命中子命令模块
          -> 调本地逻辑 或 gatewayCall
              -> stdout/stderr 人类可读结果
```

### 3) 时序图

```text
User shell    CLI program        Gateway process     Config disk
     |             |                    |                |
     | openclaw …  |                    |                |
     |------------>| health/probe       |                |
     |             |------------------->|                |
     |             | 读配置（若本地）    |--------------->|
```

### 4) 数据闭环图

```text
CLI 写配置 / doctor 修复
        |
        v
磁盘与内存视图一致
        |
        v
Gateway 热更感知
        |
        v
再用 CLI 验证状态位（channels status 等）
```

---

## 一、入口与结构

- **buildProgram**：**`src/cli/program/build-program.ts`**，创建 Commander 程序，调用 `registerProgramCommands`。
- **register.subclis.ts**：懒加载子 CLI（gateway、agent、message、onboard、doctor、channels、models、plugins、memory、hooks、cron、webhooks、skills、sandbox 等）。
- **command-registry.ts**：`registerProgramCommands` 注册主命令与子 CLI。

---

## 二、主要子命令与对应模块

| 子命令 | 模块 | 说明 |
|-------|------|------|
| gateway | Gateway | 启动、停止、重启、健康 |
| agent | Agents | 通过 Gateway 或 --local 运行 Agent |
| message | Channels/Outbound | 发送消息、react、poll 等 |
| onboard | Wizard | 交互式初始化 |
| doctor | Commands | 健康检查与修复 |
| channels | Channels | 通道管理 |
| models | Config | 模型配置 |
| plugins | Plugins | 插件管理 |
| memory | Memory | 记忆索引与搜索 |
| hooks | Hooks | 钩子安装与状态 |
| cron | Cron | 定时任务 |
| webhooks | Hooks/Gmail | Gmail Pub/Sub |
| skills | Skills | 技能状态 |
| sandbox | Sandbox | 沙箱管理 |

---

## 三、与 Gateway 的调用方式

- **call**：通过 GatewayClient 调用 Methods。
- **health**：健康检查。
- **probe**：深度探测（channels 等）。

---

## 常见误会

- **误会**：没起 Gateway 也能 `openclaw agent` 干所有事。**正解**：很多子命令要 **先有塔台**；少数有 `--local` 一类旁路（看 help）。  
- **误会**：CLI 和 Control UI 状态一定一致。**正解**：都可能读写磁盘；**以 `~/.openclaw` 实际文件** 为准。  
- **误会**：子命令全在启动时装进内存。**正解**：大量是 **懒加载**，第一次跑得稍慢是正常的。
