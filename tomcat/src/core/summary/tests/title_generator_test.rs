use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;

use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider, StreamEvent,
};
use crate::core::summary::{
    fallback_turn_summary, generate_session_title, generate_turn_summary, ToolSnapshot,
};
use crate::infra::error::AppError;

struct MockUtilityLlm {
    response: String,
    fail: bool,
}

impl MockUtilityLlm {
    fn ok(response: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            response: response.into(),
            fail: false,
        })
    }

    fn failing() -> Arc<Self> {
        Arc::new(Self {
            response: String::new(),
            fail: true,
        })
    }
}

fn mock_chat_response(text: &str) -> ChatResponse {
    ChatResponse {
        id: None,
        choices: vec![ChatResponseChoice {
            index: 0,
            message: ChatMessage::assistant(text),
            finish_reason: Some("stop".into()),
        }],
        usage: None,
    }
}

#[async_trait]
impl LlmProvider for MockUtilityLlm {
    fn provider_name(&self) -> &str {
        "mock-utility"
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, AppError> {
        if self.fail {
            return Err(AppError::internal("mock failure"));
        }
        let prompt = req
            .messages
            .last()
            .and_then(|m| m.text_content())
            .unwrap_or("");
        info!(target: "test", phase = "arrange", prompt_len = prompt.len());
        info!(target: "test", phase = "act", model = %req.model);
        Ok(mock_chat_response(&self.response))
    }

    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        Ok(Box::new(tokio_stream::empty()))
    }
}

#[tokio::test]
async fn generate_turn_summary_success_returns_sanitized_title() {
    info!(target: "test", phase = "arrange");
    let llm = MockUtilityLlm::ok("Reviewed 3 files");
    let tools = vec![ToolSnapshot {
        tool_name: "read".into(),
        summary: "path=a.rs".into(),
    }];

    info!(target: "test", phase = "act");
    let title = generate_turn_summary(Some("thinking"), &tools, llm.as_ref(), "utility-flash").await;

    info!(target: "test", phase = "assert");
    assert_eq!(title, "Reviewed 3 files");
}

#[tokio::test]
async fn generate_turn_summary_failure_falls_back_to_rules() {
    info!(target: "test", phase = "arrange");
    let llm = MockUtilityLlm::failing();
    let tools = vec![
        ToolSnapshot {
            tool_name: "read".into(),
            summary: "path=a.rs".into(),
        },
        ToolSnapshot {
            tool_name: "read".into(),
            summary: "path=b.rs".into(),
        },
    ];

    info!(target: "test", phase = "act");
    let title = generate_turn_summary(Some("x"), &tools, llm.as_ref(), "utility-flash").await;

    info!(target: "test", phase = "assert");
    assert_eq!(title, "Reviewed 2 files");
}

#[tokio::test]
async fn generate_turn_summary_timeout_falls_back_to_rules() {
    info!(target: "test", phase = "arrange");
    struct SlowLlm;
    #[async_trait]
    impl LlmProvider for SlowLlm {
        fn provider_name(&self) -> &str {
            "slow"
        }

        fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
            Ok(0)
        }

        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            Ok(mock_chat_response("late"))
        }

        async fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> Result<
            Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
            AppError,
        > {
            Ok(Box::new(tokio_stream::empty()))
        }
    }
    let tools = vec![ToolSnapshot {
        tool_name: "bash".into(),
        summary: "command=ls".into(),
    }];

    info!(target: "test", phase = "act");
    let title = generate_turn_summary(None, &tools, &SlowLlm, "utility-flash").await;

    info!(target: "test", phase = "assert");
    assert_eq!(title, "Ran ls");
}

#[tokio::test]
async fn generate_session_title_success_returns_title() {
    info!(target: "test", phase = "arrange");
    let llm = MockUtilityLlm::ok("Fix login bug");

    info!(target: "test", phase = "act");
    let title =
        generate_session_title("please fix the login bug", llm.as_ref(), "utility-flash")
            .await
            .expect("ok");

    info!(target: "test", phase = "assert");
    assert_eq!(title, "Fix login bug");
}

#[tokio::test]
async fn generate_session_title_failure_returns_err() {
    info!(target: "test", phase = "arrange");
    let llm = MockUtilityLlm::failing();

    info!(target: "test", phase = "act");
    let err = generate_session_title("hello", llm.as_ref(), "utility-flash")
        .await
        .expect_err("fail");

    info!(target: "test", phase = "assert");
    assert!(err.to_string().contains("mock failure"));
}

#[test]
fn fallback_turn_summary_multiple_reads_uses_file_count() {
    info!(target: "test", phase = "arrange");
    let tools = vec![
        ToolSnapshot {
            tool_name: "read".into(),
            summary: "path=a".into(),
        },
        ToolSnapshot {
            tool_name: "read_file".into(),
            summary: "path=b".into(),
        },
    ];

    info!(target: "test", phase = "act");
    let title = fallback_turn_summary(&tools);

    info!(target: "test", phase = "assert");
    assert_eq!(title, "Reviewed 2 files");
}
