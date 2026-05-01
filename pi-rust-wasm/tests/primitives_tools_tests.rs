//! 集成测试：4 原语执行引擎与工具注册中心（005/006）。
//! 黑盒测试，仅通过 pi_wasm 公共 API；满足日志门禁（第 9 章）与鲁棒性场景（第 10 章）。

mod common;

use pi_wasm::core::permission::{DefaultPermissionGate, GateConfig, PermissionGate, SessionGrants};
use pi_wasm::{
    AllowAllConfirmation, DefaultPrimitiveExecutor, DefaultToolRegistry, DenyAllConfirmation,
    EditOperation, EditOperationType, PrimitiveConfig, PrimitiveExecutor, Tool, ToolExecutor,
    ToolRegistry, TracingAuditRecorder,
};
use std::sync::Arc;
use tempfile::TempDir;

/// 测试 helper：把 `dir` 作为 `agent_definition_dir` 注入 gate（默认 writable）。
fn make_gate(definition: &std::path::Path, auto_confirm: bool) -> Arc<dyn PermissionGate> {
    DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: definition.to_path_buf(),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm,
        },
        SessionGrants::new(),
    )
    .into_arc()
}

/// 集成测试用 stub：仅通过 pub API 注入，返回固定 JSON。
struct StubToolExecutor;

#[async_trait::async_trait]
impl ToolExecutor for StubToolExecutor {
    async fn execute(
        &self,
        _tool: &Tool,
        params: serde_json::Value,
        _caller_plugin_id: &str,
    ) -> Result<serde_json::Value, pi_wasm::AppError> {
        Ok(serde_json::json!({ "result": "ok", "params": params }))
    }
}

fn make_tool(name: &str, plugin_id: &str) -> Tool {
    Tool {
        name: name.to_string(),
        label: name.to_string(),
        description: format!("integration test tool {}", name),
        parameters: serde_json::json!({}),
        plugin_id: plugin_id.to_string(),
        is_enabled: true,
        created_at: 0,
    }
}

// ---------- ToolRegistry 集成测试 ----------

/// [ToolRegistry CRUD] 注册工具后 list_tools 含该工具、call_tool 可调用
///
/// 验证：list_tools 非空、call_tool 返回 content+details 结构
/// 意义：TASK-04 006 工具注册中心——注册、发现、调用端到端
#[tokio::test]
async fn test_tool_registry_register_list_and_call_returns_ok(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_tool_registry_register_list_and_call_returns_ok").entered();

    let registry =
        DefaultToolRegistry::new(Arc::new(StubToolExecutor), Arc::new(TracingAuditRecorder));
    tracing::info!("Arrange: DefaultToolRegistry + StubToolExecutor + TracingAuditRecorder");
    let tool = make_tool("echo", "plugin_a");
    registry.register_tool(tool, "plugin_a").await?;
    tracing::info!("Act: register_tool(echo, plugin_a), list_tools(None), call_tool(echo, ...)");
    let list = registry.list_tools(None).await?;
    assert!(!list.is_empty(), "list_tools 应包含已注册工具");
    let out = registry
        .call_tool("echo", serde_json::json!({ "x": 1 }), "plugin_a")
        .await?;
    tracing::info!("Assert: call_tool 返回 content/details 结构");
    assert!(out.get("content").is_some());
    assert!(out.get("details").is_some());
    let content = out.get("content").unwrap();
    assert!(content.get("result").is_some());
    Ok(())
}

/// [ToolRegistry 卸载] unregister_plugin_tools 移除指定插件全部工具
///
/// 验证：卸载 p1 后 get_tool(a)/(b) 返回 Err，p2 的 c 仍在
/// 意义：TASK-04 006——插件卸载时工具批量释放，不影响其他插件
#[tokio::test]
async fn test_tool_registry_unregister_plugin_tools_removes_all(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_tool_registry_unregister_plugin_tools_removes_all").entered();

    let registry =
        DefaultToolRegistry::new(Arc::new(StubToolExecutor), Arc::new(TracingAuditRecorder));
    registry.register_tool(make_tool("a", "p1"), "p1").await?;
    registry.register_tool(make_tool("b", "p1"), "p1").await?;
    registry.register_tool(make_tool("c", "p2"), "p2").await?;
    tracing::info!("Arrange: 注册 p1 的 a/b、p2 的 c");
    registry.unregister_plugin_tools("p1");
    tracing::info!("Act: unregister_plugin_tools(p1)");
    assert!(registry.get_tool("a").await.is_err());
    assert!(registry.get_tool("b").await.is_err());
    let c = registry.get_tool("c").await?;
    assert_eq!(c.name, "c");
    tracing::info!("Assert: p1 工具已移除，p2 的 c 仍在");
    Ok(())
}

// ---------- PrimitiveExecutor 集成测试（含鲁棒性：路径白名单拒绝） ----------

fn temp_whitelist_config(_dir: &std::path::Path) -> PrimitiveConfig {
    PrimitiveConfig {
        auto_confirm: true,
        ..PrimitiveConfig::default()
    }
}

/// [read_file 白名单内] 白名单内路径可正常读取文件内容
///
/// 验证：read_file 返回写入的内容 "hello"
/// 意义：TASK-03 005 read_file 正向路径——白名单机制允许合法读取
#[tokio::test]
async fn test_primitive_executor_read_file_in_whitelist_succeeds(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_primitive_executor_read_file_in_whitelist_succeeds").entered();

    let tmp = TempDir::new()?;
    let canonical_dir = tmp.path().canonicalize()?;
    let config = PrimitiveConfig::default();
    let executor = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&canonical_dir, false),
    );
    let file_path = canonical_dir.join("hello.txt");
    std::fs::write(&file_path, "hello")?;
    let path_str = file_path.to_string_lossy().to_string();
    tracing::info!("Arrange: 临时目录、白名单包含该目录、写入 hello.txt");
    let content = executor.read_file(&path_str, "test_plugin").await?;
    tracing::info!("Act: read_file(hello.txt, test_plugin)");
    assert_eq!(content, "hello");
    tracing::info!("Assert: 返回文件内容");
    Ok(())
}

/// [read_file 白名单外] 白名单外路径返回 Permission 错误
///
/// 验证：read_file 返回 Err 且信息含"白名单"或"Permission"
/// 意义：TASK-03 005 安全边界——非法路径被拦截，不得静默成功
#[tokio::test]
async fn test_primitive_executor_read_file_path_not_in_whitelist_returns_permission_error(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!(
        "test_primitive_executor_read_file_path_not_in_whitelist_returns_permission_error"
    )
    .entered();

    let config = PrimitiveConfig::default();
    let executor = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(std::path::Path::new("/nonexistent_pi_workspace"), false),
    );
    tracing::info!("Arrange: PrimitiveConfig 空 path_whitelist");
    let res = executor.read_file("/etc/hosts", "test_plugin").await;
    tracing::info!("Act: read_file(/etc/hosts, test_plugin)");
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(
        err.to_string().contains("白名单")
            || err.to_string().contains("Permission")
            || err.to_string().contains("权限"),
        "期望权限/白名单错误，got: {}",
        err
    );
    tracing::info!("Assert: 返回 Permission/白名单 错误（鲁棒性：非法路径）");
    Ok(())
}

/// [空 path_whitelist + workspace_dir] 仅 workspace 内路径可访问，workspace 外仍 Permission
///
/// 验证：read_file(workspace 内) 成功；read_file(workspace 外) 返回 Err 且为 Permission/白名单
/// 意义：INTEGRATION_TEST_SPEC 主路径覆盖——空 path_whitelist 时默认 workspace 白名单
#[tokio::test]
async fn test_primitive_executor_empty_whitelist_allows_workspace_dir_only(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_primitive_executor_empty_whitelist_allows_workspace_dir_only")
            .entered();

    let tmp = TempDir::new()?;
    let canonical_dir = tmp.path().canonicalize()?;
    let config = PrimitiveConfig::default();
    let executor = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&canonical_dir, false),
    );
    let allowed_path = canonical_dir.join("allowed.txt");
    std::fs::write(&allowed_path, "workspace_content")?;
    let allowed_str = allowed_path.to_string_lossy().to_string();

    tracing::info!("Arrange: 空 path_whitelist、workspace_dir=临时目录、allowed.txt");
    let content = executor.read_file(&allowed_str, "test_plugin").await?;
    tracing::info!("Act: read_file(workspace 内 allowed.txt)");
    assert_eq!(content, "workspace_content");
    tracing::info!("Assert: 返回文件内容");

    let res = executor.read_file("/etc/hosts", "test_plugin").await;
    tracing::info!("Act: read_file(/etc/hosts)");
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(
        err.to_string().contains("白名单")
            || err.to_string().contains("Permission")
            || err.to_string().contains("权限"),
        "workspace 外路径应返回 Permission/白名单 错误，got: {}",
        err
    );
    tracing::info!("Assert: workspace 外返回 Permission/白名单 错误");
    Ok(())
}

/// [write_file 用户确认] AllowAllConfirmation + 白名单内路径可写入文件
///
/// 验证：write_file 成功且文件内容一致
/// 意义：TASK-03 005 write_file 正向路径——确认策略与白名单联合验证
#[tokio::test]
async fn test_primitive_executor_write_file_with_allow_all_succeeds(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_primitive_executor_write_file_with_allow_all_succeeds").entered();

    let tmp = TempDir::new()?;
    let canonical_dir = tmp.path().canonicalize()?;
    let mut config = temp_whitelist_config(&canonical_dir);
    config.auto_confirm = true;
    let executor = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&canonical_dir, true),
    );
    let file_path = canonical_dir.join("out.txt");
    let path_str = file_path.to_string_lossy().to_string();
    tracing::info!("Arrange: 白名单临时目录、AllowAllConfirmation");
    executor
        .write_file(&path_str, "content", true, "test_plugin")
        .await?;
    tracing::info!("Act: write_file(out.txt, content, test_plugin)");
    let read = std::fs::read_to_string(&file_path)?;
    assert_eq!(read, "content");
    tracing::info!("Assert: 文件已写入且内容一致");
    Ok(())
}

/// [write_file 用户拒绝] DenyAllConfirmation 时写入被拒绝
///
/// 验证：write_file 返回 Err 且信息含"确认"/"Permission"/"denied"
/// 意义：TASK-03 005 安全边界——用户拒绝时写入操作不执行
#[tokio::test]
async fn test_primitive_executor_write_file_user_denied_returns_permission_error(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!(
        "test_primitive_executor_write_file_user_denied_returns_permission_error"
    )
    .entered();

    let tmp = TempDir::new()?;
    let canonical_dir = tmp.path().canonicalize()?;
    let mut config = temp_whitelist_config(&canonical_dir);
    config.auto_confirm = false;
    let executor = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(std::path::Path::new("/nonexistent_pi_workspace"), false),
    );
    let file_path = canonical_dir.join("denied.txt");
    let path_str = file_path.to_string_lossy().to_string();
    tracing::info!("Arrange: 需用户确认、DenyAllConfirmation");
    let res = executor
        .write_file(&path_str, "content", true, "test_plugin")
        .await;
    tracing::info!("Act: write_file(denied.txt, ...)");
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(
        err.to_string().contains("确认")
            || err.to_string().contains("Permission")
            || err.to_string().contains("denied"),
        "期望用户拒绝确认错误，got: {}",
        err
    );
    tracing::info!("Assert: 返回用户拒绝确认相关错误（鲁棒性：用户拒绝）");
    Ok(())
}

/// [edit_file 替换] 白名单内编辑文件替换指定内容成功
///
/// 验证：edit_file(Replace) 后文件内容被正确替换
/// 意义：TASK-03 005 edit_file 正向路径——编辑操作端到端
#[tokio::test]
async fn test_primitive_executor_edit_file_replaces_content(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_primitive_executor_edit_file_replaces_content").entered();

    let tmp = TempDir::new()?;
    let canonical_dir = tmp.path().canonicalize()?;
    let config = temp_whitelist_config(&canonical_dir);
    let executor = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&canonical_dir, false),
    );

    let file_path = canonical_dir.join("edit_target.txt");
    std::fs::write(&file_path, "hello world")?;
    let path_str = file_path.to_string_lossy().to_string();

    tracing::info!("Arrange: 白名单临时目录、写入 edit_target.txt='hello world'");
    let edits = vec![EditOperation {
        operation_type: EditOperationType::Replace,
        start_line: None,
        end_line: None,
        old_content: Some("hello".to_string()),
        new_content: "goodbye".to_string(),
    }];
    let result = executor.edit_file(&path_str, edits, "test_plugin").await?;
    tracing::info!("Act: edit_file(Replace, 'hello' -> 'goodbye')");

    assert!(result.applied, "edit_file 应返回 applied=true");
    let content = std::fs::read_to_string(&file_path)?;
    assert!(
        content.contains("goodbye"),
        "编辑后文件应含 'goodbye'，实际: {}",
        content
    );
    tracing::info!("Assert: 文件内容已替换");
    Ok(())
}

/// [execute_bash echo] 白名单目录内执行 echo 命令返回输出
///
/// 验证：execute_bash 成功且 stdout 包含 "hello"
/// 意义：TASK-03 005 execute_bash 正向路径——命令执行端到端
#[tokio::test]
async fn test_primitive_executor_execute_bash_echo_succeeds(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_primitive_executor_execute_bash_echo_succeeds").entered();

    let tmp = TempDir::new()?;
    let canonical_dir = tmp.path().canonicalize()?;
    let config = temp_whitelist_config(&canonical_dir);
    let executor = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&canonical_dir, false),
    );

    tracing::info!("Arrange: 白名单临时目录");
    let result = executor
        .execute_bash(
            "echo hello",
            Some(canonical_dir.to_str().unwrap()),
            "test_plugin",
            None,
        )
        .await?;
    tracing::info!("Act: execute_bash('echo hello')");

    assert_eq!(result.exit_code, 0, "echo 应以 exit_code=0 退出");
    assert!(
        result.stdout.contains("hello"),
        "stdout 应包含 'hello'，实际: {}",
        result.stdout
    );
    tracing::info!("Assert: exit_code=0, stdout 含 'hello'");
    Ok(())
}

/// [execute_bash argv] pi-mono 风格 command + args 不经 shell 拼接
#[tokio::test]
async fn test_primitive_executor_execute_bash_argv_echo() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let tmp = TempDir::new()?;
    let canonical_dir = tmp.path().canonicalize()?;
    let config = temp_whitelist_config(&canonical_dir);
    let executor = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(&canonical_dir, false),
    );
    let argv = vec!["hello".to_string(), "argv".to_string()];
    let result = executor
        .execute_bash(
            "echo",
            Some(canonical_dir.to_str().unwrap()),
            "test_plugin",
            Some(&argv),
        )
        .await?;
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("hello"));
    assert!(result.stdout.contains("argv"));
    Ok(())
}
