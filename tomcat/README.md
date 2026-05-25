# tomcat

基于 Rust 构建的轻量、高安全、可自进化 AI Agent 核心运行时。默认构建可在无 WasmEdge 环境下编译；启用 `--features wasmedge` 或 `--features standalone` 后接入 WasmEdge + QuickJS 插件运行时，提供沙箱隔离的插件系统、原子化 4 原语能力。

## 快速开始

### 前置依赖

- Rust 1.70+（推荐 stable）
- WasmEdge C 库 0.13.5（仅 `--features wasmedge` 时需要）

### 按需安装 WasmEdge（真实 Wasm 模式）

```bash
bash scripts/install-wasmedge.sh -y
source $HOME/.wasmedge/env
```

### 构建与运行

```bash
cargo build --release                     # 默认 no-wasm 构建
# cargo build --release --features wasmedge   # 启用真实 WasmEdge（需预装 C 库）
# cargo build --release --features standalone # 自动下载并链接 WasmEdge
./target/release/tomcat init    # 生成配置文件
./target/release/tomcat doctor # 检查环境
```

### 运行测试

```bash
# 需要先配置 .env（参考 .env.example）
RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh integration
# 真实 Wasm 验收需单独执行：
# RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh integration-wasm
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
- [src/README.md](src/README.md) — 模块技术文档索引（与 `src/` 对照，含 ASCII 总览）
- [docs/INTEGRATION.md](docs/INTEGRATION.md) — 集成进度看板

## 规范文档

- [Constitution.md](openspec/specs/Constitution.md) — 项目宪法（不可违反的核心规则）
- [编码与架构规范](openspec/specs/guides/coding/Codeing&Architecture_Spec.md)
- [单元测试规范](openspec/specs/guides/testing/UNIT_TEST_SPEC.md)
- [集成测试规范](openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md)

## 许可

私有项目，未公开许可。
