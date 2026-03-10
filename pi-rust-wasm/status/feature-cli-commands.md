| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-03-10 | DONE | feature/cli-commands | 65.6 |

### TASK-02 | T1-P0-010-completion | CLI 子命令补完

**目标**：将 CLI 中仍为占位的子命令补充为真实实现。

**已完成子项**：
- [x] 10.3 `pi-awsm doctor`：补全 WasmEdge/QuickJS 可用性检测与修复建议
- [x] 10.4 `pi-awsm config`：补全 get(key)、set（加载→修改→校验→写回）、edit（启动编辑器）
- [x] 10.6 `pi-awsm plugin`：对接 PluginManager，实现 list/load/unload/enable/disable/info
- [x] 10.7 `pi-awsm audit`：实现 list/show/export，读取 tracing 日志文件过滤审计记录
- [x] 10.8 完善帮助文档与参数校验

**门禁**：
- `cargo fmt -- --check`：通过
- `cargo clippy --lib --tests`：通过（0 warnings）
- `cargo test --lib`：211 passed, 0 failed
- 覆盖率：65.6%（cli.rs 233/414）

### 接口变更

- 新增 `config_file_path`、`resolve_toml_key`、`set_toml_key` 私有函数（cli.rs 内部）
- 新增 `PluginContext`、`build_plugin_context`、`cli_confirm_permissions`、`format_plugin_info` 私有函数
- 新增 `AuditDisplayEntry`、`parse_audit_line`、`read_audit_entries` 私有函数/结构
- 无新增 pub API
