# pi-rust-wasm 操作说明

本文档面向第一次使用 pi 的用户，按"前置准备 → 构建 → 逐功能体验"顺序，覆盖 pi 的全部可用功能。

---

## 目录

- [0. 前置准备](#0-前置准备)
- [1. 构建](#1-构建)
- [2. 初始化与环境检测](#2-初始化与环境检测)
- [3. 配置管理](#3-配置管理)
- [4. 会话管理](#4-会话管理)
- [5. 对话模式（chat）](#5-对话模式chat)
- [6. 插件管理](#6-插件管理)
- [7. 审计日志](#7-审计日志)
- [8. 附录](#8-附录)

---

## 0. 前置准备

### 用户安装（预编译二进制）

预编译二进制已内嵌 WasmEdge 运行时、QuickJS wasm 和 Node.js 兼容模块。下载后只需两步：

```bash
# 1. 下载并放入 PATH
chmod +x pi && mv pi /usr/local/bin/

# 2. 初始化
pi init
```

`pi init` 会通过交互式向导完成 LLM 配置、API Key 输入和资源部署。

### 开发者编译

| 依赖 | 版本要求 | 用途 |
|------|----------|------|
| Rust | stable 1.70+ | 编译 |
| WasmEdge C 库 | 0.13.5 | 默认需要（通过 `install-wasmedge.sh` 安装） |
| CMake + C 编译器 | 任意 | 仅 `--features standalone` 模式需要（自动下载并链接 WasmEdge） |

```bash
cd pi-rust-wasm
cargo build --release          # 默认使用系统已安装的 WasmEdge，编译快
# 或自动下载并链接 WasmEdge（无需预装，但首次编译慢）：
# cargo build --release --features standalone
```

### 工作目录

pi 默认将所有数据存放在 `~/.pi_/`。可在 `pi.config.toml` 的 `[storage]` 段修改 `work_dir`。

子目录结构（由 `pi init` 或首次启动自动创建）：

```
~/.pi_/
├── pi.config.toml                 # 主配置文件（pi init 生成）
├── agents/
│   └── main/
│       ├── agent/                 # 身份与凭据
│       ├── sessions/              # 会话 JSONL transcript
│       ├── logs/                  # 日志文件
│       └── audit/                 # 审计日志
├── workspace-main/                # 默认 agent 工作区
├── assets/
│   ├── .env                       # API Key 等敏感配置（0600 权限）
│   ├── .versions.json             # 内嵌资源 SHA-256 版本记录
│   ├── .lock                      # 并发写入保护锁
│   ├── wasm/
│   │   └── wasmedge_quickjs.wasm  # 内嵌资源自动释放
│   └── modules/                   # Node.js 兼容模块（内嵌资源自动释放）
├── plugins/                       # 全局共享插件
└── memory/                        # 向量检索索引
```

详见 [directory-structure.md](technical/directory-structure.md)。

---

## 1. 构建

```bash
cd pi-rust-wasm
cargo build --release
```

编译成功后，可执行文件位于 `./target/release/pi`。

```bash
./target/release/pi --help
# 或直接加到 PATH：
export PATH="$PWD/target/release:$PATH"
pi --help
```

查看版本：

```bash
pi --version
# pi 0.1.0
```

---

## 2. 初始化与环境检测

### pi init — 交互式向导

```bash
pi init
```

`pi init` 是两步交互式向导：

1. **[1/2] LLM 配置**：选择 Provider（openai/azure/anthropic/custom） → 输入 API Key → 选择默认模型
2. **[2/2] 资源检查**：自动创建目录结构、释放内嵌资源（wasm + modules）、写入 `.env`

预期输出：

```
[1/2] 选择 LLM Provider: openai
  默认模型: gpt-4.1-mini
  API Base URL（回车使用默认）:
✓ 配置文件已写入: ~/.pi_/pi.config.toml

[2/2] 资源检查
  ✓ 目录结构就绪
  ✓ 内嵌资源已释放（wasm + modules）
  ✓ API Key 已写入 .env

初始化完成！运行 `pi doctor` 验证环境。

提示：若 pi 不在 PATH 中，请执行：
  export PATH="/path/to/pi/bin:$PATH"
```

**幂等性**：二次运行 `pi init` 会询问「配置文件已存在，是否覆盖？」（默认否），选择否则仅刷新资源，不影响已有配置和 API Key。

**旧目录迁移**：若检测到 `~/.pi_wasm/`（旧版工作目录），会提示迁移到 `~/.pi_/`。

### pi doctor — 环境诊断

```bash
pi doctor
```

doctor 逐项检查环境并给出可执行的修复建议：

| 检查项 | 通过示例 | 失败示例 |
|--------|---------|---------|
| 配置文件 | `✓ 配置合法 (~/.pi_/pi.config.toml)` | `✗ 未找到配置文件` |
| 内嵌资源 | `✓ 内嵌资源已就绪` | `✗ 资源释放失败` |
| QuickJS wasm | `✓ QuickJS wasm：~/.pi_/assets/wasm/...` | `✗ QuickJS wasm 未找到` |
| WasmEdge 运行时 | `✓ WasmEdge 运行时：可用` | `✗ WasmEdge 运行时：不可用` |
| 资源版本 | `资源版本: wasm=abc123... modules=def456...` | — |
| .env 权限 | `✓ .env 权限: 0600` | `⚠ .env 权限: 0644（建议 0600）` |
| API Key | `✓ OPENAI_API_KEY 已设置` | `⚠ OPENAI_API_KEY 未设置` |

每个失败/警告项都会给出 `→ 运行 pi init 或...` 修复建议。

---

## 3. 配置管理

`pi config` 提供对 `pi.config.toml` 的读写操作，无需手动编辑文件。

### 查看完整配置

```bash
pi config get
```

输出当前配置的完整 TOML 内容：

```toml
[log]
level = "info"
file_enabled = true
file_roll_size_mb = 10

[llm]
provider = "openai"
api_key_env = "OPENAI_API_KEY"
default_model = "gpt-5.2"
...
```

### 查询单个配置项

使用点路径格式查询，支持所有 TOML 嵌套字段：

```bash
pi config get log.level
# info

pi config get llm.default_model
# gpt-5.2

pi config get llm.proxy
# （空或当前代理地址）
```

如果字段不存在：

```
未找到配置项: nonexistent.key
  同级可用项: [...]
```

### 修改配置项

```bash
pi config set log.level debug
# 已设置 log.level = debug

pi config set llm.default_model gpt-4o
# 已设置 llm.default_model = gpt-4o

pi config set log.file_enabled true
# 已设置 log.file_enabled = true
```

值类型自动推断：整数、布尔（`true`/`false`）、字符串。修改后程序会对新配置做合法性校验。

### 用编辑器打开配置

```bash
pi config edit
# 使用 $EDITOR（默认 vi）打开配置文件，保存后自动重新校验
```

---

## 4. 会话管理

pi 将每次对话存为**会话**（session），以 JSONL transcript 格式持久化。默认会话的 key 为 `agent:main:main`。

### 创建新会话

```bash
pi session new
# 已创建会话: 1773299946709_70b6fc3c6c5723b2  agent:main:main
```

### 列出所有会话

```bash
pi session list
# key                    session_id                       updated_at
# agent:main:main     1773299946709_70b6fc3c6c5723b2   2026-03-10T...
```

无会话时输出：

```
当前无会话。使用 session new 创建。
```

### 切换会话

```bash
pi session switch agent:main:main
```

切换到不存在的会话时：

```
会话不存在: nonexistent-key
```

> MVP 说明：会话状态存储于当前进程内存，切换操作在下次启动 chat 时生效。

### 搜索会话

```bash
pi session search main
# 按 key 或 session_id 包含关键词过滤，输出匹配的会话列表
```

### 归档会话

```bash
pi session archive agent:main:main
# 已归档会话: agent:main:main
```

### 删除会话

```bash
pi session delete agent:main:main
# 已删除会话: agent:main:main
```

---

## 5. 工作区管理

pi 通过 `workspace` 子命令管理 agent 被授权访问的外部目录。授权列表持久化在 `{agent_dir}/ext_workspaces.json`。

### 添加工作区

```bash
pi workspace add /path/to/project
# 已添加工作区: /path/to/project
```

路径必须是已存在的目录，重复添加会去重。

### 列出已授权工作区

```bash
pi workspace list
# /path/to/project
# /another/project
```

### 移除工作区

```bash
pi workspace remove /path/to/project
# 已移除工作区: /path/to/project
```

---

## 6. 对话模式（chat）

`pi chat` 是核心功能，进入交互式 AI 对话界面，连接 LLM，支持流式输出与工具调用。

### 前提条件

确保已运行 `pi init` 并配置了 API Key。`pi init` 会将 Key 写入 `~/.pi_/assets/.env`，启动时自动加载。

也可手动设置环境变量：

```bash
export OPENAI_API_KEY=sk-...
```

### 启动对话

```bash
pi chat
```

banner 输出样例：

```
pi 对话模式 (模型: gpt-5.2)
输入消息开始对话，Ctrl+D 退出，Ctrl+C 中断生成。

> 
```

逐字流式输出 AI 回复：

```
u> 你好，介绍一下你自己

pi.main> 你好！我是一个通过 API 访问的 AI 助手，可以用中文或英文和你交流...
```

退出时：

```
再见！
```

### 快捷键

| 按键 | 动作 |
|------|------|
| `Ctrl+D` | 结束输入，正常退出对话模式 |
| `Ctrl+C` | 中断当前生成，可继续输入下一条消息 |
| `↑` / `↓` | 历史输入切换 |

### 恢复上次会话（--resume）

```bash
pi chat --resume
# 恢复会话: agent:main:main
# pi 对话模式 (模型: gpt-5.2)
# ...
```

会从 JSONL transcript 加载历史消息，LLM 会拥有之前对话的上下文。

### 工具调用（4 原语）

在对话中，LLM 可以通过工具调用执行以下操作（最多 10 轮自动循环）：

| 工具 | 说明 |
|------|------|
| `read_file` | 读取文件内容 |
| `write_file` | 写入文件 |
| `edit_file` | 对文件做字符串替换 |
| `execute_bash` | 执行 shell 命令 |
| `list_dir` | 列出目录内容 |

**用户确认提示**（当 `require_approval_for_all_write = true` 时）：

```
[工具调用] write_file: /path/to/file
内容预览: ...
是否执行？[y/N]
```

按 `y` 确认执行，按 `N` 或直接回车拒绝。

> 可在 `pi.config.toml` 的 `[primitive]` 段调整确认策略：
> - `auto_confirm = true`：白名单内自动确认，无需交互
> - `require_approval_for_all_write = false`：写操作不强制询问

### 无 API Key 时的行为

若未设置 `OPENAI_API_KEY`，进入 chat 后会快速失败并输出错误提示（不会挂起）：

```bash
pi chat
# Error: LLM 配置错误：...（含 key/API/失败 等关键词）
```

---

## 7. 插件管理

pi 支持加载 pi-mono 风格插件（`plugin.json` + `main.js`），在沙箱隔离环境中运行。插件加载/卸载信息自动持久化到 `{work_dir}/plugins/registry.json`，重启后可查看历史注册的插件。

> 依赖：插件的 JS 执行依赖 `wasmedge_quickjs.wasm`，请参考 [第 2 节](#2-初始化与环境检测) 完成配置。不配置 QuickJS wasm 时，`plugin load` 会报错。

### 构造最小插件

创建一个目录，包含以下两个文件：

**plugin.json**

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

**main.js**

```js
// 插件初始化脚本
pi.log("my-plugin: 已加载");
1 + 1;  // 合法的 JS，返回值无要求
```

```bash
mkdir ~/pi-plugins/my-plugin
# 写入上述两个文件到该目录
```

### 加载插件

```bash
pi plugin load ~/pi-plugins/my-plugin
```

加载时会弹出权限授权提示（`requiredPermissions` 为空时提示如下）：

```
插件 My Plugin my-plugin (v0.1.0) 请求以下权限: []
是否授权？[y/N]
```

输入 `y` 授权后：

```
插件加载成功: my-plugin
ID:      my-plugin
名称:    My Plugin
版本:    0.1.0
描述:    体验用最小插件
作者:    me
状态:    enabled
权限:    []
```

输入 `N` 或回车拒绝时：

```
插件加载失败: 权限错误: 用户拒绝插件授权
```

> 插件状态存储于进程内存，程序退出后不持久化（MVP 已知限制）。

### 查看已加载插件列表

```bash
pi plugin list
```

有插件时：

```
ID             名称         版本    状态
my-plugin      My Plugin    0.1.0   enabled
```

无插件时：

```
当前无已加载插件。
```

### 查看插件详情

```bash
pi plugin info my-plugin
```

```
ID:              my-plugin
名称:            My Plugin
版本:            0.1.0
描述:            体验用最小插件
作者:            me
状态:            enabled
所需权限:        []
API 版本要求:    1.0
注册工具:        []
加载时间:        2026-03-10T...
```

未找到时：

```
插件未找到: my-plugin
```

### 禁用与启用插件

```bash
pi plugin disable my-plugin
# 已禁用插件: my-plugin

pi plugin enable my-plugin
# 已启用插件: my-plugin
```

### 卸载插件

```bash
pi plugin unload my-plugin
# 已卸载插件: my-plugin
```

卸载后 `pi plugin list` 中不再出现该插件。

---

## 8. 审计日志

pi 将所有 4 原语操作、工具调用、插件 hostcall 与插件生命周期（加载/启用/禁用/卸载）记录到**独立审计日志**，便于事后排查。审计日志仅追加、不可篡改，与业务日志分离。

**说明**：当前审计日志为明文存储；加密存储为后续 TODO。

### 审计日志配置

审计日志**默认开启**（`security.enable_audit_log = true`），无需额外配置。如需关闭可设为 `false`：

```bash
pi config set security.enable_audit_log false
# 可选：调整保留天数（默认 90）
pi config set security.audit_log_retention_days 90
```

审计文件存放于 `~/.pi_/agents/main/audit/audit.jsonl`（专用 JSONL，仅追加）。

### 查看审计记录列表

```bash
pi audit list
# 或指定条数（默认 50）：
pi audit list --limit 20
```

输出格式（有记录时）：

```
#   时间                    类型          状态   详情
0   2026-03-10T10:00:00Z    primitive     ok     operation=Read path=/home/user/...
1   2026-03-10T10:00:05Z    tool_call     ok     tool_name=echo plugin_id=...
2   2026-03-10T10:00:10Z    hostcall      ok     module=fs method=readFile ...

共 3 条
```

无记录时给出友好提示。执行 `pi audit list` 时会按配置自动清理过期记录。

### 审计记录类型

| 类型 | 触发场景 |
|------|----------|
| `primitive` | LLM 调用 read_file / write_file / edit_file / execute_bash |
| `tool_call` | 插件或 LLM 通过工具注册表调用 tool |
| `hostcall` | 插件 JS 通过 `__pi_host_call` 调用宿主 API |
| `plugin_lifecycle` | 插件 load / enable / disable / unload |

### 查看单条审计记录

```bash
pi audit show 0
# 显示序号为 0 的记录的完整详情
```

序号不存在时：

```
未找到审计记录: 0
```

### 导出审计记录

```bash
pi audit export /tmp/audit_backup.json
# 已导出 N 条审计记录到 /tmp/audit_backup.json
```

导出为 JSON 数组格式，可用 `jq` 进一步处理：

```bash
jq '.[0]' /tmp/audit_backup.json
```

---

## 9. 附录

### 环境变量速查

| 变量名 | 是否必填 | 说明 |
|--------|----------|------|
| `OPENAI_API_KEY` | 必填（chat/LLM） | LLM API 密钥 |
| `HTTPS_PROXY` | 可选 | 全局 HTTPS 代理（curl 兼容格式）|
| `HTTP_PROXY` | 可选 | 全局 HTTP 代理 |
| `PI_WASM__LLM__PROXY` | 可选 | 仅 LLM 请求使用的代理，覆盖 config |
| `PI_WASM__LLM__API_BASE_FALLBACK` | 可选 | 主 API 不通时的备用 base URL |

> LLM 相关的 `PI_WASM__LLM__*` 变量可覆盖 `pi.config.toml` 中对应字段，`__` 作为嵌套分隔符。

### 常见问题

**Q: `pi chat` 启动后立即报错退出**

原因：未设置 `OPENAI_API_KEY`，或 key 无效。

```bash
# 检查是否已加载：
echo $OPENAI_API_KEY

# 加载 .env：
set -a && source .env && set +a
pi chat
```

---

**Q: `pi chat` 连接 OpenAI 超时（curl exit 28）**

原因：当前网络无法直连 `api.openai.com`。

```bash
# 在 .env 中配置代理：
HTTPS_PROXY=http://127.0.0.1:7890
# 确保本机代理进程已启动，然后：
set -a && source .env && set +a
pi chat
```

可用 `scripts/verify-openai-apis.sh` 验证连通性：

```bash
./scripts/verify-openai-apis.sh 1 2 3
# [PASS] GET /v1/models - HTTP 200
# ...
```

---

**Q: `pi doctor` 显示 QuickJS wasm 未找到**

预编译二进制已内嵌 QuickJS wasm，正常情况下 `pi init` 会自动释放。若仍缺失：

```bash
pi init   # 重新运行 init 触发资源释放
```

---

**Q: WasmEdge 运行时不可用**

默认编译使用系统已安装的 WasmEdge C 库，需先安装：

```bash
bash scripts/install-wasmedge.sh -y
source $HOME/.wasmedge/env
pi doctor  # 应显示 ✓ WasmEdge 运行时：可用
```

如需自动下载并链接 WasmEdge（无需预装），可使用 `--features standalone` 编译：

```bash
cargo build --release --features standalone
```

---

**Q: 从 `~/.pi_wasm/` 旧版本升级**

运行 `pi init`，向导会自动检测旧目录并提示迁移到 `~/.pi_/`。

---

### 相关文档

| 文档 | 内容 |
|------|------|
| [README.md](../README.md) | 项目简介、快速开始 |
| [Architecture.md](../openspec/specs/Architecture.md) | 系统架构与分层设计 |
| [docs/technical/01-infrastructure.md](technical/01-infrastructure.md) | 基础设施层（配置/日志/审计/事件总线）|
| [docs/technical/02-llm-module.md](technical/02-llm-module.md) | LLM 模块（OpenAI 适配器、流式输出）|
| [docs/technical/02-session-and-cli.md](technical/02-session-and-cli.md) | 会话管理与 CLI 设计 |
| [docs/technical/02-wasm-runtime-and-plugin.md](technical/02-wasm-runtime-and-plugin.md) | Wasm 运行时与插件系统 |
| [docs/technical/03-agent-loop.md](technical/03-agent-loop.md) | Agent 循环（多轮对话、工具调用、重试）|
| [openspec/specs/guides/testing/INTEGRATION_TEST_LOGGING.md](../openspec/specs/guides/testing/INTEGRATION_TEST_LOGGING.md) | 集成测试日志查看方法 |
