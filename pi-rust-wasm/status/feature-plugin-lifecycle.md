| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-03-09 | DONE | feature/plugin-lifecycle | 待测 |

说明：Cov% 需本地执行 `cargo tarpaulin --lib --packages pi_awsm` 后填入；宪法要求 ≥85%。

### TASK-01 T1-P0-009-completion 插件生命周期 — 补完加载流程（9.2）

- [✓] **PluginInstance** 增加 `plugin_root: PathBuf`、`main_script_path()`，所有构造处（含单测与 tests/）已更新。
- [✓] **PluginManager** 增加 `set_wasm_engine`、`set_host_dispatcher`、`set_confirm_permissions`；类型 `ConfirmPermissionsFn`。
- [✓] **load_plugin(path)**：解析路径（目录找 plugin.json/pi-plugin.json，文件则父目录为根）→ 读清单与 main 脚本 → 权限确认回调（可选）→ 创建 Wasm 实例 → 注册 host binding（HostApiDispatcher）→ 执行初始化脚本 → 注册并 enable。main 路径校验不逃逸插件根。
- [✓] 单测：load_plugin 未设置 wasm_engine、路径不存在、目录无清单、用户拒绝权限；全量 lib + 集成测试通过；rustfmt/clippy 通过。
- [✓] 技术文档：docs/02-wasm-runtime-and-plugin.md 已增「4. 插件完整加载流程（9.2）」与 2 节中 9.2 要点。

### 接口变更

- **ext/plugin**：`PluginManager::load_plugin(path)`、`set_wasm_engine`、`set_host_dispatcher`、`set_confirm_permissions`；`PluginInstance::plugin_root`、`main_script_path()`；`ConfirmPermissionsFn`。
