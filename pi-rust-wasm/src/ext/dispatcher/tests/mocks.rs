//! # `HostApiDispatcher` 测试共享 fixture
//!
//! 把 `MockPrimitive`、`MockLlm`、`MockToolRegistry` 三个跨文件复用的测试替身集中在此，
//! 避免在每个主题文件里重复定义；同时提供 `make_dispatcher_with_primitive` 给 async
//! 路径使用。

use std::sync::Arc;

use tokio::runtime::Handle;

use super::super::HostApiDispatcher;
use crate::core::{
    BashResult, ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, DirEntry,
    EditFileResult, EditOperation, LlmProvider, PrimitiveExecutor, PrimitiveOperation, StreamEvent,
    Tool, ToolRegistry, WriteFileResult,
};
use crate::infra::error::AppError;
use crate::infra::DefaultEventBus;

pub(super) struct MockPrimitive;

#[async_trait::async_trait]
impl PrimitiveExecutor for MockPrimitive {
    async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, AppError> {
        Ok("mock_content".to_string())
    }
    async fn list_dir(&self, _path: &str, _plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        Ok(vec![])
    }
    async fn write_file(
        &self,
        path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        Ok(WriteFileResult {
            path: path.to_string(),
            written: true,
        })
    }
    async fn edit_file(
        &self,
        path: &str,
        _edits: Vec<EditOperation>,
        _plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        Ok(EditFileResult {
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
    ) -> Result<BashResult, AppError> {
        Ok(BashResult {
            stdout: "ok".to_string(),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    async fn require_user_confirmation(
        &self,
        _op: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}

pub(super) struct MockLlm;

#[async_trait::async_trait]
impl LlmProvider for MockLlm {
    fn provider_name(&self) -> &str {
        "mock"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Ok(ChatResponse {
            id: Some("id".to_string()),
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant("hi"),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })
    }
    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn futures_util::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        use futures_util::stream;
        Ok(Box::new(stream::iter(vec![Ok(
            StreamEvent::ContentDelta {
                delta: "hi".to_string(),
            },
        )])))
    }
    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

pub(super) struct MockToolRegistry;

#[async_trait::async_trait]
impl ToolRegistry for MockToolRegistry {
    async fn register_tool(&self, _tool: Tool, _plugin_id: &str) -> Result<(), AppError> {
        Ok(())
    }
    async fn unregister_tool(&self, _name: &str, _plugin_id: &str) -> Result<(), AppError> {
        Ok(())
    }
    async fn get_tool(&self, _name: &str) -> Result<Tool, AppError> {
        Err(AppError::Tool("not found".to_string()))
    }
    async fn list_tools(&self, _plugin_id: Option<&str>) -> Result<Vec<Tool>, AppError> {
        Ok(vec![])
    }
    async fn call_tool(
        &self,
        _name: &str,
        _params: serde_json::Value,
        _plugin_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        Ok(serde_json::json!({ "content": "ok", "details": null }))
    }
    fn unregister_plugin_tools(&self, _plugin_id: &str) {}
}

pub(super) fn make_dispatcher_with_primitive() -> HostApiDispatcher {
    let bus = Arc::new(DefaultEventBus::new());
    HostApiDispatcher::new(bus)
        .with_tokio_handle(Handle::current())
        .with_primitive(Arc::new(MockPrimitive))
}
