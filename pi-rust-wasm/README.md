# pi-rust-wasm

基于 Rust + WasmEdge 构建的轻量、高安全、可自进化 AI Agent 核心运行时。通过 WasmEdge 内置的 QuickJS 引擎与 Node.js 兼容层，实现 pi-mono 生态 100% 兼容，提供沙箱隔离的插件系统、原子化 4 原语能力。

## 快速开始

### 前置依赖

- Rust 1.70+（推荐 stable）
- WasmEdge C 库 0.13.5

### 安装 WasmEdge

```bash
bash scripts/install-wasmedge.sh -y
source $HOME/.wasmedge/env
```

### 构建与运行

```bash
cargo build --release
./target/release/pi init        # 生成配置文件
./target/release/pi doctor      # 检查环境
```

### 运行测试

```bash
# 需要先配置 .env（参考 .env.example）
RUST_LOG=pi_wasm=debug,info cargo test --all -- --nocapture --test-threads=1
```

## 项目结构

```
src/
├── api/        # CLI 入口与子命令
├── core/       # 会话、LLM、4 原语、工具、确认
│   ├── llm/    # OpenAI 适配器
│   └── session/# 会话管理与 JSONL transcript
├── ext/        # Wasm 引擎、实例、Hostcall 分发、插件管理
└── infra/      # 配置、日志、事件总线、审计、错误、平台工具
```

## 架构

4 层分层架构（自下而上）：

1. **基础设施层 (infra)** — 错误、配置、日志、审计、事件总线、跨平台
2. **宿主核心能力层 (core)** — 会话、LLM、4 原语、工具
3. **宿主 API 层 (ext)** — Hostcall 协议、HostApiDispatcher、Wasm 引擎与插件管理
4. **交互层 (api)** — CLI

详见 [Architecture.md](openspec/specs/Architecture.md)。

## 文档入口

- [docs/README.md](docs/README.md) — 文档地图（技术文档、进度跟踪、分享材料）
- [docs/technical/README.md](docs/technical/README.md) — 模块技术文档索引（与 `src/` 对照，含 ASCII 总览）
- [docs/INTEGRATION.md](docs/INTEGRATION.md) — 集成进度看板

## 规范文档

- [Constitution.md](openspec/specs/Constitution.md) — 项目宪法（不可违反的核心规则）
- [编码与架构规范](openspec/specs/guides/coding/Codeing&Architecture_Spec.md)
- [单元测试规范](openspec/specs/guides/testing/UNIT_TEST_SPEC.md)
- [集成测试规范](openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md)

## 许可

私有项目，未公开许可。
