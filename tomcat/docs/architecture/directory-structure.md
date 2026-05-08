# 工作目录结构

本文档描述 **tomcat** 默认 `work_dir`（通常为 `~/.tomcat/`）下的目录布局。详细设计见 [work-dir-and-data-layout](./work-dir-and-data-layout.md)。


```
~/.tomcat/
├── tomcat.config.toml                     # 主配置文件
├── agents/                            # 各 agent 的运行时状态
│   └── <agentId>/
│       ├── agent/                     # 身份与凭据
│       │   ├── auth-profiles.json     # 认证配置（API key/OAuth）
│       │   └── models.json            # 模型配置
│       ├── sessions/                  # 会话记录
│       │   ├── sessions.json          # 会话索引
│       │   └── <sessionId>.jsonl      # 会话 transcript
│       ├── logs/                      # 业务日志
│       ├── audit/                     # 审计日志 JSONL
│       ├── tmp/                       # 临时文件
│       └── tool-results/              # Layer0 大工具结果持久化
├── workspace-main/                    # 默认 agent 的工作区
│   ├── AGENTS.md                      # Agent 引导文件
│   ├── SOUL.md                        # 人格/灵魂文件
│   ├── TOOLS.md                       # 工具说明
│   ├── IDENTITY.md                    # 身份文件
│   ├── USER.md                        # 用户描述
│   ├── HEARTBEAT.md                   # 心跳配置
│   ├── BOOTSTRAP.md                   # 启动引导
│   ├── MEMORY.md                      # 长期记忆（主文件）
│   ├── memory.md                      # 长期记忆（备选名）
│   ├── memory/                        # 记忆子目录
│   │   └── YYYY-MM-DD.md             # 按日记忆
│   ├── skills/                        # 工作区技能（优先级最高）
│   │   └── <skillName>/SKILL.md      # 各技能定义文件
│   └── .tomcat/
│       └── workspace-state.json       # 工作区状态（若启用；与 pi-mono `.pi/` 命名空间区分）
├── workspace-<agentId>/               # 非默认 agent 的工作区（结构同上）
├── memory/                            # 向量检索索引
│   └── <agentId>.sqlite               # 按 agent 分文件的 SQLite 索引
├── skills/                            # 托管技能（managed skills）
│   └── <skillName>/SKILL.md           # 通过 Gateway RPC 安装/管理的
├── credentials/                       # OAuth 凭据
├── media/                             # 媒体文件
├── subagents/
│   └── runs.json                      # 子 agent 注册表
└── plugins/                           # 全局共享插件
├── assets/                         # 全局资源目录
│   ├── .env                        # 敏感配置（API Key 等），tomcat init 自动生成，权限 0600
│   ├── .versions.json              # 内嵌资源 SHA-256 版本记录 + 释放时间戳
│   ├── .lock                       # 并发写入保护文件锁（fs2 exclusive lock）
│   ├── wasm/                       # 全局 Wasm 运行时引擎
│   │   └── wasmedge_quickjs.wasm   # 内嵌资源自动释放目标（~3.3MB）
│   └── modules/                    # 全局 JS 兼容模块（内嵌资源自动释放目标）
│       └── (79 个 Node.js 兼容 shim，~1MB)
```

以下为**多 agent 注册表示例**（规划中；文件名以最终实现为准）。

```json
{
  "agents": {
    "list": [
      {
        "id": "main",
        "workspace": "~/.tomcat/workspace-main",
        "agentDir": "~/.tomcat/agents/main/agent",
        "subagents": {
          "allowAgents": ["researcher", "writer", "coder"]
        }
      },
      {
        "id": "researcher",
        "workspace": "~/.tomcat/workspace-researcher",
        "agentDir": "~/.tomcat/agents/researcher/agent"
      },
      {
        "id": "writer",
        "workspace": "~/.tomcat/workspace-writer",
        "agentDir": "~/.tomcat/agents/writer/agent"
      },
      {
        "id": "coder",
        "workspace": "~/.tomcat/workspace-coder",
        "agentDir": "~/.tomcat/agents/coder/agent"
      }
    ]
  }
}
```

## 说明
- **`~/.tomcat/tomcat.config.toml`**：总控配置文件（与树形图顶部一致）。
- **`agent_workspace_dir`**：用户启动 `tomcat chat` 时 shell 的 `pwd`，不在 `~/.tomcat` 数据根内（`agent_workspace_dir` 通常为项目目录）。用户说“当前目录”“这个项目”“相对路径”时，优先解释为该目录；但它不自动获得文件访问权限，访问时仍需 `workspace.workspace_roots` 或会话授权。
- **`agent_definition_dir`**：指向 `workspace-main/` / `workspace-<agentId>/`，存放主 Agent 的行为规则与个性化配置，属于「设计态」数据，是权限系统的默认可写根；但不能被 Prompt 描述成用户当前目录。
- **`agent_trail_dir`**：指向 `agents/<agentId>/`，存放 Agent 的「运行态」数据（会话、日志、审计、临时文件、Layer0 `tool-results` 等），正常工具只读。当前 MVP 仅一个 agent，agentId 固定为 `main`。
- **`plugins/`**（根级）：全局共享插件，所有 agent 均可加载。`agents/<agentId>/plugins/` 为 agent 专属插件。
- **`assets/`**：全局资源目录；内嵌资源释放目标（`wasm/`、`modules/`）与敏感配置（`.env`）。主配置 **`tomcat.config.toml`** 位于 `work_dir` 根部（不在 `assets/`）。详见 [init-experience-and-embedded-assets](../../../docs/reports/init-experience-and-embedded-assets.md)。
- **`assets/.env`**：存放 API Key 等敏感配置，`tomcat init` 自动生成模板，`run_cli` 启动时通过 dotenvy 自动加载。
