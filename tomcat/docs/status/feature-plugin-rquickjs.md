| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-06-14 15:55 | ACTIVE | feature/plugin-rquickjs | — |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** 移除 WasmEdge 运行时、资产与脚本，统一 Plugin 命名 @2026-06-14
- [✓] **[P0]** 插件加载默认放行 requiredPermissions @2026-06-14
- [✓] **[P0]** 原生 aes-gcm / ed25519 与 JS crypto shim 接线 @2026-06-14
- [✓] **[P1]** 机会主义 idle VM 回收策略与配置项 @2026-06-14
- [✓] **[P1]** 引擎实现迁入 `src/ext/runtime/` 抽象边界 @2026-06-14
- [✓] **[P1]** 测试补全：runtime_manager、crypto、chat ctx、tool executor、doctor、plugin unload @2026-06-14
- [✓] **[P2]** 并发测试 cwd_lock 消除 PoisonError @2026-06-14
- [✓] **[P2]** 文档与配置表更新（plugin-system-overview、user-guide、tomcat.config.toml） @2026-06-14

### 🔌 INTERFACE (接口变更)
- `crate::ext::runtime/`：引擎/实例/crypto 迁入子模块，对外仍通过 `crate::ext::PluginEngine` 等再导出。
- `PluginConfig` / `PluginEngineConfig`：新增 idle TTL、heap/timeout 等运行时配置项（见 `tomcat.config.toml` 与 user-guide）。
- `crypto` JS shim：新增 `aesGcm.*` / `ed25519.*` 命名空间 API。
- `plugin unload`：仅注册于 registry 的插件也会从 registry.json 移除。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | — | — |
