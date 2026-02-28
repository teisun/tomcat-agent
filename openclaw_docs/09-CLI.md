# CLI

**设计思想**：CLI 通过 Commander 构建，子命令按需懒加载（register.subclis），与 Gateway 通过 call/health/probe 通信；config-guard 与 preaction 保证配置与前置条件。

---

## 一、入口与结构

- **buildProgram**：`openclaw/src/cli/program/build-program.ts`，创建 Commander 程序，调用 `registerProgramCommands`。
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
