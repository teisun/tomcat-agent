# 初始化体验与资源内嵌方案

本文为 [Architecture](../../openspec/specs/Architecture.md) 的补充设计，描述如何将初始化流程从 6+ 步手工操作简化为「下载二进制 + `pi init`」两步完成。

涉及三个关键改造：资源内嵌与自动释放、WasmEdge 静态链接、增强 `pi init` 交互式向导。

---

## 1. 现状与问题

当前用户首次使用需完成以下手动步骤：

1. 安装 Rust 工具链
2. 运行 `install-wasmedge.sh` 安装 WasmEdge C 库 0.13.5
3. `source ~/.wasmedge/env`
4. `cargo build --release`
5. `pi init` 生成 `config.toml`，再手动编辑填入 API Key
6. `export OPENAI_API_KEY=...`
7. 手动复制 `wasmedge_quickjs.wasm` 到 `~/.pi_wasm/wasm/`
8. 确保 `pi_bridge.js` 可被发现（或设置 `PI_BRIDGE_JS_PATH`）
9. 确保 `assets/modules/` 可被发现（或设置 `PI_WASM_QUICKJS_MODULES_PATH`）

**目标**：发布预编译二进制后，用户只需 `pi init` 一条命令即可完成所有初始化。

---

## 2. 目标工作目录结构

与 [工作目录结构](../technical/directory-structure.md) 一致（总控设计另见 [work-dir-and-data-layout](../../openspec/specs/architecture/work-dir-and-data-layout.md)）。本节强调与初始化、内嵌资源相关的布局；根目录承载主配置，全局运行时文件集中在 `assets/`。

```
~/.pi_/                             # work_dir 根目录
├── pi.config.toml                  # 主配置文件
├── pi.json                         # 主配置（本期暂时不做）
│
├── agents/                         # 各 Agent 运行态
│   └── <agentId>/
│       ├── agent/                  # 身份与凭据（auth-profiles.json、models.json）
│       ├── sessions/               # 会话索引与 JSONL transcript
│       ├── logs/                   # 业务日志
│       └── audit/                  # 审计日志 JSONL
│
├── workspace-main/                 # 默认 agent 的工作区（行为规则与设计态文件）
│   ├── AGENTS.md
│   ├── SOUL.md
│   ├── TOOLS.md
│   ├── IDENTITY.md
│   ├── USER.md
│   ├── HEARTBEAT.md
│   ├── BOOTSTRAP.md
│   ├── MEMORY.md
│   ├── memory.md                   # 长期记忆备选文件名
│   ├── memory/                     # 按日记忆（YYYY-MM-DD.md）
│   ├── skills/                     # 工作区技能（优先级最高）
│   │   └── <skillName>/SKILL.md
│   └── .pi/
│       └── workspace-state.json    # 工作区状态
│
├── workspace-<agentId>/            # 非默认 agent 的工作区（目录结构与 workspace-main 相同）
│
├── memory/                         # 向量检索索引（按 agent 分文件）
│   └── <agentId>.sqlite
├── skills/                         # 托管技能（Gateway RPC 安装/管理）
│   └── <skillName>/SKILL.md
├── credentials/                    # OAuth 凭据
├── media/                          # 媒体文件
├── subagents/
│   └── runs.json                   # 子 agent 注册表
├── plugins/                        # 全局共享插件
│
└── assets/                         # 全局资源目录（内嵌释放与敏感环境变量）
    ├── .env                        # 敏感配置（API Key 等），pi init 自动生成
    ├── wasm/                       # 全局 Wasm 运行时引擎
    │   └── wasmedge_quickjs.wasm   # 内嵌资源自动释放目标
    └── modules/                    # 全局 JS 兼容模块
        └── (80+ Node.js 兼容 shim) # 内嵌资源自动释放目标
```

各 agent 还可具备 `agents/<agentId>/plugins/`（agent 专属插件）；根级 `plugins/` 为全局共享，与 [directory-structure](../technical/directory-structure.md) 一致。

### 与现有代码的映射

| 现有路径 | 新路径 | 说明 |
|---------|--------|------|
| `~/.pi_wasm/` | `~/.pi_/` | work_dir 根目录更名 |
| `config.toml` | `~/.pi_/pi.config.toml`（后续改成 `pi.json`） | 主配置在根目录，不再放在 `assets/` |
| `agents/default/sessions/` | `agents/<agentId>/sessions/` | MVP 主 agentId 为 `main`（与 `pi.json` 中 `id` 一致） |
| `wasm/` | `assets/wasm/` | 移入 `assets/` |
| — | `assets/modules/` | 新增，内嵌 JS 模块的释放目标 |
| — | `assets/.env` | 新增，API Key 等敏感配置（与根目录 `pi.json` 并列层级，路径为 `assets/.env`） |
| `main/`（旧方案） | `workspace-main/` | 默认工作区目录名与 openclaw 布局对齐 |

---

## 3. Phase 1：资源内嵌与自动释放

核心思路：把运行时必需的文件编译进二进制，首次运行时自动释放到 work_dir 对应目录。

### 3.1 内嵌 `pi_bridge.js`（~28KB）

`pi_bridge.js`（源码位于 `assets/js/pi_bridge.js`）是唯一未被 `include_str!` 嵌入的 JS 文件（其他 8 个 shim 及 `pi_main_loop.js` 已嵌入）。当前 `WasmInstance::resolve_bridge_path`（`src/ext/instance_wasmedge.rs`）依赖环境变量 `PI_BRIDGE_JS_PATH` 或磁盘推导。

**设计**：
- 在 `instance_wasmedge.rs` 中添加 `include_str!("../../assets/js/pi_bridge.js")` 常量
- 修改 `resolve_bridge_path`：不再从磁盘读取，直接返回嵌入内容
- 消除对 `PI_BRIDGE_JS_PATH` 环境变量和磁盘文件路径推导的依赖

### 3.2 内嵌 `wasmedge_quickjs.wasm`（~3.3MB）

**设计**：
- 用 `include_bytes!("../../assets/wasm/wasmedge_quickjs.wasm")` 嵌入
- 首次运行时，若 `{work_dir}/assets/wasm/wasmedge_quickjs.wasm` 不存在，从嵌入数据写出
- 修改 `resolve_quickjs_path`：增加自动释放逻辑作为首选路径

### 3.3 内嵌 `assets/modules/`（~1.0MB，80+ 文件）

**设计**：
- 使用 [include_dir](https://crates.io/crates/include_dir) crate 在编译时嵌入整个目录树：

```rust
use include_dir::{include_dir, Dir};
static EMBEDDED_MODULES: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/modules");
```

- 首次运行时，自动释放到 `{work_dir}/assets/modules/`
- 修改 `resolve_quickjs_modules_dir`（当前位于 `src/ext/instance_wasmedge.rs`）：优先使用 `{work_dir}/assets/modules/`，不再依赖 `CARGO_MANIFEST_DIR` 或 `PI_WASM_QUICKJS_MODULES_PATH` 环境变量

### 3.4 统一资源释放入口

在 `config.rs` 中新增：

```rust
pub fn ensure_embedded_assets(cfg: &AppConfig) -> Result<(), AppError> {
    let work_dir = get_work_dir(cfg)?;
    extract_wasm_if_missing(&work_dir)?;
    extract_modules_if_missing(&work_dir)?;
    Ok(())
}
```

在 `cli.rs` 的 `run_cli` 中，`ensure_work_dir_structure` 之后调用 `ensure_embedded_assets`。

释放目标均为**全局资源目录** `assets/` 下（`assets/wasm/`、`assets/modules/`），不放在 `agents/<agentId>/` 下，因为这些是跨 agent 共享的运行时资源。

### 3.5 资源版本升级策略

当二进制版本升级时，内嵌资源可能已更新，需要覆盖磁盘上的旧版本。

**设计**：
- 编译期在 `build.rs` 中对每个内嵌资源生成 SHA-256 摘要，写入 `EMBEDDED_WASM_SHA256` / `EMBEDDED_MODULES_SHA256` 常量
- `ensure_embedded_assets` 释放前比对磁盘文件的 SHA-256：若不一致则覆盖写入，一致则跳过
- 在 `{work_dir}/assets/.versions.json` 中记录各资源的版本与摘要，供 `pi doctor` 读取展示

```json
{
  "wasmedge_quickjs.wasm": { "sha256": "abc...", "extracted_at": "2026-03-24T12:00:00Z" },
  "modules": { "sha256": "def...", "extracted_at": "2026-03-24T12:00:00Z" }
}
```

### 3.6 并发写入保护

多个 `pi` 进程可能同时启动并触发资源释放。

**设计**：
- 释放前在 `{work_dir}/assets/.lock` 上获取文件锁（`flock` / `fs2` crate），若已被占用则等待（超时 10s 后报错）
- 采用「先写临时文件再原子 rename」策略：写入 `.wasm.tmp` → rename 为 `.wasm`，避免其他进程读到半写文件

---

## 4. Phase 2：WasmEdge 静态链接

### 4.1 Cargo feature gate 设计

`wasmedge-sdk` 0.13 支持 `standalone` feature，构建时自动下载并静态链接 WasmEdge C 库。

```toml
[features]
default = ["standalone"]
standalone = []

[dependencies]
wasmedge-sdk = { version = "0.13.5-newapi", features = ["aot", "standalone"] }
```

**行为说明**：

| 场景 | 构建命令 | 效果 |
|------|----------|------|
| 用户安装 / CI 发布 | `cargo build --release`（默认） | 静态链接 WasmEdge C 库，无需系统安装 |
| 开发者本地编译 | `cargo build --no-default-features` | 使用系统已安装的 WasmEdge，编译更快 |

### 4.2 影响评估

| 项目 | 影响 |
|------|------|
| 构建时间 | 增加（自动下载 + 编译 C 库） |
| 二进制体积 | +20-30MB |
| 平台支持 | macOS (x86_64/aarch64)、Linux |
| `install-wasmedge.sh` | 仅在 `--no-default-features` 开发场景保留 |

### 4.3 Release profile 体积优化

发布构建启用以下 profile 压缩二进制体积：

```toml
[profile.release]
opt-level = "z"          # 极致体积优化（Rust 侧热路径少，I/O 密集型场景无明显性能损失）
lto = true               # 全程序链接时优化，消除 swc / tokio / reqwest 等庞大依赖树的冗余代码
codegen-units = 1        # 单代码单元，配合 LTO 最大化优化效果
strip = "symbols"        # 剥离符号表（发行包标准做法）
```

**注意：不使用 `panic = "abort"`**。当前 `vm_actor.rs` 和 `event_bus.rs` 中有 `catch_unwind` 做故障隔离（WasmEdge C 库 panic 时降级为错误状态而非进程退出，单个事件监听器 panic 不影响后续监听器）。abort 模式会使 `catch_unwind` 失效，导致任何 panic 直接终止进程。若后续将 VM 执行移至独立子进程，可重新评估 abort 模式。

### 4.4 总体积估算

| 嵌入资源 | 大小 |
|---------|------|
| `wasmedge_quickjs.wasm` | +3.3MB |
| `assets/modules/` | +1.0MB |
| `pi_bridge.js` | +28KB |
| WasmEdge 静态链接 | +20-30MB |
| **总计（内嵌增量）** | ~+25-35MB |

| 优化手段 | 预估缩减 |
|---------|---------|
| `opt-level = "z"` + LTO + `codegen-units = 1` | -30~40%（Rust 代码部分） |
| `strip = "symbols"` | -5~10% |

最终二进制预估 30-40MB（优化前 40-50MB），可接受范围（参考：GitHub CLI ~45MB，Deno ~100MB+）。

---

## 5. Phase 3：增强 `pi init` 交互式向导

### 5.1 简化后的交互流程

安全策略采用合理默认值（`default_plugin_permission_level = Restricted`，`enable_audit_log = true`），无需用户配置。向导仅包含两步：

```
pi init
> 欢迎使用 pi！开始初始化...
>
> [1/2] LLM 配置
>   API Provider [OpenAI]:
>   API Key: sk-****  (写入 {work_dir}/assets/.env)
>   Model [gpt-5.2]:
>
> [2/2] 资源检查
>   ✓ wasmedge_quickjs.wasm 已就绪
>   ✓ Node 兼容模块已就绪
>   ✓ 工作目录已创建
>   ✓ API Key 已配置
>
> 初始化完成！运行 `pi` 开始对话。
```

### 5.2 `pi init` 幂等性与旧目录迁移

**幂等性**：`pi init` 可多次安全执行。
- 若 `pi.config.toml` / `pi.json` 已存在，提示 `配置已存在，是否覆盖？[y/N]`；默认 N 保留
- 若 `assets/.env` 已存在且 `OPENAI_API_KEY` 非空，跳过 API Key 输入步骤，打印 `✓ API Key 已配置`
- 目录结构调用 `ensure_work_dir_structure`（已具备幂等性，目录存在则跳过）
- 资源释放调用 `ensure_embedded_assets`（SHA-256 比对，一致则跳过）

**旧目录迁移**：首次执行 `pi init` 时检测 `~/.pi_wasm/` 是否存在：
- 若存在 → 提示 `检测到旧工作目录 ~/.pi_wasm/，是否迁移到 ~/.pi_/？[Y/n]`
- 迁移逻辑：`cp -r ~/.pi_wasm/* ~/.pi_/` → 验证关键文件完整 → 重命名 `~/.pi_wasm/` 为 `~/.pi_wasm.bak/`
- 不自动删除旧目录，留 `.bak` 供用户确认后手动清理

### 5.3 `.env` 处理策略

**位置**：`{work_dir}/assets/.env`。主配置 `pi.json` / `pi.config.toml` 位于 `{work_dir}` 根目录，与 `assets/` 并列；敏感项仅写入 `assets/.env`。

**生成逻辑**：
1. `pi init` 时自动生成 `.env` 模板
2. 若用户在向导中输入了 API Key → 直接写入 `.env`
3. 若用户跳过 → 写入空模板，在「资源检查」阶段检测到 `OPENAI_API_KEY` 为空时：
   - 打印提示并自动调用系统默认编辑器（macOS `open`、Linux `xdg-open`）打开 `.env` 文件
   - 提示用户填写后保存

**文件权限**：`.env` 存放 API Key 等敏感信息，创建时设置 `0600`（仅属主可读写）。Linux/macOS 通过 `std::os::unix::fs::PermissionsExt` 设置；`pi doctor` 增加权限检查，若权限过宽则发出警告。

**模板内容**：

```env
# Pi LLM API Key 配置
# 请填入你的 API Key 后保存
OPENAI_API_KEY=

# 可选：代理配置
# HTTPS_PROXY=http://127.0.0.1:7890
```

**启动时加载**：将 `dotenvy` 从 `dev-dependencies` 提升为正式依赖，在 `run_cli` 入口处调用 `dotenvy::from_path("{work_dir}/assets/.env")` 自动加载。

### 5.4 资源检查阶段

「资源检查」在 init 向导的最后一步，同时也在每次 `run_cli` 启动时执行。检查项：

| 检查项 | 通过 | 失败处理 |
|--------|------|---------|
| `assets/wasm/wasmedge_quickjs.wasm` | ✓ 已就绪 | 触发 `ensure_embedded_assets` 自动释放 |
| `assets/modules/` 目录 | ✓ 已就绪 | 触发 `ensure_embedded_assets` 自动释放 |
| `assets/.env` 存在 | ✓ 已存在 | 自动生成模板 |
| `assets/.env` 权限 | ✓ 0600 | 自动收紧为 `0600`，打印警告 |
| `OPENAI_API_KEY` 非空 | ✓ 已配置 | 提示并打开 `assets/.env` 供用户编辑 |
| 工作目录结构 | ✓ 已创建 | 自动创建 |

### 5.5 `pi doctor` 增强

扩展检查项，发现问题时给出可执行的修复命令：

```
pi doctor
  ✓ 配置文件: ~/.pi_/pi.config.toml
  ✓ WasmEdge 运行时: 0.13.5 (静态链接)
  ✓ QuickJS WASM: ~/.pi_/assets/wasm/wasmedge_quickjs.wasm (3.3MB, sha256: abc...)
  ✓ Node 兼容模块: ~/.pi_/assets/modules/ (80 files, sha256: def...)
  ✓ .env 权限: 0600
  ✗ LLM API Key: 未设置
    → 运行 `pi init` 或编辑 ~/.pi_/assets/.env
```

---

## 6. 改动文件清单

| 文件 | 改动 |
|------|------|
| `Cargo.toml` | 添加 `include_dir`、`fs2` 依赖；`wasmedge-sdk` 加 `standalone` feature gate；`dotenvy` 提升为正式依赖；新增 `[profile.release]` 体积优化配置 |
| `build.rs` | 新增：编译期计算内嵌资源 SHA-256 摘要，写入环境变量供 `config.rs` 读取 |
| `src/ext/instance_wasmedge.rs` | `include_str!` 嵌入 `pi_bridge.js`；修改 `resolve_bridge_path`（消除磁盘依赖）；修改 `resolve_quickjs_modules_dir`（优先 `{work_dir}/assets/modules/`） |
| `src/infra/config.rs` | 新增 `ensure_embedded_assets`（SHA-256 比对 + 文件锁 + 原子写入）；修改 `resolve_quickjs_path`（增加自动释放逻辑） |
| `src/api/cli.rs` | 重写 `run_init` 为交互式向导（幂等、旧目录迁移）；增强 `run_doctor`（权限检查、SHA-256 展示）；入口加载 `.env` |
| `docs/user-guide.md` | 更新安装说明（区分用户安装与开发者编译）、目录结构、init 流程 |
| `openspec/specs/architecture/work-dir-and-data-layout.md` | 同步新目录布局 |
| `docs/technical/directory-structure.md` | 与本文档第 2 节保持一致 |

---

## 7. 设计约束与边界条件

| 约束 | 说明 |
|------|------|
| **Windows 支持** | 本方案仅覆盖 macOS / Linux。Windows 下 `flock` 不可用、`.env` 权限模型不同，需另行适配 |
| **Feature gate 条件编译** | `standalone` feature 当前在 `Cargo.toml` 注释状态；启用后 `wasmedge-sdk` 的 `standalone` feature 须**仅在 `standalone` 启用时**激活（用 `cfg` 或 feature forwarding），避免开发者构建也触发下载 |
| **`run_init` 现状** | 当前 `run_init` 仅写 TOML 配置文件；重写后需同时调用 `ensure_work_dir_structure` 和 `ensure_embedded_assets`，并集成 `dialoguer` 交互 |
| **`run_doctor` 现有 bug** | 当前 `run_doctor` 中失败提示写的是 `work_dir/wasm/`，但 `resolve_quickjs_path` 实际查找 `work_dir/assets/wasm/`；本次需一并修正 |
| **环境变量降级** | 内嵌资源就绪后，`PI_BRIDGE_JS_PATH`、`PI_WASM_QUICKJS_MODULES_PATH`、`WASMEDGE_QUICKJS_PATH` 等环境变量仍作为**覆盖项**保留，便于开发调试；优先级：环境变量 > `{work_dir}/assets/` > 内嵌默认 |

---

## 8. Phase 4（未来）：分发与安装

当以上改造完成后，分发流程变为：

- **Homebrew**: `brew install pi`（单二进制，内含所有资源）
- **curl 安装脚本**: `curl -sSf https://get.pi.dev | sh`
- **GitHub Releases**: 提供 macOS/Linux 预编译二进制

用户体验：

```bash
brew install pi        # 或下载二进制
pi init                # 一条命令完成初始化
pi                     # 开始使用
```

---

## 9. 引用

- [directory-structure](../technical/directory-structure.md) — 工作目录结构（权威树形说明）
- [work-dir-and-data-layout](../../openspec/specs/architecture/work-dir-and-data-layout.md) — 工作目录与数据布局
- [infrastructure-layer](../../openspec/specs/architecture/infrastructure-layer.md) — 基础设施层
- [host-core-layer](../../openspec/specs/architecture/host-core-layer.md) — 宿主核心层
- [plugin-system-overview](../../openspec/specs/architecture/plugin-system-overview.md) — 插件系统概览
