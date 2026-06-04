use std::sync::Arc;

use serde_json::json;

use super::super::*;
use crate::core::agent_loop::types::SubagentType;
use crate::core::agent_loop::ConfigBackend;

struct DisplayPrimitive;

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
        Ok(crate::core::tools::primitive::WriteFileResult {
            path: "~/workspace/demo.txt".to_string(),
            written: true,
            bytes_written: 12,
            diff_hint: None,
        })
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
        SubagentType::User,
        None,
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
        })
    );
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
        SubagentType::User,
        None,
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
