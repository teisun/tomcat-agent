| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-03-24 20:30 | DONE | feature/init-experience | - |

### TASK-06 初始化体验与资源内嵌（T1-P1-003）

- [✓] **[P1]** 内嵌 pi_bridge.js / wasmedge_quickjs.wasm / assets/modules，编译期 SHA-256（build.rs）、`ensure_embedded_assets` + 文件锁 + 原子写入
- [✓] **[P1]** `pi init` 交互向导、幂等、旧目录 `~/.pi_wasm/` 迁移、`.env` 与 dotenvy
- [✓] **[P1]** `pi doctor` 增强（内嵌资源、.versions.json、.env 权限、API Key 提示）
- [✓] **[P1]** `standalone` 保留为可选 `--features standalone`，默认使用系统 WasmEdge 以缩短编译时间
- [✓] **[P1]** 单元测试（config）、CLI 集成测试（init/doctor/资源释放）、Wasm E2E 39 项回归通过
- [✓] **[P1]** 文档：user-guide、directory-structure、work-dir-and-data-layout、init-experience 报告、E2E_SCENARIO_LIBRARY

### 🔌 INTERFACE（接口变更）

- `ensure_embedded_assets(cfg: &AppConfig) -> Result<(), AppError>`：库入口，启动与 init 后释放内嵌资源
- `Cargo.toml`：`[features]` 默认 `[]`，`standalone = ["wasmedge-sdk/standalone"]` 显式启用

### ⚠️ BLOCKED（阻塞/风险）

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
