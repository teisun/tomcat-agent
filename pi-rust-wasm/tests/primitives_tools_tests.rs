//! 集成测试：4 原语执行引擎与工具注册中心（005/006）。
//! 黑盒测试，仅通过 pi_awsm 公共 API；满足日志门禁（第 9 章）与鲁棒性场景（第 10 章）。

mod common;

use pi_awsm::{
    AllowAllConfirmation, DefaultPrimitiveExecutor, DefaultToolRegistry, DenyAllConfirmation,
    PrimitiveConfig, PrimitiveExecutor, Tool, ToolExecutor, ToolRegistry, TracingAuditRecorder,
};
use std::sync::Arc;
use tempfile::TempDir;

/// 集成测试用 stub：仅通过 pub API 注入，返回固定 JSON。
struct StubToolExecutor;

#[async_trait::async_trait]
impl ToolExecutor for StubToolExecutor {
    async fn execute(
        &self,
        _tool: &Tool,
        params: serde_json::Value,
        _caller_plugin_id: &str,
    ) -> Result<serde_json::Value, pi_awsm::AppError> {
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

#[tokio::test]
async fn test_tool_registry_register_list_and_call_returns_ok() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_tool_registry_register_list_and_call_returns_ok").entered();

    let registry = DefaultToolRegistry::new(
        Arc::new(StubToolExecutor),
        Arc::new(TracingAuditRecorder),
    );
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

#[tokio::test]
async fn test_tool_registry_unregister_plugin_tools_removes_all() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_tool_registry_unregister_plugin_tools_removes_all").entered();

    let registry = DefaultToolRegistry::new(
        Arc::new(StubToolExecutor),
        Arc::new(TracingAuditRecorder),
    );
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

fn temp_whitelist_config(dir: &std::path::Path) -> PrimitiveConfig {
    let mut c = PrimitiveConfig::default();
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let path = canonical.to_string_lossy().trim_end_matches(std::path::MAIN_SEPARATOR).to_string();
    c.path_whitelist.push(path);
    c.auto_confirm = true;
    c.require_approval_for_all_write = false;
    c.require_approval_for_all_bash = false;
    c
}

#[tokio::test]
async fn test_primitive_executor_read_file_in_whitelist_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_primitive_executor_read_file_in_whitelist_succeeds").entered();

    let tmp = TempDir::new()?;
    let canonical_dir = tmp.path().canonicalize()?;
    let path_whitelist = canonical_dir.to_string_lossy().trim_end_matches(std::path::MAIN_SEPARATOR).to_string();
    let mut config = PrimitiveConfig::default();
    config.path_whitelist.push(path_whitelist);
    let executor = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
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
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
    );
    tracing::info!("Arrange: PrimitiveConfig 空 path_whitelist");
    let res = executor.read_file("/etc/hosts", "test_plugin").await;
    tracing::info!("Act: read_file(/etc/hosts, test_plugin)");
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(
        err.to_string().contains("白名单") || err.to_string().contains("Permission"),
        "期望权限/白名单错误，got: {}",
        err
    );
    tracing::info!("Assert: 返回 Permission/白名单 错误（鲁棒性：非法路径）");
    Ok(())
}

#[tokio::test]
async fn test_primitive_executor_write_file_with_allow_all_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_primitive_executor_write_file_with_allow_all_succeeds").entered();

    let tmp = TempDir::new()?;
    let canonical_dir = tmp.path().canonicalize()?;
    let mut config = temp_whitelist_config(&canonical_dir);
    config.auto_confirm = true;
    let executor = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
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

#[tokio::test]
async fn test_primitive_executor_write_file_user_denied_returns_permission_error(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span = tracing::info_span!("test_primitive_executor_write_file_user_denied_returns_permission_error")
        .entered();

    let tmp = TempDir::new()?;
    let canonical_dir = tmp.path().canonicalize()?;
    let mut config = temp_whitelist_config(&canonical_dir);
    config.auto_confirm = false;
    config.require_approval_for_all_write = true;
    let executor = DefaultPrimitiveExecutor::new(
        config,
        Arc::new(DenyAllConfirmation),
        Arc::new(TracingAuditRecorder),
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
        err.to_string().contains("确认") || err.to_string().contains("Permission") || err.to_string().contains("denied"),
        "期望用户拒绝确认错误，got: {}",
        err
    );
    tracing::info!("Assert: 返回用户拒绝确认相关错误（鲁棒性：用户拒绝）");
    Ok(())
}
