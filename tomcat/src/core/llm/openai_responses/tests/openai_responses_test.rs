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
use crate::core::llm::provider::LlmProvider;
use crate::core::llm::types::{ChatMessage, ChatMessageContentPart, StreamEvent, ThinkingSource};
use crate::infra::error::{llm_stage, llm_summary, AppError, LlmErrorStage};
use crate::infra::LlmConfig;

use bytes::Bytes;
use serde_json::json;
use std::time::Duration;

const TEST_KEY_ENV: &str = "__OPENAI_RESPONSES_TEST_KEY__";

fn provider_with_stub_key() -> OpenAiResponsesProvider {
    // SAFETY: 单测内部，串行环境受 `--test-threads=1` 约束；mutate env 仅本测试感知。
    unsafe { std::env::set_var(TEST_KEY_ENV, "stub-key") };
    let cfg = LlmConfig {
        api_key_env: Some(TEST_KEY_ENV.to_string()),
        ..LlmConfig::default()
    };
    let p = OpenAiResponsesProvider::new(&cfg).expect("应该能构造 provider");
    // SAFETY: 同上，移除 env 避免污染后续用例。
    unsafe { std::env::remove_var(TEST_KEY_ENV) };
    p
}

#[test]
fn openai_files_client_is_lazy_once_per_provider() {
    // SAFETY: 单测串行，临时注入 stub key。
    unsafe { std::env::set_var(TEST_KEY_ENV, "stub-key") };
    let cfg = LlmConfig {
        api_key_env: Some(TEST_KEY_ENV.to_string()),
        ..LlmConfig::default()
    };
    let p = OpenAiResponsesProvider::new(&cfg).expect("provider new ok");
    // SAFETY: 清理环境变量，避免污染后续用例。
    unsafe { std::env::remove_var(TEST_KEY_ENV) };

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
    let (ins, input) = build_responses_input(&msgs);
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
    let (ins, input) = build_responses_input(&msgs);
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
    let (_ins, input) = build_responses_input(&msgs);
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
    let (_ins, input) = build_responses_input(&msgs);
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
    let (_ins, input) = build_responses_input(&msgs);
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
fn responses_build_request_body_disabled_thinking_omits_reasoning_field() {
    unsafe { std::env::set_var(TEST_KEY_ENV, "stub-key") };
    let cfg = LlmConfig {
        api_key_env: Some(TEST_KEY_ENV.to_string()),
        thinking: crate::infra::config::ThinkingConfig {
            enabled: false,
            ..crate::infra::config::ThinkingConfig::default()
        },
        ..LlmConfig::default()
    };
    let p = OpenAiResponsesProvider::new(&cfg).expect("provider new ok");
    unsafe { std::env::remove_var(TEST_KEY_ENV) };

    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
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
    unsafe { std::env::set_var(TEST_KEY_ENV, "stub-key") };
    let cfg = LlmConfig {
        api_key_env: Some(TEST_KEY_ENV.to_string()),
        thinking: crate::infra::config::ThinkingConfig {
            enabled: true,
            level: "high".into(),
            show: false,
            persist: false,
            ..crate::infra::config::ThinkingConfig::default()
        },
        ..LlmConfig::default()
    };
    let p = OpenAiResponsesProvider::new(&cfg).expect("provider new ok");
    unsafe { std::env::remove_var(TEST_KEY_ENV) };

    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
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
        "show=false && persist=false 时也应请求 reasoning.summary: {}",
        body
    );
    assert!(
        body.get("thinking").is_none(),
        "OpenAI 系不应同时写 thinking 对象"
    );
}

#[test]
fn responses_build_request_body_show_true_writes_reasoning_summary_auto() {
    unsafe { std::env::set_var(TEST_KEY_ENV, "stub-key") };
    let cfg = LlmConfig {
        api_key_env: Some(TEST_KEY_ENV.to_string()),
        thinking: crate::infra::config::ThinkingConfig {
            enabled: true,
            show: true,
            persist: false,
            ..crate::infra::config::ThinkingConfig::default()
        },
        ..LlmConfig::default()
    };
    let p = OpenAiResponsesProvider::new(&cfg).expect("provider new ok");
    unsafe { std::env::remove_var(TEST_KEY_ENV) };

    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        tools: None,
    };
    let body = p.build_request_body(&req, true);
    assert_eq!(
        body["reasoning"]["summary"], "auto",
        "show=true 时应请求 summary"
    );
}

#[test]
fn responses_build_request_body_persist_true_writes_reasoning_summary_auto() {
    unsafe { std::env::set_var(TEST_KEY_ENV, "stub-key") };
    let cfg = LlmConfig {
        api_key_env: Some(TEST_KEY_ENV.to_string()),
        thinking: crate::infra::config::ThinkingConfig {
            enabled: true,
            show: false,
            persist: true,
            ..crate::infra::config::ThinkingConfig::default()
        },
        ..LlmConfig::default()
    };
    let p = OpenAiResponsesProvider::new(&cfg).expect("provider new ok");
    unsafe { std::env::remove_var(TEST_KEY_ENV) };

    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
        tools: None,
    };
    let body = p.build_request_body(&req, true);
    assert_eq!(
        body["reasoning"]["summary"], "auto",
        "persist=true 时应请求 summary，即使 show=false"
    );
}

#[test]
fn responses_build_request_body_show_and_persist_false_still_writes_reasoning_summary_auto() {
    unsafe { std::env::set_var(TEST_KEY_ENV, "stub-key") };
    let cfg = LlmConfig {
        api_key_env: Some(TEST_KEY_ENV.to_string()),
        thinking: crate::infra::config::ThinkingConfig {
            enabled: true,
            show: false,
            persist: false,
            ..crate::infra::config::ThinkingConfig::default()
        },
        ..LlmConfig::default()
    };
    let p = OpenAiResponsesProvider::new(&cfg).expect("provider new ok");
    unsafe { std::env::remove_var(TEST_KEY_ENV) };

    let req = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        model: "gpt-5".into(),
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        model_override: None,
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
fn responses_chunk_failed_event_emits_error_finish_reason() {
    let mut tracks: Vec<ToolCallTrack> = Vec::new();
    let value = json!({
        "type": "response.failed",
        "response": {"error": {"message": "boom"}}
    });
    let events = responses_chunk_to_events(&value, &mut tracks);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], StreamEvent::FinishReason { reason } if reason == "error:boom"),
        "unexpected {:?}",
        events[0]
    );
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
    let mut s = ResponsesStream::new(stream, false);
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
    let mut s = ResponsesStream::new(stream, true);
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
async fn responses_stream_auto_detects_ndjson_when_no_data_prefix() {
    use tokio_stream::StreamExt;

    // 不传 prefer_ndjson；首帧无 SSE `data: ` 前缀但有换行 → 应自动切 NDJSON。
    let chunks: Vec<Result<Bytes, AppError>> = vec![Ok(Bytes::from(
        "{\"type\":\"response.output_text.delta\",\"item_id\":\"m1\",\"content_index\":0,\"delta\":\"x\"}\n",
    ))];
    let stream = tokio_stream::iter(chunks);
    let mut s = ResponsesStream::new(stream, false);
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
    assert!(OpenAiResponsesProvider::is_retriable(&AppError::Llm(
        "API 错误 429: rate limit".to_string()
    )));
    assert!(OpenAiResponsesProvider::is_retriable(&AppError::Llm(
        "API 错误 503: bad gateway".to_string()
    )));
    assert!(!OpenAiResponsesProvider::is_retriable(&AppError::Llm(
        "API 错误 400: bad request".to_string()
    )));
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
    let (_ins, input) = build_responses_input(&[msg]);
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
fn user_file_b64_renders_input_file_data_url() {
    // 一段最小合法 base64（解码后仅 "PDF"），不真发 API；只断言 wire 形状。
    let pdf_b64 = "UERG"; // base64("PDF")
    let f = write_tempfile_from_b64(pdf_b64);
    let part = ChatMessageContentPart::file_b64("sample.pdf", "application/pdf", f.path())
        .expect("file_b64 should accept valid input");
    let msg = ChatMessage::user_with_parts(vec![part]);
    let (_ins, input) = build_responses_input(&[msg]);
    let content = &input[0]["content"];
    assert_eq!(content[0]["type"], "input_file");
    assert_eq!(content[0]["filename"], "sample.pdf");
    let data = content[0]["file_data"].as_str().expect("file_data present");
    assert_eq!(data, "data:application/pdf;base64,UERG");
    assert!(content[0].get("file_id").is_none());
}

#[test]
fn user_image_file_id_renders_file_id_field() {
    let part = ChatMessageContentPart::image_file_id("file-abc")
        .expect("image_file_id should accept non-empty id");
    let msg = ChatMessage::user_with_parts(vec![part]);
    let (_ins, input) = build_responses_input(&[msg]);
    let content = &input[0]["content"];
    assert_eq!(content[0]["type"], "input_image");
    assert_eq!(content[0]["file_id"], "file-abc");
    assert!(content[0].get("image_url").is_none());
}

#[test]
fn user_file_file_id_renders_file_id_field() {
    let part = ChatMessageContentPart::file_file_id("file-xyz", Some("notes.pdf".to_string()))
        .expect("file_file_id should accept non-empty id");
    let msg = ChatMessage::user_with_parts(vec![part]);
    let (_ins, input) = build_responses_input(&[msg]);
    let content = &input[0]["content"];
    assert_eq!(content[0]["type"], "input_file");
    assert_eq!(content[0]["filename"], "notes.pdf");
    assert_eq!(content[0]["file_id"], "file-xyz");
    assert!(content[0].get("file_data").is_none());
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
    let (ins, input) = build_responses_input(&[sys, user]);
    assert_eq!(ins.as_deref(), Some("system rules"));
    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[0]["content"][0]["type"], "input_text");
}
