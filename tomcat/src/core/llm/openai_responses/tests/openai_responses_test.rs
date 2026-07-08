//! # `OpenAiResponsesProvider` wire 翻译 + 流式解析焦小测
//!
//! 覆盖（plan §5 Phase E.2 / E.3）：
//!
//! - `build_responses_input`：system→instructions、user/assistant 顺序、tool_call 配对
//!   翻译；多 system 退化进 input；空 assistant 跳过。
//! - `convert_tools_to_responses`：function shape 翻译；空 description 不写出。
//! - `count_tokens`：复用 chars/3 启发式，与 Completions 同口径。
//! - `responses_payload_to_chat_response`：text + usage 抽取；function_call 翻成 tool_calls。
//! - `responses_chunk_to_events`：text delta、function_call.added、arguments.delta、completed
//!   + usage 等映射到 `StreamEvent`。
//! - `ResponsesStream`：SSE 帧切分、NDJSON fallback；上层与 `OpenAiProvider` 同 Stream 契约。

use super::*;
use crate::core::llm::multimodal::{
    UNSUPPORTED_FILE_INPUT_PLACEHOLDER, UNSUPPORTED_IMAGE_INPUT_PLACEHOLDER,
};
use crate::core::llm::provider::LlmProvider;
use crate::core::llm::tests::mocks::{MockHttpServer, ScriptedHttpResponse};
use crate::core::llm::types::{
    ChatMessage, ChatMessageContentPart, ChatRequest, ContextReference, StreamEvent, ThinkingSource,
};
use crate::core::llm::{Capabilities, Credential, ModelEntry};
use crate::infra::error::{
    llm_http_status, llm_http_status_error, llm_stage, llm_summary, AppError, LlmErrorStage,
};
use crate::infra::LlmConfig;

use bytes::Bytes;
use serde_json::json;
use std::time::Duration;

const TEST_KEY_ENV: &str = "__OPENAI_RESPONSES_TEST_KEY__";

fn responses_entry() -> ModelEntry {
    ModelEntry {
        id: "gpt-5.4".to_string(),
        model_name: None,
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        api_key_env: Some(TEST_KEY_ENV.to_string()),
        base_url: Some("https://api.openai.com".to_string()),
        capabilities: Capabilities {
            vision: true,
            files: true,
            tools: true,
            reasoning: true,
            web_search: false,
        },
        context_window: None,
        thinking_format: Some("openai".to_string()),
    }
}

fn provider_from_cfg(cfg: LlmConfig) -> OpenAiResponsesProvider {
    provider_from_entry(responses_entry(), cfg)
}

fn provider_from_entry(entry: ModelEntry, cfg: LlmConfig) -> OpenAiResponsesProvider {
    let runtime = cfg.runtime();
    let credential = Credential {
        provider: "openai".to_string(),
        env_name: TEST_KEY_ENV.to_string(),
        value: "stub-key".to_string(),
    };
    OpenAiResponsesProvider::new(&entry, &runtime, &credential).expect("provider new ok")
}

fn provider_with_stub_key() -> OpenAiResponsesProvider {
    provider_from_cfg(LlmConfig::default())
}

fn test_profile() -> crate::core::llm::ProviderCompatProfile {
    crate::core::llm::ProviderCompatProfile::openai_responses("gpt-5")
}

fn build_responses_input_test(
    messages: &[ChatMessage],
) -> (Option<String>, Vec<serde_json::Value>) {
    let profile = test_profile();
    build_responses_input(messages, &profile, true, true)
}

fn new_responses_stream<S>(stream: S, prefer_ndjson: bool) -> ResponsesStream<S> {
    ResponsesStream::new(stream, prefer_ndjson, test_profile(), true)
}

#[test]
fn openai_files_client_is_lazy_once_per_provider() {
    let cfg = LlmConfig::default();
    let p = provider_from_cfg(cfg.clone());

    let c1 = p
        .openai_files_client(&cfg.files)
        .expect("openai-responses should support files");
    let c2 = p
        .openai_files_client(&cfg.files)
        .expect("openai-responses should support files");
    assert_eq!(
        c1.instance_id(),
        c2.instance_id(),
        "same provider should lazily init files client once"
    );
}

#[test]
fn build_responses_input_extracts_first_system_to_instructions() {
    let msgs = vec![ChatMessage::system("be helpful"), ChatMessage::user("hi")];
    let (ins, input) = build_responses_input_test(&msgs);
    assert_eq!(ins.as_deref(), Some("be helpful"));
    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[0]["content"][0]["type"], "input_text");
    assert_eq!(input[0]["content"][0]["text"], "hi");
}

#[test]
fn build_responses_input_second_system_falls_back_to_input_message() {
    let msgs = vec![
        ChatMessage::system("primary"),
        ChatMessage::system("secondary"),
        ChatMessage::user("ping"),
    ];
    let (ins, input) = build_responses_input_test(&msgs);
    assert_eq!(ins.as_deref(), Some("primary"));
    assert_eq!(input.len(), 2);
    assert_eq!(input[0]["role"], "system");
    assert_eq!(input[0]["content"][0]["type"], "input_text");
    assert_eq!(input[0]["content"][0]["text"], "secondary");
    assert_eq!(input[1]["role"], "user");
}

#[test]
fn build_responses_input_keeps_user_assistant_order() {
    let msgs = vec![
        ChatMessage::user("q1"),
        ChatMessage::assistant("a1"),
        ChatMessage::user("q2"),
    ];
    let (_ins, input) = build_responses_input_test(&msgs);
    assert_eq!(input.len(), 3);
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[1]["role"], "assistant");
    assert_eq!(input[1]["content"][0]["type"], "output_text");
    assert_eq!(input[1]["content"][0]["text"], "a1");
    assert_eq!(input[2]["role"], "user");
}

#[test]
fn build_responses_input_translates_tool_call_pair() {
    let assistant = ChatMessage::assistant_with_tool_calls(
        Some("calling tool"),
        vec![json!({
            "id": "call_1",
            "type": "function",
            "function": {"name": "search", "arguments": "{\"q\":\"x\"}"}
        })],
    );
    let tool_msg = ChatMessage::tool("call_1", "found 3 items");
    let msgs = vec![ChatMessage::user("please search"), assistant, tool_msg];
    let (_ins, input) = build_responses_input_test(&msgs);
    assert_eq!(input.len(), 4);
    assert_eq!(input[1]["role"], "assistant");
    assert_eq!(input[1]["content"][0]["text"], "calling tool");
    assert_eq!(input[2]["type"], "function_call");
    assert_eq!(input[2]["call_id"], "call_1");
    assert_eq!(input[2]["name"], "search");
    assert_eq!(input[2]["arguments"], "{\"q\":\"x\"}");
    assert_eq!(input[3]["type"], "function_call_output");
    assert_eq!(input[3]["call_id"], "call_1");
    assert_eq!(input[3]["output"], "found 3 items");
}

#[test]
fn build_responses_input_translates_hydrate_recovered_interrupted_tool_result() {
    let assistant = ChatMessage::assistant_with_tool_calls(
        Some("calling tool"),
        vec![json!({
            "id": "call_1",
            "type": "function",
            "function": {"name": "read", "arguments": "{}"}
        })],
    );
    let msgs = vec![
        ChatMessage::user("resume"),
        assistant,
        ChatMessage::tool("call_1", "[interrupted]"),
    ];
    let (_ins, input) = build_responses_input_test(&msgs);
    assert_eq!(input[2]["type"], "function_call");
    assert_eq!(input[2]["call_id"], "call_1");
    assert_eq!(input[3]["type"], "function_call_output");
    assert_eq!(input[3]["call_id"], "call_1");
    assert_eq!(input[3]["output"], "[interrupted]");
}

#[test]
fn convert_tools_to_responses_translates_function_shape() {
    let tools = vec![json!({
        "type": "function",
        "function": {
            "name": "echo",
            "description": "Echo back",
            "parameters": {"type": "object", "properties": {"text": {"type": "string"}}}
        }
    })];
    let out = convert_tools_to_responses(&tools);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["type"], "function");
    assert_eq!(out[0]["name"], "echo");
    assert_eq!(out[0]["description"], "Echo back");
    assert_eq!(out[0]["parameters"]["type"], "object");
}

#[test]
fn convert_tools_to_responses_drops_blank_description() {
    let tools = vec![json!({
        "type": "function",
        "function": {
            "name": "blank",
            "description": "   ",
            "parameters": {"type": "object"}
        }
    })];
    let out = convert_tools_to_responses(&tools);
    assert!(out[0].get("description").is_none());
}

#[test]
fn count_tokens_uses_chars_div_3_heuristic() {
    let p = provider_with_stub_key();
    let msgs = vec![ChatMessage::user("abcdef")]; // 6 chars / 3 = 2
    let n = p.count_tokens(&msgs).expect("count tokens");
    assert_eq!(n, 2);
}

#[test]
fn responses_payload_to_chat_response_extracts_text_and_usage() {
    let raw = json!({
        "id": "resp_1",
        "status": "completed",
        "output": [{
            "type": "message",
            "content": [{"type": "output_text", "text": "hello"}]
        }],
        "usage": {"input_tokens": 5, "output_tokens": 3, "total_tokens": 8}
    });
    let r = responses_payload_to_chat_response(&raw);
    assert_eq!(r.id.as_deref(), Some("resp_1"));
    assert_eq!(r.choices.len(), 1);
    assert_eq!(r.choices[0].message.text_content(), Some("hello"));
    assert_eq!(r.choices[0].finish_reason.as_deref(), Some("stop"));
    let u = r.usage.as_ref().expect("usage present");
    assert_eq!(u.prompt_tokens, 5);
    assert_eq!(u.completion_tokens, 3);
    assert_eq!(u.total_tokens, Some(8));
}

#[test]
fn responses_payload_translates_function_call_to_tool_calls() {
    let raw = json!({
        "id": "resp_2",
        "status": "completed",
        "output": [{
            "type": "function_call",
            "call_id": "call_x",
            "name": "lookup",
            "arguments": "{\"id\":42}"
        }]
    });
    let r = responses_payload_to_chat_response(&raw);
    let calls = r.choices[0]
        .message
        .tool_calls
        .as_ref()
        .expect("tool_calls present");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["id"], "call_x");
    assert_eq!(calls[0]["function"]["name"], "lookup");
    assert_eq!(calls[0]["function"]["arguments"], "{\"id\":42}");
}

#[test]
fn responses_payload_completed_function_call_prefers_tool_calls_finish_reason() {
    let raw = json!({
        "id": "resp_2b",
        "status": "completed",
        "output": [{
            "type": "function_call",
            "call_id": "call_x",
            "name": "lookup",
            "arguments": "{\"id\":42}"
        }]
    });
    let r = responses_payload_to_chat_response(&raw);
    assert_eq!(r.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    assert_eq!(
        r.choices[0].message.finish_reason.as_deref(),
        Some("tool_calls")
    );
}

#[test]
fn responses_payload_incomplete_content_filter_maps_to_error_metadata() {
    let raw = json!({
        "id": "resp_2c",
        "status": "incomplete",
        "incomplete_details": {"reason": "content_filter"},
        "output": []
    });
    let r = responses_payload_to_chat_response(&raw);
    assert_eq!(
        r.choices[0].finish_reason.as_deref(),
        Some("error:content_filter")
    );
    assert_eq!(
        r.choices[0].message.error_message.as_deref(),
        Some("content_filter")
    );
    assert_eq!(
        r.choices[0].message.finish_reason.as_deref(),
        Some("error:content_filter")
    );
}

#[test]
fn responses_chunk_text_delta_emits_content_delta() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let value = json!({
        "type": "response.output_text.delta",
        "item_id": "msg_1",
        "content_index": 0,
        "delta": "Hello"
    });
    let events = responses_chunk_to_events(&value, &mut tracks);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], StreamEvent::ContentDelta { delta } if delta == "Hello"),
        "unexpected {:?}",
        events[0]
    );
}

#[test]
fn responses_chunk_function_call_added_emits_initial_tool_call() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let value = json!({
        "type": "response.output_item.added",
        "item": {
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_1",
            "name": "echo",
            "arguments": ""
        }
    });
    let events = responses_chunk_to_events(&value, &mut tracks);
    assert_eq!(events.len(), 1);
    if let StreamEvent::ToolCallDelta {
        index,
        id,
        name,
        arguments_delta,
    } = &events[0]
    {
        assert_eq!(*index, 0);
        assert_eq!(id.as_deref(), Some("call_1"));
        assert_eq!(name.as_deref(), Some("echo"));
        assert!(arguments_delta.is_none());
    } else {
        panic!("expected ToolCallDelta, got {:?}", events[0]);
    }
}

#[test]
fn responses_chunk_function_call_arguments_delta_appends_to_track() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let init = json!({
        "type": "response.output_item.added",
        "item": {
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_1",
            "name": "echo",
            "arguments": ""
        }
    });
    responses_chunk_to_events(&init, &mut tracks);
    let value = json!({
        "type": "response.function_call_arguments.delta",
        "item_id": "fc_1",
        "delta": "{\"q\":\"x\"}"
    });
    let events = responses_chunk_to_events(&value, &mut tracks);
    assert_eq!(events.len(), 1);
    if let StreamEvent::ToolCallDelta {
        index,
        id,
        name,
        arguments_delta,
    } = &events[0]
    {
        assert_eq!(*index, 0);
        // 之前 added 帧已 emit 过 name，所以这里不再重复。
        assert!(id.is_none());
        assert!(name.is_none());
        assert_eq!(arguments_delta.as_deref(), Some("{\"q\":\"x\"}"));
    } else {
        panic!("expected ToolCallDelta, got {:?}", events[0]);
    }
}

#[test]
fn responses_chunk_completed_emits_finish_and_usage() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let value = json!({
        "type": "response.completed",
        "response": {
            "usage": {"input_tokens": 7, "output_tokens": 4, "total_tokens": 11}
        }
    });
    let events = responses_chunk_to_events(&value, &mut tracks);
    assert_eq!(events.len(), 2);
    assert!(
        matches!(&events[0], StreamEvent::FinishReason { reason } if reason == "stop"),
        "unexpected {:?}",
        events[0]
    );
    if let StreamEvent::Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
    } = &events[1]
    {
        assert_eq!(*prompt_tokens, 7);
        assert_eq!(*completion_tokens, 4);
        assert_eq!(*total_tokens, Some(11));
    } else {
        panic!("expected Usage, got {:?}", events[1]);
    }
}

#[test]
fn responses_chunk_completed_emits_reasoning_snapshot_with_response_id() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let mut reasoning = ReasoningState::default();
    let item_done = json!({
        "type": "response.output_item.done",
        "item": {
            "id": "rs_1",
            "type": "reasoning",
            "encrypted_content": "enc_123",
            "summary": [{"type": "summary_text", "text": "safe summary"}]
        }
    });
    let completed = json!({
        "type": "response.completed",
        "response": {
            "id": "resp_123",
            "status": "completed"
        }
    });
    let events = [item_done, completed]
        .into_iter()
        .flat_map(|value| {
            let profile = test_profile();
            responses_chunk_to_events_with_state(
                &value,
                &mut tracks,
                &mut reasoning,
                &profile,
                true,
            )
        })
        .collect::<Vec<_>>();
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ReasoningSnapshot {
            thinking_text: Some(text),
            reasoning_continuation: Some(continuation),
            continuity: Some(continuity)
        } if text == "safe summary"
            && continuity.replay_requirement == crate::core::llm::ReplayRequirement::SameProfileOptional
            && continuation.provider_refs.as_ref().and_then(|refs| refs.openai_response_id.as_deref()) == Some("resp_123")
            && continuation.opaque_payload[0]["encrypted_content"] == json!("enc_123")
    )));
}

#[test]
fn responses_chunk_completed_skips_snapshot_when_profile_capture_mode_is_none() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let mut reasoning = ReasoningState::default();
    let profile = crate::core::llm::ProviderCompatProfile::chat_completions("gpt-5");
    let item_done = json!({
        "type": "response.output_item.done",
        "item": {
            "id": "rs_1",
            "type": "reasoning",
            "encrypted_content": "enc_123",
            "summary": [{"type": "summary_text", "text": "safe summary"}]
        }
    });
    let completed = json!({
        "type": "response.completed",
        "response": {
            "id": "resp_123",
            "status": "completed"
        }
    });
    let events = [item_done, completed]
        .into_iter()
        .flat_map(|value| {
            responses_chunk_to_events_with_state(
                &value,
                &mut tracks,
                &mut reasoning,
                &profile,
                true,
            )
        })
        .collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, StreamEvent::ReasoningSnapshot { .. })),
        "capture_mode=None 时不应产生 continuity snapshot: {events:?}"
    );
}

#[test]
fn responses_chunk_completed_with_function_call_emits_tool_calls_finish_reason() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let init = json!({
        "type": "response.output_item.added",
        "item": {
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_1",
            "name": "echo",
            "arguments": ""
        }
    });
    responses_chunk_to_events(&init, &mut tracks);
    let done = json!({
        "type": "response.completed",
        "response": {
            "status": "completed"
        }
    });
    let events = responses_chunk_to_events(&done, &mut tracks);
    assert!(
        matches!(&events[0], StreamEvent::FinishReason { reason } if reason == "tool_calls"),
        "unexpected {:?}",
        events
    );
}

#[test]
fn responses_chunk_incomplete_max_output_tokens_emits_notice_finish_and_usage() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let value = json!({
        "type": "response.incomplete",
        "response": {
            "status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"},
            "usage": {"input_tokens": 7, "output_tokens": 4, "total_tokens": 11}
        }
    });
    let events = responses_chunk_to_events(&value, &mut tracks);
    assert_eq!(events.len(), 3);
    assert!(matches!(
        &events[0],
        StreamEvent::LlmNotice {
            finish_reason,
            message
        } if finish_reason == "max_output_tokens" && message.contains("max_output_tokens")
    ));
    assert!(matches!(
        &events[1],
        StreamEvent::FinishReason { reason } if reason == "max_output_tokens"
    ));
    assert!(matches!(&events[2], StreamEvent::Usage { .. }));
}

#[test]
fn responses_build_request_body_uses_model_name_when_present() {
    let mut entry = responses_entry();
    entry.id = "gpt-5.4_litellm-sunmi".to_string();
    entry.model_name = Some("gpt-5.4".to_string());
    let runtime = LlmConfig::default().runtime();
    let credential = Credential {
        provider: "litellm-sunmi".to_string(),
        env_name: "LITELLM_SUNMI_API_KEY".to_string(),
        value: "stub-key".to_string(),
    };
    let provider =
        OpenAiResponsesProvider::new(&entry, &runtime, &credential).expect("provider new ok");
    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: String::new(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let body = provider.build_request_body(&req, true);
    assert_eq!(body["model"], "gpt-5.4");
}

#[test]
fn responses_build_request_body_maps_catalog_id_to_model_name() {
    let mut entry = responses_entry();
    entry.id = "gpt-5.4_litellm-sunmi".to_string();
    entry.model_name = Some("gpt-5.4".to_string());
    let runtime = LlmConfig::default().runtime();
    let credential = Credential {
        provider: "litellm-sunmi".to_string(),
        env_name: "LITELLM_SUNMI_API_KEY".to_string(),
        value: "stub-key".to_string(),
    };
    let provider =
        OpenAiResponsesProvider::new(&entry, &runtime, &credential).expect("provider new ok");
    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5.4_litellm-sunmi".to_string(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let body = provider.build_request_body(&req, true);
    assert_eq!(body["model"], "gpt-5.4");
}

#[test]
fn responses_build_request_body_without_model_name_uses_id() {
    let mut entry = responses_entry();
    entry.id = "custom-responses-id".to_string();
    entry.model_name = None;
    let runtime = LlmConfig::default().runtime();
    let credential = Credential {
        provider: "openai".to_string(),
        env_name: TEST_KEY_ENV.to_string(),
        value: "stub-key".to_string(),
    };
    let provider =
        OpenAiResponsesProvider::new(&entry, &runtime, &credential).expect("provider new ok");
    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: String::new(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let body = provider.build_request_body(&req, true);
    assert_eq!(body["model"], "custom-responses-id");
}

#[test]
fn responses_build_request_body_disabled_thinking_omits_reasoning_field() {
    let cfg = LlmConfig {
        thinking: crate::infra::config::ThinkingConfig {
            enabled: false,
            ..crate::infra::config::ThinkingConfig::default()
        },
        ..LlmConfig::default()
    };
    let p = provider_from_cfg(cfg.clone());

    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let body = p.build_request_body(&req, true);
    assert!(
        body.get("reasoning").is_none(),
        "thinking.enabled=false 不应写 reasoning: {}",
        body
    );
    assert!(body.get("thinking").is_none());
}

#[test]
fn responses_build_request_body_high_writes_reasoning_effort() {
    let cfg = LlmConfig {
        thinking: crate::infra::config::ThinkingConfig {
            enabled: true,
            level: "high".into(),
            show: crate::infra::config::ThinkingDisplay::Summary,
            persist: false,
            ..crate::infra::config::ThinkingConfig::default()
        },
        ..LlmConfig::default()
    };
    let p = provider_from_cfg(cfg.clone());

    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let body = p.build_request_body(&req, true);
    assert_eq!(
        body["reasoning"]["effort"], "high",
        "thinking 启用后应写 reasoning.effort: {}",
        body
    );
    assert_eq!(
        body["reasoning"]["summary"], "auto",
        "show=summary && persist=false 时也应请求 reasoning.summary: {}",
        body
    );
    assert!(
        body.get("thinking").is_none(),
        "OpenAI 系不应同时写 thinking 对象"
    );
}

#[test]
fn responses_build_request_body_show_true_writes_reasoning_summary_auto() {
    let cfg = LlmConfig {
        thinking: crate::infra::config::ThinkingConfig {
            enabled: true,
            show: crate::infra::config::ThinkingDisplay::Full,
            persist: false,
            ..crate::infra::config::ThinkingConfig::default()
        },
        ..LlmConfig::default()
    };
    let p = provider_from_cfg(cfg.clone());

    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let body = p.build_request_body(&req, true);
    assert_eq!(
        body["reasoning"]["summary"], "auto",
        "show=full 时应请求 summary"
    );
}

#[test]
fn responses_build_request_body_persist_true_writes_reasoning_summary_auto() {
    let cfg = LlmConfig {
        thinking: crate::infra::config::ThinkingConfig {
            enabled: true,
            show: crate::infra::config::ThinkingDisplay::Summary,
            persist: true,
            ..crate::infra::config::ThinkingConfig::default()
        },
        ..LlmConfig::default()
    };
    let p = provider_from_cfg(cfg.clone());

    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let body = p.build_request_body(&req, true);
    assert_eq!(
        body["reasoning"]["summary"], "auto",
        "persist=true 时应请求 summary，即使 show=summary"
    );
}

#[test]
fn responses_build_request_body_show_and_persist_false_still_writes_reasoning_summary_auto() {
    let cfg = LlmConfig {
        thinking: crate::infra::config::ThinkingConfig {
            enabled: true,
            show: crate::infra::config::ThinkingDisplay::Summary,
            persist: false,
            ..crate::infra::config::ThinkingConfig::default()
        },
        ..LlmConfig::default()
    };
    let p = provider_from_cfg(cfg.clone());

    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let body = p.build_request_body(&req, true);
    assert_eq!(
        body["reasoning"]["summary"], "auto",
        "show/persist 都为 false 时仍应请求 summary: {}",
        body
    );
}

#[test]
fn responses_build_request_body_continuity_enabled_requests_encrypted_content() {
    let cfg = LlmConfig {
        reasoning_continuity: crate::infra::config::ReasoningContinuityConfig { enabled: true },
        ..LlmConfig::default()
    };
    let p = provider_from_cfg(cfg.clone());

    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let body = p.build_request_body(&req, true);
    assert_eq!(body["store"], false);
    assert_eq!(body["include"][0], "reasoning.encrypted_content");
}

#[test]
fn openai_responses_roundtrip_replays_reasoning_items() {
    let cfg = LlmConfig {
        reasoning_continuity: crate::infra::config::ReasoningContinuityConfig { enabled: true },
        ..LlmConfig::default()
    };
    let p = provider_from_cfg(cfg.clone());

    let assistant = ChatMessage::assistant("prior answer").with_reasoning_state(
        Some("safe summary".to_string()),
        Some(crate::core::llm::ReasoningContinuation {
            source_provider: "openai".to_string(),
            source_api: "responses".to_string(),
            source_model: "gpt-5".to_string(),
            format: crate::core::llm::ReasoningFormat::OpenaiResponsesReasoningItems,
            opaque_payload: json!([{
                "type": "reasoning",
                "encrypted_content": "enc_123"
            }]),
            fallback_text: Some("safe summary".to_string()),
            provider_refs: None,
        }),
        Some(crate::core::llm::ContinuityMetadata {
            had_tool_call: false,
            replay_requirement: crate::core::llm::ReplayRequirement::SameProfileOptional,
        }),
    );
    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi"), assistant],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let body = p.build_request_body(&req, true);
    assert_eq!(body["input"][1]["type"], "reasoning");
    assert_eq!(body["input"][1]["encrypted_content"], "enc_123");
}

#[test]
fn responses_build_request_body_previous_response_id_switches_to_store_true() {
    let cfg = LlmConfig {
        reasoning_continuity: crate::infra::config::ReasoningContinuityConfig { enabled: true },
        openai_responses: crate::infra::config::OpenAiResponsesConfig {
            use_previous_response_id: true,
        },
        ..LlmConfig::default()
    };
    let p = provider_from_cfg(cfg.clone());

    let assistant = ChatMessage::assistant("prior answer").with_reasoning_state(
        Some("safe summary".to_string()),
        Some(crate::core::llm::ReasoningContinuation {
            source_provider: "openai".to_string(),
            source_api: "responses".to_string(),
            source_model: "gpt-5".to_string(),
            format: crate::core::llm::ReasoningFormat::OpenaiResponsesReasoningItems,
            opaque_payload: json!([{
                "type": "reasoning",
                "encrypted_content": "enc_123"
            }]),
            fallback_text: Some("safe summary".to_string()),
            provider_refs: Some(crate::core::llm::ProviderRefs {
                openai_response_id: Some("resp_123".to_string()),
            }),
        }),
        Some(crate::core::llm::ContinuityMetadata {
            had_tool_call: false,
            replay_requirement: crate::core::llm::ReplayRequirement::SameProfileOptional,
        }),
    );
    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi"), assistant],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let body = p.build_request_body(&req, true);
    assert_eq!(body["store"], true);
    assert_eq!(body["previous_response_id"], "resp_123");
    assert!(
        body.get("include").is_none(),
        "previous_response_id 分支不应再请求 encrypted_content"
    );
    let input = body["input"].as_array().expect("input array");
    assert!(
        input
            .iter()
            .all(|item| item.get("type") != Some(&json!("reasoning"))),
        "previous_response_id 分支不应显式 replay reasoning items"
    );
}

#[test]
fn responses_build_request_body_without_hint_falls_back_to_explicit_replay() {
    let cfg = LlmConfig {
        reasoning_continuity: crate::infra::config::ReasoningContinuityConfig { enabled: true },
        openai_responses: crate::infra::config::OpenAiResponsesConfig {
            use_previous_response_id: true,
        },
        ..LlmConfig::default()
    };
    let p = provider_from_cfg(cfg.clone());

    let assistant = ChatMessage::assistant("prior answer").with_reasoning_state(
        Some("safe summary".to_string()),
        Some(crate::core::llm::ReasoningContinuation {
            source_provider: "openai".to_string(),
            source_api: "responses".to_string(),
            source_model: "gpt-5".to_string(),
            format: crate::core::llm::ReasoningFormat::OpenaiResponsesReasoningItems,
            opaque_payload: json!([{
                "type": "reasoning",
                "encrypted_content": "enc_123"
            }]),
            fallback_text: Some("safe summary".to_string()),
            provider_refs: Some(crate::core::llm::ProviderRefs {
                openai_response_id: Some("resp_123".to_string()),
            }),
        }),
        Some(crate::core::llm::ContinuityMetadata {
            had_tool_call: false,
            replay_requirement: crate::core::llm::ReplayRequirement::SameProfileOptional,
        }),
    );
    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi"), assistant],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let body = p.build_request_body_with_hint(&req, true, false);
    assert_eq!(body["store"], false);
    assert_eq!(body["include"][0], "reasoning.encrypted_content");
    assert!(body.get("previous_response_id").is_none());
    assert_eq!(body["input"][1]["type"], "reasoning");
}

#[test]
fn previous_response_id_error_detection_matches_api_error_body() {
    let err = llm_http_status_error(
        "openai-responses",
        400,
        r#"{"error":{"message":"invalid previous_response_id"}}"#,
    );
    assert!(is_previous_response_id_error(&err));
    assert!(request_uses_previous_response_id(&json!({
        "previous_response_id": "resp_123"
    })));
}

#[test]
fn responses_chunk_reasoning_delta_emits_thinking() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    // 旧命名：response.reasoning.delta
    let v1 = json!({"type": "response.reasoning.delta", "delta": "step a"});
    let e1 = responses_chunk_to_events(&v1, &mut tracks);
    assert_eq!(e1.len(), 1);
    assert!(
        matches!(
            &e1[0],
            StreamEvent::Thinking {
                delta,
                source: ThinkingSource::Raw,
                signature: None
            } if delta == "step a"
        ),
        "got {:?}",
        e1[0]
    );

    // 主流命名：response.reasoning_text.delta
    let v2 = json!({"type": "response.reasoning_text.delta", "delta": "step b"});
    let e2 = responses_chunk_to_events(&v2, &mut tracks);
    assert!(
        matches!(
            &e2[0],
            StreamEvent::Thinking {
                delta,
                source: ThinkingSource::Raw,
                ..
            } if delta == "step b"
        ),
        "got {:?}",
        e2[0]
    );

    // Summary 流：response.reasoning_summary_text.delta
    let v3 = json!({"type": "response.reasoning_summary_text.delta", "delta": "outline"});
    let e3 = responses_chunk_to_events(&v3, &mut tracks);
    assert!(
        matches!(
            &e3[0],
            StreamEvent::Thinking {
                delta,
                source: ThinkingSource::Summary,
                ..
            } if delta == "outline"
        ),
        "got {:?}",
        e3[0]
    );

    // 兼容形态：response.reasoning_summary.delta（summary 数组）
    let v4 = json!({
        "type": "response.reasoning_summary.delta",
        "summary": [{"type": "summary_text", "text": "plan first"}]
    });
    let e4 = responses_chunk_to_events(&v4, &mut tracks);
    assert!(
        matches!(
            &e4[0],
            StreamEvent::Thinking {
                delta,
                source: ThinkingSource::Summary,
                ..
            } if delta == "plan first"
        ),
        "got {:?}",
        e4[0]
    );

    // 空格敏感：前导空格必须保留，避免词边界丢失。
    let v5 = json!({"type": "response.reasoning_text.delta", "delta": " step c"});
    let e5 = responses_chunk_to_events(&v5, &mut tracks);
    assert!(
        matches!(
            &e5[0],
            StreamEvent::Thinking {
                delta,
                source: ThinkingSource::Raw,
                ..
            } if delta == " step c"
        ),
        "got {:?}",
        e5[0]
    );

    // 仅空格分片也应透传（用于跨帧拼词）。
    let v6 = json!({"type": "response.reasoning_text.delta", "delta": " "});
    let e6 = responses_chunk_to_events(&v6, &mut tracks);
    assert!(
        matches!(
            &e6[0],
            StreamEvent::Thinking {
                delta,
                source: ThinkingSource::Raw,
                ..
            } if delta == " "
        ),
        "got {:?}",
        e6[0]
    );

    // 部分网关仅在 summary_part.done 给出文本。
    let v7 = json!({
        "type": "response.reasoning_summary_part.done",
        "part": {"type": "summary_text", "text": "final summary"}
    });
    let e7 = responses_chunk_to_events(&v7, &mut tracks);
    assert!(
        matches!(
            &e7[0],
            StreamEvent::Thinking {
                delta,
                source: ThinkingSource::Summary,
                ..
            } if delta == "final summary"
        ),
        "got {:?}",
        e7[0]
    );

    // 兼容 output_item.done 中的 reasoning item（无 delta 事件时兜底）。
    let v8 = json!({
        "type": "response.output_item.done",
        "item": {
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": "from output item"}]
        }
    });
    let e8 = responses_chunk_to_events(&v8, &mut tracks);
    assert!(
        matches!(
            &e8[0],
            StreamEvent::Thinking {
                delta,
                source: ThinkingSource::Summary,
                ..
            } if delta == "from output item"
        ),
        "got {:?}",
        e8[0]
    );
}

#[test]
fn responses_chunk_reasoning_delta_preserves_word_boundaries_between_frames() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let v1 = json!({"type": "response.reasoning_text.delta", "delta": "hello "});
    let v2 = json!({"type": "response.reasoning_text.delta", "delta": "world"});
    let e1 = responses_chunk_to_events(&v1, &mut tracks);
    let e2 = responses_chunk_to_events(&v2, &mut tracks);
    let joined = [e1, e2]
        .into_iter()
        .flatten()
        .filter_map(|ev| match ev {
            StreamEvent::Thinking { delta, .. } => Some(delta),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    assert_eq!(joined, "hello world");
}

#[test]
fn responses_chunk_reasoning_done_emits_only_missing_suffix() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let mut reasoning = ReasoningState::default();
    let v1 = json!({
        "type": "response.reasoning_summary_text.delta",
        "item_id": "rs1",
        "summary_index": 0,
        "delta": "hello"
    });
    let v2 = json!({
        "type": "response.reasoning_summary_text.done",
        "item_id": "rs1",
        "summary_index": 0,
        "text": "hello world"
    });
    let joined = [v1, v2]
        .into_iter()
        .flat_map(|v| {
            let profile = test_profile();
            responses_chunk_to_events_with_state(&v, &mut tracks, &mut reasoning, &profile, true)
        })
        .filter_map(|ev| match ev {
            StreamEvent::Thinking { delta, .. } => Some(delta),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(joined, vec!["hello".to_string(), " world".to_string()]);
}

#[test]
fn responses_chunk_reasoning_mixed_events_are_deduped() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let mut reasoning = ReasoningState::default();
    let events = [
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "sum-1",
            "summary_index": 0,
            "delta": "plan first"
        }),
        json!({
            "type": "response.reasoning_summary_text.done",
            "item_id": "sum-1",
            "summary_index": 0,
            "text": "plan first"
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": "sum-1",
            "summary_index": 0,
            "part": {"type": "summary_text", "text": "plan first"}
        }),
        json!({
            "type": "response.output_item.done",
            "item": {
                "id": "sum-1",
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "plan first"}]
            }
        }),
        json!({
            "type": "response.reasoning_text.delta",
            "item_id": "raw-1",
            "content_index": 0,
            "delta": "raw step"
        }),
        json!({
            "type": "response.reasoning_text.done",
            "item_id": "raw-1",
            "content_index": 0,
            "text": "raw step"
        }),
        json!({
            "type": "response.output_item.done",
            "item": {
                "id": "raw-1",
                "type": "reasoning",
                "content": [{"type": "reasoning_text", "text": "raw step"}]
            }
        }),
    ];
    let observed = events
        .into_iter()
        .flat_map(|v| {
            let profile = test_profile();
            responses_chunk_to_events_with_state(&v, &mut tracks, &mut reasoning, &profile, true)
        })
        .filter_map(|ev| match ev {
            StreamEvent::Thinking { delta, source, .. } => Some((source, delta)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            (ThinkingSource::Summary, "plan first".to_string()),
            (ThinkingSource::Raw, "raw step".to_string()),
        ]
    );
}

#[test]
fn responses_chunk_reasoning_done_is_silent() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let v = json!({"type": "response.reasoning_text.done"});
    let e = responses_chunk_to_events(&v, &mut tracks);
    assert!(e.is_empty(), "reasoning *.done 不应额外发事件: {:?}", e);

    let v2 = json!({"type": "response.reasoning_summary.done"});
    let e2 = responses_chunk_to_events(&v2, &mut tracks);
    assert!(
        e2.is_empty(),
        "reasoning summary *.done 不应额外发事件: {:?}",
        e2
    );
}

#[test]
fn responses_chunk_reasoning_empty_delta_is_skipped() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let v = json!({"type": "response.reasoning_text.delta", "delta": ""});
    let e = responses_chunk_to_events(&v, &mut tracks);
    assert!(
        e.is_empty(),
        "空 reasoning delta 不应触发 Thinking: {:?}",
        e
    );
}

#[test]
fn responses_chunk_unknown_event_is_silent_not_panic() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let v = json!({"type": "response.something.never.heard.of"});
    let e = responses_chunk_to_events(&v, &mut tracks);
    assert!(e.is_empty(), "未知事件应静默忽略: {:?}", e);
}

#[test]
fn responses_chunk_output_item_done_non_reasoning_is_silent() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let v = json!({
        "type": "response.output_item.done",
        "item": {
            "type": "message",
            "content": [{"type": "output_text", "text": "final text"}]
        }
    });
    let e = responses_chunk_to_events(&v, &mut tracks);
    assert!(
        e.is_empty(),
        "非 reasoning output_item.done 不应映射 Thinking: {:?}",
        e
    );
}

#[test]
fn responses_chunk_output_item_done_function_call_appends_missing_suffix() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let init = json!({
        "type": "response.output_item.added",
        "item": {
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_1",
            "name": "echo",
            "arguments": "{\"q\":"
        }
    });
    responses_chunk_to_events(&init, &mut tracks);
    let done = json!({
        "type": "response.output_item.done",
        "item": {
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_1",
            "name": "echo",
            "arguments": "{\"q\":\"x\"}"
        }
    });
    let events = responses_chunk_to_events(&done, &mut tracks);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        StreamEvent::ToolCallDelta {
            index,
            id: None,
            name: None,
            arguments_delta: Some(delta)
        } if *index == 0 && delta == "\"x\"}"
    ));
}

#[test]
fn responses_chunk_failed_event_emits_structured_error_and_finish_reason() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let value = json!({
        "type": "response.failed",
        "response": {"status": "failed", "error": {"code": "server_error", "message": "boom"}}
    });
    let events = responses_chunk_to_events(&value, &mut tracks);
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        StreamEvent::LlmError {
            reason,
            message,
            code: Some(code)
        } if reason == "error:boom" && message == "boom" && code == "server_error"
    ));
    assert!(
        matches!(&events[1], StreamEvent::FinishReason { reason } if reason == "error:boom"),
        "unexpected {:?}",
        events
    );
}

#[test]
fn responses_chunk_incomplete_content_filter_emits_structured_error_and_finish_reason() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let value = json!({
        "type": "response.incomplete",
        "response": {
            "status": "incomplete",
            "incomplete_details": {"reason": "content_filter"}
        }
    });
    let events = responses_chunk_to_events(&value, &mut tracks);
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        StreamEvent::LlmError {
            reason,
            message,
            code: None
        } if reason == "error:content_filter" && message == "content_filter"
    ));
    assert!(matches!(
        &events[1],
        StreamEvent::FinishReason { reason } if reason == "error:content_filter"
    ));
}

#[tokio::test]
async fn responses_stream_parses_sse_chunks() {
    use tokio_stream::StreamExt;

    let chunks: Vec<Result<Bytes, AppError>> = vec![
        Ok(Bytes::from(
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"item_id\":\"m1\",\"content_index\":0,\"delta\":\"Hello\"}\n\n",
        )),
        Ok(Bytes::from(
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"m1\",\"content_index\":0,\"delta\":\" world\"}\n\n",
        )),
        Ok(Bytes::from(
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
        )),
    ];
    let stream = tokio_stream::iter(chunks);
    let mut s = new_responses_stream(stream, false);
    let mut events = Vec::new();
    while let Some(item) = s.next().await {
        events.push(item.expect("ok"));
    }
    assert_eq!(events.len(), 4);
    assert!(
        matches!(&events[0], StreamEvent::ContentDelta { delta } if delta == "Hello"),
        "{:?}",
        events[0]
    );
    assert!(
        matches!(&events[1], StreamEvent::ContentDelta { delta } if delta == " world"),
        "{:?}",
        events[1]
    );
    assert!(
        matches!(&events[2], StreamEvent::FinishReason { reason } if reason == "stop"),
        "{:?}",
        events[2]
    );
    assert!(
        matches!(&events[3], StreamEvent::Usage { .. }),
        "{:?}",
        events[3]
    );
}

#[tokio::test]
async fn responses_stream_parses_ndjson_fallback() {
    use tokio_stream::StreamExt;

    let chunks: Vec<Result<Bytes, AppError>> = vec![
        Ok(Bytes::from(
            "{\"type\":\"response.output_text.delta\",\"item_id\":\"m1\",\"content_index\":0,\"delta\":\"a\"}\n",
        )),
        Ok(Bytes::from(
            "{\"type\":\"response.output_text.delta\",\"item_id\":\"m1\",\"content_index\":0,\"delta\":\"b\"}\n",
        )),
        Ok(Bytes::from(
            "{\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n",
        )),
    ];
    let stream = tokio_stream::iter(chunks);
    let mut s = new_responses_stream(stream, true);
    let mut events = Vec::new();
    while let Some(item) = s.next().await {
        events.push(item.expect("ok"));
    }
    assert!(events.len() >= 3);
    assert!(
        matches!(&events[0], StreamEvent::ContentDelta { delta } if delta == "a"),
        "{:?}",
        events[0]
    );
    assert!(
        matches!(&events[1], StreamEvent::ContentDelta { delta } if delta == "b"),
        "{:?}",
        events[1]
    );
    assert!(
        matches!(&events[2], StreamEvent::FinishReason { .. }),
        "{:?}",
        events[2]
    );
}

#[tokio::test]
async fn responses_stream_early_close_does_not_fabricate_finish_reason() {
    use tokio_stream::StreamExt;

    let chunks: Vec<Result<Bytes, AppError>> = vec![Ok(Bytes::from(
        "{\"type\":\"response.output_text.delta\",\"item_id\":\"m1\",\"content_index\":0,\"delta\":\"partial\"}\n",
    ))];
    let stream = tokio_stream::iter(chunks);
    let mut s = new_responses_stream(stream, true);
    let mut events = Vec::new();
    while let Some(item) = s.next().await {
        events.push(item.expect("ok"));
    }
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        StreamEvent::ContentDelta { delta } if delta == "partial"
    ));
}

#[tokio::test]
async fn responses_stream_auto_detects_ndjson_when_no_data_prefix() {
    use tokio_stream::StreamExt;

    // 不传 prefer_ndjson；首帧无 SSE `data: ` 前缀但有换行 → 应自动切 NDJSON。
    let chunks: Vec<Result<Bytes, AppError>> = vec![Ok(Bytes::from(
        "{\"type\":\"response.output_text.delta\",\"item_id\":\"m1\",\"content_index\":0,\"delta\":\"x\"}\n",
    ))];
    let stream = tokio_stream::iter(chunks);
    let mut s = new_responses_stream(stream, false);
    let evt = s.next().await.expect("event").expect("ok");
    assert!(
        matches!(&evt, StreamEvent::ContentDelta { delta } if delta == "x"),
        "{:?}",
        evt
    );
}

#[tokio::test(start_paused = true)]
async fn responses_idle_timeout_errors_when_no_bytes_arrive() {
    use tokio_stream::StreamExt;

    let source = tokio_stream::pending::<Result<Bytes, AppError>>();
    let mut stream = apply_stream_idle_timeout(source, 3);
    let next_task = tokio::spawn(async move { stream.next().await });

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(4)).await;

    let item = next_task
        .await
        .expect("join ok")
        .expect("should produce timeout error");
    match item {
        Err(err) => {
            let msg = llm_summary(&err).unwrap_or_else(|| err.to_string());
            assert_eq!(llm_stage(&err), Some(LlmErrorStage::IdleTimeout));
            assert!(msg.contains("流式空闲超时"), "unexpected msg: {}", msg);
            assert!(
                msg.contains("stream_timeout_sec=3s"),
                "unexpected msg: {}",
                msg
            );
        }
        other => panic!("expected timeout AppError, got {:?}", other),
    }
}

#[tokio::test(start_paused = true)]
async fn responses_keepalive_bytes_do_not_trigger_idle_timeout() {
    use tokio_stream::wrappers::IntervalStream;
    use tokio_stream::StreamExt;

    let interval = tokio::time::interval(Duration::from_millis(200));
    let source = IntervalStream::new(interval)
        .take(3)
        .map(|_| Ok(Bytes::from_static(b": keepalive\n\n")));
    let mut stream = apply_stream_idle_timeout(source, 1);
    let collect_task = tokio::spawn(async move {
        let mut out = Vec::new();
        while let Some(item) = stream.next().await {
            out.push(item);
        }
        out
    });

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;

    let out = collect_task.await.expect("join ok");
    assert_eq!(out.len(), 3);
    assert!(out.into_iter().all(|item| item.is_ok()));
}

#[test]
fn is_retriable_detects_429_and_5xx() {
    assert!(OpenAiResponsesProvider::is_retriable(
        &llm_http_status_error("openai-responses", 429, "rate limit",)
    ));
    assert!(OpenAiResponsesProvider::is_retriable(
        &llm_http_status_error("openai-responses", 503, "bad gateway",)
    ));
    assert!(!OpenAiResponsesProvider::is_retriable(
        &llm_http_status_error("openai-responses", 400, "bad request",)
    ));
}

fn responses_stream_test_provider(
    base_url: String,
    api_base_fallback: Option<String>,
    retry_count: u32,
) -> OpenAiResponsesProvider {
    let mut provider = provider_with_stub_key();
    provider.base_url = base_url;
    provider.api_base_fallback = api_base_fallback;
    provider.retry_count = retry_count;
    provider.stream_timeout_sec = 0;
    provider.client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build no-proxy reqwest client");
    provider
}

fn responses_stream_test_request() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5".to_string(),
        temperature: Some(0.0),
        max_tokens: Some(16),
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    }
}

fn responses_sse_body(events: &[&str]) -> String {
    let mut body = String::new();
    for event in events {
        body.push_str("data: ");
        body.push_str(event);
        body.push_str("\n\n");
    }
    body
}

#[tokio::test]
async fn chat_inner_gateway_503_sets_connect_stage() {
    let server = MockHttpServer::start(vec![ScriptedHttpResponse::json(
        503,
        r#"{"error":"upstream connect error or disconnect/reset before headers. reset reason: connection timeout"}"#,
    )])
    .await;
    let provider = responses_stream_test_provider(server.base_url.clone(), None, 0);
    let body = provider.build_request_body(&responses_stream_test_request(), false);
    let err = provider
        .chat_inner_with_body(&body, &server.base_url)
        .await
        .expect_err("503 网关错误应直接返回");
    assert_eq!(llm_http_status(&err), Some(503));
    assert_eq!(llm_stage(&err), Some(LlmErrorStage::Connect));
    server.shutdown().await;
}

#[tokio::test]
async fn responses_stream_post_once_header_read_timeout_maps_to_retryable_read_timeout() {
    let mut delayed = ScriptedHttpResponse::json(
        200,
        r#"{"id":"resp_timeout","status":"completed","output":[]}"#,
    );
    delayed.delay_ms = 1_100;
    let server = MockHttpServer::start(vec![delayed]).await;
    let mut provider = responses_stream_test_provider(server.base_url.clone(), None, 0);
    provider.http_read_timeout_sec = 1;
    provider.client = reqwest::Client::builder()
        .no_proxy()
        .read_timeout(Duration::from_secs(1))
        .build()
        .expect("build read-timeout reqwest client");
    let body = provider.build_request_body(&responses_stream_test_request(), true);
    let err = provider
        .stream_post_once(&server.base_url, &body)
        .await
        .expect_err("响应头迟迟不来时应命中读超时");
    assert_eq!(llm_stage(&err), Some(LlmErrorStage::ReadTimeout));
    assert!(OpenAiResponsesProvider::is_retriable(&err));
    let msg = llm_summary(&err).unwrap_or_else(|| err.to_string());
    assert!(
        msg.contains("等待响应头"),
        "错误文案应说明卡在响应头阶段，实际: {}",
        msg
    );
    assert!(
        msg.contains("http_read_timeout_sec=1s"),
        "错误文案应带 read timeout 配置，实际: {}",
        msg
    );
    assert!(
        !msg.contains("1800"),
        "短超时不应再冒名为 1800s 总超时，实际: {}",
        msg
    );
    server.shutdown().await;
}

#[tokio::test]
async fn responses_chat_stream_retries_503_before_first_delta_and_succeeds() {
    use tokio_stream::StreamExt;

    let server = MockHttpServer::start(vec![
        ScriptedHttpResponse::json(
            503,
            r#"{"error":"upstream connect error or disconnect/reset before headers. reset reason: connection timeout"}"#,
        ),
        ScriptedHttpResponse {
            status: 200,
            headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
            body: responses_sse_body(&[
                r#"{"type":"response.output_text.delta","item_id":"m1","content_index":0,"delta":"Hello"}"#,
                r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}}"#,
            ]),
            delay_ms: 0,
            declared_content_length: None,
        },
    ])
    .await;
    let provider = responses_stream_test_provider(server.base_url.clone(), None, 1);
    let mut stream = provider
        .chat_stream(responses_stream_test_request())
        .await
        .expect("503 后应自动重试成功");
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        if let StreamEvent::ContentDelta { delta } = item.expect("ok") {
            text.push_str(&delta);
        }
    }
    assert_eq!(server.request_count(), 2);
    assert_eq!(text, "Hello");
    server.shutdown().await;
}

#[tokio::test]
async fn responses_chat_stream_retry_exhaustion_returns_structured_503() {
    let server = MockHttpServer::start(vec![
        ScriptedHttpResponse::json(
            503,
            r#"{"error":"upstream connect error or disconnect/reset before headers. reset reason: connection timeout"}"#,
        ),
        ScriptedHttpResponse::json(
            503,
            r#"{"error":"upstream connect error or disconnect/reset before headers. reset reason: connection timeout"}"#,
        ),
    ])
    .await;
    let provider = responses_stream_test_provider(server.base_url.clone(), None, 1);
    let err = match provider.chat_stream(responses_stream_test_request()).await {
        Ok(_) => panic!("503 重试耗尽应返回错误"),
        Err(err) => err,
    };
    assert_eq!(llm_http_status(&err), Some(503));
    assert_eq!(llm_stage(&err), Some(LlmErrorStage::Connect));
    assert_eq!(server.request_count(), 2);
    server.shutdown().await;
}

#[tokio::test]
async fn responses_chat_stream_non_retryable_401_returns_immediately() {
    let server = MockHttpServer::start(vec![ScriptedHttpResponse::json(
        401,
        r#"{"error":"unauthorized"}"#,
    )])
    .await;
    let provider = responses_stream_test_provider(server.base_url.clone(), None, 2);
    let err = match provider.chat_stream(responses_stream_test_request()).await {
        Ok(_) => panic!("401 不应重试"),
        Err(err) => err,
    };
    assert_eq!(llm_http_status(&err), Some(401));
    assert_eq!(server.request_count(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn responses_chat_stream_after_first_delta_body_read_error_is_not_retried() {
    use tokio_stream::StreamExt;

    let body = responses_sse_body(&[
        r#"{"type":"response.output_text.delta","item_id":"m1","content_index":0,"delta":"Hello"}"#,
    ]);
    let server = MockHttpServer::start(vec![ScriptedHttpResponse {
        status: 200,
        headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
        body,
        delay_ms: 0,
        declared_content_length: None,
    }
    .with_declared_content_length(256)])
    .await;
    let provider = responses_stream_test_provider(server.base_url.clone(), None, 2);
    let mut stream = provider
        .chat_stream(responses_stream_test_request())
        .await
        .expect("首帧出 delta 后的断流应在消费阶段上抛");

    match stream.next().await {
        Some(Ok(StreamEvent::ContentDelta { delta })) => assert_eq!(delta, "Hello"),
        other => panic!("首帧应先拿到 content delta，实际: {:?}", other),
    }

    let err = match stream.next().await {
        Some(Err(err)) => err,
        other => panic!("断流后应上抛错误且不重试，实际: {:?}", other),
    };
    assert_eq!(llm_stage(&err), Some(LlmErrorStage::BodyRead));
    assert!(
        llm_summary(&err)
            .unwrap_or_else(|| err.to_string())
            .contains("流读取"),
        "错误摘要应保留流读取失败语义，实际: {}",
        err
    );
    assert_eq!(server.request_count(), 1, "首个 delta 后不应重新建连");
    server.shutdown().await;
}

#[tokio::test]
async fn responses_chat_stream_fallback_after_gateway_503_uses_secondary_base() {
    use tokio_stream::StreamExt;

    let primary = MockHttpServer::start(vec![ScriptedHttpResponse::json(
        503,
        r#"{"error":"upstream connect error or disconnect/reset before headers. reset reason: connection timeout"}"#,
    )])
    .await;
    let fallback = MockHttpServer::start(vec![ScriptedHttpResponse {
        status: 200,
        headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
        body: responses_sse_body(&[
            r#"{"type":"response.output_text.delta","item_id":"m1","content_index":0,"delta":"fallback"}"#,
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#,
        ]),
        delay_ms: 0,
        declared_content_length: None,
    }])
    .await;
    let provider = responses_stream_test_provider(
        primary.base_url.clone(),
        Some(fallback.base_url.clone()),
        0,
    );
    let mut stream = provider
        .chat_stream(responses_stream_test_request())
        .await
        .expect("fallback 应成功接管");
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        if let StreamEvent::ContentDelta { delta } = item.expect("ok") {
            text.push_str(&delta);
        }
    }
    assert_eq!(primary.request_count(), 1);
    assert_eq!(fallback.request_count(), 1);
    assert_eq!(text, "fallback");
    primary.shutdown().await;
    fallback.shutdown().await;
}

// ============================================================================
// 多模态 wire 翻译（plan §5 单元测试）
// ============================================================================

/// 一段固定的 1x1 PNG base64（仅供 wire 形状断言，不做大小/视觉判断）。
const TINY_PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

/// PR-RJ-0：把 inline base64 fixture 解码后写到 tempfile，
/// 供新签名 `image_b64(mime, &Path)` / `file_b64(filename, mime, &Path)` 使用。
fn write_tempfile_from_b64(b64: &str) -> tempfile::NamedTempFile {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    let mut f = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut f, &bytes).unwrap();
    f
}

#[test]
fn user_image_b64_renders_input_image_data_url() {
    let f = write_tempfile_from_b64(TINY_PNG_B64);
    let part = ChatMessageContentPart::image_b64("image/png", f.path())
        .expect("image_b64 should accept valid input");
    let msg = ChatMessage::user_with_parts(vec![ChatMessageContentPart::text("see this:"), part]);
    let (_ins, input) = build_responses_input_test(&[msg]);
    assert_eq!(input.len(), 1);
    let content = &input[0]["content"];
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "see this:");
    assert_eq!(content[1]["type"], "input_image");
    let url = content[1]["image_url"].as_str().expect("image_url present");
    assert!(
        url.starts_with("data:image/png;base64,"),
        "data URL prefix wrong: {}",
        url
    );
    assert!(content[1].get("file_id").is_none());
}

#[test]
fn user_references_flatten_into_single_input_text_and_keep_attachments_afterwards() {
    let msg = ChatMessage::user_with_parts(vec![
        ChatMessageContentPart::text("before "),
        ChatMessageContentPart::reference(ContextReference::selection(
            "src/lib.rs",
            "lib.rs:10-12",
            Some(10),
            Some(12),
            Some("fn hello() {}".to_string()),
        )),
        ChatMessageContentPart::text(" between "),
        ChatMessageContentPart::reference(ContextReference::file("docs/guide.md", "guide.md")),
        ChatMessageContentPart::image_file_id("img-file").expect("image file id"),
    ]);
    let (_ins, input) = build_responses_input_test(&[msg]);
    let content = &input[0]["content"];
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(
        content[0]["text"],
        "before <selection file=\"src/lib.rs\" lines=\"10-12\">\nfn hello() {}\n</selection> between [file reference] docs/guide.md"
    );
    assert_eq!(content[1]["type"], "input_image");
    assert_eq!(content[1]["file_id"], "img-file");
}

#[test]
fn user_file_b64_renders_input_file_data_url() {
    // 一段最小合法 base64（解码后仅 "PDF"），不真发 API；只断言 wire 形状。
    let pdf_b64 = "UERG"; // base64("PDF")
    let f = write_tempfile_from_b64(pdf_b64);
    let part = ChatMessageContentPart::file_b64("sample.pdf", "application/pdf", f.path())
        .expect("file_b64 should accept valid input");
    let msg = ChatMessage::user_with_parts(vec![part]);
    let (_ins, input) = build_responses_input_test(&[msg]);
    let content = &input[0]["content"];
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "");
    assert_eq!(content[1]["type"], "input_file");
    assert_eq!(content[1]["filename"], "sample.pdf");
    let data = content[1]["file_data"].as_str().expect("file_data present");
    assert_eq!(data, "data:application/pdf;base64,UERG");
    assert!(content[1].get("file_id").is_none());
}

#[test]
fn user_image_file_id_renders_file_id_field() {
    let part = ChatMessageContentPart::image_file_id("file-abc")
        .expect("image_file_id should accept non-empty id");
    let msg = ChatMessage::user_with_parts(vec![part]);
    let (_ins, input) = build_responses_input_test(&[msg]);
    let content = &input[0]["content"];
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "");
    assert_eq!(content[1]["type"], "input_image");
    assert_eq!(content[1]["file_id"], "file-abc");
    assert!(content[1].get("image_url").is_none());
}

#[test]
fn user_file_file_id_renders_file_id_field() {
    let part = ChatMessageContentPart::file_file_id("file-xyz", Some("notes.pdf".to_string()))
        .expect("file_file_id should accept non-empty id");
    let msg = ChatMessage::user_with_parts(vec![part]);
    let (_ins, input) = build_responses_input_test(&[msg]);
    let content = &input[0]["content"];
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "");
    assert_eq!(content[1]["type"], "input_file");
    assert_eq!(content[1]["file_id"], "file-xyz");
    assert!(content[1].get("filename").is_none());
    assert!(content[1].get("file_data").is_none());
}

#[test]
fn responses_build_request_body_degrades_unsupported_history_attachments_to_input_text() {
    let mut entry = responses_entry();
    entry.id = "text-only-responses".to_string();
    entry.capabilities.vision = false;
    entry.capabilities.files = false;
    let provider = provider_from_entry(entry.clone(), LlmConfig::default());
    let req = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(vec![
            ChatMessageContentPart::text("before "),
            ChatMessageContentPart::image_file_id("file-image").unwrap(),
            ChatMessageContentPart::text(" between "),
            ChatMessageContentPart::file_file_id("file-pdf", Some("guide.pdf".to_string()))
                .unwrap(),
        ])],
        model: entry.id,
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };

    let expected = format!(
        "before {UNSUPPORTED_IMAGE_INPUT_PLACEHOLDER} between {UNSUPPORTED_FILE_INPUT_PLACEHOLDER}"
    );
    let body = provider.build_request_body(&req, true);
    let input = body["input"].as_array().expect("responses input");
    let content = input[0]["content"].as_array().expect("responses content");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"].as_str(), Some(expected.as_str()));
}

#[test]
fn system_with_image_part_silently_drops_non_text() {
    // System / Assistant / Tool 角色出现非 text part 时 warn 并丢弃；wire 仅取文本。
    let mut sys = ChatMessage::system("");
    let f = write_tempfile_from_b64(TINY_PNG_B64);
    sys.content = Some(crate::core::llm::types::ChatMessageContent::Parts(vec![
        ChatMessageContentPart::text("system rules"),
        ChatMessageContentPart::image_b64("image/png", f.path()).expect("image_b64 ok"),
    ]));
    let user = ChatMessage::user("ping");
    let (ins, input) = build_responses_input_test(&[sys, user]);
    assert_eq!(ins.as_deref(), Some("system rules"));
    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[0]["content"][0]["type"], "input_text");
}
