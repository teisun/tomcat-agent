# Rust 依赖治理

本文件定义 `tomcat` 的依赖取舍规则，并记录 2026-09 的去重审计结论。

## 决策原则

```
需求是否存在？
  ├─ 否 → 删除直接依赖或未使用 feature
  └─ 是
      ├─ 同一功能是否已有依赖提供？
      │   ├─ 是 → 复用，禁止再引入第二套实现
      │   └─ 否 → 只启用满足需求的最小 feature 集
      └─ 出现多个同名版本？
          ├─ 我方直接依赖能升级以对齐 → 评估迁移并单独回归
          └─ 上游锁定不兼容版本 → 记录来源，禁止无收益的 fork/替换
```

依赖数量只是代理指标。真正的成本是构建时间、二进制体积、C/C++ 构建链、供应链审计面和升级维护成本。没有进入目标构建的 lockfile 条目不构成运行时或编译成本。

## 已执行的最小化

### 配置加载

`infra/config/load.rs` 是 `config` crate 的唯一调用点：它只读取 TOML 主配置，再叠加 `TOMCAT__` 环境变量。`mcp.json` 和 `model-thinking.json` 均由 `serde_json` 独立读取。

因此 `Cargo.toml` 明确禁用 `config` 的默认 feature，只保留 `toml`：

```toml
config = { version = "0.14", default-features = false, features = ["toml"] }
```

这避免把未支持的 JSON、YAML、INI、RON、JSON5 配置加载器编译进生产图，并移除了 `ron → base64 0.21` 这条重复版本来源。

如果将来公开支持 `--config *.yaml` 或 `--config *.json`，只添加相应的 `yaml` 或 `json` feature；不要恢复 `default-features`。

### 终端语法高亮

`api/render/mod.rs` 仅使用 syntect 渲染内置语法集和主题。项目选择纯 Rust 的 `default-fancy` 正则后端：

```toml
syntect = { version = "5", default-features = false, features = ["default-fancy"] }
```

这取代默认的 Oniguruma 路径，移除 `onig` / `onig_sys` 的 C 构建；代价是引入纯 Rust 的 `fancy-regex`，crate 数不一定显著下降，但构建和供应链边界更简单。

### 死直接依赖

`cargo machete` 审计确认 `swc_ecma_visit` 没有被本项目直接引用，已从 `Cargo.toml` 移除。它仍可能作为 SWC 传递依赖存在；这不是重复声明，也不应靠 patch 或 fork 强行移除。

## 已统一的 HTTP 客户端版本

### reqwest 0.13

```
tomcat 直接 HTTP 调用 ───┐
                         ├───────────► reqwest 0.13
远程 MCP/OAuth: rmcp ────┘
```

项目直接依赖已从 reqwest 0.12 升至 0.13，并显式启用 `native-tls`、`charset`、`http2`、`system-proxy`、`json`、`stream`、`form`、`query`、`multipart`。`default-features = false` 是必须的：reqwest 0.13 的默认 TLS 后端是 rustls；只写版本号与业务 feature 会在没有任何源码改动时悄悄把 TLS 后端换掉。

```
直接 HTTP / 远程 MCP OAuth
           │
           └── reqwest 0.13 + native-tls
                 └── 操作系统证书库、企业代理与自定义 CA 行为保持一致
```

保留 native TLS 不是为了减少 crate 数，而是为了保持既有 HTTPS 行为。代理、HTTP/2、系统代理发现和 OS 根证书信任均由显式 feature 保障；OAuth token 交换和带查询参数的扩展 HTTP 调用仍依赖 `form`、`query` 与 `multipart`。`rmcp` 必须选 `reqwest-native-tls`，不能选名称相近的 `reqwest`：后者会启用 `reqwest/rustls`，Cargo feature 合并后会重新把 rustls 拉回生产图。

推翻条件：若 rmcp 在未来公开版本中强制 reqwest 的 rustls feature，或真实目标网络的 HTTPS 冒烟证明 native TLS 不可用，再单独评估全栈切换及企业代理/自定义 CA 回归。不能把「少一个 C 构建边界」当成切换理由；TLS 后端是网络兼容性决定。

### 由上游版本边界决定的重复

| crate | 主要来源 | 决策 |
| --- | --- | --- |
| `thiserror` 1 / 2 | `dialoguer`、`oauth2` 与本项目/rmcp | 保留；由上游版本线决定 |
| `base64` 0.22 / 0.23 | 本项目直接使用与 rmcp | 保留；config 引入的 0.21 已移除 |
| `hashbrown` 0.14 / 0.16 / 0.17 | SWC、HTTP 栈、rquickjs | 保留；替换任一主功能库无收益 |
| `siphasher` 0.3 / 1 | SWC 与 phf | 保留；上游锁定 |
| `core-foundation` 0.9 / 0.10 | macOS 系统集成依赖 | 保留；平台过渡版本 |
| `getrandom` 0.2 / 0.4 | 加密依赖与本项目 | 保留；加密链的版本边界 |

## 不应处理的 lockfile 条目

- `axum` 仅由 `test-streamable-http-server` feature 的 HTTP/OAuth 测试服务器使用；生产 CLI 默认图不启用它。
- `quinn`、`quinn-proto`、`quinn-udp` 若存在于 lockfile，是 reqwest 可选 HTTP/3 边的解析记录；不在默认生产依赖图中。
- `rustls`、`tokio-rustls` 及其下游若仍显示在 lockfile，是上游 crate 的可选 feature 解析记录；以 `cargo tree -i rustls --locked` 和 `cargo tree -i tokio-rustls --locked` 为空作为生产图验收，而不是手工删除条目。

删除这些 lockfile 条目不减少默认构建。Cargo 会在不再有解析路径时自行清理；禁止手工编辑 `Cargo.lock`。

## 构建 profile

`[profile.release]` 继续以体积优先：`opt-level = "z"`、fat LTO 和单个 codegen unit 不变，只用于正式交付。

本地接近发布的验证使用 `[profile.fast]`：

```text
cargo build --profile fast
```

该 profile 保留 release 优化与 strip，但关闭 LTO、开启 16 个 codegen unit 和增量编译。首次冷编仍会构建全部依赖；它优化的是后续修改后的增量重编，产物位于 `target/fast/`，不能替代正式发布构建。

## 2026-09 审计记录

- `Cargo.lock` 包条目：573 → 541（-32）。
- 默认生产图已不含 `base64 0.21`、`ron`、`rust-ini`、`json5`、`convert_case`、`yaml-rust2`、`onig`、`onig_sys` 或 `reqwest 0.12`；reqwest 0.13 的唯一生产 TLS 路径是显式的 `native-tls`。保留 `base64 0.22/0.23` 的原因见上文。
- 配置测试：95 通过；终端渲染测试：4 通过；HTTP OAuth 连接器测试：2 通过；`cargo machete` 无未使用的直接依赖。
- 配置/高亮瘦身后的 `cargo build --release` 成功，耗时 10m58s；reqwest 统一后的 release 构建也成功，耗时 9m31s。两次构建缓存状态不同，不能将它们当作性能对比。
- `fast` 是独立产物图，首次冷编仍需 12m27s；依赖变更后的重建为 7m53s，无改动热构建为 1.91s。不能把热构建当作“改一行”的基准；应在下一次真实源码编辑后记录增量耗时。
