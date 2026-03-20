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

### 依赖清单

| 依赖 | 版本要求 | 用途 |
|------|----------|------|
| Rust | stable 1.70+ | 编译 |
| WasmEdge C 库 | 0.13.5 | Wasm 运行时（插件与 QuickJS 引擎） |
| `jq` | 任意版本（可选） | `verify-openai-apis.sh` 输出格式化 |

### 安装 WasmEdge

```bash
cd pi-rust-wasm
bash scripts/install-wasmedge.sh -y
source $HOME/.wasmedge/env
```

验证安装：

```bash
wasmedge --version
# 应输出: wasmedge version 0.13.x
```

### 配置 .env

```bash
cp .env.example .env
```

打开 `.env`，至少填写以下两项：

```env
OPENAI_API_KEY=sk-...         # 必填，LLM 调用凭证
HTTPS_PROXY=http://127.0.0.1:7890  # 若需要代理才能访问 OpenAI，取消注释并填写
```

> 说明：`OPENAI_API_KEY` 在运行 `pi chat` 以及 LLM 集成测试时是必需的；不填则 chat 进入后会立即报错退出。

### 工作目录

pi 默认将所有数据存放在 `~/.pi_wasm/`。可在 `config.toml` 的 `[storage]` 段或通过环境变量 `PI_WASM__STORAGE__WORK_DIR` 覆盖：

```bash
export PI_WASM__STORAGE__WORK_DIR=/your/custom/path
```

子目录结构（由程序自动创建）：

```
~/.pi_wasm/
├── agent/
│   └── config.toml          # 配置文件（pi init 生成）
├── agents/
│   └── default/
│       ├── sessions/        # 会话 JSONL transcript
│       ├── plugins/         # 插件目录
│       ├── logs/            # 日志文件（file_enabled=true 时）
│       └── tmp/
└── wasm/
    └── wasmedge_quickjs.wasm  # QuickJS 运行时（可手动放置）
```

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

### pi init — 生成配置文件

```bash
pi init
```

默认在 `~/.pi_wasm/agent_default/config.toml` 生成初始配置文件。也可以指定自定义路径：

```bash
pi init --config /tmp/my-pi/config.toml
```

预期输出：

```
已生成配置文件: ~/.pi_wasm/agent_default/config.toml
请编辑 ~/.pi_wasm/agent_default/config.toml 填写 LLM API 与安全策略。
```

生成后**至少需要**在配置文件中确认 `[llm]` 段的 `api_key_env` 指向你的环境变量名（默认已配置为 `OPENAI_API_KEY`）。

### pi doctor — 环境检测

```bash
pi doctor
```

doctor 会逐项检查以下内容并给出修复建议：

| 检查项 | 通过示例 | 失败示例 |
|--------|---------|---------|
| 配置文件 | `✓ 配置合法` | `未找到配置文件。请先运行: pi init` |
| WasmEdge 运行时 | `✓ WasmEdge 运行时：可用` | `✗ WasmEdge 运行时：不可用` |
| QuickJS wasm 路径 | `✓ QuickJS 运行时：可用 (/path/to/wasmedge_quickjs.wasm)` | `✗ QuickJS 路径未配置` |

典型输出（初始化后，未放置 QuickJS wasm 时）：

```
✓ 配置合法
✓ WasmEdge 运行时：可用
✗ QuickJS 路径未配置
  修复建议：下载 wasmedge_quickjs.wasm 到 work_dir/wasm/ 或设置环境变量 WASMEDGE_QUICKJS_PATH
```

> QuickJS wasm 由 `scripts/build-custom-quickjs.sh` 构建，或手动复制 `assets/wasm/wasmedge_quickjs.wasm` 到 `~/.pi_wasm/wasm/wasmedge_quickjs.wasm`。插件功能依赖此文件；chat 与其他 CLI 功能不依赖。

也可针对特定配置文件运行：

```bash
pi doctor --config /tmp/my-pi/config.toml
```

---

## 3. 配置管理

`pi config` 提供对 `config.toml` 的读写、导入导出操作，无需手动编辑文件。

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

### 导出配置

将当前配置导出为文件（可用于备份或迁移）：

```bash
pi config export /tmp/pi_cfg_backup.toml
# 已导出到 /tmp/pi_cfg_backup.toml
```

### 导入配置

从文件导入配置（会做格式校验）：

```bash
pi config import /tmp/pi_cfg_backup.toml
# 已从 /tmp/pi_cfg_backup.toml 导入（当前仅校验格式，未写入默认路径）
```

> MVP 说明：导入目前只校验格式，不会覆盖默认配置路径。如需应用，结合 `pi config set` 或直接编辑文件。

---

## 4. 会话管理

pi 将每次对话存为**会话**（session），以 JSONL transcript 格式持久化。默认会话的 key 为 `agent:default:main`。

### 创建新会话

```bash
pi session new
# 已创建会话: 1773299946709_70b6fc3c6c5723b2  agent:default:main
```

也可以指定工作目录（即对话时的当前目录）：

```bash
pi session new --cwd /path/to/project
```

### 列出所有会话

```bash
pi session list
# key                    session_id                       updated_at
# agent:default:main     1773299946709_70b6fc3c6c5723b2   2026-03-10T...
```

无会话时输出：

```
当前无会话。使用 session new 创建。
```

### 切换会话

```bash
pi session switch agent:default:main
```

切换到不存在的会话时：

```
会话不存在: nonexistent-key
```

> MVP 说明：会话状态存储于当前进程内存，切换操作在下次启动 chat 时生效。

### 搜索会话

```bash
pi session search default
# 按 key 或 session_id 包含关键词过滤，输出匹配的会话列表
```

### 归档会话

```bash
pi session archive agent:default:main
# 已归档会话: agent:default:main
```

### 删除会话

```bash
pi session delete agent:default:main
# 已删除会话: agent:default:main
```

---

## 5. 对话模式（chat）

`pi chat` 是核心功能，进入交互式 AI 对话界面，连接 LLM，支持流式输出与工具调用。

### 前提条件

确保 `.env` 中已设置 `OPENAI_API_KEY`，并在当前 shell 中加载：

```bash
set -a && source .env && set +a
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
> 你好，介绍一下你自己

AI> 你好！我是一个通过 API 访问的 AI 助手，可以用中文或英文和你交流...
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
# 恢复会话: agent:default:main
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

> 可在 `config.toml` 的 `[primitive]` 段调整确认策略：
> - `auto_confirm = true`：白名单内自动确认，无需交互
> - `require_approval_for_all_write = false`：写操作不强制询问

### 无 API Key 时的行为

若未设置 `OPENAI_API_KEY`，进入 chat 后会快速失败并输出错误提示（不会挂起）：

```bash
pi chat
# Error: LLM 配置错误：...（含 key/API/失败 等关键词）
```

---

## 6. 插件管理

pi 支持加载 pi-mono 风格插件（`plugin.json` + `main.js`），在沙箱隔离环境中运行。

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

## 7. 审计日志

pi 将所有 4 原语操作、工具调用、插件 hostcall 与插件生命周期（加载/启用/禁用/卸载）记录到**独立审计日志**，便于事后排查。审计日志仅追加、不可篡改，与业务日志分离。

**说明**：当前审计日志为明文存储；加密存储为后续 TODO。

### 前提：开启审计日志

需要在 `config.toml` 中开启审计并（可选）设置保留天数：

```bash
pi config set security.enable_audit_log true
# 可选：保留最近 N 天（默认 90）
pi config set security.audit_log_retention_days 90
```

审计文件存放于 `~/.pi_wasm/agents/default/audit/audit.jsonl`（专用 JSONL，仅追加）。

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

未开启审计或无记录时给出友好提示。执行 `pi audit list` 时会按配置自动清理过期记录。

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

## 8. 附录

### 环境变量速查

| 变量名 | 是否必填 | 说明 |
|--------|----------|------|
| `OPENAI_API_KEY` | 必填（chat/LLM） | LLM API 密钥 |
| `HTTPS_PROXY` | 可选 | 全局 HTTPS 代理（curl 兼容格式）|
| `HTTP_PROXY` | 可选 | 全局 HTTP 代理 |
| `PI_WASM__LLM__PROXY` | 可选 | 仅 LLM 请求使用的代理，覆盖 config |
| `PI_WASM__LLM__API_BASE_FALLBACK` | 可选 | 主 API 不通时的备用 base URL |
| `PI_WASM__STORAGE__WORK_DIR` | 可选 | 工作根目录，覆盖 config |
| `PI_WASM__WASM__QUICKJS_PATH` | 可选 | QuickJS wasm 路径，覆盖 config 与 work_dir 推导 |
| `WASMEDGE_QUICKJS_PATH` | 可选 | 备用 QuickJS wasm 路径（低优先级回退）|

> 所有 `PI_WASM__<SECTION>__<FIELD>` 变量均可覆盖 `config.toml` 中对应字段，`__` 作为嵌套分隔符。

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

**Q: `pi doctor` 显示 QuickJS 路径未配置**

原因：`~/.pi_wasm/wasm/wasmedge_quickjs.wasm` 不存在，且未设置 `WASMEDGE_QUICKJS_PATH`。

方案一：复制项目内置文件：

```bash
mkdir -p ~/.pi_wasm/wasm
cp pi-rust-wasm/assets/wasm/wasmedge_quickjs.wasm ~/.pi_wasm/wasm/
```

方案二：通过环境变量指定：

```bash
export WASMEDGE_QUICKJS_PATH=/path/to/wasmedge_quickjs.wasm
```

方案三：自行构建（需要 `wasm32-wasip1` target）：

```bash
bash scripts/build-custom-quickjs.sh
```

---

**Q: WasmEdge 未安装或版本不对**

```bash
bash scripts/install-wasmedge.sh -y
source $HOME/.wasmedge/env
pi doctor  # 应显示 ✓ WasmEdge 运行时：可用
```

---

**Q: 插件加载失败（QuickJS wasm 不存在）**

插件执行依赖 QuickJS wasm，参考上方"QuickJS 路径未配置"的解决方案后重试。

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
