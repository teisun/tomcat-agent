# 工作目录结构（可视化 companion）

本文档只负责展示默认 `work_dir`（通常为 `~/.tomcat/`）的目录树，**规则、优先级、三层安装布局与账本语义以 [`work-dir-and-data-layout.md`](./work-dir-and-data-layout.md) 为唯一事实源**。

## 目录树

```text
~/.tomcat/
├── tomcat.config.toml
├── agents/
│   └── <agentId>/
│       ├── agent/
│       ├── sessions/
│       ├── todos/
│       ├── logs/
│       ├── audit/
│       ├── checkpoints/
│       ├── plugins/
│       ├── skills/
│       └── packages/
├── workspace-main/
├── workspace-<agentId>/
├── plugins/            # global plugins
├── skills/             # global managed skills
├── packages/           # global package ledger
├── memory/
├── credentials/
├── media/
├── subagents/
└── assets/
    ├── .env
    ├── .versions.json
    ├── .lock
    └── js/
```

## 读这张图时要记住

- `agent_workspace_dir/.tomcat/plugins/` 与 `agent_workspace_dir/.tomcat/skills/` 属于 **scope 私有层**，不在 `~/.tomcat/` 数据根内。
- `agents/<agentId>/plugins|skills|packages/` 属于 **agent 私有层**。
- 根级 `plugins|skills|packages/` 属于 **global 层**。
- `packages/` 只存安装账本，不存 plugin/skill 正文。

## 继续下钻

- 规则与语义：[`work-dir-and-data-layout.md`](./work-dir-and-data-layout.md)
- 技能根与覆盖顺序：[`skill-system.md`](./skill-system.md)
- package 安装与 ledger：[`package-manager.md`](./package-manager.md)
