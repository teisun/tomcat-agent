use std::sync::Arc;

use serde_json::json;

use super::super::*;
use crate::core::agent_loop::types::SubagentType;
use crate::core::agent_loop::ConfigBackend;

struct DisplayPrimitive;

fn sample_diff() -> Vec<crate::core::tools::primitive::FileDiffLine> {
    vec![
        crate::core::tools::primitive::FileDiffLine {
            tag: crate::core::tools::primitive::DiffTag::Add,
            old_line: None,
            new_line: Some(1),
            skipped_lines: None,
            text: "hello".to_string(),
        },
        crate::core::tools::primitive::FileDiffLine {
            tag: crate::core::tools::primitive::DiffTag::Add,
            old_line: None,
            new_line: Some(2),
            skipped_lines: None,
            text: "world".to_string(),
        },
        crate::core::tools::primitive::FileDiffLine {
            tag: crate::core::tools::primitive::DiffTag::Add,
            old_line: None,
            new_line: Some(3),
            skipped_lines: None,
            text: "!".to_string(),
        },
    ]
}

#[async_trait::async_trait]
impl PrimitiveExecutor for DisplayPrimitive {
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
        Ok("hello\nworld\n!\n".to_string())
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
        Ok(crate::core::tools::primitive::WriteFileResult {
            path: "~/workspace/demo.txt".to_string(),
            written: true,
            bytes_written: 12,
            diff_hint: None,
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
        })
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
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
        })
    }

    async fn execute_bash(
        &self,
        _command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _foreground_wait_ms: Option<u64>,
    ) -> Result<crate::core::tools::primitive::BashResult, AppError> {
        unreachable!()
    }

    async fn hashline_edit(
        &self,
        path: &str,
        _segments: Vec<crate::core::tools::primitive::HashlineSegment>,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
        Ok(crate::core::tools::primitive::EditFileResult {
            path: path.to_string(),
            applied: true,
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
        })
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

struct DisplayConfigBackend;

#[async_trait::async_trait]
impl ConfigBackend for DisplayConfigBackend {
    async fn config_get(&self, _key: &str) -> Result<serde_json::Value, AppError> {
        unreachable!()
    }

    async fn config_set(&self, _key: &str, _value: &str) -> Result<serde_json::Value, AppError> {
        Ok(json!({
            "applied": true,
            "message": "已设置 llm.default_model = gpt-5.4"
        }))
    }
}

#[tokio::test]
async fn write_success_populates_file_display() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let tc = ToolCallInfo {
        id: "w1".into(),
        name: "write".into(),
        arguments: json!({
            "path": "~/workspace/demo.txt",
            "content": "hello",
            "overwrite": false
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
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    assert_eq!(
        outcome.display,
        Some(ToolDisplay::File {
            file: "~/workspace/demo.txt".to_string(),
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
            expired: false,
        })
    );
}

#[tokio::test]
async fn edit_success_populates_file_display() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let tc = ToolCallInfo {
        id: "edit1".into(),
        name: "edit".into(),
        arguments: json!({
            "path": "~/workspace/demo.txt",
            "old_content": "before",
            "new_content": "after"
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
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    assert_eq!(
        outcome.display,
        Some(ToolDisplay::File {
            file: "~/workspace/demo.txt".to_string(),
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
            expired: false,
        })
    );
}

#[tokio::test]
async fn hashline_edit_success_populates_file_display() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let tc = ToolCallInfo {
        id: "hedit1".into(),
        name: "hashline_edit".into(),
        arguments: json!({
            "path": "~/workspace/demo.txt",
            "edits": [{
                "op": "replace",
                "pos": "1#ab",
                "end": "1#ab",
                "lines": "after"
            }]
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
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    assert_eq!(
        outcome.display,
        Some(ToolDisplay::File {
            file: "~/workspace/demo.txt".to_string(),
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
            expired: false,
        })
    );
}

#[tokio::test]
async fn batch_edit_with_one_file_still_populates_file_display() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let tc = ToolCallInfo {
        id: "edit-batch-1".into(),
        name: "edit".into(),
        arguments: json!({
            "files": [{
                "path": "~/workspace/demo.txt",
                "old_content": "before",
                "new_content": "after"
            }]
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
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    assert_eq!(
        outcome.display,
        Some(ToolDisplay::File {
            file: "~/workspace/demo.txt".to_string(),
            added: Some(3),
            removed: Some(0),
            diff: Some(sample_diff()),
            diff_truncated: false,
            expired: false,
        })
    );
}

#[tokio::test]
async fn batch_edit_with_multiple_files_uses_files_display() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let tc = ToolCallInfo {
        id: "edit-batch-2".into(),
        name: "edit".into(),
        arguments: json!({
            "files": [
                {
                    "path": "~/workspace/demo-a.txt",
                    "old_content": "before",
                    "new_content": "after"
                },
                {
                    "path": "~/workspace/demo-b.txt",
                    "old_content": "before",
                    "new_content": "after"
                }
            ]
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
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    match outcome.display {
        Some(ToolDisplay::Files { files, .. }) => {
            assert_eq!(files.len(), 2);
            assert_eq!(files[0].file, "~/workspace/demo-a.txt");
            assert_eq!(files[1].file, "~/workspace/demo-b.txt");
        }
        other => panic!("expected Files display, got {other:?}"),
    }
}

#[tokio::test]
async fn config_set_success_populates_text_display() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(DisplayPrimitive);
    let backend: SharedConfigBackend = Arc::new(DisplayConfigBackend);
    let config_backend = Some(backend);
    let tc = ToolCallInfo {
        id: "cfg1".into(),
        name: "config_set".into(),
        arguments: json!({
            "key": "llm.default_model",
            "value": "gpt-5.4"
        })
        .to_string(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &config_backend,
        &None,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    assert_eq!(
        outcome.display,
        Some(ToolDisplay::Text {
            text: "已设置 llm.default_model = gpt-5.4".to_string(),
        })
    );
}
