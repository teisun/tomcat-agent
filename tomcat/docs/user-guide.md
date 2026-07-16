# tomcat User Guide

<p align="center">
  <a href="user-guide.md">English</a> |
  <a href="user-guide.zh.md">简体中文</a>
</p>

This document is for first-time tomcat users. If you mainly use tomcat inside VS Code, start with the extension route first. If you mainly use it in the terminal, continue to the CLI sections below. The guide still follows the order "get the program -> initialize -> try features one by one" and covers tomcat's main usage patterns and available capabilities.

---

## Table of Contents

- [Choose Your Path First: VS Code Extension / CLI](#choose-your-path-first-vs-code-extension--cli)
- [0. Use the Release Binary Directly](#0-use-the-release-binary-directly)
- [1. Build from Source](#1-build-from-source)
- [2. Initialization and Environment Checks](#2-initialization-and-environment-checks)
- [3. Configuration Management](#3-configuration-management)
- [4. Session Management](#4-session-management)
- [5. Workspace Management](#5-workspace-management)
- [6. Chat Modes](#6-chat-modes)
- [7. Audit Log](#7-audit-log)
- [8. Appendix](#8-appendix)
- [9. rquickjs Plugin Runtime](#9-rquickjs-plugin-runtime)

---

## Choose Your Path First: VS Code Extension / CLI

### A. I mainly use VS Code (recommended)

Go straight through the VS Code extension path. Do not start by manually researching CLI installation details:

```text
GitHub Release
   -> download the bundled .vsix for your platform
   -> code --install-extension xxx.vsix
   -> Cmd/Ctrl+Shift+P -> Tomcat: Focus Agent Box
   -> or open the Secondary Side Bar on the right and click Tomcat Agent Box
   -> if first-time setup appears, click Start Setup and let VS Code run tomcat init for you
```

For which extension package to choose, how to install it, and where the bundled CLI lives after installation, see:

- [`tomcat-vscode-ext/README.md`](../../tomcat-vscode-ext/README.md)

### B. I mainly use the terminal

Continue with the CLI installation and initialization sections below.

---

## 0. Use the Release Binary Directly

This path is for users who just want to get the CLI running quickly. The recommended option is the one-line install script: it automatically detects your platform, downloads the matching release archive, verifies `SHA256SUMS`, extracts the archive, and installs it into `~/.local/bin`. If that directory is not yet in your `PATH`, the script appends `export PATH="$HOME/.local/bin:$PATH"` to the right shell startup files (for zsh: both `~/.zprofile` and `~/.zshrc`).

```bash
# 1. One-line install of the latest release
curl -sSf https://raw.githubusercontent.com/teisun/tomcat-agent/main/tomcat/scripts/install.sh | bash

# 2. Initialize
~/.local/bin/tomcat init
# If your current shell already includes ~/.local/bin, you can also run:
tomcat init

# 3. Verify
~/.local/bin/tomcat --help
# Or:
tomcat --help
```

To install a specific version, pass `-v` after the script:

```bash
curl -sSf https://raw.githubusercontent.com/teisun/tomcat-agent/main/tomcat/scripts/install.sh | bash -s -- -v 0.1.8
```

If you prefer to download the Release archive manually, first choose the matching `target` for your platform:

- Apple Silicon (M1/M2/M3): `aarch64-apple-darwin`
- Intel Mac: `x86_64-apple-darwin`
- Linux x86_64: `x86_64-unknown-linux-gnu`

Release assets are named `tomcat-<tag>-<target>.tar.gz`. The CLI now uses its own tag namespace, so recent artifacts look like `tomcat-cli-v0.1.8-aarch64-apple-darwin.tar.gz`. After downloading manually, install it like this:

```bash
# 1. Extract the release archive
tar -xzf tomcat-<tag>-<target>.tar.gz

# 2. On macOS, if the file came from a browser download, clear quarantine first
xattr -dr com.apple.quarantine ./tomcat

# 3. Install into a user-level PATH directory (no sudo)
mkdir -p "$HOME/.local/bin"
install -m 755 ./tomcat "$HOME/.local/bin/tomcat"

# 4. Initialize
"$HOME/.local/bin/tomcat" init
```

`tomcat init` writes the default config, creates the working directory structure, deploys embedded assets, and configures API keys. After initialization, continue with [Section 2](#2-initialization-and-environment-checks) and [Section 6](#6-chat-modes).

---

## 1. Build from Source

This path is for users who need to run from source, debug, or contribute to development.

### Prerequisites

| Dependency | Version Requirement | Purpose |
|------------|---------------------|---------|
| Rust | stable 1.70+ | Build |

### Build

First get the repository source, then enter the `tomcat/` directory inside the repo and build there:

```bash
# After cloning the repository, enter the tomcat/ directory inside it
cd tomcat
cargo build --release
```

```bash
./target/release/tomcat --help
# Then initialize once; init creates ~/.local/bin/tomcat and writes a stable PATH entry:
./target/release/tomcat init
# Open a new shell (or source your profile), then:
tomcat --help
```

Check the version:

```bash
tomcat --version
# tomcat 0.1.8
```

### Working Directory

By default, tomcat stores all data under `~/.tomcat/`. You can change `work_dir` in the `[storage]` section of `tomcat.config.toml`.

Subdirectories are created automatically by `tomcat init` or on first startup:

```
~/.tomcat/
├── tomcat.config.toml                 # main config file (generated by tomcat init)
├── models.toml                        # model catalog (generated by tomcat init; add/remove models here only)
├── agents/
│   └── main/
│       ├── agent/                 # identity and credentials
│       ├── sessions/              # session JSONL transcripts
│       ├── logs/                  # log files
│       └── audit/                 # audit logs
├── workspace-main/                # agent_definition_dir: default design-time directory
├── assets/
│   ├── .env                       # sensitive config such as API keys (0600 permissions)
│   ├── .versions.json             # embedded asset SHA-256 version record
│   ├── .lock                      # lock for concurrent writes
│   └── js/                        # runtime injected scripts and shim assets
├── plugins/                       # plugin directory (see Section 9)
└── memory/                        # vector retrieval index
```

See [directory-structure.md](architecture/directory-structure.md) for details.

Quick glossary: `agent_workspace_dir` is the working directory bound to the current session (usually the shell's current directory when you start `tomcat code`). It is the source of meanings such as "current directory", "this project", and relative paths, but it does not automatically grant file access. `agent_definition_dir` is the design-time directory `workspace-<agentId>/` and is also the default writable root. `agent_trail_dir` is the runtime directory `agents/<agentId>/`.

---

## 2. Initialization and Environment Checks

### tomcat init - Three-Step Wizard

```bash
tomcat init
```

`tomcat init` is an **interactive, model-first** three-step wizard. The main config file `tomcat.config.toml` now only handles:

- which model to use: `default_model`, `vision_model`, and `title_model` under `[llm]`
- global runtime knobs: concurrency, retry, timeout, proxy, files, continuity, and so on

Connection details for models (`api`, `provider`, `base_url`, `api_key_env`) have all moved into `~/.tomcat/models.toml`.

The three steps are:

1. **[1/3] Environment initialization**: ensure that `~/.tomcat/models.toml` contains the managed preset entries released from the embedded `builtin_models.toml` preset source (OpenAI `gpt-5.2/5.4/5.5/5.6`, DeepSeek `deepseek-v4-pro/-flash`, `utility-flash`, MiMo `mimo-v2.5-pro`, GLM `glm-5.2`, Kimi `kimi-k2.7-code`, and Anthropic `claude-opus-4-8/4-7/4-6`), then load the model catalog and enter interactive model selection; after that, write `~/.tomcat/tomcat.config.toml` if it does not already exist, create the directory structure, extract embedded assets (modules and so on), create a stable `~/.local/bin/tomcat` command entry when the current binary comes from a local build, prune old tomcat-injected `target/*` PATH exports, and append the stable line `export PATH="$HOME/.local/bin:$PATH"` to the right startup files (`~/.zprofile` + `~/.zshrc` for zsh, `~/.bashrc` for bash while keeping `~/.bash_profile` sourcing both `.profile` and `.bashrc`, or `~/.profile` for other shells)
2. **[2/3] Asset checks**: the same checks as `tomcat doctor` (config validity, embedded assets, asset versions, and so on), **excluding** `.env` permissions and `OPENAI_API_KEY` environment variable reminders
3. **[3/3] API key setup**: first prompt for the credential variable that matches the default model you selected in the wizard (for example `OPENAI_API_KEY`, `OPENAI_GATEWAY_API_KEY`, `DEEPSEEK_API_KEY`, `MIMO_API_KEY`, `ANTHROPIC_API_KEY`), then optionally let you add keys for other providers. You can press Enter to skip; **skipping does not write an empty key**. Later you can run `tomcat init` again, edit `~/.tomcat/assets/.env` yourself, or use `tomcat model key set`

Expected output (excerpt):

```
[1/3] 环境初始化
  ✓ 配置文件已写入: ~/.tomcat/tomcat.config.toml
  ✓ 默认模型: gpt-5.4
  ✓ 默认模型协议线: openai-responses
  ✓ 模型逻辑厂商: openai
  ✓ 目录结构就绪
  ✓ 已生成模型清单 models.toml（含全部受管预置模型）
  ✓ 内嵌资源已释放
  ✓ 已加入 PATH 环境变量

[2/3] 资源检查
✓ 配置合法 (~/.tomcat/tomcat.config.toml)
✓ 内嵌资源已就绪
  资源版本: modules=...

[3/3] API Key 配置
  ✓ API Key 已配置
  （或 ⚠ 未设置时的提示与 `.env` 路径）

初始化完成！运行 `tomcat code` 开始对话。
```

If tomcat cannot update your shell config automatically, it prints `⚠ 无法自动配置 PATH` plus the exact one-line command `export PATH="$HOME/.local/bin:$PATH"` that you can run manually.

**Idempotency**: if `~/.tomcat/tomcat.config.toml` **already exists**, tomcat updates it using the existing file as the baseline. `models.toml` never rewrites your existing entries or comments; it only fills in missing managed preset models. On the second run and later, you will see messages such as "configuration file updated" and "managed preset models are complete".

**Legacy config migration**: if old fields such as `[llm].provider`, `[llm].api_base`, or `[llm].api_key_env` still exist in an older config, `tomcat init` lets you go through the wizard and removes those legacy connection fields when it writes back, moving the connection facts into `models.toml`.

**Relationship to `tomcat doctor`**: step 2 of `init` shares the same checking logic as `doctor`. If you are unsure about config or embedded assets, run `tomcat doctor` again to see the full diagnostics including API key and `.env` checks.

### models.toml - Add/Remove Models and MiMo Token Plan

`tomcat init` generates `~/.tomcat/models.toml` (the **model catalog**). This is the only entry point for adding or removing models:

- **Auto-loaded on startup**: startup paths such as `tomcat code` and `tomcat claw` merge the built-in model table with `models.toml`. **The same id overrides the built-in entry, and a new id adds a new model**. No code changes or recompilation are required.
- **Single runtime source, user-visible seed file**: the embedded `builtin_models.toml` is the single runtime source of truth for common presets: OpenAI (`gpt-5.2` / `gpt-5.4` / `gpt-5.5` / `gpt-5.6`), DeepSeek (`deepseek-v4-pro` / `deepseek-v4-flash` / `utility-flash`), MiMo (`mimo-v2.5-pro`), GLM (`glm-5.2`), Kimi (`kimi-k2.7-code`), and Anthropic Messages (`claude-opus-4-8` / `4-7` / `4-6`). `tomcat init` releases that same embedded preset list into `models.toml`, so you can inspect, edit, or delete the seeded entries without creating a second hand-maintained source of truth.
- **Idempotent generation**: if `models.toml` does not exist, tomcat creates it with the managed preset list. If it exists but is missing managed preset entries, tomcat **only appends the missing entries**. If it is already complete, tomcat leaves it untouched. It **never overwrites your existing entries or comments**. Running `tomcat init` repeatedly is safe.
- **Add one more model**: copy an existing `[[models]]` block and explicitly fill in `api`, `provider`, and `base_url`. `api_key_env` is optional; when omitted it defaults to `<PROVIDER>_API_KEY`.

Field meanings:

- `id`: the local model id used by `/model list`, default-model selection, and session persistence
- `model_name`: the real model name sent upstream; when omitted it defaults to `id`
- `api`: which wire protocol to use; currently `openai`, `openai-responses`, and `anthropic-messages` are supported
- `provider`: the logical vendor name, used only for credential inference, display, and auditing
- `api_key_env`: explicitly names the environment variable; when omitted it is inferred as `<PROVIDER>_API_KEY`
- `base_url`: either a bare host or a host with an explicit provider path. tomcat appends the leaf automatically, so `https://api.openai.com` becomes `/v1/...`, while GLM-style paths such as `https://open.bigmodel.cn/api/paas/v4` keep `/api/paas/v4/...`
- `kimi-k2.7-code` / Moonshot endpoint note: the built-in preset currently defaults to Moonshot China `https://api.moonshot.cn`, because that is the endpoint verified by the live smoke setup in this repo. Moonshot's global platform uses `https://api.moonshot.ai` instead. If your API key was created on the global platform, override `base_url` in `models.toml` to `https://api.moonshot.ai`.

### tomcat model - Manage models and keys without hand-editing files

If you do not want to edit `models.toml` or `.env` manually, use the dedicated model-management subcommands:

```bash
tomcat model list
tomcat model add claude-opus-gateway \
  --api anthropic-messages \
  --provider anthropic \
  --model-name claude-opus-4-6 \
  --base-url https://api.anthropic.com/v1 \
  --reasoning --tools \
  --thinking-format anthropic

tomcat model key set anthropic
# or: tomcat model key set anthropic sk-ant-xxxxx
tomcat model key list
tomcat model default claude-opus-gateway
tomcat model remove claude-opus-gateway
```

What these commands do:

- `tomcat model add/remove` updates `~/.tomcat/models.toml` through the same validation path used at runtime.
- `tomcat model key set` accepts either `tomcat model key set <provider>` (interactive password prompt) or `tomcat model key set <provider> <value>` (script-friendly form); it writes `~/.tomcat/assets/.env` with `0600` permissions, keeps other keys intact, and never prints the plaintext key back in responses.
- `tomcat model list` shows whether each model comes from the built-in table or the user catalog, plus whether its API key is already present.
- `tomcat model default` switches `[llm].default_model` in `tomcat.config.toml`.

**Tomcat Settings → Models** in VS Code uses the same credential rules:

- The key-slot inventory comes only from non-empty, valid `*_API_KEY` entries in `~/.tomcat/assets/.env`. **Refresh** reloads `.env` first and then refreshes each model's `Configured` / `Missing` state; restarting serve is unnecessary.
- Key slot is the only place to edit the environment-variable name. You can search saved slots or type a new one. Relay suggestions combine the site and API family, for example `FCODEX_OPENAI_API_KEY` and `FCODEX_ANTHROPIC_API_KEY`. Existing models keep their saved `api_key_env` and are not migrated automatically.
- While entering a new key, the focused field uses password dots; on blur, only that unsaved in-memory draft gets a local prefix/suffix preview. Saving clears the draft. Saved keys expose status only: the backend never returns complete or partial key characters to the Webview.
- Writing a new value into a configured slot shared by other models requires confirmation with the affected model list. Leaving the API-key field blank safely reuses the slot without rewriting `.env`.

```text
valid *_API_KEY entries in .env ──> Key slot combobox ──> model.api_key_env
                   Refresh ───────> Configured / Missing
```

**Minimal MiMo Token Plan config** (written by `init` by default):

```toml
[[models]]
id = "mimo-v2.5-pro"
api = "openai"                                  # uses OpenAI-compatible /v1/chat/completions
provider = "mimo"                               # determines MIMO_API_KEY
base_url = "https://token-plan-cn.xiaomimimo.com"  # host only; the program appends the suffix
thinking_format = "doubao"
context_window = 1000000
capabilities = { vision = false, files = false, tools = true, reasoning = true }
```

- **Key**: set `MIMO_API_KEY=tp-xxxxx` either in `~/.tomcat/assets/.env` or in the current shell. `tp-xxxxx` (Token Plan) cannot be mixed with usage-based `sk-xxxxx`.
- **base_url**: enter the **host only**. The program appends `/v1/chat/completions` for you, so **do not** write the full endpoint. Use the host shown on your subscription page (CN / SGP / AMS map to China / Singapore / Europe).
- **Usage**: switch to it with `/model mimo-v2.5-pro` inside `tomcat code` or `tomcat claw`, or set `[llm].default_model` to it.
- **Capability boundary**: `mimo-v2.5-pro` is text-only officially (no images or files), so `vision/files=false`; server-side thinking is enabled by default, and this integration uses the Doubao-style `thinking` wire format.

### OpenAI and Gateway Side by Side

If you want to keep the official `gpt-5.4` and also add a gateway-backed `gpt-5.4`, do not reuse the same `id`. The recommended approach is to add another entry in `models.toml`:

```toml
[[models]]
id = "gpt-5.4_gateway"
model_name = "gpt-5.4"
api = "openai-responses"
provider = "openai-gateway"
base_url = "https://gateway.example.com"
thinking_format = "openai"
capabilities = { vision = true, files = true, tools = true, reasoning = true }
```

- Switch to the gateway: set `[llm].default_model` to `gpt-5.4_gateway`
- Switch back to the official endpoint: change it back to `gpt-5.4`
- `provider = "openai-gateway"` defaults to the key name `OPENAI_GATEWAY_API_KEY`

### Legacy Config Migration

The old `[llm].provider`, `[llm].api_base`, and `[llm].api_key_env` fields have been removed. If you keep writing them now, tomcat raises a migration error and tells you to move the connection data into `models.toml`.

### tomcat doctor - Environment Diagnostics

```bash
tomcat doctor
```

`doctor` checks the environment item by item and gives you executable remediation advice:

| Check Item | Passing Example | Failure Example |
|------------|-----------------|-----------------|
| Config file | `✓ 配置合法 (~/.tomcat/tomcat.config.toml)` | `✗ 未找到配置文件` |
| Embedded assets | `✓ 内嵌资源已就绪` | `✗ 资源释放失败` |
| Asset version | `资源版本: modules=def456...` | — |
| Proxy semantics | `✓ llm.proxy 已配置，将优先于环境代理` / `✓ 检测到环境代理（HTTPS_PROXY）` | `⚠ HTTPS_PROXY 含首尾空格` / `⚠ ALL_PROXY 使用 socks5://...` |
| `.env` permissions | `✓ .env 权限: 0600` | `⚠ .env 权限: 0644（建议 0600）` |
| API key | `✓ OPENAI_API_KEY 已设置` | `⚠ OPENAI_API_KEY 未设置` |

Each failed or warning item includes a fix suggestion such as `→ 运行 tomcat init 或...`. For plugin runtime-related checks, see [Section 9](#9-rquickjs-plugin-runtime).

Proxy diagnostics follow these rules:

- If `llm.proxy` is configured, it takes precedence over `HTTPS_PROXY`, `HTTP_PROXY`, and `ALL_PROXY`.
- If `llm.proxy` is not configured, tomcat's outbound client respects environment proxies. `web_search`, `web_fetch`, and plugin `pi.fetch` follow the same rules.
- `doctor` warns about leading or trailing whitespace in proxy values because this kind of configuration often causes "it looks configured, but behaves inconsistently" problems.
- The current build does not enable reqwest's socks feature, so `ALL_PROXY=socks5://...` is not the recommended configuration for `web_search`; prefer `HTTPS_PROXY=http://...`.

---

## 3. Configuration Management

`tomcat config` reads and writes `tomcat.config.toml`, so you do not need to edit the file by hand.

### View the Full Config

```bash
tomcat config get
```

This prints the full TOML content of the current configuration:

```toml
[log]
level = "warn"
file_enabled = true

[llm]
default_model = "gpt-5.4"

[context]
compaction_model = "gpt-5.4"
...
```

Note: `tomcat config get` only shows `tomcat.config.toml`. Model connection details (`api`, `provider`, `base_url`, `api_key_env`) live in `~/.tomcat/models.toml` and do not appear here.

### serve Gateway

```bash
tomcat serve --stdio
```

`tomcat serve` is the Agent Server entry point for IDE / GUI hosts. In Phase 1, only `--stdio` is exposed:

- Upstream: the host sends one NDJSON command frame per line over stdin (for example `initialize`, `prompt`, `new_session`, `interrupt`)
- Downstream: tomcat writes only NDJSON response/event frames to stdout, without mixing in normal logs
- Multi-session: routing happens via `sessionId`; if no `sessionId` is provided, the current active session is used
- Approval loop: `ask_question` round-trips through `control_request`, `control_response`, and `control_cancel`

To export protocol artifacts for the extension side:

```bash
tomcat serve --print-schema
```

This command prints the schema directory path. That directory contains `serve.schema.json` and `serve.d.ts`.

### Query a Single Config Item

Use dot-path syntax. All nested TOML fields are supported:

```bash
tomcat config get log.level
# warn

tomcat config get llm.default_model
# gpt-5.4

tomcat config get llm.proxy
# (empty or the current proxy address)
```

If the field does not exist:

```
未找到配置项: nonexistent.key
  同级可用项: [...]
```

### Change a Config Item

```bash
tomcat config set log.level debug
# sets log.level = debug

tomcat config set llm.default_model gpt-4o
# sets llm.default_model = gpt-4o

tomcat config set log.file_enabled true
# sets log.file_enabled = true
```

Value types are inferred automatically: integers, booleans (`true` / `false`), and strings. After each change, tomcat validates the updated configuration.

### Startup Preflight and Search-Tool Installation

After you enter `tomcat code` or `tomcat claw`, tomcat checks in the background for fast implementations used by `search_files` (`rg`/ripgrep and `fd`/`fdfind`). If they are missing, tomcat attempts a background install for the current platform. This does not block chat. If installation fails, `search_files` automatically falls back to the in-process Tier 2 implementation (`walkdir + globset + Rust regex`). These preflights still run by default, but the `[tools]` / `[git]` terminal prompts are **off** by default to avoid interrupting input.

To disable background installation attempts, set:

```toml
[preflight]
auto_install_search_tools = false
```

You can also change it through the config command:

```bash
tomcat config set preflight.auto_install_search_tools false
```

If you want to turn on terminal prompts without disabling background preflight, set:

```toml
[preflight]
show_search_tools_ui = true
show_git_ui = true
```

The equivalent config commands are:

```bash
tomcat config set preflight.show_search_tools_ui true
tomcat config set preflight.show_git_ui true
```

For CI or one-off runs, you can override it with an environment variable:

```bash
TOMCAT__PREFLIGHT__AUTO_INSTALL_SEARCH_TOOLS=false tomcat code
```

### Open the Config in an Editor

```bash
tomcat config edit
# opens the config file in $EDITOR (default: vi) and re-validates after save
```

### Path Rules (pathrules)

Besides the workspace root, you can use `tomcat pathrules` to add fine-grained path rules (written into `primitive.path_rules` and merged with the built-in deny/readonly rules):

```bash
tomcat pathrules add ~/.ssh --mode deny      # deny read and write
tomcat pathrules add ~/notes --mode readonly # read-only
tomcat pathrules list                        # show builtin / user / session rule groups
```

Paths support the `~` prefix. If the target does not exist, tomcat only warns and still allows the config write.

---

## 4. Session Management

tomcat organizes sessions by **scope**: `tomcat claw` binds to the global scope, and `tomcat code` binds to the current project scope. Each scope can contain multiple sessions, persisted as JSONL transcripts. `tomcat session` operates on the current default mode by default, and you can also pass `--scope claw|code` explicitly.

### Create a New Session

```bash
tomcat session new --scope claw
# creates session: 1773299946709_70b6fc3c6c5723b2  agent:main:main
```

### List Sessions in the Current Scope

```bash
tomcat session list --scope claw
# * 1773299946709_70b6fc3c6c5723b2  agent:main:main
#   1773299947001_aabbccddeeff0011  agent:main:main
```

When there are no sessions, the output is:

```
当前无会话。使用 session new 创建。
```

### Switch Sessions

```bash
tomcat session switch 1773299946709_70b6fc3c6c5723b2 --scope claw
```

If you switch to a non-existent session:

```
会话不存在: nonexistent-session-id
```

### Search Sessions

```bash
tomcat session search 1773299 --scope claw
# filters sessions in the current scope whose key or session_id contains the keyword
```

### Archive a Session

```bash
tomcat session archive 1773299946709_70b6fc3c6c5723b2 --scope claw
# archives session: 1773299946709_70b6fc3c6c5723b2
```

### Delete a Session

```bash
tomcat session delete 1773299946709_70b6fc3c6c5723b2 --scope claw
# deletes session: 1773299946709_70b6fc3c6c5723b2
```

---

## 5. Workspace Management

tomcat uses the `workspace` subcommand to manage **additional** accessible external directory roots. This is different from the design-time directory `agent_definition_dir`, which each agent can write to by default. The authorization list is a **global** configuration stored in the `[workspace]` table of `~/.tomcat/tomcat.config.toml` (field `workspace_roots`), and **all agents share the same list**. The current directory when you start `tomcat code` is not automatically added. If you need access to the current project, explicitly add it with `tomcat workspace add --cwd`, the cwd lazy prompt inside chat, or the `/path <path>` authorization command. The cwd lazy prompt supports `[s]` for this session only, `[w]` to write into `workspace_roots`, and `[c]` to cancel. Invalid input shows the valid choices and is treated as cancel, rather than being silently swallowed. If you cancel and a tool later fails again because the current directory is still unauthorized, the error explains that the next cwd access can choose `[s]` / `[w]` / `[c]` again, or you can run `tomcat workspace add <path>` for permanent authorization. The old `primitive.path_whitelist` has been removed; use `workspace.workspace_roots` or `primitive.path_rules` instead.

### Add a Workspace

```bash
tomcat workspace add /path/to/project
# adds workspace: /path/to/project
```

To add the **current working directory** to the authorization list without typing an absolute path:

```bash
cd /path/to/project
tomcat workspace add --cwd
# adds workspace: /path/to/project
```

The path must be an existing directory. `tomcat workspace add` requires either a path **or** `--cwd`. Duplicate additions are deduplicated.

### List Authorized Workspaces

```bash
tomcat workspace list
# /path/to/project
# /another/project
```

### Remove a Workspace

```bash
tomcat workspace remove /path/to/project
# removes workspace: /path/to/project
```

### PackageManager Installation

Starting from T2-P1-017, the **official installation entry point** for plugins and skills is unified under `PackageManager`. It only performs static validation of local sources, writes to the three storage layers, and maintains the registries. Actual runtime discovery still reuses the existing three-layer scans for `plugins/` and `skills/`.

Supported source shapes:

- a package directory whose top-level `package.json` contains a `tomcat` block
- a bare plugin: a directory with `plugin.json`
- a bare skill: a directory with `SKILL.md`

The three visibility layers are:

- `scope`: the current project, written to `<scope_root>/.tomcat/{plugins,skills,packages}`
- `agent`: the current agent, written to `~/.tomcat/agents/<id>/{plugins,skills,packages}`
- `global`: the globally shared layer, written to `~/.tomcat/{plugins,skills,packages}`

Shell CLI:

```bash
# Install into the current project (in a non-interactive shell the default is scope; in an interactive TTY, a chooser appears if omitted)
tomcat install ./my-package --visibility scope

# Install into the agent / global layer
tomcat install ./my-plugin --visibility agent
tomcat install ./my-skill --visibility global

# Explicitly specify the scope root
tomcat install ./my-package --visibility scope --scope-root /path/to/project

# Allow overwriting when the same-name resource already exists in the same layer
tomcat install ./my-package --visibility scope --force

# View the installed package registries (by default: current scope + agent + global)
tomcat packages
tomcat packages --visibility agent

# Uninstall resources and registry entries in one layer by package name
tomcat uninstall my-package --visibility scope
```

Install from inside chat:

```text
/install ./my-package
/install ./my-plugin agent
/install ./my-skill current-project
```

Notes:

- `current-project` inside `/install` is only a user-facing label; internally it maps to `scope`.
- In `code` / `claw` sessions, after `/install` succeeds, the current session immediately refreshes `SkillSet` plus the plugin catalog/static tool list, so newly installed skills and static plugin capabilities become visible without restarting the session.
- This live refresh **does not** execute plugin code inside the install path, and **does not** hot-swap plugin instances that are already loaded. If a plugin is already running in the current session, it keeps using the old instance until a later normal runtime path reactivates it.

---

## 6. Chat Modes

tomcat provides two official chat entry points:

- `tomcat code`: project-isolated chat mode, recommended as the default
- `tomcat claw`: global chat mode, not bound to a project scope

The historical command `tomcat chat` still exists as a **hidden compatibility alias** and behaves the same as `tomcat code`, but public docs and acceptance criteria are standardized on `code` / `claw`.

### Prerequisites

Make sure you have already run `tomcat init` and configured an API key. `tomcat init` writes keys into `~/.tomcat/assets/.env`, which is loaded automatically on startup.

You can also set the environment variable manually:

```bash
export OPENAI_API_KEY=sk-...
```

### Start a Chat

```bash
# Project-isolated (recommended)
tomcat code

# Global mode
tomcat claw
```

Sample banner output:

```
tomcat 对话模式 (模型: gpt-5.4)
输入消息开始对话，Ctrl+D 退出，Ctrl+C 中断生成。
输入 /help 查看命令列表。

> 
```

### Startup Screen (Splash Mascot)

When you start `tomcat code` or `tomcat claw` in an **interactive terminal**, tomcat first renders the pixel-art mascot "Tommy" above the banner (a four-frame idle animation that plays once and then stops on the final frame), and only then prints the regular text banner above. In **non-TTY** scenarios such as pipes, redirection, or CI, rendering is skipped automatically.

```toml
# ~/.tomcat/tomcat.config.toml
[splash]
enabled = true       # set false to disable the mascot
animations = true    # set false to show only the first static frame
max_width = 56       # maximum reference width for centering
```

The environment variable `TOMCAT_SPLASH=0` disables it temporarily. `NO_COLOR` removes color escapes. Screen-reader users should consider `animations = false` or `enabled = false`.

AI replies stream out incrementally:

```
u> Hi, introduce yourself

agent.main> Hi! I'm an AI assistant accessed through an API, and I can talk with you in Chinese or English...
```

On exit:

```
再见！
```

### Local Commands

Inside interactive chat, tomcat intercepts local commands starting with `/` before the message is sent to the LLM (use `/help` for the complete list):

| Command | Description |
|---------|-------------|
| `/help` | Show the currently supported local commands |
| `/path <absolute path>` | Open the authorization menu for one path (this session / write to workspace / read-only / deny / cancel) |
| `/model` (`current` / `list` / `use <id>`) | View or switch the current session model (catalog = built-in table + `models.toml`) |
| `/thinking` (`minimal` / `summary` / `full` / `toggle`) | Switch the Thinking display level (default is `toggle`) |
| `/ckpt list [--limit N]` | List recent checkpoints |
| `/ckpt show <id>` / `/ckpt diff <id>` | View checkpoint metadata or workspace diffs |
| `/restore <id> [--path <rel>]... [--dry-run]` | Restore from a checkpoint |
| `/plan` | Enter PLAN mode (plans are stored under `~/.tomcat/plans/`) |
| `/plan exit` | Return to Chat mode |
| `/plan build <plan_id or path>` | Enter EXEC mode |
| `/plan list` | List plan files under `~/.tomcat/plans/` |

Dragging or pasting a path and pressing Enter does not open the authorization menu. It is sent to the LLM as a normal chat message. If you need to authorize a path explicitly, use `/path <absolute path>`.

#### Recommended Startup Directory for Checkpoints

The checkpoint system in `tomcat code` treats the **current directory at startup** as the workspace root. That means if you start it inside a shared directory that contains many projects, every checkpoint turn scans the entire large tree.

Measured on 2026-06-09 in `~/.tomcat/temp` on the local machine:

- shared directory `~/.tomcat/temp`: about `33031` files, one `os.walk` takes about `0.313s`
- single-project directory `~/.tomcat/temp/iron-vanguard`: about `3026` files, one `os.walk` takes about `0.053s`

So the safer pattern is: **enter the specific project directory first, then start `tomcat code`**.

```bash
cd ~/.tomcat/temp/iron-vanguard
tomcat code
```

If you start directly from a shared directory such as `~/.tomcat/temp`, checkpoints still watch the whole shared tree even though timeout backoff and noisy-directory exclusions have already been added, so the chance of noticeable stalls is still much higher.

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `Ctrl+D` | End input and exit chat mode normally |
| `Ctrl+C` | Interrupt the current generation and keep chatting |
| `↑` / `↓` | Browse input history |

### Resume the Previous Session (`--resume`)

```bash
tomcat code --resume
# If you use global mode, the equivalent command is:
tomcat claw --resume
```

This loads prior messages from the JSONL transcript in the current scope (`code` or `claw`), so the LLM resumes with the previous conversation context.

### Tool Calls

During chat, the LLM interacts with the host through built-in tools (the Agent loop runs multi-turn automatically; the max turn count is controlled by config). Common tools are:

| Tool | Description |
|------|-------------|
| `read` | Read files (optional hashline mode, useful with `hashline_edit`) |
| `write` | Create or overwrite files |
| `edit` | Perform precise replacements in existing files (supports multi-entry `edits`) |
| `bash` | Run shell commands (supports `run_in_background` plus `task_output` / `task_stop` / `task_list`) |
| `list_dir` | List directories |
| `search_files` | Search files under authorized paths (prefers ripgrep/fd, falls back to in-process search when missing) |

Other tools such as `create_plan`, `update_plan`, `todos`, `web_search`, and `web_fetch` are also available depending on the model and configuration.

**User confirmation prompt** (when `require_approval_for_all_write = true`):

```
[工具调用] write: /path/to/file
内容预览: ...
是否执行？[y/N]
```

Press `y` to confirm. Press `N` or just Enter to deny.

> You can adjust the confirmation strategy in the `[primitive]` section of `tomcat.config.toml`:
> - `auto_confirm = true`: auto-confirm within the allowlist, with no interaction
> - `require_approval_for_all_write = false`: do not force prompts for write operations

### Behavior Without an API Key

If `OPENAI_API_KEY` is not set, entering `tomcat code` or `tomcat claw` fails quickly with an error message instead of hanging:

```bash
tomcat code
# Error: LLM configuration error: ... (contains keywords such as key/API/failure)
```

---

## 7. Audit Log

tomcat records the four primitive operations, tool calls, and related events in a **separate audit log** for after-the-fact investigation. The audit log is append-only, tamper-resistant by design, and separate from business logs. (For plugin-related audit event types, see [Section 9](#9-rquickjs-plugin-runtime).)

**Note**: audit logs are currently stored in plaintext. Encrypted storage is a future TODO.

### Audit Log Configuration

The audit log is **enabled by default** (`security.enable_audit_log = true`) and needs no extra setup. To disable it, set it to `false`:

```bash
tomcat config set security.enable_audit_log false
# Optional: adjust retention days (default 90)
tomcat config set security.audit_log_retention_days 90
```

Audit records are stored in `~/.tomcat/agents/main/audit/audit.jsonl` (a dedicated append-only JSONL file).

### List Audit Records

```bash
tomcat audit list
# Or specify the count (default 50):
tomcat audit list --limit 20
```

Output format when records exist:

```
#   时间                    类型          状态   详情
0   2026-03-10T10:00:00Z    primitive     ok     operation=Read path=/home/user/...
1   2026-03-10T10:00:05Z    tool_call     ok     tool_name=echo plugin_id=...
2   2026-03-10T10:00:10Z    hostcall      ok     module=fs method=readFile ...

共 3 条
```

If there are no records, tomcat prints a friendly message. Running `tomcat audit list` also cleans up expired records automatically according to config.

### Audit Record Types

| Type | Trigger |
|------|---------|
| `primitive` | The LLM calls a built-in primitive such as `read`, `write`, `edit`, or `bash` |
| `tool_call` | A plugin or the LLM calls a tool through the tool registry |
| `hostcall` | Plugin JavaScript calls host APIs through `__pi_host_call` |
| `plugin_lifecycle` | Plugin `load` / `enable` / `disable` / `unload` |

### View One Audit Record

```bash
tomcat audit show 0
# shows the full details of record 0
```

If the index does not exist:

```
未找到审计记录: 0
```

### Export Audit Records

```bash
tomcat audit export /tmp/audit_backup.json
# exports N audit records to /tmp/audit_backup.json
```

The export format is a JSON array, which you can process further with `jq`:

```bash
jq '.[0]' /tmp/audit_backup.json
```

---

## 8. Appendix

### Environment Variable Quick Reference

| Variable | Required | Description |
|----------|----------|-------------|
| `OPENAI_API_KEY` | Required (`code` / `claw` / LLM) | LLM API key |
| `HTTPS_PROXY` | Optional | Global HTTPS proxy (curl-compatible format) |
| `HTTP_PROXY` | Optional | Global HTTP proxy |
| `ALL_PROXY` | Optional | Fallback proxy. `HTTPS_PROXY=http://...` is currently preferred; `socks5://` is not the official path for `web_search` |
| `TOMCAT__LLM__PROXY` | Optional | Proxy used only for LLM requests; overrides config |
| `TOMCAT__LLM__API_BASE_FALLBACK` | Optional | Backup base URL when the primary API is unreachable |
| `TOMCAT__LLM__DEFAULT_MODEL` | Optional | Overrides `[llm].default_model`; if unset, tomcat uses the config file or code default `gpt-5.4` |

> `TOMCAT__LLM__*` variables override the matching fields in `tomcat.config.toml`, with `__` used as the nesting separator. The repository and installation packages do **not** inject these variables by default. If your local shell still exports an old model id for a long time, it can diverge from the TOML newly written by `tomcat init`.
>
> `web_search`, `web_fetch`, and plugin `pi.fetch` follow the same proxy priority: `llm.proxy` (or `TOMCAT__LLM__PROXY`) comes first; if unset, tomcat falls back to environment `HTTPS_PROXY`, `HTTP_PROXY`, and `ALL_PROXY`.
>
> `tomcat/.env` inside the test repository is only for local/CI test fixtures. The production CLI actually reads `~/.tomcat/assets/.env`. Both can inject proxy variables into the current process environment, but they serve different purposes, so do not mix their debugging conclusions.

### FAQ

**Q: `tomcat code` / `tomcat claw` exits immediately with an error after startup**

Reason: `OPENAI_API_KEY` is not set, or the key is invalid.

```bash
# Check whether it is already loaded:
echo $OPENAI_API_KEY

# Load .env:
set -a && source .env && set +a
tomcat code
```

---

**Q: `tomcat code` / `tomcat claw` times out when connecting to OpenAI (`curl exit 28`)**

Reason: the current network cannot reach `api.openai.com` directly.

```bash
# Configure a proxy in .env:
HTTPS_PROXY=http://127.0.0.1:7890
# Make sure your local proxy process is running, then:
set -a && source .env && set +a
tomcat code
```

You can verify connectivity with `scripts/verify-openai-apis.sh`:

```bash
./scripts/verify-openai-apis.sh 1 2 3
# [PASS] GET /v1/models - HTTP 200
# ...
```

---

### Related Documents

| Document | Content |
|----------|---------|
| [README.md](../README.md) | Project overview and quick start |
| [Architecture.md](openspec/specs/Architecture.md) | System architecture and layered design |
| [src/infra/README.md](../src/infra/README.md) | Infrastructure layer (config / logging / audit / event bus) |
| [src/core/llm/README.md](../src/core/llm/README.md) | LLM module (OpenAI adapter, streaming output) |
| [src/core/session/README.md](../src/core/session/README.md) | Session management and CLI design |
| [src/core/README.md](../src/core/README.md) | Agent loop (multi-turn chat, tool calls, retries) |
| [src/api/README.md](../src/api/README.md) | CLI / chat / render entry layer |
| [INTEGRATION_TEST_LOGGING.md](openspec/specs/guides/testing/INTEGRATION_TEST_LOGGING.md) | How to inspect integration test logs |

---

## 9. rquickjs Plugin Runtime

> **Status**: the plugin system now runs on **in-process `rquickjs`**. It no longer depends on WasmEdge, QuickJS wasm files, or extra C runtime installation. `tomcat doctor` only checks whether the current build and the `rquickjs` runtime are usable.

### Runtime Characteristics

- The entry points are still `plugin.json` plus `main.js` / `main.ts`.
- The manifest has two registration surfaces: `tools[]` for the LLM and `functions[]` for the host. Both may be empty, but every declared `functions[]` entry must provide non-empty `point` and `function`.
- Sensitive capabilities always go through `pi.*` hostcalls, such as `pi.readFile()`, `pi.writeFile()`, `pi.editFile()`, and `pi.exec()`.
- `node:fs`, `node:child_process`, and `node:os` are not exposed directly to plugins; they return explicit fail-closed errors.
- The lightweight capabilities currently provided by default include:
  - `path`
  - `util.format`
  - `events.EventEmitter`
  - `Buffer`
  - `crypto` (including `hash`, `hmac`, `randomBytes`, `randomUUID`, `aes-gcm`, and `ed25519`)
  - `@sinclair/typebox`
  - `ms`
- Available environment variable:
  - `PI_PLUGIN_DISABLE=1|true|yes|on`: short-circuits the entire plugin runtime entry path.

**Minimal plugin example**

`plugin.json`:

```json
{
  "id": "my-plugin",
  "name": "My Plugin",
  "version": "0.1.0",
  "description": "Minimal plugin for experimentation",
  "author": "me",
  "main": "main.js",
  "requiredPermissions": [],
  "tools": [
    {
      "name": "hello_world",
      "description": "Return a greeting",
      "parameters": {
        "type": "object",
        "properties": {}
      }
    }
  ],
  "events": ["session_start"],
  "activation": "lazy",
  "requiredApiVersion": "1.0",
  "tags": []
}
```

`main.js`:

```js
pi.on("session_start", function () {
  pi.log("my-plugin: session_start");
});

pi.log("my-plugin: loaded");
```

Notes:

- Sensitive capabilities always go through `pi.*`. For example, use `pi.readFile()` / `pi.writeFile()` for file I/O and `pi.exec()` for commands.
- `tools[]` is the static tool contract for the LLM. `functions[]` is the static function contract for the host itself. Host functions do not enter `ToolRegistry`, and they do not appear in the tool list passed to the LLM.
- The `point` field in `functions[]` is a host extension point id such as `web_search.backend` or `test.echo`. The host enumerates candidate functions by `point`, then calls back into the VM using the JavaScript entry name stored in `function`.
- `pi.registerFunction(name, handler)` only binds the JavaScript implementation into the current VM. Whether the host can "see" that capability still depends entirely on `functions[]` in the manifest.
- Node built-ins such as `node:fs` and `node:child_process` are not exposed directly to plugins. Only a small set of lightweight capabilities and fail-closed aliases remain.
- During this phase, `requiredPermissions` is allowed by default at load time, but truly sensitive file, command, and session capabilities are still routed through `pi.*` for unified authorization and auditing.

**Minimal host-function example**

`plugin.json`:

```json
{
  "id": "my-host-function-plugin",
  "name": "My Host Function Plugin",
  "version": "0.1.0",
  "description": "Expose one echo extension point to the host",
  "author": "me",
  "main": "main.js",
  "requiredPermissions": [],
  "requiredApiVersion": "1.0",
  "tags": [],
  "tools": [],
  "functions": [
    {
      "point": "test.echo",
      "function": "echoHost"
    }
  ]
}
```

`main.js`:

```js
pi.registerFunction("echoHost", function (params) {
  return {
    echoed: params && params.text ? params.text : null
  };
});
```

Notes:

- `functions[]` should only declare which host extension point the plugin provides and which JavaScript function name the VM should call. Do not lift plugin-internal default parameters, ordering, or vendor-specific details into the host surface.
- Host-facing functions reuse the same `project > agent > global` discovery and installation chain as ordinary plugins. The only special behavior is at registration time: before entering `FunctionRegistry`, functions are overridden by `point`, with higher-layer declarations taking precedence over lower-layer ones.
- A plugin can expose `tools[]`, `functions[]`, and `events[]` together, or any one of them alone.

```bash
tomcat plugin load ~/tomcat-plugins/my-plugin
tomcat plugin list
tomcat plugin info my-plugin
tomcat plugin disable my-plugin
tomcat plugin enable my-plugin
tomcat plugin unload my-plugin
```

`tomcat plugin load` is still the **runtime loading entry point**. It performs a short-lived initialization check once and writes the registration data into the global `{work_dir}/plugins/registry.json`. By contrast, `tomcat install` and `/install` are the **installation management entry points**: they write the corresponding layer's `plugins/registry.json` and `packages/registry.json` under `scope|agent|global`, but they do not execute plugin code inside the install path. The actual long-lived session VM is still created lazily only when the session first needs that plugin.

Implementation details: [architecture/plugin-system-overview.md](./architecture/plugin-system-overview.md) and [src/ext/README.md](../src/ext/README.md).
