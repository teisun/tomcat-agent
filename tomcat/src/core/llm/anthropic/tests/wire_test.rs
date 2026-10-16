use async_trait::async_trait;
use serde_json::{json, Value};

use super::{
    build_messages, build_request_body, final_stream_events, fingerprint,
    response_to_chat_response, rolling_array_fingerprints, usage_from_value,
    user_message_tail_fingerprints, ANTHROPIC_MAX_CACHE_BREAKPOINTS,
};
use crate::core::llm::files_api::FilesApiAdapter;
use crate::core::llm::openai_files::{FilePurpose, OpenAiFileMeta};
use crate::core::llm::replay_policy::ProviderCompatProfile;
use crate::core::llm::thinking_policy::ThinkingFormat;
use crate::core::llm::types::{
    ChatMessage, ChatMessageContentPart, ChatRequest, ReasoningFormat, StreamEvent,
};
use crate::core::llm::Capabilities;
use crate::infra::config::ThinkingConfig;
use crate::infra::error::AppError;
use crate::infra::events::ToolDisplay;

#[derive(Debug)]
struct StaticFilesAdapter {
    prefix: &'static str,
}

#[async_trait]
impl FilesApiAdapter for StaticFilesAdapter {
    async fn upload(
        &self,
        _purpose: FilePurpose,
        _filename: &str,
        _mime_type: &str,
        _bytes: &[u8],
    ) -> Result<OpenAiFileMeta, AppError> {
        unreachable!("upload should not be called in wire tests");
    }

    async fn delete(&self, _file_id: &str) -> Result<(), AppError> {
        unreachable!("delete should not be called in wire tests");
    }

    fn expires_after_seconds(&self) -> u64 {
        3600
    }

    fn reference_token(&self, file_id: &str) -> String {
        format!("{}{}", self.prefix, file_id)
    }
}

fn count_cache_controls(value: &Value) -> usize {
    match value {
        Value::Array(items) => items.iter().map(count_cache_controls).sum(),
        Value::Object(entries) => {
            usize::from(entries.contains_key("cache_control"))
                + entries.values().map(count_cache_controls).sum::<usize>()
        }
        _ => 0,
    }
}

fn anthropic_body(messages: Vec<ChatMessage>) -> Value {
    build_request_body(
        &ChatRequest {
            messages,
            model: "ignored".to_string(),
            ..Default::default()
        },
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        true,
        &Capabilities::default(),
        None,
    )
}

#[test]
fn tool_owned_media_anthropic_native_split_batch_and_downgrade() {
    use crate::core::llm::{tool_result_media::ToolResultMediaMode, ChatMessageContent};
    let image = ChatMessageContentPart::image_base64_data("image/png", "aGVsbG8=").unwrap();
    let file = ChatMessageContentPart::file_file_id("file-pdf", Some("notes.pdf".into())).unwrap();
    let mut first = ChatMessage::tool("one", "");
    first.content = Some(ChatMessageContent::Parts(vec![
        ChatMessageContentPart::text("first"),
        image.clone(),
        file,
    ]));
    let mut second = ChatMessage::tool("two", "");
    second.content = Some(ChatMessageContent::Parts(vec![
        ChatMessageContentPart::text("second"),
        image,
    ]));
    let messages = vec![
        ChatMessage::user("request"),
        ChatMessage::assistant_with_tool_calls(
            None,
            vec![
                json!({"id":"one","function":{"name":"read","arguments":"{}"}}),
                json!({"id":"two","function":{"name":"read","arguments":"{}"}}),
            ],
        ),
        first,
        second,
    ];
    let before = serde_json::to_value(&messages).unwrap();
    let target = ProviderCompatProfile::anthropic_messages("claude-opus-4-6");
    let caps = Capabilities {
        vision: true,
        files: true,
        ..Default::default()
    };
    let adapter = StaticFilesAdapter { prefix: "" };
    for mode in [ToolResultMediaMode::Native, ToolResultMediaMode::Split] {
        let rendered = super::build_messages_with_media_mode(
            &messages,
            &target,
            false,
            &caps,
            Some(&adapter),
            mode,
        );
        let blocks = rendered.messages[2]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["tool_use_id"], "one");
        assert_eq!(blocks[1]["tool_use_id"], "two");
        if mode == ToolResultMediaMode::Native {
            assert_eq!(blocks.len(), 2);
            assert_eq!(blocks[0]["content"][0]["text"], "first");
            assert_eq!(blocks[0]["content"][1]["type"], "image");
            assert_eq!(blocks[0]["content"][2]["source"]["file_id"], "file-pdf");
        } else {
            assert_eq!(blocks[0]["content"], "first");
            assert_eq!(blocks[1]["content"], "second");
            assert_eq!(
                blocks
                    .iter()
                    .skip(2)
                    .map(|p| p["type"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                vec!["image", "document", "image"]
            );
        }
    }
    let degraded = super::build_messages_with_media_mode(
        &messages,
        &target,
        false,
        &Capabilities::default(),
        None,
        ToolResultMediaMode::Native,
    );
    assert!(!serde_json::to_string(&degraded.messages)
        .unwrap()
        .contains("image/png"));
    assert_eq!(serde_json::to_value(messages).unwrap(), before);
}

#[test]
fn tool_media_anthropic_wire_equals_user_media() {
    let parts = vec![ChatMessageContentPart::image_base64_data("image/png", "aGVsbG8=").unwrap()];
    let body = |message| {
        build_request_body(
            &ChatRequest {
                messages: vec![message],
                model: "claude-opus".into(),
                ..Default::default()
            },
            "claude-opus",
            &ThinkingConfig::default(),
            ThinkingFormat::AnthropicAdaptive,
            true,
            true,
            &Capabilities {
                vision: true,
                ..Default::default()
            },
            None,
        )
    };
    assert_eq!(
        body(ChatMessage::tool_media(parts.clone())),
        body(ChatMessage::user_with_parts(parts))
    );
}

#[test]
fn messages_wire_omits_local_message_metadata() {
    let mut message = ChatMessage::assistant("wire-visible text");
    message.summary_title = Some("local summary".to_string());
    message.is_error = Some(true);
    message.tool_display = Some(ToolDisplay::Text {
        text: "local tool display".to_string(),
    });
    message.usage = Some(Default::default());

    let body = anthropic_body(vec![message]);
    let serialized = serde_json::to_string(&body).expect("serialize wire body");
    for local_field in ["tool_display", "summary_title", "usage", "is_error"] {
        assert!(
            !serialized.contains(local_field),
            "Anthropic Messages wire leaked local field `{local_field}`: {serialized}"
        );
    }
}

fn assistant_tool_call(id: &str, path: &str) -> ChatMessage {
    let mut message = ChatMessage::assistant(format!("I will inspect {path}"));
    message.tool_calls = Some(vec![json!({
        "id": id,
        "function": {
            "name": "read",
            "arguments": format!(r#"{{"path":"{path}"}}"#),
        }
    })]);
    message
}

#[test]
fn rolling_prefix_hashes_match_the_legacy_serialized_array_hashes() {
    let messages = vec![
        json!({"role": "user", "content": [{"type": "text", "text": "one"}]}),
        json!({"role": "assistant", "content": [{"type": "text", "text": "two"}]}),
        json!({"role": "user", "content": [{"type": "text", "text": "three"}]}),
    ];
    let legacy = (1..=messages.len())
        .map(|end| fingerprint(&Value::Array(messages[..end].to_vec())))
        .collect::<Vec<_>>();

    assert_eq!(rolling_array_fingerprints(&messages), legacy);
}

#[test]
fn user_tail_hash_pair_distinguishes_the_volatile_tail() {
    let message = json!({
        "role": "user",
        "content": [
            {"type": "text", "text": "persisted input"},
            {"type": "text", "text": "<system_reminder>volatile</system_reminder>"}
        ]
    });
    let (with_tail, without_tail) = user_message_tail_fingerprints(&message, 1);
    assert_ne!(with_tail, without_tail);
    assert_eq!(
        without_tail,
        fingerprint(&json!({
            "role": "user",
            "content": [{"type": "text", "text": "persisted input"}]
        }))
    );
}

#[test]
fn non_stream_anthropic_usage_counts_the_complete_input() {
    let usage = usage_from_value(Some(&json!({
        "input_tokens": 10,
        "cache_read_input_tokens": 202_789,
        "cache_creation_input_tokens": 6_801,
        "output_tokens": 1_257,
    })))
    .expect("usage fields were provided");

    assert_eq!(usage.prompt_tokens, 209_600);
    assert_eq!(usage.completion_tokens, 1_257);
    assert_eq!(usage.total_tokens, Some(210_857));
    assert_eq!(usage.cache_read_tokens, Some(202_789));
    assert_eq!(usage.cache_write_tokens, Some(6_801));
}

#[test]
fn build_request_body_extracts_system_and_user_messages() {
    let request = ChatRequest {
        messages: vec![
            ChatMessage::system("be helpful"),
            ChatMessage::user("hello"),
        ],
        model: "ignored".to_string(),
        temperature: Some(0.2),
        max_tokens: Some(4096),
        resolved_output_limit: None,
        diagnostic_request_id: None,
        stream: Some(true),
        model_override: None,
        speed: None,
        thinking_level: None,
        cache_key: None,
        tools: None,
    };

    let body = build_request_body(
        &request,
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        true,
        &Capabilities::default(),
        None,
    );

    assert_eq!(body["model"], "claude-opus-4-6");
    assert_eq!(body["system"][0]["type"], "text");
    assert_eq!(body["system"][0]["text"], "be helpful");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"][0]["text"], "hello");
    assert_eq!(body["stream"], true);
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "high");
    let temperature = body["temperature"]
        .as_f64()
        .expect("temperature serialized as number");
    assert!((temperature - 0.2).abs() < 1e-6);
}

#[test]
fn model_output_limit_drives_anthropic_wire_max_tokens() {
    let request = ChatRequest {
        messages: vec![ChatMessage::user("hello")],
        model: "ignored".to_string(),
        temperature: None,
        max_tokens: None,
        resolved_output_limit: Some(128_000),
        diagnostic_request_id: None,
        stream: Some(true),
        model_override: None,
        speed: None,
        thinking_level: None,
        cache_key: None,
        tools: None,
    };
    let body = build_request_body(
        &request,
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        true,
        &Capabilities::default(),
        None,
    );
    assert_eq!(body["max_tokens"], 128_000);

    let capped = build_request_body(
        &ChatRequest {
            max_tokens: Some(256_000),
            resolved_output_limit: Some(128_000),
            diagnostic_request_id: None,
            ..request
        },
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        true,
        &Capabilities::default(),
        None,
    );
    assert_eq!(capped["max_tokens"], 128_000);
}

#[test]
fn anthropic_wire_uses_a_legal_conservative_cap_without_resolved_limits() {
    let request = ChatRequest {
        messages: vec![ChatMessage::user("hello")],
        model: "ignored".to_string(),
        ..Default::default()
    };
    let body = build_request_body(
        &request,
        "unknown-anthropic-model",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        true,
        &Capabilities::default(),
        None,
    );
    assert_eq!(body["max_tokens"], 32_000);
}

#[test]
fn ephemeral_tail_is_an_uncached_system_suffix_and_d_ends_the_latest_message() {
    let mut summary = ChatMessage::user("compacted history");
    summary.kind = crate::core::llm::MessageKind::CompactionSummary;
    let mut tail = ChatMessage::user("<system_reminder>runtime state</system_reminder>");
    tail.kind = crate::core::llm::MessageKind::EphemeralTail;
    let request = ChatRequest {
        messages: vec![
            ChatMessage::system("stable system"),
            summary,
            ChatMessage::user("latest persisted input"),
            tail,
        ],
        model: "ignored".to_string(),
        tools: Some(vec![json!({
            "type": "function",
            "function": {
                "name": "read",
                "description": "read a file",
                "parameters": {"type": "object"},
            }
        })]),
        ..Default::default()
    };

    let body = build_request_body(
        &request,
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        true,
        &Capabilities::default(),
        None,
    );

    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(body["tools"][0]["cache_control"]["type"], "ephemeral");
    let content = body["messages"][0]["content"]
        .as_array()
        .expect("merged user content");
    assert!(
        content
            .get(1)
            .is_some_and(|block| block["cache_control"]["type"] == "ephemeral"),
        "the latest persisted block is the deepest cache write anchor"
    );
    assert!(
        body["system"][1].get("cache_control").is_none(),
        "the runtime tail must not receive its own cache-control marker"
    );
    assert_eq!(
        body["system"][1]["text"],
        "<system_reminder>runtime state</system_reminder>"
    );
    assert!(
        !body["messages"]
            .to_string()
            .contains("<system_reminder>runtime state</system_reminder>"),
        "EphemeralTail must never be serialized as an Anthropic dialogue message"
    );
}

#[test]
fn cache_breakpoints_mark_c_d_and_do_not_mark_the_system_tail_itself() {
    let mut assistant = ChatMessage::assistant("I will inspect it");
    assistant.tool_calls = Some(vec![json!({
        "id": "call-1",
        "function": {"name": "read", "arguments": "{\"path\":\"src/lib.rs\"}"}
    })]);
    let mut tail = ChatMessage::user("<system_reminder>runtime state</system_reminder>");
    tail.kind = crate::core::llm::MessageKind::EphemeralTail;
    let request = ChatRequest {
        messages: vec![
            ChatMessage::user("inspect the file"),
            assistant,
            ChatMessage::tool("call-1", "file contents"),
            tail,
        ],
        model: "ignored".to_string(),
        ..Default::default()
    };

    let body = build_request_body(
        &request,
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        true,
        &Capabilities::default(),
        None,
    );

    assert_eq!(
        body["messages"][0]["content"][0]["cache_control"]["type"], "ephemeral",
        "the earlier user message is the rolling read boundary"
    );
    assert_eq!(
        body["messages"][2]["content"][0]["cache_control"]["type"], "ephemeral",
        "the tool-result message is the newest stable write boundary"
    );
    assert_eq!(
        body["system"][0]["text"],
        "<system_reminder>runtime state</system_reminder>"
    );
    assert!(body["system"][0].get("cache_control").is_none());
}

/// One user turn can contain many tool rounds. Each request receives a
/// transient tail, but that tail must never move either rolling cache
/// anchor back by one completed tool result.
#[test]
fn eight_round_marathon_keeps_rolling_c_and_d_with_a_system_tail() {
    let read_tool = json!({
        "type": "function",
        "function": {
            "name": "read",
            "description": "Read a file for the marathon cache regression.",
            "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }
        }
    });
    let mut history = vec![
        ChatMessage::system("stable system"),
        ChatMessage::user("Start one long tool-driven task."),
    ];

    for round in 1..=8 {
        let mut request_messages = history.clone();
        let mut tail = ChatMessage::user(format!(
            "<system_reminder>round {round} runtime state</system_reminder>"
        ));
        tail.kind = crate::core::llm::MessageKind::EphemeralTail;
        request_messages.push(tail);

        let body = build_request_body(
            &ChatRequest {
                messages: request_messages,
                model: "ignored".to_string(),
                tools: Some(vec![read_tool.clone()]),
                ..Default::default()
            },
            "claude-opus-4-6",
            &ThinkingConfig::default(),
            ThinkingFormat::AnthropicAdaptive,
            true,
            true,
            &Capabilities::default(),
            None,
        );
        let wire_messages = body["messages"].as_array().expect("wire messages");
        let user_indices: Vec<_> = wire_messages
            .iter()
            .enumerate()
            .filter_map(|(idx, message)| (message["role"].as_str() == Some("user")).then_some(idx))
            .collect();
        let newest_user = *user_indices.last().expect("newest wire user message");
        let newest_content = wire_messages[newest_user]["content"]
            .as_array()
            .expect("newest user content");

        assert_eq!(
            newest_content.last().unwrap()["cache_control"]["type"],
            "ephemeral",
            "round {round}: D must mark the end of the newest stable user/tool-result"
        );
        assert!(
            body["system"]
                .as_array()
                .and_then(|blocks| blocks.last())
                .is_some_and(|block| block.get("cache_control").is_none()),
            "round {round}: runtime tail must not receive its own cache-control marker"
        );

        if let Some(&previous_user) = user_indices.iter().rev().nth(1) {
            let previous_content = wire_messages[previous_user]["content"]
                .as_array()
                .expect("previous user content");
            assert_eq!(
                previous_content
                    .last()
                    .and_then(|block| block["cache_control"]["type"].as_str()),
                Some("ephemeral"),
                "round {round}: C must remain on the previous completed user/tool-result"
            );
        }

        let call_id = format!("marathon-read-{round}");
        let mut assistant = ChatMessage::assistant(format!("Reading round {round}"));
        assistant.tool_calls = Some(vec![json!({
            "id": call_id,
            "type": "function",
            "function": {
                "name": "read",
                "arguments": format!(r#"{{"path":"/virtual/round-{round}.txt"}}"#),
            }
        })]);
        history.push(assistant);
        let tool_result = format!(
            "tool-result round {round}: {}",
            "stable output ".repeat(200)
        );
        history.push(ChatMessage::tool(&call_id, &tool_result));
    }
}

#[test]
fn rendered_anthropic_messages_never_repeat_adjacent_roles() {
    let mut assistant = ChatMessage::assistant("calling a tool");
    assistant.tool_calls = Some(vec![json!({
        "id": "call-1",
        "function": {"name": "read", "arguments": "{\"path\":\"src/lib.rs\"}"}
    })]);
    let mut tail = ChatMessage::user("<system_reminder>volatile state</system_reminder>");
    tail.kind = crate::core::llm::MessageKind::EphemeralTail;
    let request = ChatRequest {
        messages: vec![
            ChatMessage::user("first user part"),
            ChatMessage::user("second user part"),
            ChatMessage::assistant("first assistant part"),
            assistant,
            ChatMessage::tool("call-1", "file contents"),
            tail,
            ChatMessage::assistant("final answer"),
            ChatMessage::assistant("additional answer text"),
        ],
        model: "ignored".to_string(),
        ..Default::default()
    };

    let body = build_request_body(
        &request,
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        true,
        &Capabilities::default(),
        None,
    );
    let roles = body["messages"]
        .as_array()
        .expect("wire messages")
        .iter()
        .map(|message| message["role"].as_str().expect("role"))
        .collect::<Vec<_>>();
    assert!(
        roles.windows(2).all(|pair| pair[0] != pair[1]),
        "Anthropic's strict alternation must survive all merge paths: {roles:?}"
    );
}

#[test]
fn cache_breakpoints_never_exceed_the_provider_budget_with_many_summaries() {
    let mut summary = ChatMessage::user("compacted history");
    summary.kind = crate::core::llm::MessageKind::CompactionSummary;
    let mut tail = ChatMessage::user("<system_reminder>runtime state</system_reminder>");
    tail.kind = crate::core::llm::MessageKind::EphemeralTail;
    let mut first_tool_call = ChatMessage::assistant("I will inspect the first file");
    first_tool_call.tool_calls = Some(vec![json!({
        "id": "call-1",
        "function": {"name": "read", "arguments": "{\"path\":\"first.rs\"}"}
    })]);
    let mut second_tool_call = ChatMessage::assistant("I will inspect the second file");
    second_tool_call.tool_calls = Some(vec![json!({
        "id": "call-2",
        "function": {"name": "read", "arguments": "{\"path\":\"second.rs\"}"}
    })]);
    let request = ChatRequest {
        messages: vec![
            ChatMessage::system("stable system"),
            summary,
            first_tool_call,
            ChatMessage::tool("call-1", "first file contents"),
            second_tool_call,
            ChatMessage::tool("call-2", "second file contents"),
            tail,
        ],
        model: "ignored".to_string(),
        tools: Some(vec![json!({
            "type": "function",
            "function": {
                "name": "read",
                "description": "read a file",
                "parameters": {"type": "object"},
            }
        })]),
        ..Default::default()
    };

    let body = build_request_body(
        &request,
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        true,
        &Capabilities::default(),
        None,
    );

    assert_eq!(count_cache_controls(&body), ANTHROPIC_MAX_CACHE_BREAKPOINTS);
    assert!(
        body["messages"][0]["content"][0]
            .get("cache_control")
            .is_none(),
        "CompactionSummary must not consume the rolling user breakpoint"
    );
    let previous_tool_result = body["messages"][2]["content"]
        .as_array()
        .expect("first tool result is a distinct wire user message");
    assert!(
        previous_tool_result
            .last()
            .is_some_and(|block| block["cache_control"]["type"] == "ephemeral"),
        "the third slot must stay on the penultimate wire user message"
    );
    assert!(
        body["messages"][4]["content"]
            .as_array()
            .and_then(|blocks| blocks.first())
            .is_some_and(|block| block["cache_control"]["type"] == "ephemeral"),
        "the fourth slot must mark the newest stable tool-result block"
    );
    assert!(
        body["messages"][4]["content"][1]
            .get("cache_control")
            .is_none(),
        "the tail remains outside the deepest write prefix"
    );
}

#[test]
fn cache_breakpoints_handle_empty_system_and_tools() {
    let mut summary = ChatMessage::user("compacted history");
    summary.kind = crate::core::llm::MessageKind::CompactionSummary;
    let mut tail = ChatMessage::user("<system_reminder>runtime state</system_reminder>");
    tail.kind = crate::core::llm::MessageKind::EphemeralTail;
    let request = ChatRequest {
        messages: vec![summary, tail],
        model: "ignored".to_string(),
        ..Default::default()
    };

    let body = build_request_body(
        &request,
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        true,
        &Capabilities::default(),
        None,
    );

    assert_eq!(count_cache_controls(&body), 1);
    let content = body["messages"][0]["content"]
        .as_array()
        .expect("contiguous user messages merge into blocks");
    assert_eq!(
        content[0]["cache_control"]["type"], "ephemeral",
        "the persisted summary is the only cacheable message"
    );
    assert!(body["system"][0].get("cache_control").is_none());
}

#[test]
fn cache_breakpoints_allow_an_empty_history() {
    let body = build_request_body(
        &ChatRequest::default(),
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        true,
        &Capabilities::default(),
        None,
    );

    assert_eq!(count_cache_controls(&body), 0);
    assert_eq!(body["messages"], json!([]));
    assert!(body.get("system").is_none());
    assert!(body.get("tools").is_none());
}

#[test]
fn cache_breakpoints_never_mark_an_ephemeral_only_history() {
    let mut tail = ChatMessage::user("<system_reminder>runtime-only state</system_reminder>");
    tail.kind = crate::core::llm::MessageKind::EphemeralTail;
    let request = ChatRequest {
        messages: vec![tail],
        ..Default::default()
    };
    let body = build_request_body(
        &request,
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        true,
        &Capabilities::default(),
        None,
    );

    assert_eq!(count_cache_controls(&body), 0);
    assert_eq!(body["messages"], json!([]));
    assert_eq!(
        body["system"][0]["text"],
        "<system_reminder>runtime-only state</system_reminder>"
    );
    assert!(body["system"][0].get("cache_control").is_none());
}

#[test]
fn deepest_breakpoint_ends_the_real_message_before_the_system_tail() {
    let mut tail = ChatMessage::user("<system_reminder>runtime-only state</system_reminder>");
    tail.kind = crate::core::llm::MessageKind::EphemeralTail;
    let body = anthropic_body(vec![
        ChatMessage::user("persisted input"),
        ChatMessage::assistant("plain assistant response"),
        tail,
    ]);

    assert_eq!(
        body["messages"][1]["content"][0]["cache_control"]["type"], "ephemeral",
        "D ends the latest persisted message before the system suffix"
    );
    assert_eq!(
        body["system"][0]["text"],
        "<system_reminder>runtime-only state</system_reminder>"
    );
    assert!(body["system"][0].get("cache_control").is_none());
}

#[test]
fn cache_breakpoints_mark_a_single_persisted_message_without_a_tail() {
    let body = anthropic_body(vec![ChatMessage::user("persisted input")]);

    assert_eq!(count_cache_controls(&body), 1);
    assert_eq!(
        body["messages"][0]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
}

#[test]
fn cache_breakpoints_ignore_an_empty_rendered_tail() {
    let mut tail = ChatMessage::user_with_parts(Vec::new());
    tail.kind = crate::core::llm::MessageKind::EphemeralTail;
    let body = anthropic_body(vec![ChatMessage::user("persisted input"), tail]);

    assert_eq!(
        body["messages"],
        json!([{
            "role": "user",
            "content": [{
                "type": "text",
                "text": "persisted input",
                "cache_control": {"type": "ephemeral"}
            }]
        }])
    );
}

#[test]
fn rolling_breakpoint_c_marks_the_penultimate_wire_tool_result() {
    let mut first_tool_call = ChatMessage::assistant("I will inspect the first file");
    first_tool_call.tool_calls = Some(vec![json!({
        "id": "call-1",
        "function": {"name": "read", "arguments": "{\"path\":\"first.rs\"}"}
    })]);
    let mut second_tool_call = ChatMessage::assistant("I will inspect the second file");
    second_tool_call.tool_calls = Some(vec![json!({
        "id": "call-2",
        "function": {"name": "read", "arguments": "{\"path\":\"second.rs\"}"}
    })]);
    let mut tail = ChatMessage::user("<system_reminder>runtime state</system_reminder>");
    tail.kind = crate::core::llm::MessageKind::EphemeralTail;

    let body = anthropic_body(vec![
        ChatMessage::user("original question"),
        first_tool_call,
        ChatMessage::tool("call-1", "first result"),
        second_tool_call,
        ChatMessage::tool("call-2", "second result"),
        tail,
    ]);

    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(
        body["messages"][0]["content"][0].get("cache_control"),
        None,
        "breakpoint C is a wire-message position, not the logical user prompt"
    );
    assert_eq!(
        body["messages"][2]["content"][0]["cache_control"]["type"], "ephemeral",
        "breakpoint C must mark the previous tool-result wire message"
    );
    assert_eq!(
        body["messages"][4]["content"][0]["cache_control"]["type"], "ephemeral",
        "breakpoint D must mark the newest complete tool-result message"
    );
    assert_eq!(
        body["system"][0]["text"],
        "<system_reminder>runtime state</system_reminder>"
    );
    assert!(body["system"][0].get("cache_control").is_none());
}

#[test]
fn rolling_boundary_is_explicitly_remarked_across_tool_rounds() {
    let mut first_tail = ChatMessage::user("<system_reminder>runtime state</system_reminder>");
    first_tail.kind = crate::core::llm::MessageKind::EphemeralTail;
    let first_round = anthropic_body(vec![
        ChatMessage::user("original question"),
        assistant_tool_call("call-1", "first.rs"),
        ChatMessage::tool("call-1", "first result"),
        first_tail,
    ]);
    assert_eq!(
        first_round["messages"][2]["content"][0]["cache_control"]["type"], "ephemeral",
        "round N writes the newest stable tool-result boundary"
    );

    let mut second_tail = ChatMessage::user("<system_reminder>runtime state</system_reminder>");
    second_tail.kind = crate::core::llm::MessageKind::EphemeralTail;
    let second_round = anthropic_body(vec![
        ChatMessage::user("original question"),
        assistant_tool_call("call-1", "first.rs"),
        ChatMessage::tool("call-1", "first result"),
        assistant_tool_call("call-2", "second.rs"),
        ChatMessage::tool("call-2", "second result"),
        second_tail,
    ]);

    assert_eq!(
        second_round["messages"][2]["content"][0]["cache_control"]["type"], "ephemeral",
        "round N+1 must explicitly remark round N's tool-result boundary"
    );
    assert_eq!(
        second_round["messages"][4]["content"][0]["cache_control"]["type"], "ephemeral",
        "round N+1 also writes its new stable boundary"
    );
}

#[test]
fn persistent_wire_prefix_is_byte_identical_across_main_and_subagent_requests() {
    let persistent_messages = vec![
        ChatMessage::system("stable system"),
        ChatMessage::user("original question"),
        assistant_tool_call("call-1", "src/lib.rs"),
        ChatMessage::tool("call-1", "file contents"),
    ];
    let mut main_messages = persistent_messages.clone();
    let mut tail = ChatMessage::user("<system_reminder>runtime state</system_reminder>");
    tail.kind = crate::core::llm::MessageKind::EphemeralTail;
    main_messages.push(tail);

    let target = ProviderCompatProfile::anthropic_messages("claude-opus-4-6");
    let main_wire = build_messages(
        &main_messages,
        &target,
        true,
        &Capabilities::default(),
        None,
    );
    let repeated_main_wire = build_messages(
        &main_messages,
        &target,
        true,
        &Capabilities::default(),
        None,
    );
    let subagent_wire = build_messages(
        &persistent_messages,
        &target,
        true,
        &Capabilities::default(),
        None,
    );

    assert_eq!(
        serde_json::to_vec(&main_wire.system).unwrap(),
        serde_json::to_vec(&repeated_main_wire.system).unwrap()
    );
    assert_eq!(
        serde_json::to_vec(&main_wire.messages).unwrap(),
        serde_json::to_vec(&repeated_main_wire.messages).unwrap(),
        "without compaction, a repeated request must preserve its full wire prefix byte-for-byte"
    );

    assert_eq!(
        serde_json::to_vec(&main_wire.messages).unwrap(),
        serde_json::to_vec(&subagent_wire.messages).unwrap(),
        "runtime state is a system suffix, so main and subagent messages match exactly"
    );
    assert!(main_wire.has_ephemeral_system_tail);
    assert_eq!(&main_wire.system[..1], &subagent_wire.system);
    assert_eq!(
        main_wire
            .system
            .last()
            .and_then(|block| block["text"].as_str()),
        Some("<system_reminder>runtime state</system_reminder>")
    );
}

#[test]
fn build_request_body_serializes_inline_pdf_for_anthropic() {
    let request = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(vec![
            ChatMessageContentPart::file_base64_data("notes.pdf", "application/pdf", "cGRm")
                .expect("valid inline pdf"),
        ])],
        model: "ignored".to_string(),
        temperature: None,
        max_tokens: None,
        resolved_output_limit: None,
        diagnostic_request_id: None,
        stream: Some(false),
        model_override: None,
        speed: None,
        thinking_level: None,
        cache_key: None,
        tools: None,
    };

    let body = build_request_body(
        &request,
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        false,
        &Capabilities {
            vision: true,
            files: true,
            ..Capabilities::default()
        },
        None,
    );

    assert_eq!(body["messages"][0]["content"][0]["type"], "document");
    assert_eq!(
        body["messages"][0]["content"][0]["source"]["type"],
        "base64"
    );
    assert_eq!(
        body["messages"][0]["content"][0]["source"]["media_type"],
        "application/pdf"
    );
    assert_eq!(body["messages"][0]["content"][0]["title"], "notes.pdf");
}

#[test]
fn build_request_body_serializes_uploaded_image_and_pdf_file_ids() {
    let adapter = StaticFilesAdapter { prefix: "anth-" };
    let request = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(vec![
            ChatMessageContentPart::image_file_id("file-img").expect("valid image id"),
            ChatMessageContentPart::file_file_id("file-doc", Some("report.pdf".to_string()))
                .expect("valid file id"),
        ])],
        model: "ignored".to_string(),
        temperature: None,
        max_tokens: None,
        resolved_output_limit: None,
        diagnostic_request_id: None,
        stream: Some(false),
        model_override: None,
        speed: None,
        thinking_level: None,
        cache_key: None,
        tools: None,
    };

    let body = build_request_body(
        &request,
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        false,
        &Capabilities {
            vision: true,
            files: true,
            ..Capabilities::default()
        },
        Some(&adapter),
    );

    assert_eq!(body["messages"][0]["content"][0]["type"], "image");
    assert_eq!(body["messages"][0]["content"][0]["source"]["type"], "file");
    assert_eq!(
        body["messages"][0]["content"][0]["source"]["file_id"],
        "anth-file-img"
    );
    assert_eq!(body["messages"][0]["content"][1]["type"], "document");
    assert_eq!(body["messages"][0]["content"][1]["source"]["type"], "file");
    assert_eq!(
        body["messages"][0]["content"][1]["source"]["file_id"],
        "anth-file-doc"
    );
    assert_eq!(body["messages"][0]["content"][1]["title"], "report.pdf");
}

#[test]
fn build_request_body_degrades_when_anthropic_files_or_vision_disabled() {
    let request = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(vec![
            ChatMessageContentPart::image_file_id("file-img").expect("valid image id"),
            ChatMessageContentPart::file_file_id("file-doc", Some("report.pdf".to_string()))
                .expect("valid file id"),
        ])],
        model: "ignored".to_string(),
        temperature: None,
        max_tokens: None,
        resolved_output_limit: None,
        diagnostic_request_id: None,
        stream: Some(false),
        model_override: None,
        speed: None,
        thinking_level: None,
        cache_key: None,
        tools: None,
    };

    let body = build_request_body(
        &request,
        "claude-opus-4-6",
        &ThinkingConfig::default(),
        ThinkingFormat::AnthropicAdaptive,
        true,
        false,
        &Capabilities {
            vision: false,
            files: false,
            ..Capabilities::default()
        },
        None,
    );

    assert_eq!(body["messages"][0]["content"][0]["type"], "text");
    assert_eq!(
        body["messages"][0]["content"][0]["text"],
        "[Image omitted: the current model does not support image input]"
    );
    assert_eq!(body["messages"][0]["content"][1]["type"], "text");
    assert_eq!(
        body["messages"][0]["content"][1]["text"],
        "[File omitted: the current model does not support file input]"
    );
}

#[test]
fn final_stream_events_notices_anthropic_max_tokens() {
    let events = final_stream_events(
        &ProviderCompatProfile::anthropic_messages("claude-opus-4-6"),
        true,
        vec![],
        None,
        false,
        None,
        Some("max_tokens".to_string()),
    );
    assert!(events.iter().any(|event| {
        matches!(
            event,
            StreamEvent::LlmNotice {
                finish_reason,
                message,
            } if finish_reason == "max_tokens" && message.contains(&crate::infra::i18n::tr_in(crate::infra::i18n::Locale::En, "llm.anthropicMaxOutputNotice", &[]))
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            StreamEvent::FinishReason { reason } if reason == "max_tokens"
        )
    }));
}

#[test]
fn response_to_chat_response_preserves_thinking_and_tool_calls() {
    let profile = ProviderCompatProfile::anthropic_messages("claude-opus-4-6");
    let raw = json!({
        "id": "msg_1",
        "content": [
            {
                "type": "thinking",
                "thinking": "reason step",
                "signature": "sig_1",
            },
            {
                "type": "text",
                "text": "Need one tool call.",
            },
            {
                "type": "tool_use",
                "id": "tool_1",
                "name": "read_file",
                "input": {
                    "path": "README.md"
                }
            }
        ],
        "stop_reason": "tool_use",
        "usage": {
            "input_tokens": 11,
            "output_tokens": 22
        }
    });

    let response = response_to_chat_response(&raw, &profile, true);
    let choice = &response.choices[0];
    let message = &choice.message;

    assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
    assert_eq!(message.thinking_text.as_deref(), Some("reason step"));
    assert_eq!(
        message
            .tool_calls
            .as_ref()
            .and_then(|calls| calls.first())
            .and_then(|call| call["function"]["name"].as_str()),
        Some("read_file")
    );
    assert_eq!(
        message
            .reasoning_continuation
            .as_ref()
            .map(|continuation| continuation.format.clone()),
        Some(ReasoningFormat::AnthropicThinkingBlocks)
    );
    assert_eq!(
        response.usage.as_ref().map(|usage| usage.total_tokens),
        Some(Some(33))
    );
}
