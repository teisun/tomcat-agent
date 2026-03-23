# 工作目录结构

本文档描述 pi（openclaw agent）运行时的工作目录布局。详细设计见 [work-dir-and-data-layout](./work-dir-and-data-layout.md)。


```
~/.pi_/
├── pi.toml                            # 主配置文件（原 config.toml）
├── pi.json                            # 主配置文件（后续要改成pi.json）
├── agents/                            # 各 agent 的运行时状态
│   └── <agentId>/
│       ├── agent/                     # 身份与凭据
│       │   ├── auth-profiles.json     # 认证配置（API key/OAuth）
│       │   └── models.json            # 模型配置
│       ├── sessions/                  # 会话记录
│       │   ├── sessions.json          # 会话索引
│       │   └── <sessionId>.jsonl      # 会话 transcript
│       ├── logs/                      # 业务日志
│       └── audit/                     # 审计日志 JSONL
│   └── <agentId>/
│       ├── agent/                     # 身份与凭据
│       │   ├── auth-profiles.json     # 认证配置（API key/OAuth）
│       │   └── models.json            # 模型配置
│       ├── sessions/                  # 会话记录
│       │   ├── sessions.json          # 会话索引
│       │   └── <sessionId>.jsonl      # 会话 transcript
│       ├── logs/                      # 业务日志
│       └── audit/                     # 审计日志 JSONL
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
│   └── .openclaw/
│       └── workspace-state.json       # 工作区状态
├── workspace-<agentId>/               # 非默认 agent 的工作区（结构同上）
├── memory/                            # 向量检索索引
│   └── <agentId>.sqlite               # 按 agent 分文件的 SQLite 索引
├── credentials/                       # OAuth 凭据
├── media/                             # 媒体文件
├── subagents/
│   └── runs.json                      # 子 agent 注册表
└── plugins/                           # 全局共享插件
├── assets/                         # 全局资源目录
│   ├── .env                        # 敏感配置（API Key 等），pi init 自动生成
│   ├── wasm/                       # 全局 Wasm 运行时引擎
│   │   └── wasmedge_quickjs.wasm   # 内嵌资源自动释放目标
│   └── modules/                    # 全局 JS 兼容模块（内嵌资源自动释放目标）
│       └── (80+ Node.js 兼容 shim)
```
```
pi.json
{
  "agents": {
    "list": [
      {
        "id": "main",
        "workspace": "~/.pi_/workspace-main",
        "agentDir": "~/.pi_/agents/main/agent",
        "subagents": {
          "allowAgents": ["researcher", "writer", "coder"]
        }
      },
      {
        "id": "researcher",
        "workspace": "~/.pi_/workspace-researcher",
        "agentDir": "~/.pi_/agents/researcher/agent"
      },
      {
        "id": "writer",
        "workspace": "~/.pi_/workspace-writer",
        "agentDir": "~/.pi_/agents/writer/agent"
      },
      {
        "id": "coder",
        "workspace": "~/.pi_/workspace-coder",
        "agentDir": "~/.pi_/agents/coder/agent"
      }
    ]
  }
}
```

## 说明
- **`~/.pi_/pi.toml`**：总控配置文件
- **`workspace-main/`**：存放主 Agent 的行为规则与个性化配置，属于「设计态」数据。
- **`agents/<agentId>/`**：存放 Agent 的「运行态」数据（会话、日志、临时文件等）。当前 MVP 仅一个 agent，agentId 固定为 `main`。
- **`plugins/`**（根级）：全局共享插件，所有 agent 均可加载。`agents/<agentId>/plugins/` 为 agent 专属插件。
- **`assets/`**：全局资源目录，包含MVP阶段配置文件（`config.toml`）、内嵌资源释放目标（`wasm/`、`modules/`）和敏感配置（`.env`）。详见 [init-experience-and-embedded-assets](./init-experience-and-embedded-assets.md)。
- **`assets/.env`**：存放 API Key 等敏感配置，`pi init` 自动生成模板，`run_cli` 启动时通过 dotenvy 自动加载。
