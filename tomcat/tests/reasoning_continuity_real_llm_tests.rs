mod common;

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tomcat::core::llm::{ContinuityMetadata, ReasoningContinuation};
use tomcat::{
    build_context_from_state, init_context_state, AppConfig, AppError, ChatMessage, ChatRequest,
    ContextConfig, LlmProvider, SessionManager, StreamEvent, TranscriptEntry,
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
    common::e2e_openai_model()
}

fn live_openai_responses_opt_in(test_name: &str) -> bool {
    match std::env::var("PI_LIVE_OPENAI_RESPONSES") {
        Ok(value) if matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on") => {
            true
        }
        _ => {
            eprintln!(
                "skip {test_name}: set PI_LIVE_OPENAI_RESPONSES=1 to enable live OpenAI Responses reasoning continuity tests"
            );
            false
        }
    }
}

fn openai_api_key_env() -> &'static str {
    if openai_model() == "gpt-5.4" {
        "OPENAI_API_KEY"
    } else {
        common::OPENAI_GATEWAY_TEST_API_KEY_ENV
    }
}

fn deepseek_model() -> String {
    std::env::var("TOMCAT_E2E_DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-v4-pro".to_string())
}

fn deepseek_alt_model() -> String {
    std::env::var("TOMCAT_E2E_DEEPSEEK_ALT_MODEL")
        .unwrap_or_else(|_| "deepseek-v4-flash".to_string())
}

fn mimo_model() -> String {
    common::mimo_test_model()
}

fn base_continuity_config(label: &str) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(common::dot_tomcat_e2e_workdir(label).display().to_string());
    cfg
}

fn openai_responses_continuity_config() -> AppConfig {
    let mut cfg = base_continuity_config("reasoning_continuity_openai");
    common::apply_openai_app_config(&mut cfg);
    cfg.llm.default_model = openai_model();
    cfg.llm.reasoning_continuity.enabled = true;
    cfg
}

fn deepseek_continuity_config() -> AppConfig {
    let mut cfg = base_continuity_config("reasoning_continuity_deepseek");
    common::apply_deepseek_app_config(&mut cfg);
    cfg.llm.default_model = deepseek_model();
    cfg.llm.reasoning_continuity.enabled = true;
    cfg.llm.thinking.enabled = true;
    cfg.llm.thinking.level = "high".to_string();
    cfg
}

fn mimo_continuity_config() -> AppConfig {
    let mut cfg = base_continuity_config("reasoning_continuity_mimo");
    common::apply_deepseek_app_config(&mut cfg);
    cfg.llm.default_model = mimo_model();
    cfg.llm.reasoning_continuity.enabled = true;
    cfg.llm.thinking.enabled = true;
    cfg.llm.thinking.level = "high".to_string();
    cfg.llm.thinking.format = Some("doubao".to_string());
    cfg
}

fn append_chat_message(session: &SessionManager, message: &ChatMessage) -> Result<(), AppError> {
    let value = serde_json::to_value(message)
        .map_err(|err| AppError::Config(format!("序列化 ChatMessage 失败: {err}")))?;
    session.append_message(value)?;
    Ok(())
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
    if !live_openai_responses_opt_in("openai_responses_roundtrip_replays_reasoning_items") {
        return Ok(());
    }
    require_api_key(openai_api_key_env());

    let config = openai_responses_continuity_config();
    let call = common::resolve_main_call(&config);
    let provider = call.provider_impl;
    let model = call.model;
    let prompt =
        "Compute 387 * 249 carefully, think step by step, then answer with the final number only.";

    for attempt in 1..=OPENAI_CAPTURE_ATTEMPTS {
        let first_turn = capture_stream_turn(
            provider.clone(),
            ChatRequest {
                messages: vec![ChatMessage::user(prompt)],
                model: model.clone(),
                temperature: None,
                max_tokens: Some(256),
                stream: Some(true),
                model_override: None,
                thinking_level: None,
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
                model: model.clone(),
                temperature: None,
                max_tokens: Some(128),
                stream: Some(false),
                model_override: None,
                thinking_level: None,
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
    let provider = common::resolve_main_provider(&config);
    let prompt =
        "Call lookup_weather exactly once for Hangzhou on tomorrow, then wait for the tool result.";

    for attempt in 1..=DEEPSEEK_CAPTURE_ATTEMPTS {
        let first_turn = capture_stream_turn(
            provider.clone(),
            ChatRequest {
                messages: vec![ChatMessage::user(prompt)],
                model: config.llm.default_model.clone(),
                temperature: None,
                max_tokens: Some(512),
                stream: Some(true),
                model_override: None,
                thinking_level: None,
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
                model: config.llm.default_model.clone(),
                temperature: None,
                max_tokens: Some(256),
                stream: Some(false),
                model_override: None,
                thinking_level: None,
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

#[tokio::test]
async fn mimo_chat_roundtrip_replays_tool_turn_reasoning_content(
) -> Result<(), Box<dyn std::error::Error>> {
    require_api_key("MIMO_API_KEY");

    let config = mimo_continuity_config();
    let provider = common::resolve_main_provider(&config);
    let prompt =
        "Call lookup_weather exactly once for Hangzhou on tomorrow, then wait for the tool result.";

    for attempt in 1..=DEEPSEEK_CAPTURE_ATTEMPTS {
        let first_turn = capture_stream_turn(
            provider.clone(),
            ChatRequest {
                messages: vec![ChatMessage::user(prompt)],
                model: config.llm.default_model.clone(),
                temperature: None,
                max_tokens: Some(512),
                stream: Some(true),
                model_override: None,
                thinking_level: None,
                tools: Some(weather_tool_definitions()),
            },
        )
        .await?;

        let Some(reasoning_continuation) = first_turn.reasoning_continuation.clone() else {
            tracing::warn!(
                attempt,
                "MiMo tool turn 未返回 continuity snapshot，重试捕获"
            );
            continue;
        };
        let Some(continuity) = first_turn.continuity.clone() else {
            tracing::warn!(
                attempt,
                "MiMo tool turn 未返回 continuity metadata，重试捕获"
            );
            continue;
        };
        if first_turn.tool_calls.is_empty() {
            tracing::warn!(attempt, "MiMo 首轮未触发 tool call，重试捕获");
            continue;
        }

        assert!(
            continuity.had_tool_call,
            "MiMo tool turn continuity 应标记 had_tool_call=true"
        );
        assert_eq!(reasoning_continuation.source_provider, "mimo");
        assert_eq!(reasoning_continuation.source_api, "chat_completions");
        assert!(
            reasoning_continuation.opaque_payload["reasoning_content"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty()),
            "MiMo continuity snapshot 应携带 reasoning_content"
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
                model: config.llm.default_model.clone(),
                temperature: None,
                max_tokens: Some(256),
                stream: Some(false),
                model_override: None,
                thinking_level: None,
                tools: None,
            },
        )
        .await?;
        assert!(
            !second.choices.is_empty(),
            "MiMo tool-turn continuity replay 第二轮应返回 choices"
        );
        return Ok(());
    }

    Err(
        std::io::Error::other("MiMo 在多次尝试后仍未拿到带 tool-call 的 continuity snapshot")
            .into(),
    )
}

#[tokio::test]
async fn deepseek_switch_model_roundtrip_replays_tool_turn_reasoning_content(
) -> Result<(), Box<dyn std::error::Error>> {
    require_api_key("DEEPSEEK_API_KEY");

    let config = deepseek_continuity_config();
    let switched_model = deepseek_alt_model();
    assert_ne!(
        config.llm.default_model, switched_model,
        "切 model replay 用例需要两个不同的 DeepSeek model；可通过 TOMCAT_E2E_DEEPSEEK_ALT_MODEL 覆盖"
    );

    let provider = common::resolve_main_provider(&config);
    let prompt =
        "Call lookup_weather exactly once for Hangzhou on tomorrow, then wait for the tool result.";
    let followup =
        "We switched to another model in the same session. Do not call more tools. Answer directly in one short sentence.";

    for attempt in 1..=DEEPSEEK_CAPTURE_ATTEMPTS {
        let first_turn = capture_stream_turn(
            provider.clone(),
            ChatRequest {
                messages: vec![ChatMessage::user(prompt)],
                model: config.llm.default_model.clone(),
                temperature: None,
                max_tokens: Some(512),
                stream: Some(true),
                model_override: None,
                thinking_level: None,
                tools: Some(weather_tool_definitions()),
            },
        )
        .await?;

        let Some(reasoning_continuation) = first_turn.reasoning_continuation.clone() else {
            tracing::warn!(
                attempt,
                "DeepSeek cross-model 用例未返回 continuity snapshot，重试捕获"
            );
            continue;
        };
        let Some(continuity) = first_turn.continuity.clone() else {
            tracing::warn!(
                attempt,
                "DeepSeek cross-model 用例未返回 continuity metadata，重试捕获"
            );
            continue;
        };
        if first_turn.tool_calls.is_empty() {
            tracing::warn!(
                attempt,
                "DeepSeek cross-model 首轮未触发 tool call，重试捕获"
            );
            continue;
        }

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

        let temp_dir = tempfile::tempdir()?;
        let session = SessionManager::new(temp_dir.path().to_path_buf());
        let key = session.current_session_key();
        session.create_session(key, None)?;
        append_chat_message(&session, &ChatMessage::user(prompt))?;
        append_chat_message(&session, &assistant)?;
        append_chat_message(
            &session,
            &ChatMessage::tool(
                &tool_call_id,
                r#"{"city":"Hangzhou","day":"tomorrow","forecast":"sunny 25C"}"#,
            ),
        )?;

        session.switch_current_model(Some("deepseek"), Some(&switched_model))?;
        let entry = session
            .current_session_entry()?
            .expect("切 model 后当前 session 应存在");
        assert_eq!(
            entry.model_override.as_deref(),
            Some(switched_model.as_str())
        );
        let entries = session.get_entries(16)?;
        assert!(
            entries.iter().any(|entry| matches!(
                entry,
                TranscriptEntry::ModelChange(change)
                    if change.provider.as_deref() == Some("deepseek")
                        && change.model_id.as_deref() == Some(switched_model.as_str())
            )),
            "切 model 应写入 model_change transcript 事件"
        );

        append_chat_message(&session, &ChatMessage::user(followup))?;
        let state = init_context_state(&session, &ContextConfig::default(), "system")?;
        let second = run_chat(
            provider.clone(),
            ChatRequest {
                messages: build_context_from_state(&state),
                model: config.llm.default_model.clone(),
                temperature: None,
                max_tokens: Some(256),
                stream: Some(false),
                model_override: entry.model_override.clone(),
                thinking_level: None,
                tools: None,
            },
        )
        .await?;
        assert!(
            !second.choices.is_empty(),
            "切 model 后 replay 第二轮应返回 choices"
        );
        return Ok(());
    }

    Err(
        std::io::Error::other("DeepSeek 在多次尝试后仍未完成“切 model 后 replay” continuity 验证")
            .into(),
    )
}

#[tokio::test]
async fn deepseek_non_tool_turn_roundtrip_replays_reasoning_content(
) -> Result<(), Box<dyn std::error::Error>> {
    require_api_key("DEEPSEEK_API_KEY");

    let config = deepseek_continuity_config();
    let provider = common::resolve_main_provider(&config);
    let prompt =
        "Call lookup_weather exactly once for Hangzhou on tomorrow, then wait for the tool result.";
    let followup =
        "Do not call any more tools. Answer directly in one short sentence about the forecast.";
    let third_prompt = "Continue the same thread in one short sentence without using any tools.";

    for attempt in 1..=DEEPSEEK_CAPTURE_ATTEMPTS {
        let first_turn = capture_stream_turn(
            provider.clone(),
            ChatRequest {
                messages: vec![ChatMessage::user(prompt)],
                model: config.llm.default_model.clone(),
                temperature: None,
                max_tokens: Some(512),
                stream: Some(true),
                model_override: None,
                thinking_level: None,
                tools: Some(weather_tool_definitions()),
            },
        )
        .await?;

        let Some(first_reasoning) = first_turn.reasoning_continuation.clone() else {
            tracing::warn!(
                attempt,
                "DeepSeek 非 tool turn roundtrip：首轮未返回 continuity snapshot，重试捕获"
            );
            continue;
        };
        let Some(first_continuity) = first_turn.continuity.clone() else {
            tracing::warn!(
                attempt,
                "DeepSeek 非 tool turn roundtrip：首轮未返回 continuity metadata，重试捕获"
            );
            continue;
        };
        if first_turn.tool_calls.is_empty() {
            tracing::warn!(
                attempt,
                "DeepSeek 非 tool turn roundtrip：首轮未触发 tool call，重试捕获"
            );
            continue;
        }

        let tool_call_id = first_turn.tool_calls[0]["id"]
            .as_str()
            .expect("tool_call id missing")
            .to_string();
        let first_assistant = ChatMessage::assistant_with_tool_calls(
            (!first_turn.assistant_text.trim().is_empty())
                .then_some(first_turn.assistant_text.as_str()),
            first_turn.tool_calls.clone(),
        )
        .with_reasoning_state(
            first_turn.thinking_text.clone(),
            Some(first_reasoning),
            Some(first_continuity),
        );
        let tool_output = ChatMessage::tool(
            &tool_call_id,
            r#"{"city":"Hangzhou","day":"tomorrow","forecast":"sunny 25C"}"#,
        );
        let second_turn = capture_stream_turn(
            provider.clone(),
            ChatRequest {
                messages: vec![
                    ChatMessage::user(prompt),
                    first_assistant.clone(),
                    tool_output.clone(),
                    ChatMessage::user(followup),
                ],
                model: config.llm.default_model.clone(),
                temperature: None,
                max_tokens: Some(256),
                stream: Some(true),
                model_override: None,
                thinking_level: None,
                tools: None,
            },
        )
        .await?;

        let Some(second_reasoning) = second_turn.reasoning_continuation.clone() else {
            tracing::warn!(
                attempt,
                "DeepSeek 非 tool turn roundtrip：第二轮未返回 continuity snapshot，重试捕获"
            );
            continue;
        };
        let Some(second_continuity) = second_turn.continuity.clone() else {
            tracing::warn!(
                attempt,
                "DeepSeek 非 tool turn roundtrip：第二轮未返回 continuity metadata，重试捕获"
            );
            continue;
        };
        assert!(
            second_turn.tool_calls.is_empty(),
            "第二轮 followup 已禁用 tools，不应再触发 tool call"
        );
        assert!(
            !second_continuity.had_tool_call,
            "第二轮应是非 tool turn continuity"
        );
        assert!(
            second_reasoning.opaque_payload["reasoning_content"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty()),
            "非 tool turn continuity snapshot 应携带 reasoning_content"
        );

        let second_assistant = ChatMessage::assistant(second_turn.assistant_text.clone())
            .with_reasoning_state(
                second_turn.thinking_text.clone(),
                Some(second_reasoning),
                Some(second_continuity),
            );
        let third = run_chat(
            provider.clone(),
            ChatRequest {
                messages: vec![
                    ChatMessage::user(prompt),
                    first_assistant,
                    tool_output,
                    ChatMessage::user(followup),
                    second_assistant,
                    ChatMessage::user(third_prompt),
                ],
                model: config.llm.default_model.clone(),
                temperature: None,
                max_tokens: Some(256),
                stream: Some(false),
                model_override: None,
                thinking_level: None,
                tools: None,
            },
        )
        .await?;
        assert!(
            !third.choices.is_empty(),
            "DeepSeek 非 tool turn continuity replay 第三轮应返回 choices"
        );
        return Ok(());
    }

    Err(std::io::Error::other(
        "DeepSeek 在多次尝试后仍未完成“非 tool turn reasoning_content replay”验证",
    )
    .into())
}
