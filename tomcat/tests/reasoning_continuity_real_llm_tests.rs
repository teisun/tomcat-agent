mod common;

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tomcat::core::llm::{ContinuityMetadata, ReasoningContinuation};
use tomcat::{
    resolve_llm, AppError, ChatMessage, ChatRequest, LlmConfig, LlmProvider, StreamEvent,
};

const STREAM_TIMEOUT: Duration = Duration::from_secs(120);
const CHAT_TIMEOUT: Duration = Duration::from_secs(120);
const OPENAI_CAPTURE_ATTEMPTS: usize = 3;
const DEEPSEEK_CAPTURE_ATTEMPTS: usize = 2;

#[derive(Debug, Default)]
struct ToolCallAccum {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

#[derive(Debug, Default)]
struct CapturedTurn {
    assistant_text: String,
    thinking_text: Option<String>,
    reasoning_continuation: Option<ReasoningContinuation>,
    continuity: Option<ContinuityMetadata>,
    tool_calls: Vec<Value>,
    finish_reason: Option<String>,
}

fn require_api_key(env_key: &str) {
    common::setup_logging();
    common::load_openai_test_env();
    if std::env::var(env_key).is_err() {
        panic!(
            "reasoning_continuity_real_llm_tests 必须设置 {}（环境变量或 tomcat/.env）",
            env_key
        );
    }
}

fn openai_model() -> String {
    std::env::var("TOMCAT_E2E_LLM_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string())
}

fn deepseek_model() -> String {
    std::env::var("TOMCAT_E2E_DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-chat".to_string())
}

fn openai_responses_continuity_config() -> LlmConfig {
    let mut cfg = LlmConfig {
        provider: "openai-responses".to_string(),
        default_model: openai_model(),
        ..LlmConfig::default()
    };
    cfg.reasoning_continuity.enabled = true;
    cfg
}

fn deepseek_continuity_config() -> LlmConfig {
    let mut cfg = LlmConfig {
        provider: "openai".to_string(),
        api_base: Some("https://api.deepseek.com".to_string()),
        api_key_env: Some("DEEPSEEK_API_KEY".to_string()),
        default_model: deepseek_model(),
        ..LlmConfig::default()
    };
    cfg.reasoning_continuity.enabled = true;
    cfg.thinking.enabled = true;
    cfg.thinking.level = "high".to_string();
    cfg.thinking.format = Some("deepseek".to_string());
    cfg
}

fn weather_tool_definitions() -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "lookup_weather",
            "description": "Look up a city forecast once",
            "parameters": {
                "type": "object",
                "properties": {
                    "city": {"type": "string"},
                    "day": {"type": "string"}
                },
                "required": ["city", "day"]
            }
        }
    })]
}

fn merge_tool_call_delta(
    tool_calls: &mut Vec<ToolCallAccum>,
    index: u32,
    id: Option<String>,
    name: Option<String>,
    arguments_delta: Option<String>,
) {
    let index = index as usize;
    if tool_calls.len() <= index {
        tool_calls.resize_with(index + 1, ToolCallAccum::default);
    }
    let entry = &mut tool_calls[index];
    if let Some(id) = id {
        entry.id = Some(id);
    }
    if let Some(name) = name {
        entry.name = Some(name);
    }
    if let Some(arguments_delta) = arguments_delta {
        entry.arguments.push_str(&arguments_delta);
    }
}

fn finalized_tool_calls(tool_calls: Vec<ToolCallAccum>) -> Vec<Value> {
    tool_calls
        .into_iter()
        .filter(|tool_call| tool_call.name.is_some())
        .map(|tool_call| {
            json!({
                "id": tool_call.id.unwrap_or_else(|| "call_missing".to_string()),
                "type": "function",
                "function": {
                    "name": tool_call.name.unwrap_or_else(|| "unknown".to_string()),
                    "arguments": tool_call.arguments,
                }
            })
        })
        .collect()
}

async fn capture_stream_turn(
    provider: Arc<dyn LlmProvider>,
    request: ChatRequest,
) -> Result<CapturedTurn, AppError> {
    let mut stream = tokio::time::timeout(STREAM_TIMEOUT, provider.chat_stream(request))
        .await
        .map_err(|_| AppError::Llm("chat_stream 启动超时 120s".to_string()))??;

    tokio::time::timeout(STREAM_TIMEOUT, async move {
        let mut captured = CapturedTurn::default();
        let mut tool_calls = Vec::<ToolCallAccum>::new();
        while let Some(item) = stream.next().await {
            match item? {
                StreamEvent::ContentDelta { delta } => captured.assistant_text.push_str(&delta),
                StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                } => merge_tool_call_delta(&mut tool_calls, index, id, name, arguments_delta),
                StreamEvent::ReasoningSnapshot {
                    thinking_text,
                    reasoning_continuation,
                    continuity,
                } => {
                    if thinking_text.is_some() {
                        captured.thinking_text = thinking_text;
                    }
                    if reasoning_continuation.is_some() {
                        captured.reasoning_continuation = reasoning_continuation;
                    }
                    if continuity.is_some() {
                        captured.continuity = continuity;
                    }
                }
                StreamEvent::FinishReason { reason } => captured.finish_reason = Some(reason),
                _ => {}
            }
        }
        captured.tool_calls = finalized_tool_calls(tool_calls);
        Ok::<CapturedTurn, AppError>(captured)
    })
    .await
    .map_err(|_| AppError::Llm("chat_stream 消费超时 120s".to_string()))?
}

async fn run_chat(
    provider: Arc<dyn LlmProvider>,
    request: ChatRequest,
) -> Result<tomcat::ChatResponse, AppError> {
    tokio::time::timeout(CHAT_TIMEOUT, provider.chat(request))
        .await
        .map_err(|_| AppError::Llm("chat 超时 120s".to_string()))?
}

#[tokio::test]
async fn openai_responses_roundtrip_replays_reasoning_items(
) -> Result<(), Box<dyn std::error::Error>> {
    require_api_key("OPENAI_API_KEY");

    let config = openai_responses_continuity_config();
    let provider = resolve_llm(&config)
        .expect("resolve_llm(openai-responses) 失败：请检查 OPENAI_API_KEY / OpenAI 配置");
    let prompt =
        "Compute 387 * 249 carefully, think step by step, then answer with the final number only.";

    for attempt in 1..=OPENAI_CAPTURE_ATTEMPTS {
        let first_turn = capture_stream_turn(
            provider.clone(),
            ChatRequest {
                messages: vec![ChatMessage::user(prompt)],
                model: config.default_model.clone(),
                temperature: None,
                max_tokens: Some(256),
                stream: Some(true),
                model_override: None,
                tools: None,
            },
        )
        .await?;

        let Some(reasoning_continuation) = first_turn.reasoning_continuation.clone() else {
            tracing::warn!(
                attempt,
                "OpenAI Responses 未返回 continuity snapshot，重试捕获"
            );
            continue;
        };
        let Some(continuity) = first_turn.continuity.clone() else {
            tracing::warn!(
                attempt,
                "OpenAI Responses 未返回 continuity metadata，重试捕获"
            );
            continue;
        };
        assert_eq!(reasoning_continuation.source_provider, "openai");
        assert_eq!(reasoning_continuation.source_api, "responses");
        assert!(
            reasoning_continuation
                .provider_refs
                .as_ref()
                .and_then(|refs| refs.openai_response_id.as_deref())
                .is_some(),
            "Responses continuity snapshot 应写入 response_id"
        );

        let assistant = ChatMessage::assistant(first_turn.assistant_text.clone())
            .with_reasoning_state(
                first_turn.thinking_text.clone(),
                Some(reasoning_continuation),
                Some(continuity),
            );
        let second = run_chat(
            provider.clone(),
            ChatRequest {
                messages: vec![
                    ChatMessage::user(prompt),
                    assistant,
                    ChatMessage::user(
                        "Continue the same thread and answer with the same final number only.",
                    ),
                ],
                model: config.default_model.clone(),
                temperature: None,
                max_tokens: Some(128),
                stream: Some(false),
                model_override: None,
                tools: None,
            },
        )
        .await?;
        assert!(
            !second.choices.is_empty(),
            "显式 replay 第二轮应返回 choices"
        );
        return Ok(());
    }

    Err(std::io::Error::other("OpenAI Responses 在多次尝试后仍未拿到 continuity snapshot").into())
}

#[tokio::test]
async fn deepseek_chat_roundtrip_replays_tool_turn_reasoning_content(
) -> Result<(), Box<dyn std::error::Error>> {
    require_api_key("DEEPSEEK_API_KEY");

    let config = deepseek_continuity_config();
    let provider = resolve_llm(&config).expect(
        "resolve_llm(deepseek via provider=openai) 失败：请检查 DEEPSEEK_API_KEY / api_base 配置",
    );
    let prompt =
        "Call lookup_weather exactly once for Hangzhou on tomorrow, then wait for the tool result.";

    for attempt in 1..=DEEPSEEK_CAPTURE_ATTEMPTS {
        let first_turn = capture_stream_turn(
            provider.clone(),
            ChatRequest {
                messages: vec![ChatMessage::user(prompt)],
                model: config.default_model.clone(),
                temperature: None,
                max_tokens: Some(512),
                stream: Some(true),
                model_override: None,
                tools: Some(weather_tool_definitions()),
            },
        )
        .await?;

        let Some(reasoning_continuation) = first_turn.reasoning_continuation.clone() else {
            tracing::warn!(
                attempt,
                "DeepSeek tool turn 未返回 continuity snapshot，重试捕获"
            );
            continue;
        };
        let Some(continuity) = first_turn.continuity.clone() else {
            tracing::warn!(
                attempt,
                "DeepSeek tool turn 未返回 continuity metadata，重试捕获"
            );
            continue;
        };
        if first_turn.tool_calls.is_empty() {
            tracing::warn!(attempt, "DeepSeek 首轮未触发 tool call，重试捕获");
            continue;
        }

        assert!(
            continuity.had_tool_call,
            "DeepSeek tool turn continuity 应标记 had_tool_call=true"
        );
        assert_eq!(reasoning_continuation.source_provider, "deepseek");
        assert_eq!(reasoning_continuation.source_api, "chat_completions");
        assert!(
            reasoning_continuation.opaque_payload["reasoning_content"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty()),
            "DeepSeek continuity snapshot 应携带 reasoning_content"
        );

        let tool_call_id = first_turn.tool_calls[0]["id"]
            .as_str()
            .expect("tool_call id missing")
            .to_string();
        let assistant = ChatMessage::assistant_with_tool_calls(
            (!first_turn.assistant_text.trim().is_empty())
                .then_some(first_turn.assistant_text.as_str()),
            first_turn.tool_calls.clone(),
        )
        .with_reasoning_state(
            first_turn.thinking_text.clone(),
            Some(reasoning_continuation),
            Some(continuity),
        );
        let second = run_chat(
            provider.clone(),
            ChatRequest {
                messages: vec![
                    ChatMessage::user(prompt),
                    assistant,
                    ChatMessage::tool(
                        &tool_call_id,
                        r#"{"city":"Hangzhou","day":"tomorrow","forecast":"sunny 25C"}"#,
                    ),
                    ChatMessage::user(
                        "Do not call any more tools. Answer directly in one short sentence.",
                    ),
                ],
                model: config.default_model.clone(),
                temperature: None,
                max_tokens: Some(256),
                stream: Some(false),
                model_override: None,
                tools: None,
            },
        )
        .await?;
        assert!(
            !second.choices.is_empty(),
            "DeepSeek tool-turn continuity replay 第二轮应返回 choices"
        );
        return Ok(());
    }

    Err(
        std::io::Error::other("DeepSeek 在多次尝试后仍未拿到带 tool-call 的 continuity snapshot")
            .into(),
    )
}
