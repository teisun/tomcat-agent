//! # 测试 Mock 设施（共享）
//!
//! 所有 `agent_loop::tests::*` 子 mod 共用的 LLM / Primitive Mock。
//! 维持原 `tests.rs` 中 Mock 的字段、行为、错误返回完全一致；调用方通过
//! `use super::mocks::*;` 引入。

use std::sync::Arc;
use std::sync::Mutex;

use crate::core::llm::ChatMessage;
use crate::core::llm::{ChatRequest, ChatResponse, LlmProvider, StreamEvent};
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::infra::error::AppError;

/// 标准流式 Mock：构造时传入"每次调用"返回的事件序列；调用时按顺序消费。
pub(super) struct MockLlmProvider {
    streams: Mutex<Vec<Vec<Result<StreamEvent, AppError>>>>,
}

impl MockLlmProvider {
    pub(super) fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: Mutex::new(streams),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for MockLlmProvider {
    fn provider_name(&self) -> &str {
        "mock"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("mock chat not used".to_string()))
    }
    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        let mut guard = self.streams.lock().unwrap();
        let events = guard.remove(0);
        drop(guard);
        let stream = tokio_stream::iter(events);
        Ok(Box::new(stream))
    }
    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

/// `chat_stream` 直接返回 Err（用于 Fatal 401 / 503 等"系统错误"测试）。
pub(super) struct MockLlmProviderFatal {
    pub(super) err: String,
}

#[async_trait::async_trait]
impl LlmProvider for MockLlmProviderFatal {
    fn provider_name(&self) -> &str {
        "mock_fatal"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("mock chat not used".to_string()))
    }
    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        Err(AppError::Llm(self.err.clone()))
    }
    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

/// 默认 Primitive Mock：每个工具同步返回常量结果，无 sleep。
pub(super) struct MockPrimitiveExecutor;

#[async_trait::async_trait]
impl PrimitiveExecutor for MockPrimitiveExecutor {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        Ok(format!("content:{}", path))
    }
    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::core::tools::primitive::DirEntry>, AppError> {
        Ok(vec![])
    }
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::WriteFileResult, AppError> {
        Ok(crate::core::tools::primitive::WriteFileResult {
            path: path.to_string(),
            written: overwrite || content.is_empty(),
            bytes_written: 0,
            diff_hint: None,
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
        })
    }
    async fn execute_bash(
        &self,
        command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
    ) -> Result<crate::core::tools::primitive::BashResult, AppError> {
        Ok(crate::core::tools::primitive::BashResult {
            stdout: format!("out:{}", command),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    async fn require_user_confirmation(
        &self,
        _operation: crate::core::tools::primitive::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}

/// 工具执行时 sleep 100ms，便于在测试中 spawn 一个任务设置 abort，让取消落在
/// "工具执行中"的 select! 内（而不是退化到工具间）。
pub(super) struct SleepyMockPrimitive;

#[async_trait::async_trait]
impl PrimitiveExecutor for SleepyMockPrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        Ok(format!("content:{}", path))
    }
    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::core::tools::primitive::DirEntry>, AppError> {
        Ok(vec![])
    }
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::WriteFileResult, AppError> {
        Ok(crate::core::tools::primitive::WriteFileResult {
            path: path.to_string(),
            written: overwrite || content.is_empty(),
            bytes_written: 0,
            diff_hint: None,
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
        })
    }
    async fn execute_bash(
        &self,
        command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
    ) -> Result<crate::core::tools::primitive::BashResult, AppError> {
        Ok(crate::core::tools::primitive::BashResult {
            stdout: format!("out:{}", command),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    async fn require_user_confirmation(
        &self,
        _operation: crate::core::tools::primitive::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}

/// 第一次 `read_file` 时向共享的 steering_queue 推入 Steering，用于测试
/// "工具执行后 steering 注入即刻跳过剩余工具"分支（tool_dispatcher 的核心契约）。
pub(super) struct SteerableMockPrimitive {
    pub(super) steering_queue: Arc<parking_lot::Mutex<Vec<ChatMessage>>>,
    pub(super) read_count: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl PrimitiveExecutor for SteerableMockPrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        let n = self
            .read_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            self.steering_queue
                .lock()
                .push(ChatMessage::steering("stop after first tool"));
        }
        Ok(format!("content:{}", path))
    }
    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::core::tools::primitive::DirEntry>, AppError> {
        Ok(vec![])
    }
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::WriteFileResult, AppError> {
        Ok(crate::core::tools::primitive::WriteFileResult {
            path: path.to_string(),
            written: overwrite || content.is_empty(),
            bytes_written: 0,
            diff_hint: None,
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
        })
    }
    async fn execute_bash(
        &self,
        command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
    ) -> Result<crate::core::tools::primitive::BashResult, AppError> {
        Ok(crate::core::tools::primitive::BashResult {
            stdout: format!("out:{}", command),
            stderr: String::new(),
            exit_code: 0,
        })
    }
    async fn require_user_confirmation(
        &self,
        _operation: crate::core::tools::primitive::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }
}
