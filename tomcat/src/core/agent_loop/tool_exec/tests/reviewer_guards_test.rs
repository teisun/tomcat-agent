#![allow(clippy::await_holding_lock)]

use std::sync::Arc;

use serial_test::serial;

use super::super::*;
use crate::core::agent_loop::types::SubagentType;

struct UnusedPrimitive;

#[async_trait::async_trait]
impl PrimitiveExecutor for UnusedPrimitive {
    async fn read(
        &self,
        _path: &str,
        _offset: Option<u64>,
        _limit: Option<u64>,
        _line_numbers: bool,
        _hashline: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::ReadResult, AppError> {
        unreachable!("reviewer guard 应在 primitive 之前 short-circuit")
    }
    async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, AppError> {
        unreachable!()
    }
    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::core::tools::primitive::DirEntry>, AppError> {
        unreachable!()
    }
    async fn write_file(
        &self,
        _path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::WriteFileResult, AppError> {
        unreachable!()
    }
    async fn edit_file(
        &self,
        _path: &str,
        _edits: Vec<crate::core::tools::primitive::EditOperation>,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
        unreachable!()
    }
    async fn execute_bash(
        &self,
        _command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
        _timeout_ms_override: Option<u64>,
    ) -> Result<crate::core::tools::primitive::BashResult, AppError> {
        unreachable!()
    }
    async fn hashline_edit(
        &self,
        _path: &str,
        _segments: Vec<crate::core::tools::primitive::HashlineSegment>,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
        unreachable!()
    }
    async fn search_files(
        &self,
        _args: crate::core::tools::primitive::SearchFilesArgs,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::SearchFilesOutput, AppError> {
        unreachable!()
    }
    async fn require_user_confirmation(
        &self,
        _operation: crate::core::tools::primitive::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        unreachable!()
    }
}

struct EditOkPrimitive;

#[async_trait::async_trait]
impl PrimitiveExecutor for EditOkPrimitive {
    async fn read(
        &self,
        _path: &str,
        _offset: Option<u64>,
        _limit: Option<u64>,
        _line_numbers: bool,
        _hashline: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::ReadResult, AppError> {
        unreachable!()
    }
    async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, AppError> {
        unreachable!()
    }
    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::core::tools::primitive::DirEntry>, AppError> {
        unreachable!()
    }
    async fn write_file(
        &self,
        _path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::WriteFileResult, AppError> {
        unreachable!()
    }
    async fn edit_file(
        &self,
        path: &str,
        _edits: Vec<crate::core::tools::primitive::EditOperation>,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
        Ok(crate::core::tools::primitive::EditFileResult {
            path: path.to_string(),
            applied: true,
        })
    }
    async fn execute_bash(
        &self,
        _command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
        _timeout_ms_override: Option<u64>,
    ) -> Result<crate::core::tools::primitive::BashResult, AppError> {
        unreachable!()
    }
    async fn hashline_edit(
        &self,
        _path: &str,
        _segments: Vec<crate::core::tools::primitive::HashlineSegment>,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
        unreachable!()
    }
    async fn search_files(
        &self,
        _args: crate::core::tools::primitive::SearchFilesArgs,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::SearchFilesOutput, AppError> {
        unreachable!()
    }
    async fn require_user_confirmation(
        &self,
        _operation: crate::core::tools::primitive::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        unreachable!()
    }
}

#[tokio::test]
async fn reviewer_blocks_non_whitelisted_tool() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(UnusedPrimitive);
    let tc = ToolCallInfo {
        id: "tc1".into(),
        name: "bash".into(),
        arguments: "{}".into(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        SubagentType::Reviewer,
        Some(crate::core::plan_runtime::review::ReviewKind::Plan),
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(outcome.is_error);
    assert!(outcome
        .model_text
        .contains("reviewer 子 Agent 禁止调用工具"));
}

#[tokio::test]
async fn reviewer_blocks_web_search_tool() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(UnusedPrimitive);
    let tc = ToolCallInfo {
        id: "tc_web_search".into(),
        name: "web_search".into(),
        arguments: r#"{"query":"rust tokio tutorial"}"#.into(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        SubagentType::Reviewer,
        Some(crate::core::plan_runtime::review::ReviewKind::Plan),
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(outcome.is_error);
    assert!(outcome
        .model_text
        .contains("reviewer 子 Agent 禁止调用工具 `web_search`"));
}

#[tokio::test]
async fn code_reviewer_blocks_write_capable_tools() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(UnusedPrimitive);
    let tc = ToolCallInfo {
        id: "tc_code_edit".into(),
        name: "edit".into(),
        arguments: r#"{"path":"~/.tomcat/plans/demo.plan.md","old_string":"a","new_string":"b"}"#
            .into(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        SubagentType::Reviewer,
        Some(crate::core::plan_runtime::review::ReviewKind::Code),
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(outcome.is_error);
    assert!(outcome
        .model_text
        .contains("仅允许 read/search_files/list_dir/bash"));
}

#[tokio::test]
async fn reviewer_blocks_create_plan_subagent() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(UnusedPrimitive);
    let tc = ToolCallInfo {
        id: "tc1".into(),
        name: "create_plan".into(),
        arguments: "{}".into(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        SubagentType::Reviewer,
        Some(crate::core::plan_runtime::review::ReviewKind::Plan),
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(outcome.is_error);
    assert!(outcome
        .model_text
        .contains("reviewer 子 Agent 禁止调用工具 `create_plan`"));
}

#[tokio::test]
async fn verifier_blocks_non_whitelisted_tools() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(UnusedPrimitive);
    for (name, arguments) in [
        ("update_plan", "{}"),
        ("web_search", r#"{"query":"rust async"}"#),
        ("write", r#"{"path":"/tmp/demo.txt","content":"hi"}"#),
        (
            "edit",
            r#"{"path":"/tmp/demo.txt","old_string":"a","new_string":"b"}"#,
        ),
    ] {
        let tc = ToolCallInfo {
            id: format!("tc_{name}"),
            name: name.into(),
            arguments: arguments.into(),
        };
        let outcome = execute_tool_full(
            &primitive,
            &None,
            &None,
            None,
            None,
            None,
            None,
            SubagentType::Verifier,
            None,
            &tokio_util::sync::CancellationToken::new(),
            &tc,
            None,
            None,
        )
        .await;
        assert!(outcome.is_error, "{name} 应被 verifier 白名单拒绝");
        assert!(
            outcome
                .model_text
                .contains(&format!("verifier 子 Agent 禁止调用工具 `{name}`")),
            "{name} 拒绝文案异常: {}",
            outcome.model_text
        );
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn reviewer_edit_precheck_accepts_tilde_plan_path() {
    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let old_home = std::env::var("HOME").ok();
    let temp_home = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", temp_home.path());

    struct HomeGuard(Option<String>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(home) => std::env::set_var("HOME", home),
                None => std::env::remove_var("HOME"),
            }
        }
    }
    let _guard = HomeGuard(old_home);

    let plan_id = "reviewer_tilde_smoke";
    let plan_path = crate::core::plan_runtime::file_store::plan_path_for_id(plan_id).unwrap();
    std::fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
    std::fs::write(
        &plan_path,
        "---\nplan_id: reviewer_tilde_smoke\ngoal: smoke\nmode: planning\nschema_version: 1\ntodos: []\n---\n## Goal\n\nsmoke\n\n## Notes\n\nold note\n\n## Todos Board\n\n<!-- todos-board:auto:begin -->\n<!-- todos-board:auto:end -->\n",
    )
    .unwrap();

    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(EditOkPrimitive);
    let tc = ToolCallInfo {
        id: "tc1".into(),
        name: "edit".into(),
        arguments: serde_json::json!({
            "path": format!("~/.tomcat/plans/{plan_id}.plan.md"),
            "old_content": "## Goal\n\nsmoke\n\n## Notes",
            "new_content": "## Goal\n\nupdated smoke\n\n## Notes"
        })
        .to_string(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        SubagentType::Reviewer,
        Some(crate::core::plan_runtime::review::ReviewKind::Plan),
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;

    assert!(
        !outcome.is_error,
        "unexpected error: {}",
        outcome.model_text
    );
    assert!(outcome
        .model_text
        .contains("已编辑: ~/.tomcat/plans/reviewer_tilde_smoke.plan.md"));
}
