use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;

use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider, StreamEvent,
};
use crate::core::summary::title_generator::{is_bare_tool_count, sanitize_purpose_clause};
use crate::core::summary::{
    fallback_command_summary, fallback_turn_summary, generate_command_summary,
    generate_session_title, generate_turn_summary, ToolSnapshot,
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
    let title =
        generate_turn_summary(Some("thinking"), &tools, llm.as_ref(), "utility-flash").await;

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
    let title = generate_session_title("please fix the login bug", llm.as_ref(), "utility-flash")
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

#[test]
fn fallback_turn_summary_avoids_raw_json_for_ask_question() {
    let tools = vec![ToolSnapshot {
        tool_name: "ask_question".into(),
        summary: r#"questions=1"#.into(),
    }];

    let title = fallback_turn_summary(&tools);

    assert_eq!(title, "Asked question");
}

/// 两阶段 mock：purpose 从句调用（prompt 含 "PURPOSE"）返回 purpose，其余返回 title。
struct MockTwoPhaseLlm {
    title: String,
    purpose: String,
}

#[async_trait]
impl LlmProvider for MockTwoPhaseLlm {
    fn provider_name(&self) -> &str {
        "mock-two-phase"
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, AppError> {
        let prompt = req
            .messages
            .last()
            .and_then(|m| m.text_content())
            .unwrap_or("");
        let text = if prompt.contains("describe the PURPOSE") {
            &self.purpose
        } else {
            &self.title
        };
        Ok(mock_chat_response(text))
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

fn mixed_tools() -> Vec<ToolSnapshot> {
    vec![
        ToolSnapshot {
            tool_name: "read".into(),
            summary: "path=a.rs".into(),
        },
        ToolSnapshot {
            tool_name: "bash".into(),
            summary: "command=cargo test".into(),
        },
        ToolSnapshot {
            tool_name: "edit".into(),
            summary: "path=b.rs (replace)".into(),
        },
    ]
}

#[tokio::test]
async fn generate_turn_summary_appends_purpose_when_bare_count() {
    info!(target: "test", phase = "arrange");
    // 首次标题调用吐裸计数，触发第二次 purpose 从句调用。
    let llm = Arc::new(MockTwoPhaseLlm {
        title: "Used 3 tools".into(),
        purpose: "finding coffee shops in Shenzhen".into(),
    });
    let tools = mixed_tools();

    info!(target: "test", phase = "act");
    let title = generate_turn_summary(Some("think"), &tools, llm.as_ref(), "utility-flash").await;

    info!(target: "test", phase = "assert");
    assert_eq!(title, "Used 3 tools for finding coffee shops in Shenzhen");
}

#[tokio::test]
async fn generate_turn_summary_keeps_descriptive_title_without_downgrade() {
    info!(target: "test", phase = "arrange");
    // 描述句本身够好，不该被降级成 "Used N tools for ..."。
    let llm = Arc::new(MockTwoPhaseLlm {
        title: "Reviewed search results and updated plan".into(),
        purpose: "should not be used".into(),
    });
    let tools = mixed_tools();

    info!(target: "test", phase = "act");
    let title = generate_turn_summary(Some("think"), &tools, llm.as_ref(), "utility-flash").await;

    info!(target: "test", phase = "assert");
    assert_eq!(title, "Reviewed search results and updated plan");
}

#[tokio::test]
async fn generate_turn_summary_keeps_bare_count_when_purpose_empty() {
    info!(target: "test", phase = "arrange");
    // purpose 从句为空 → 保留裸计数，绝不产出 "Used N tools for"（悬空）。
    let llm = Arc::new(MockTwoPhaseLlm {
        title: "Used 3 tools".into(),
        purpose: String::new(),
    });
    let tools = mixed_tools();

    info!(target: "test", phase = "act");
    let title = generate_turn_summary(Some("think"), &tools, llm.as_ref(), "utility-flash").await;

    info!(target: "test", phase = "assert");
    assert_eq!(title, "Used 3 tools");
}

#[test]
fn is_bare_tool_count_matches_only_plain_counts() {
    info!(target: "test", phase = "assert");
    assert!(is_bare_tool_count("Used 4 tools"));
    assert!(is_bare_tool_count("Used 1 tool"));
    assert!(!is_bare_tool_count("Used 4 tools for finding cafes"));
    assert!(!is_bare_tool_count("Used web fetch"));
    assert!(!is_bare_tool_count("Reviewed 2 files"));
    assert!(!is_bare_tool_count("Ran cargo test"));
}

#[tokio::test]
async fn generate_command_summary_success_returns_sanitized_phrase() {
    info!(target: "test", phase = "arrange");
    let llm = MockUtilityLlm::ok("Gather git status and recent commits");

    info!(target: "test", phase = "act");
    let title = generate_command_summary(
        "git status && git log -1",
        None,
        llm.as_ref(),
        "utility-flash",
    )
    .await;

    info!(target: "test", phase = "assert");
    assert_eq!(title, "Gather git status and recent commits");
}

#[tokio::test]
async fn generate_command_summary_failure_falls_back_to_run_first_binary() {
    info!(target: "test", phase = "arrange");
    let llm = MockUtilityLlm::failing();

    info!(target: "test", phase = "act");
    let title = generate_command_summary(
        "FOO=bar sudo ./deploy.sh --now",
        None,
        llm.as_ref(),
        "utility-flash",
    )
    .await;

    info!(target: "test", phase = "assert");
    assert_eq!(title, "Run deploy.sh");
}

#[test]
fn fallback_command_summary_extracts_first_real_binary() {
    info!(target: "test", phase = "assert");
    assert_eq!(
        fallback_command_summary("git status && echo done"),
        "Run git"
    );
    assert_eq!(
        fallback_command_summary("/usr/local/bin/node app.js"),
        "Run node"
    );
    assert_eq!(
        fallback_command_summary("FOO=1 BAR=2 python x.py"),
        "Run python"
    );
    assert_eq!(fallback_command_summary("   "), "Ran command");
}

#[test]
fn sanitize_purpose_clause_strips_prefix_punctuation_and_caps_words() {
    info!(target: "test", phase = "assert");
    assert_eq!(
        sanitize_purpose_clause("for finding coffee shops.".into()),
        "finding coffee shops"
    );
    assert_eq!(
        sanitize_purpose_clause("\"the plan preview layout\"".into()),
        "the plan preview layout"
    );
    assert_eq!(
        sanitize_purpose_clause("one two three four five six seven".into()),
        "one two three four five six"
    );
}
