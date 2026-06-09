# tomcat 操作说明

本文档面向第一次使用 tomcat 的用户，按"获取程序 → 初始化 → 逐功能体验"顺序，覆盖 tomcat 的主要使用方式与可用功能。

---

## 目录

- [0. 下载 Release 二进制直接使用](#0-下载-release-二进制直接使用)
- [1. 下载源码并编译构建](#1-下载源码并编译构建)
- [2. 初始化与环境检测](#2-初始化与环境检测)
- [3. 配置管理](#3-配置管理)
- [4. 会话管理](#4-会话管理)
- [5. 工作区管理](#5-工作区管理)
- [6. 对话模式（chat）](#6-对话模式chat)
- [7. 审计日志](#7-审计日志)
- [8. 附录](#8-附录)
- [9. Wasm / WasmEdge 与插件（建设中）](#9-wasm--wasmedge-与插件建设中)

---

## 0. 下载 Release 二进制直接使用

适合只想尽快运行 CLI 的用户。下载当前平台的 release 二进制后，放入 `PATH` 并执行初始化即可。

```bash
# 1. 给二进制加执行权限并放入 PATH
chmod +x tomcat && mv tomcat /usr/local/bin/

# 2. 初始化
tomcat init

# 3. 验证
tomcat --help
```

`tomcat init` 会完成默认配置写入、工作目录创建、资源部署与 API Key 配置。初始化完成后，可继续阅读 [第 2 节](#2-初始化与环境检测) 与 [第 6 节](#6-对话模式chat)。

---

## 1. 下载源码并编译构建

适合需要从源码运行、调试或参与开发的用户。

### 前置依赖

| 依赖 | 版本要求 | 用途 |
|------|----------|------|
| Rust | stable 1.70+ | 编译 |

### 构建

先获取仓库源码，再进入仓库中的 `tomcat/` 目录执行构建：

```bash
# 拉取源码后进入仓库中的 tomcat/ 目录
cd tomcat
cargo build --release
```

```bash
./target/release/tomcat --help
# 或直接加到 PATH：
export PATH="$PWD/target/release:$PATH"
tomcat --help
```

查看版本：

```bash
tomcat --version
# tomcat 0.1.1
```

### 工作目录

tomcat 默认将所有数据存放在 `~/.tomcat/`。可在 `tomcat.config.toml` 的 `[storage]` 段修改 `work_dir`。

子目录结构（由 `tomcat init` 或首次启动自动创建）：

```
~/.tomcat/
├── tomcat.config.toml                 # 主配置文件（tomcat init 生成）
├── models.toml                        # 模型清单（tomcat init 生成；增删模型只改这里）
├── agents/
│   └── main/
│       ├── agent/                 # 身份与凭据
│       ├── sessions/              # 会话 JSONL transcript
│       ├── logs/                  # 日志文件
│       └── audit/                 # 审计日志
├── workspace-main/                # agent_definition_dir：默认 agent 设计态目录
├── assets/
│   ├── .env                       # API Key 等敏感配置（0600 权限）
│   ├── .versions.json             # 内嵌资源 SHA-256 版本记录
│   ├── .lock                      # 并发写入保护锁
│   └── modules/                   # 内嵌资源自动释放（预留）
├── plugins/                       # 插件目录（Wasm 能力建设中，见第 9 节）
└── memory/                        # 向量检索索引
```

详见 [directory-structure.md](architecture/directory-structure.md)。

目录概念速记：`agent_workspace_dir` 是用户启动 `tomcat chat` 时的 shell 当前目录，是“当前目录 / 这个项目 / 相对路径”的语义来源，但不自动授权文件访问；`agent_definition_dir` 是 `workspace-<agentId>/` 设计态目录，也是默认可写根；`agent_trail_dir` 是 `agents/<agentId>/` 运行态目录。

---

## 2. 初始化与环境检测

### tomcat init — 三步向导

```bash
tomcat init
```

`tomcat init` 为**非交互式**三步流程（不再选择 Provider / 模型 / API Base URL；首次写入默认 `openai-responses` + `gpt-5.4`，与 `tomcat.config.toml.example` 及代码中 `DEFAULT_LLM_MODEL` 一致）：

1. **[1/3] 环境初始化**：写入 `~/.tomcat/tomcat.config.toml`（若尚不存在）、创建目录结构、生成模型清单 `~/.tomcat/models.toml`（含一条可用的 `mimo-v2.5-pro` 样板，见下）、释放内嵌资源（modules 等）、按 `$SHELL` 将 `export PATH="…"` 追加到 `~/.zshrc` / `~/.bash_profile` 或 `~/.bashrc` / `~/.profile`（带 `# Added by tomcat init` 标记；已存在同序 export 则跳过）
2. **[2/3] 资源检查**：与 `tomcat doctor` 相同的检查项（配置合法、内嵌资源、资源版本等），**不包含** `.env` 权限与 `OPENAI_API_KEY` 环境变量提示
3. **[3/3] API Key 配置**：若 `~/.tomcat/assets/.env` 中尚无有效 `OPENAI_API_KEY`，提示输入（可回车跳过）。**回车跳过不会创建或写入 `.env`**（避免留下空 Key 文件）；可稍后再次运行 `tomcat init` 输入 Key，或自行创建/编辑 `~/.tomcat/assets/.env`

预期输出（节选）：

```
[1/3] 环境初始化
  ✓ 配置文件已写入: ~/.tomcat/tomcat.config.toml
  ✓ 默认 LLM Provider: openai-responses
  ✓ 默认模型: gpt-5.4
  ✓ 目录结构就绪
  ✓ 已生成模型清单 models.toml（含 mimo-v2.5-pro）
  ✓ 内嵌资源已释放
  ✓ 已加入 PATH 环境变量

[2/3] 资源检查
✓ 配置合法 (~/.tomcat/tomcat.config.toml)
✓ 内嵌资源已就绪
  资源版本: modules=...

[3/3] API Key 配置
  ✓ API Key 已配置
  （或 ⚠ 未设置时的提示与 `.env` 路径）

初始化完成！运行 `tomcat chat` 开始对话。
```

若无法自动写入 shell 配置，会打印 `⚠ 无法自动配置 PATH` 及一行可手动执行的 `export PATH=...`。

**幂等性**：若 `~/.tomcat/tomcat.config.toml` **已存在**，默认**不覆盖**，仅打印保留说明并继续执行目录/资源/PATH/检查/API Key 步骤；第二次起可看到「使用已有配置文件」类提示。若要换新默认模型等，请用 `tomcat config set` 或自行编辑配置后重跑。

**与 `tomcat doctor`**：`init` 第二步与 `doctor` 共用同一套检查逻辑；配置与资源有疑义时可再运行 `tomcat doctor` 查看含 API Key / `.env` 的完整诊断。

### models.toml — 增删模型与 MiMo Token Plan

`tomcat init` 会生成 `~/.tomcat/models.toml`（**模型清单**）。这是「增删模型」的唯一入口：

- **启动自动加载**：`tomcat chat` 等启动路径会合并「内置模型表（gpt / deepseek）+ `models.toml`」，**同 id 覆盖内置、新 id 新增**，无需改代码、无需重新编译。
- **幂等生成**：`models.toml` 不存在则创建；已存在但缺 `mimo-v2.5-pro` 则**仅追加**；已含则原样不动——**绝不覆盖你已有的条目或注释**。重复 `tomcat init` 安全。
- **再加一个模型**：复制现有 `[[models]]` 段，改 `id` / `provider` / `base_url` 即可。

**MiMo Token Plan 最小配置**（init 默认就帮你写好一条）：

```toml
[[models]]
id = "mimo-v2.5-pro"
api = "openai"                                  # 走 OpenAI-compatible /v1/chat/completions
provider = "mimo"                               # 决定取 MIMO_API_KEY
base_url = "https://token-plan-cn.xiaomimimo.com"  # 只填 host，后缀程序拼接
thinking_format = "doubao"
context_window = 1000000
capabilities = { vision = false, files = false, tools = true, reasoning = true }
```

- **Key**：设置环境变量 `MIMO_API_KEY=tp-xxxxx`（写入 `~/.tomcat/assets/.env` 或当前 shell）。`tp-xxxxx`（Token Plan）不能与按量 `sk-xxxxx` 混用。
- **base_url**：只填 **host**，`/v1/chat/completions` 由程序拼接，**不要**写成完整 endpoint。host 以你订阅页显示的为准（CN / SGP / AMS 区域代号对应中国 / 新加坡 / 欧洲）。
- **使用**：`tomcat chat` 里 `/model mimo-v2.5-pro` 切换，或把 `[llm] default_model` 设成它。
- **能力边界**：`mimo-v2.5-pro` 官方仅文本（无图片/文件），故 `vision/files=false`；thinking 服务端默认开，本集成走豆包系 `thinking` 线格式。

### tomcat doctor — 环境诊断

```bash
tomcat doctor
```

doctor 逐项检查环境并给出可执行的修复建议：

| 检查项 | 通过示例 | 失败示例 |
|--------|---------|---------|
| 配置文件 | `✓ 配置合法 (~/.tomcat/tomcat.config.toml)` | `✗ 未找到配置文件` |
| 内嵌资源 | `✓ 内嵌资源已就绪` | `✗ 资源释放失败` |
| 资源版本 | `资源版本: modules=def456...` | — |
| .env 权限 | `✓ .env 权限: 0600` | `⚠ .env 权限: 0644（建议 0600）` |
| API Key | `✓ OPENAI_API_KEY 已设置` | `⚠ OPENAI_API_KEY 未设置` |

每个失败/警告项都会给出 `→ 运行 tomcat init 或...` 修复建议。Wasm / 插件相关检查见 [第 9 节](#9-wasm--wasmedge-与插件建设中)（能力建设中）。

---

## 3. 配置管理

`tomcat config` 提供对 `tomcat.config.toml` 的读写操作，无需手动编辑文件。

### 查看完整配置

```bash
tomcat config get
```

输出当前配置的完整 TOML 内容：

```toml
[log]
level = "warn"
file_enabled = true

[llm]
provider = "openai-responses"
api_key_env = "OPENAI_API_KEY"
default_model = "gpt-5.4"
...
```

### 查询单个配置项

使用点路径格式查询，支持所有 TOML 嵌套字段：

```bash
tomcat config get log.level
# warn

tomcat config get llm.default_model
# gpt-5.4

tomcat config get llm.proxy
# （空或当前代理地址）
```

如果字段不存在：

```
未找到配置项: nonexistent.key
  同级可用项: [...]
```

### 修改配置项

```bash
tomcat config set log.level debug
# 已设置 log.level = debug

tomcat config set llm.default_model gpt-4o
# 已设置 llm.default_model = gpt-4o

tomcat config set log.file_enabled true
# 已设置 log.file_enabled = true
```

值类型自动推断：整数、布尔（`true`/`false`）、字符串。修改后程序会对新配置做合法性校验。

### 启动预检与搜索工具安装

进入 `tomcat chat` 后，tomcat 会在后台探测 `search_files` 的快速实现依赖（`rg`/ripgrep 与 `fd`/`fdfind`）。若缺失，会按当前平台尝试后台安装；该流程不阻塞聊天，失败时 `search_files` 会自动使用进程内 Tier2 兜底（walkdir + globset + Rust regex）。默认情况下这些预检仍会执行，但 `[tools]` / `[git]` 终端提示默认**关闭**，避免打扰输入。

如需关闭后台安装尝试，可在配置中设置：

```toml
[preflight]
auto_install_search_tools = false
```

也可以通过配置命令修改：

```bash
tomcat config set preflight.auto_install_search_tools false
```

如果想单独打开终端提示而不影响后台预检功能，可设置：

```toml
[preflight]
show_search_tools_ui = true
show_git_ui = true
```

对应配置命令：

```bash
tomcat config set preflight.show_search_tools_ui true
tomcat config set preflight.show_git_ui true
```

CI 或一次性运行可用环境变量覆盖：

```bash
TOMCAT__PREFLIGHT__AUTO_INSTALL_SEARCH_TOOLS=false tomcat chat
```

### 用编辑器打开配置

```bash
tomcat config edit
# 使用 $EDITOR（默认 vi）打开配置文件，保存后自动重新校验
```

### 路径规则（pathrules）

除工作区根目录外，可用 `tomcat pathrules` 追加细粒度路径规则（写入 `primitive.path_rules`，与内置 deny/readonly 规则合并生效）：

```bash
tomcat pathrules add ~/.ssh --mode deny      # 拒绝读写
tomcat pathrules add ~/notes --mode readonly # 仅可读
tomcat pathrules list                      # 查看 builtin / user / session 三段规则
```

路径支持 `~` 前缀；目标不存在时仅警告，仍允许写入配置。

---

## 4. 会话管理

tomcat 将每次对话存为**会话**（session），以 JSONL transcript 格式持久化。默认会话的 key 为 `agent:main:main`。

### 创建新会话

```bash
tomcat session new
# 已创建会话: 1773299946709_70b6fc3c6c5723b2  agent:main:main
```

### 列出所有会话

```bash
tomcat session list
# key                    session_id                       updated_at
# agent:main:main     1773299946709_70b6fc3c6c5723b2   2026-03-10T...
```

无会话时输出：

```
当前无会话。使用 session new 创建。
```

### 切换会话

```bash
tomcat session switch agent:main:main
```

切换到不存在的会话时：

```
会话不存在: nonexistent-key
```

> MVP 说明：会话状态存储于当前进程内存，切换操作在下次启动 chat 时生效。

### 搜索会话

```bash
tomcat session search main
# 按 key 或 session_id 包含关键词过滤，输出匹配的会话列表
```

### 归档会话

```bash
tomcat session archive agent:main:main
# 已归档会话: agent:main:main
```

### 删除会话

```bash
tomcat session delete agent:main:main
# 已删除会话: agent:main:main
```

---

## 5. 工作区管理

tomcat 通过 `workspace` 子命令管理 **额外**可访问的外部目录根（与每个 agent 默认可写的设计态目录 `agent_definition_dir` 不同）。授权列表为**全局**配置，持久化在 `~/.tomcat/tomcat.config.toml` 的 `[workspace]` 表（字段 `workspace_roots`），**所有 agent 共用同一份列表**。启动 `tomcat chat` 时的当前目录不会自动加入该列表；需要访问当前项目时，可用 `tomcat workspace add --cwd`、对话中的 cwd lazy prompt 或 `/path <路径>` 授权命令显式加入。cwd lazy prompt 使用 `[s]` 本次会话允许、`[w]` 写入 `workspace_roots`、`[c]` 取消；输错选项会提示可选项并按取消处理，不会静默吞掉。若取消后工具再次因当前目录未授权失败，错误会提示下次触达 cwd 可重新选择 `[s]/[w]/[c]`，也可执行 `tomcat workspace add <路径>` 永久授权。旧的 `primitive.path_whitelist` 已删除，请使用 `workspace.workspace_roots` 或 `primitive.path_rules`。

### 添加工作区

```bash
tomcat workspace add /path/to/project
# 已添加工作区: /path/to/project
```

将**当前工作目录**加入授权列表（无需敲绝对路径）：

```bash
cd /path/to/project
tomcat workspace add --cwd
# 已添加工作区: /path/to/project
```

路径必须是已存在的目录；`tomcat workspace add` 须提供路径**或** `--cwd`。重复添加会去重。

### 列出已授权工作区

```bash
tomcat workspace list
# /path/to/project
# /another/project
```

### 移除工作区

```bash
tomcat workspace remove /path/to/project
# 已移除工作区: /path/to/project
```

---

## 6. 对话模式（chat）

`tomcat chat` 是核心功能，进入交互式 AI 对话界面，连接 LLM，支持流式输出与工具调用。

### 前提条件

确保已运行 `tomcat init` 并配置了 API Key。`tomcat init` 会将 Key 写入 `~/.tomcat/assets/.env`，启动时自动加载。

也可手动设置环境变量：

```bash
export OPENAI_API_KEY=sk-...
```

### 启动对话

```bash
tomcat chat
```

banner 输出样例：

```
tomcat 对话模式 (模型: gpt-5.4)
输入消息开始对话，Ctrl+D 退出，Ctrl+C 中断生成。
输入 /help 查看命令列表。

> 
```

### 启动画面（Splash 吉祥物）

在**交互式终端**下启动 `tomcat chat` 时，banner 上方会先绘制像素风吉祥物「Tommy」（4 帧 idle 动画，播放一次后定格），随后照常打印上面的文本 banner。管道 / 重定向 / CI 等**非 TTY** 场景自动跳过绘制。

```toml
# ~/.tomcat/tomcat.config.toml
[splash]
enabled = true       # 关闭吉祥物：false
animations = true    # 只显示静态首帧：false
max_width = 56       # 居中参考宽度上限
```

环境变量 `TOMCAT_SPLASH=0` 可临时关闭；`NO_COLOR` 去除颜色转义。读屏器用户建议设 `animations = false` 或 `enabled = false`。

逐字流式输出 AI 回复：

```
u> 你好，介绍一下你自己

agent.main> 你好！我是一个通过 API 访问的 AI 助手，可以用中文或英文和你交流...
```

退出时：

```
再见！
```

### 本地命令

`tomcat chat` 会在消息发送给 LLM 前识别以 `/` 开头的本地命令（完整列表以 `/help` 为准）：

| 命令 | 说明 |
|------|------|
| `/help` | 显示当前支持的本地命令 |
| `/path <绝对路径>` | 为单个路径打开授权菜单（本次会话 / 写入工作区 / 只读 / 禁止 / 取消） |
| `/model`（`current` / `list` / `use <id>`） | 查看或切换当前会话模型（catalog 来自内置表 + `models.toml`） |
| `/thinking`（`minimal` / `summary` / `full` / `toggle`） | 切换 Thinking 展示档位（缺省为 toggle） |
| `/ckpt list [--limit N]` | 列出最近 checkpoint |
| `/ckpt show <id>` / `/ckpt diff <id>` | 查看 checkpoint 元数据或与工作区差异 |
| `/restore <id> [--path <rel>]... [--dry-run]` | 从 checkpoint 恢复 |
| `/plan` | 进入 PLAN 规划模式（计划落盘 `~/.tomcat/plans/`） |
| `/plan exit` | 退回 Chat 模式 |
| `/plan build <plan_id或路径>` | 进入 EXEC 执行模式 |
| `/plan list` | 列出 `~/.tomcat/plans/` 下计划文件 |

直接拖拽或粘贴路径后回车不会触发授权菜单，会按普通聊天消息发送给 LLM。需要显式授权路径时，请输入 `/path <绝对路径>`。

#### Checkpoint 启动目录建议

`tomcat chat` 的 checkpoint 会把**启动时当前目录**当成工作区根。也就是说，如果你在一个塞了多个项目的共享目录里启动，checkpoint 每回合都会把整棵大树当成扫描范围。

实测（2026-06-09，本机 `~/.tomcat/temp`）：

- 共享目录 `~/.tomcat/temp`：约 `33031` 个文件，单次 `os.walk` 约 `0.313s`
- 单项目目录 `~/.tomcat/temp/iron-vanguard`：约 `3026` 个文件，单次 `os.walk` 约 `0.053s`

因此更稳妥的做法是：**先进入具体项目目录，再启动 `tomcat chat`**。

```bash
cd ~/.tomcat/temp/iron-vanguard
tomcat chat
```

如果你在 `~/.tomcat/temp` 这类共享目录里直接启动，即使现在已经加入了超时退避和噪音目录排除，checkpoint 仍然会盯住整棵共享树，卡顿概率会明显更高。

### 快捷键

| 按键 | 动作 |
|------|------|
| `Ctrl+D` | 结束输入，正常退出对话模式 |
| `Ctrl+C` | 中断当前生成，可继续输入下一条消息 |
| `↑` / `↓` | 历史输入切换 |

### 恢复上次会话（--resume）

```bash
tomcat chat --resume
# 恢复会话: agent:main:main
# tomcat 对话模式 (模型: gpt-5.4)
# ...
```

会从 JSONL transcript 加载历史消息，LLM 会拥有之前对话的上下文。

### 工具调用

在对话中，LLM 通过内置工具与宿主交互（Agent 循环自动多轮，轮次上限见配置）。常用工具如下（名称以 catalog 为准；旧名如 `read_file` 已不再接受）：

| 工具 | 说明 |
|------|------|
| `read` | 读取文件（可选 hashline 模式，配合 `hashline_edit`） |
| `write` | 创建或覆盖文件 |
| `edit` | 对已有文件做精确替换（支持多段 `edits`） |
| `bash` | 执行 shell 命令（可 `run_in_background` + `task_output` / `task_stop` / `task_list`） |
| `list_dir` | 列出目录 |
| `search_files` | 在授权路径内搜索文件（优先 ripgrep/fd，缺失时进程内兜底） |

此外还有 `create_plan`、`update_plan`、`todos`、`web_search`、`web_fetch` 等，按模型与配置启用。

**用户确认提示**（当 `require_approval_for_all_write = true` 时）：

```
[工具调用] write: /path/to/file
内容预览: ...
是否执行？[y/N]
```

按 `y` 确认执行，按 `N` 或直接回车拒绝。

> 可在 `tomcat.config.toml` 的 `[primitive]` 段调整确认策略：
> - `auto_confirm = true`：白名单内自动确认，无需交互
> - `require_approval_for_all_write = false`：写操作不强制询问

### 无 API Key 时的行为

若未设置 `OPENAI_API_KEY`，进入 chat 后会快速失败并输出错误提示（不会挂起）：

```bash
tomcat chat
# Error: LLM 配置错误：...（含 key/API/失败 等关键词）
```

---

## 7. 审计日志

tomcat 将 4 原语操作、工具调用等记录到**独立审计日志**，便于事后排查。审计日志仅追加、不可篡改，与业务日志分离。（插件相关审计类型见 [第 9 节](#9-wasm--wasmedge-与插件建设中)，能力建设中。）

**说明**：当前审计日志为明文存储；加密存储为后续 TODO。

### 审计日志配置

审计日志**默认开启**（`security.enable_audit_log = true`），无需额外配置。如需关闭可设为 `false`：

```bash
tomcat config set security.enable_audit_log false
# 可选：调整保留天数（默认 90）
tomcat config set security.audit_log_retention_days 90
```

审计文件存放于 `~/.tomcat/agents/main/audit/audit.jsonl`（专用 JSONL，仅追加）。

### 查看审计记录列表

```bash
tomcat audit list
# 或指定条数（默认 50）：
tomcat audit list --limit 20
```

输出格式（有记录时）：

```
#   时间                    类型          状态   详情
0   2026-03-10T10:00:00Z    primitive     ok     operation=Read path=/home/user/...
1   2026-03-10T10:00:05Z    tool_call     ok     tool_name=echo plugin_id=...
2   2026-03-10T10:00:10Z    hostcall      ok     module=fs method=readFile ...

共 3 条
```

无记录时给出友好提示。执行 `tomcat audit list` 时会按配置自动清理过期记录。

### 审计记录类型

| 类型 | 触发场景 |
|------|----------|
| `primitive` | LLM 调用内置原语（如 read / write / edit / bash） |
| `tool_call` | 插件或 LLM 通过工具注册表调用 tool |
| `hostcall` | 插件 JS 通过 `__pi_host_call` 调用宿主 API |
| `plugin_lifecycle` | 插件 load / enable / disable / unload |

### 查看单条审计记录

```bash
tomcat audit show 0
# 显示序号为 0 的记录的完整详情
```

序号不存在时：

```
未找到审计记录: 0
```

### 导出审计记录

```bash
tomcat audit export /tmp/audit_backup.json
# 已导出 N 条审计记录到 /tmp/audit_backup.json
```

导出为 JSON 数组格式，可用 `jq` 进一步处理：

```bash
jq '.[0]' /tmp/audit_backup.json
```

---

## 8. 附录

### 环境变量速查

| 变量名 | 是否必填 | 说明 |
|--------|----------|------|
| `OPENAI_API_KEY` | 必填（chat/LLM） | LLM API 密钥 |
| `HTTPS_PROXY` | 可选 | 全局 HTTPS 代理（curl 兼容格式）|
| `HTTP_PROXY` | 可选 | 全局 HTTP 代理 |
| `TOMCAT__LLM__PROXY` | 可选 | 仅 LLM 请求使用的代理，覆盖 config |
| `TOMCAT__LLM__API_BASE_FALLBACK` | 可选 | 主 API 不通时的备用 base URL |
| `TOMCAT__LLM__DEFAULT_MODEL` | 可选 | 覆盖 `[llm] default_model`；未设置时以配置文件 / 代码默认 `gpt-5.4` 为准 |

> LLM 相关的 `TOMCAT__LLM__*` 变量可覆盖 `tomcat.config.toml` 中对应字段，`__` 作为嵌套分隔符。仓库与安装包**不会**默认注入这些变量；若本机 shell 里长期设置了旧模型 id，会导致与 `tomcat init` 新写入的 toml 不一致。

### 常见问题

**Q: `tomcat chat` 启动后立即报错退出**

原因：未设置 `OPENAI_API_KEY`，或 key 无效。

```bash
# 检查是否已加载：
echo $OPENAI_API_KEY

# 加载 .env：
set -a && source .env && set +a
tomcat chat
```

---

**Q: `tomcat chat` 连接 OpenAI 超时（curl exit 28）**

原因：当前网络无法直连 `api.openai.com`。

```bash
# 在 .env 中配置代理：
HTTPS_PROXY=http://127.0.0.1:7890
# 确保本机代理进程已启动，然后：
set -a && source .env && set +a
tomcat chat
```

可用 `scripts/verify-openai-apis.sh` 验证连通性：

```bash
./scripts/verify-openai-apis.sh 1 2 3
# [PASS] GET /v1/models - HTTP 200
# ...
```

---

### 相关文档

| 文档 | 内容 |
|------|------|
| [README.md](../README.md) | 项目简介、快速开始 |
| [Architecture.md](openspec/specs/Architecture.md) | 系统架构与分层设计 |
| [src/infra/README.md](../src/infra/README.md) | 基础设施层（配置/日志/审计/事件总线）|
| [src/core/llm/README.md](../src/core/llm/README.md) | LLM 模块（OpenAI 适配器、流式输出）|
| [src/core/session/README.md](../src/core/session/README.md) | 会话管理与 CLI 设计 |
| [src/core/README.md](../src/core/README.md) | Agent 循环（多轮对话、工具调用、重试）|
| [src/api/README.md](../src/api/README.md) | CLI / chat / render 入口层 |
| [INTEGRATION_TEST_LOGGING.md](openspec/specs/guides/testing/INTEGRATION_TEST_LOGGING.md) | 集成测试日志查看方法 |

---

## 9. Wasm / WasmEdge 与插件（建设中）

> **状态**：Wasm 沙箱插件与 WasmEdge 运行时集成**尚未完成产品化建设**。默认 `cargo build --release` 为 no-wasm 构建，`tomcat plugin *` 与 `doctor` 中的 WasmEdge / QuickJS 检查仅在与 `--features wasmedge` 或 `standalone` 编译时才有意义。下文为规划中的操作预览，供后续验收参考。

### 构建与依赖（预览）

| 依赖 | 版本要求 | 用途 |
|------|----------|------|
| WasmEdge C 库 | 0.13.5 | `--features wasmedge`（`scripts/install-wasmedge.sh`） |
| CMake + C 编译器 | 任意 | `--features standalone`（构建时自动链接 WasmEdge） |

```bash
cd tomcat
# cargo build --release --features wasmedge
# cargo build --release --features standalone
bash scripts/install-wasmedge.sh -y
source $HOME/.wasmedge/env
```

`tomcat doctor` 在启用 Wasm feature 时会检查 QuickJS wasm（`~/.tomcat/assets/wasm/wasmedge_quickjs.wasm`）与 WasmEdge 运行时；默认构建会提示「当前构建未启用 Wasm/插件能力」，属预期。

### 插件 CLI（预览）

tomcat 规划支持 Wasm 沙箱插件（`plugin.json` + `main.js`）。注册信息写入 `{work_dir}/plugins/registry.json`；进程内需 `plugin load` 载入 VM。

**最小插件示例**

`plugin.json`：

```json
{
  "id": "my-plugin",
  "name": "My Plugin",
  "version": "0.1.0",
  "description": "体验用最小插件",
  "author": "me",
  "main": "main.js",
  "requiredPermissions": [],
  "requiredApiVersion": "1.0",
  "tags": []
}
```

`main.js`：

```js
pi.log("my-plugin: 已加载");
```

```bash
tomcat plugin load ~/tomcat-plugins/my-plugin
tomcat plugin list
tomcat plugin info my-plugin
tomcat plugin disable my-plugin
tomcat plugin enable my-plugin
tomcat plugin unload my-plugin
```

实现细节见 [src/ext/README.md](../src/ext/README.md)。
