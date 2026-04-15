# pi-mono_docs 阅读指南

本目录是 **Tomcat 仓库里的中文配套笔记**，用来解释上游 **[pi-mono](https://github.com/badlogic/pi-mono)** 各包在干什么。**实现以 pi-mono 源码为准**；这里负责**好读、好对齐、降低协作门槛**。

## 建议阅读顺序

1. [00-pi-mono 技术总览](00-pi-mono%20技术总览.md) — 地图：有哪些包、谁先谁后。
2. [01-pi-ai 包](01-pi-ai%20包.md) — 怎么跟各家模型说话。
3. [02-pi-agent-core 包](02-pi-agent-core%20包.md) — 工具循环与事件。
4. [03-pi-coding-agent 包](03-pi-coding-agent%20包.md) — 终端里的 `pi`、会话、扩展。
5. [04-tui 与 web-ui](04-tui%20与%20web-ui.md) — 终端 UI 与网页 UI。
6. [05-mom 与 pods](05-mom%20与%20pods.md) — Slack 与 GPU 上的 vLLM CLI。
7. [storage-design-comparison](storage-design-comparison.md) — 会话文件和别家项目怎么对比。

## 术语迷你表（扫一眼即可）

| 词 | 可以把它想成 |
|----|----------------|
| **Provider** | 某一家模型厂商的「接线方式」 |
| **Agent** | 会多步循环：问模型 → 可能要跑工具 → 再问模型 |
| **Session / JSONL** | 把一整段对话和元事件追加写进一个文本文件，一行一条 JSON |
| **Extension** | 给 `pi` 加能力（命令、工具、主题…）的插件包 |
| **Runtime（coding-agent）** | CLI 里比 `createAgentSession` 更大一号的组装体，带扩展诊断等 |

## 英文上游 README 副本

- [pi-ai-README.md](pi-ai-README.md)
- [pi-coding-agent-README.md](pi-coding-agent-README.md)
- [pi-tui-README.md](pi-tui-README.md)

每份副本开头有一段 **中文导读**，正文仍以英文仓库 README 为主（便于和上游 diff）。
