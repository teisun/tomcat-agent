| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| wasm_plugin_agent | 2026-03-08 10:15 | DONE | feature/wasm-plugin | - |

**PLAN.md 防遗漏表述已更新**：已改为列表与分段表述、无表格，见 [agents/PLAN.md](../agents/PLAN.md)。

### ✅ 007/008 规范审查与补漏（宪法流程）
- [✓] 导出 `invoke_host_func_with`（ext/mod.rs、lib.rs），与 INTERFACE 一致。
- [✓] 更新 `docs/02-wasm-runtime-and-plugin.md`：真实 WasmEdge（默认构建即包含）、Node 兼容层、线性内存边界说明。
- [✓] 新增 `tests/hostcall_tests.rs`：Hostcall 全链路集成测试（仅公共 API）。
- [✓] `instance_wasmedge.rs` 中 `host_call_impl` 注释：响应缓冲区不足与线性内存边界由 WasmEdge 保证。
- [✓] 全量单测通过（`cargo test --all`）；提交前需跑 `cargo tarpaulin --packages pi_awsm` 取覆盖率并填 commit message。

### ✅ 完整研发流程与全模块单测补全（2026-03-07）
- [✓] **api/cli.rs**：解析（Cli::try_parse_from init/doctor/config/session/plugin/audit/chat）、run_init（temp 目录生成配置）、run_doctor（None/Some 合法配置）、run_plugin/run_audit/run_chat 占位；run_* 改为 pub(crate) 便于同 crate 单测。
- [✓] **ext/dispatcher.rs**：Mock PrimitiveExecutor/LlmProvider/ToolRegistry；do_read_file/do_write_file/do_edit_file/do_execute_bash、do_chat/do_chat_stream、do_register_tool/do_list_tools/do_call_tool、do_get_current_session/do_get_messages/do_send_message 成功路径（SessionManager 用 tempdir + create_session）。
- [✓] **core/session**：manager 补 from_sessions_dir、transcript_path、get_session Some  after create、read_session_header；transcript 补 read_header 失败（缺失/空文件）、read_entries_tail 仅 header、get_branch/get_children 边界；write_header_and_read_header 改用 tempfile::tempdir 避免并行冲突。
- [✓] **core/llm/openai.rs**：is_retriable 对非 Llm 错误返回 false。
- [✓] **core/executor.rs**：list_dir 路径在黑名单返回 Err、read_file 对目录返回 Err。
- [✓] **ext/plugin.rs**：get_plugin 注册后 Some/未知 None、register_plugin 重复返回 Err。
- [✓] 全量 lib 单测 144 passed、1 ignored；提交前本地执行 `cargo tarpaulin --lib --packages pi_awsm` 取覆盖率填 commit message `[cov = xx.x%]`。
- [✓] **宪法流程走查**（2026-03-07）：开发前（分支、同步 develop）、开发流程验证（test/tarpaulin/文档）、提交前（status 更新、全量 add、门禁）、提交规约（commit 含 [cov]）。session CLI 单测使用 canonicalize 与二次 set_var 稳定 env，建议 `cargo test -- --test-threads=1` 或 CI 单线程跑以避竞态。

### ✅ 注释规范整改与 wasm quickjs 路径配置（2026-03-07）
- [✓] **配置**：`AppConfig.wasm`（`WasmConfig`）、`quickjs_path` 纳入 config；`config.toml` / `config.toml.example` / `.env.example` 增加 `[wasm]` 与 `PI_AWSM__WASM__QUICKJS_PATH` 说明；`WasmEngineConfig.quickjs_path`、engine/instance 贯通，优先 config 再回退 `WASMEDGE_QUICKJS_PATH`。
- [✓] **注释**：按 COMMENT_SPEC 为 engine_wasmedge、instance_wasmedge、host_binding、dispatcher 补充 `# Errors`/`# Arguments`/`# Returns`；dispatcher 中 `Runtime::new().expect` 增加说明。

### ✅ 提交规范与文档（2026-03-08）
- [✓] **Commit Message**：Constitution 附录增加 what+why 示例；新增 [COMMIT_MESSAGE_SPEC.md](../openspec/specs/guides/COMMIT_MESSAGE_SPEC.md)，commit-guard、commit-with-status 引用该规范；详细描述须写动机、作用与意义，禁止流水账。
- [✓] **资源**：assets/wasm/wasmedge_quickjs.wasm 纳入仓库，便于本地与 CI 使用配置路径。

### ✅ DONE (已完成)
- [✓] **[P0]** T1-P0-007 WasmEdge 运行时与 QuickJS 集成：WasmEngine/WasmInstance 桩、宿主导入绑定骨架（HostRequest/HostResponse、invoke_host_func）、Standard 资源上限预留 @2025-03-05
- [✓] **T1-P0-007 真实实现（第 4 波次）**（2026-03-07）：默认构建即包含真实 WasmEngine 单例（Config + WASI/统计/内存上限）、WasmInstance 每插件独立 Vm、宿主导入 `env.__pi_host_call`（线性内存 get_data/set_data 与边界校验）、run_script 通过 wasmedge_quickjs.wasm（需设置 `WASMEDGE_QUICKJS_PATH`）、`set_memory_limit` 预留。需先安装 WasmEdge C 库（见 https://wasmedge.org/docs/start/install）。7.6 跨平台：Windows/macOS/Linux 各需在对应环境安装 WasmEdge 后执行 `cargo build` 验证。
- [✓] **[P0]** T1-P0-008 宿主 API 层与 JS 绑定：HostApiDispatcher 单入口多路复用、core Trait（PrimitiveExecutor/ToolRegistry/LlmProvider）定义、log/fs/llm/tools/events 路由与占位、invoke_host_func_with 接入 @2025-03-05
- [✓] **T1-P0-008 第 4 波次落地**（2026-03-07）：协议与 DTO 保持 camelCase；Dispatcher 实现 4 原语、LLM、工具、事件、会话 API 真实调用（do_read_file / do_write_file / do_edit_file / do_execute_bash、do_chat / do_chat_stream、do_register_tool / do_unregister_tool / do_list_tools / do_call_tool、do_events on/once/off/emit、session getCurrentSession / getMessages / sendMessage）；新增 `dispatch_async` 异步入口，同步 `dispatch` 使用独立 Runtime block_on；注入 SessionManager（with_session）与 AuditRecorder（with_audit）；每笔 Hostcall 审计（HostcallAuditEntry、record_hostcall）；错误统一透传为 HostResponse::err；单测与 host_binding 集成测试通过。
- [✓] **[P0]** T1-P0-009 插件生命周期管理：PluginManifest/PluginInstance/PluginStatus、parse_manifest 与校验、PluginManager 注册/启用/禁用/卸载、EventBus.remove_plugin_listeners 与 ToolRegistry.unregister_plugin_tools 清理 @2025-03-05
- [✓] 技术文档：`docs/02-wasm-runtime-and-plugin.md` 已编写

### 🔌 INTERFACE (接口变更)
- **ext 层**：`HostApiDispatcher` 新增 `with_session(s: Arc<SessionManager>)`、`with_audit(a: Arc<dyn AuditRecorder>)`；新增异步入口 `dispatch_async(instance_id, request) -> impl Future<Output = Result<HostResponse, AppError>>`；`dispatch` 保持同步，内部使用 `Runtime::new().block_on(dispatch_async(...))`。
- **infra 层**：`AuditRecorder` 新增 `record_hostcall(entry: HostcallAuditEntry)`；新增类型 `HostcallAuditEntry`（plugin_id, module, method, success, detail）。
- **ext 层（沿用）**：`WasmEngine`、`WasmEngineConfig`、`WasmInstance`、`HostRequest`、`HostResponse`、`invoke_host_func`、`invoke_host_func_with`、`PluginManager`、`PluginManifest`、`PluginInstance`、`PluginStatus`、`PluginInfo`、`parse_manifest`。
- **infra 层**：`AppConfig.wasm.quickjs_path` 纳入配置；优先级与现有一致：默认值 → config 文件 → 环境变量 `PI_AWSM__WASM__QUICKJS_PATH`（env 覆盖 config）；未配置时 instance 回退 `WASMEDGE_QUICKJS_PATH`。`WasmEngineConfig.quickjs_path` 由调用方从 `AppConfig.wasm` 传入。
- **core 层**：`PrimitiveExecutor`、`ToolRegistry`、`LlmProvider`、`SessionManager` 及配套类型，供 008 分发与 009 卸载对接。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
