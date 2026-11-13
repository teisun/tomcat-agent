use super::*;
use crate::core::llm::thinking_policy::ThinkingLevel;
use crate::core::llm::types::{
    ChatRequest, ContinuityMetadata, ProviderRefs, ReasoningContinuation, ReplayRequirement,
    StreamEvent,
};
use crate::core::tools::contract::catalog::build_function_definitions;

fn gemini_route_a() -> ProviderCompatProfile {
    ProviderCompatProfile::google_gemini_routed(
        "gemini-4-carbon",
        "fcodex",
        "https://hk.fcodex.top/v1beta",
        "cred_aaaa111122223333",
    )
}

fn gemini_route_b() -> ProviderCompatProfile {
    ProviderCompatProfile::google_gemini_routed(
        "gemini-4-carbon",
        "fcodex",
        "https://hk.fcodex.top/v1beta",
        "cred_bbbb444455556666",
    )
}

fn signed_assistant_calls(
    profile: &ProviderCompatProfile,
    calls: Vec<(&str, &str, Value)>,
    sigs: Vec<(&str, &str)>,
) -> ChatMessage {
    let mut parts = Vec::new();
    for (id, name, args) in &calls {
        let mut part = json!({"functionCall": {"id": id, "name": name, "args": args}});
        if let Some((_, sig)) = sigs.iter().find(|(call_id, _)| call_id == id) {
            part["thoughtSignature"] = json!(sig);
        }
        parts.push(part);
    }
    let tool_calls = calls
        .into_iter()
        .map(|(id, name, args)| {
            json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": args.to_string(),
                }
            })
        })
        .collect();
    ChatMessage::assistant_with_tool_calls(None, tool_calls).with_reasoning_state(
        Some("prior thought summary".to_string()),
        Some(ReasoningContinuation {
            source_provider: profile.provider.clone(),
            source_api: profile.api_family.clone(),
            source_model: profile.model_family.clone(),
            format: ReasoningFormat::GeminiThoughtSignatures,
            opaque_payload: Value::Array(parts),
            fallback_text: None,
            provider_refs: Some(ProviderRefs {
                openai_response_id: None,
                replay_profile_id: Some(profile.profile_id.clone()),
            }),
        }),
        Some(ContinuityMetadata {
            had_tool_call: true,
            replay_requirement: ReplayRequirement::SameProfileRequired,
        }),
    )
}

fn render(messages: Vec<ChatMessage>, target: &ProviderCompatProfile, continuity: bool) -> Value {
    let req = ChatRequest {
        model: "gemini-4-carbon".to_string(),
        messages,
        ..Default::default()
    };
    let thinking_cfg = ThinkingConfig {
        enabled: true,
        level: "high".to_string(),
        ..Default::default()
    };
    build_request_body(
        &req,
        target,
        &thinking_cfg,
        &["low".into(), "medium".into(), "high".into()],
        continuity,
        &Capabilities {
            vision: true,
            files: true,
            tools: true,
            reasoning: true,
            web_search: false,
        },
    )
}

#[test]
fn in_window_same_route_replays_real_signature_on_first_parallel_call_only() {
    let route = gemini_route_a();
    let assistant = signed_assistant_calls(
        &route,
        vec![
            ("c1", "list_dir", json!({"path": "."})),
            ("c2", "list_dir", json!({"path": "tomcat"})),
        ],
        vec![("c1", "REAL_SIG_STEP1")],
    );
    let body = render(
        vec![
            ChatMessage::user("inspect repo"),
            assistant,
            ChatMessage::tool("c1", "root entries"),
            ChatMessage::tool("c2", "crate entries"),
        ],
        &route,
        true,
    );

    let model_parts = body["contents"][1]["parts"].as_array().unwrap();
    assert_eq!(model_parts.len(), 2);
    assert_eq!(model_parts[0]["thoughtSignature"], "REAL_SIG_STEP1");
    assert!(model_parts[1].get("thoughtSignature").is_none());
}

#[test]
fn in_window_cross_route_or_unsigned_puts_sentinel_on_first_call_only() {
    let route_a = gemini_route_a();
    let route_b = gemini_route_b();
    let assistant = signed_assistant_calls(
        &route_a,
        vec![
            ("c1", "read", json!({"path": "a.rs"})),
            ("c2", "read", json!({"path": "b.rs"})),
        ],
        vec![("c1", "SIG_FROM_ROUTE_A")],
    );

    for (target, continuity) in [(&route_b, true), (&route_a, false)] {
        let body = render(
            vec![
                ChatMessage::user("read files"),
                assistant.clone(),
                ChatMessage::tool("c1", "a"),
                ChatMessage::tool("c2", "b"),
            ],
            target,
            continuity,
        );
        let parts = body["contents"][1]["parts"].as_array().unwrap();
        assert_eq!(
            parts[0]["thoughtSignature"],
            SKIP_THOUGHT_SIGNATURE_VALIDATOR
        );
        assert!(parts[1].get("thoughtSignature").is_none());
    }
}

#[test]
fn new_user_turn_keeps_old_signatures_and_unsigned_calls_get_sentinel() {
    let route = gemini_route_a();
    let old_step = signed_assistant_calls(
        &route,
        vec![("old_1", "read", json!({"path": "old.rs"}))],
        vec![("old_1", "OLD_SIG")],
    );
    let current_step = ChatMessage::assistant_with_tool_calls(
        None,
        vec![json!({
            "id": "cur_1",
            "type": "function",
            "function": {"name": "read", "arguments": "{\"path\":\"new.rs\"}"}
        })],
    );
    let body = render(
        vec![
            ChatMessage::user("first turn"),
            old_step,
            ChatMessage::tool("old_1", "old content"),
            ChatMessage::assistant("first answer"),
            ChatMessage::user("second turn"),
            current_step,
            ChatMessage::tool("cur_1", "new content"),
        ],
        &route,
        true,
    );

    let old_model_parts = body["contents"][1]["parts"].as_array().unwrap();
    assert_eq!(old_model_parts[0]["thoughtSignature"], "OLD_SIG");

    let cur_model_parts = body["contents"][5]["parts"].as_array().unwrap();
    assert_eq!(
        cur_model_parts[0]["thoughtSignature"],
        SKIP_THOUGHT_SIGNATURE_VALIDATOR
    );
}

#[test]
fn system_instruction_merges_ephemeral_tail_and_bridges_tool_before_user() {
    let route = gemini_route_a();
    let call = ChatMessage::assistant_with_tool_calls(
        Some("Checking files"),
        vec![
            json!({"id": "call_a", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"tsconfig.json\"}"}}),
            json!({"id": "call_b", "type": "function", "function": {"name": "list_dir", "arguments": "{\"path\":\".\"}"}}),
        ],
    );
    let mut ephemeral = ChatMessage::user("active_plan=none");
    ephemeral.kind = crate::core::llm::MessageKind::EphemeralTail;

    // Deliberately pass tool results in reverse order (`call_b` then `call_a`) and include `$ref` JSON.
    let body = render(
        vec![
            ChatMessage::system("Core prompt"),
            ChatMessage::user("hello"),
            ChatMessage::user("follow-up detail"),
            call,
            ChatMessage::tool("call_b", "src/\n"),
            ChatMessage::tool("call_a", r##"{"$ref":"#/$defs/CompilerOptions"}"##),
            ChatMessage::user("interrupted — switch repo"),
            ephemeral,
        ],
        &route,
        true,
    );

    let sys_parts = body["systemInstruction"]["parts"].as_array().unwrap();
    assert_eq!(sys_parts.len(), 2);
    assert_eq!(sys_parts[0]["text"], "Core prompt");
    assert_eq!(sys_parts[1]["text"], "active_plan=none");

    let contents = body["contents"].as_array().unwrap();
    let roles: Vec<&str> = contents
        .iter()
        .map(|c| c["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "model", "user", "model", "user"]);

    // Adjacent human user messages merged into contents[0]
    assert_eq!(contents[0]["parts"].as_array().unwrap().len(), 2);

    // Parallel tool responses sorted to match call order (`call_a` then `call_b`) and wrapped in `{output}`
    let resp_parts = contents[2]["parts"].as_array().unwrap();
    assert_eq!(resp_parts.len(), 2);
    assert_eq!(resp_parts[0]["functionResponse"]["id"], "call_a");
    assert_eq!(resp_parts[0]["functionResponse"]["name"], "read");
    assert_eq!(
        resp_parts[0]["functionResponse"]["response"],
        json!({"output": r##"{"$ref":"#/$defs/CompilerOptions"}"##})
    );
    assert_eq!(resp_parts[1]["functionResponse"]["id"], "call_b");
    assert_eq!(resp_parts[1]["functionResponse"]["name"], "list_dir");

    // Synthetic bridge interposed before human user turn
    assert_eq!(contents[3]["parts"][0]["text"], TOOL_TURN_BRIDGE);
    assert_eq!(contents[4]["parts"][0]["text"], "interrupted — switch repo");

    // Unsigned history uses the sentinel even after a new user turn.
    let expected = json!({
        "systemInstruction": {"parts": [{"text": "Core prompt"}, {"text": "active_plan=none"}]},
        "contents": [
            {"role": "user", "parts": [{"text": "hello"}, {"text": "follow-up detail"}]},
            {"role": "model", "parts": [
                {"text": "Checking files"},
                {"functionCall": {"id": "call_a", "name": "read", "args": {"path": "tsconfig.json"}}, "thoughtSignature": SKIP_THOUGHT_SIGNATURE_VALIDATOR},
                {"functionCall": {"id": "call_b", "name": "list_dir", "args": {"path": "."}}}
            ]},
            {"role": "user", "parts": [
                {"functionResponse": {"id": "call_a", "name": "read", "response": {"output": r##"{"$ref":"#/$defs/CompilerOptions"}"##}}},
                {"functionResponse": {"id": "call_b", "name": "list_dir", "response": {"output": "src/\n"}}}
            ]},
            {"role": "model", "parts": [{"text": TOOL_TURN_BRIDGE}]},
            {"role": "user", "parts": [{"text": "interrupted — switch repo"}]}
        ],
        "generationConfig": {"thinkingConfig": {"includeThoughts": true, "thinkingLevel": "high"}}
    });
    assert_eq!(
        serde_json::to_vec(&body).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
}

#[test]
fn missing_call_and_result_ids_are_omitted_for_positional_matching() {
    let route = gemini_route_a();
    let call = ChatMessage::assistant_with_tool_calls(
        None,
        vec![json!({
            "type": "function", "function": {"name": "read", "arguments": "{}"}
        })],
    );
    let mut result = ChatMessage::tool("", "body");
    result.name = Some("read".to_string());
    let body = render(vec![ChatMessage::user("read"), call, result], &route, true);
    assert!(body["contents"][1]["parts"][0]["functionCall"]
        .get("id")
        .is_none());
    assert_eq!(
        body["contents"][1]["parts"][0]["thoughtSignature"],
        SKIP_THOUGHT_SIGNATURE_VALIDATOR
    );
    let response = &body["contents"][2]["parts"][0]["functionResponse"];
    assert!(response.get("id").is_none());
    assert_eq!(response["name"], "read");
}

#[test]
fn user_content_before_tool_results_does_not_insert_a_model_bridge() {
    let mut turns = Vec::new();
    push_turn(
        &mut turns,
        WireTurn {
            kind: TurnKind::UserContent,
            parts: vec![json!({"text": "user content"})],
        },
    );
    push_turn(
        &mut turns,
        WireTurn {
            kind: TurnKind::ToolResponses,
            parts: vec![
                json!({"functionResponse": {"name": "read", "response": {"output": "body"}}}),
            ],
        },
    );
    assert_eq!(
        turns.len(),
        1,
        "only tool results followed by human content need a bridge"
    );
    assert_eq!(turns[0].kind.role(), "user");
    assert_eq!(turns[0].parts.len(), 2);
}

#[test]
fn native_multimodal_tool_result_embeds_inline_data_and_degrades_foreign_file_ids() {
    let route = gemini_route_a();
    let call = ChatMessage::assistant_with_tool_calls(
        None,
        vec![
            json!({"id": "img_call", "type": "function", "function": {"name": "read", "arguments": "{}"}}),
        ],
    );
    let mut tool_msg = ChatMessage::tool("img_call", "");
    tool_msg.content = Some(ChatMessageContent::Parts(vec![
        ChatMessageContentPart::text("Read PNG"),
        ChatMessageContentPart::image_base64_data("image/png", "aGVsbG8=").unwrap(),
        ChatMessageContentPart::file_file_id("file-openai-123", None).unwrap(),
    ]));

    let body = render(
        vec![ChatMessage::user("inspect image"), call, tool_msg],
        &route,
        true,
    );
    let fr = &body["contents"][2]["parts"][0]["functionResponse"];
    let output_str = fr["response"]["output"].as_str().unwrap();
    assert!(output_str.contains("Read PNG"));
    assert!(output_str.contains("[File omitted"));
    assert_eq!(
        fr["parts"],
        json!([{
            "inlineData": {
                "mimeType": "image/png",
                "data": "aGVsbG8=",
            }
        }])
    );
}

#[test]
fn schema_sanitizer_inlines_refs_preserves_cycles_and_accepts_full_builtin_catalog() {
    let with_refs = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "item": { "$ref": "#/$defs/Item", "description": "override desc" }
        },
        "$defs": {
            "Item": {
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }
        }
    });
    let cleaned = prepare_gemini_tool_parameters(&with_refs);
    assert!(cleaned.get("$schema").is_none());
    assert!(cleaned.get("$defs").is_none());
    assert_eq!(cleaned["properties"]["item"]["type"], "object");
    assert_eq!(
        cleaned["properties"]["item"]["description"],
        "override desc"
    );
    assert_eq!(
        cleaned["properties"]["item"]["properties"]["name"]["type"],
        "string"
    );

    let circular = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": { "self": { "$ref": "#/$defs/Node" } },
        "$defs": {
            "Node": {
                "type": "object",
                "properties": { "next": { "$ref": "#/$defs/Node" } }
            }
        }
    });
    let untouched = prepare_gemini_tool_parameters(&circular);
    assert!(untouched.get("$schema").is_none());
    assert!(untouched.get("$defs").is_some());

    let bare_obj = json!({ "type": "object" });
    assert_eq!(
        prepare_gemini_tool_parameters(&bare_obj),
        json!({ "type": "object", "properties": {} })
    );

    let full_tools = build_function_definitions();
    let converted = convert_tools(&full_tools).expect("non-empty declarations");
    let decls = converted[0]["functionDeclarations"].as_array().unwrap();
    assert_eq!(decls.len(), full_tools.len());
    let todos_decl = decls
        .iter()
        .find(|d| d["name"] == "todos")
        .expect("todos declaration");
    assert!(
        todos_decl["parametersJsonSchema"]["properties"]["ops"]["items"]["oneOf"].is_array(),
        "parametersJsonSchema must preserve oneOf/const/additionalProperties intact"
    );
}

#[tokio::test]
async fn signed_text_sse_survives_restart_and_new_user_turn() {
    use super::super::stream::{GeminiResponseAccumulator, GeminiStream};
    use crate::core::session::manager::{init_context_state, SessionManager};
    use crate::infra::config::ContextConfig;
    use bytes::Bytes;
    use tokio_stream::StreamExt;

    let route = gemini_route_a();
    let fixture =
        include_str!("../../../../../tests/fixtures/llm_stream/gemini_native_text_signature.sse");
    let mut accumulator = GeminiResponseAccumulator::new(route.clone(), true);
    for block in fixture.split("\n\n").filter(|s| !s.trim().is_empty()) {
        accumulator.feed_frame(
            &serde_json::from_str::<Value>(block.trim().strip_prefix("data: ").unwrap()).unwrap(),
        );
    }
    let message = accumulator
        .into_chat_response()
        .unwrap()
        .choices
        .remove(0)
        .message;
    let expected = json!([
        {"text": "reasoning", "thought": true},
        {"text": "Hello"},
        {"text": "!", "thoughtSignature": "SIGNED_TEXT"},
        {"text": "", "thoughtSignature": "FINAL_SIGNATURE"}
    ]);
    assert_eq!(message.text_content(), Some("Hello!"));
    assert_eq!(message.thinking_text.as_deref(), Some("reasoning"));
    assert_eq!(
        message
            .reasoning_continuation
            .as_ref()
            .unwrap()
            .opaque_payload,
        expected
    );

    let chunks: Vec<Result<Bytes, reqwest::Error>> = fixture
        .as_bytes()
        .chunks(17)
        .map(|s| Ok(Bytes::copy_from_slice(s)))
        .collect();
    let mut stream = GeminiStream::new(tokio_stream::iter(chunks), route.clone(), true);
    let mut continuation = None;
    while let Some(event) = stream.next().await {
        if let StreamEvent::ReasoningSnapshot {
            reasoning_continuation,
            ..
        } = event.unwrap()
        {
            continuation = reasoning_continuation;
        }
    }
    assert_eq!(continuation, message.reasoning_continuation);
    let dir = tempfile::tempdir().unwrap();
    let manager = SessionManager::new(dir.path().to_path_buf());
    manager
        .create_session(manager.current_session_key(), None)
        .unwrap();
    for msg in [ChatMessage::user("first"), message] {
        manager
            .append_message(serde_json::to_value(msg).unwrap())
            .unwrap();
    }
    let restarted = SessionManager::new(dir.path().to_path_buf());
    let mut history = init_context_state(&restarted, &ContextConfig::default(), "sys")
        .unwrap()
        .messages;
    history.push(ChatMessage::user("next question"));
    let body = render(history, &route, true);
    assert_eq!(
        serde_json::to_vec(&body["contents"][1]["parts"]).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
}

#[test]
fn replay_without_upstream_ids_keeps_internal_ids_off_wire_and_sorts_results() {
    let route = gemini_route_a();
    let mut assistant = signed_assistant_calls(
        &route,
        vec![
            ("internal1", "read", json!({"path":"a"})),
            ("internal2", "read", json!({"path":"b"})),
        ],
        vec![("internal1", "SIG")],
    );
    for part in assistant
        .reasoning_continuation
        .as_mut()
        .unwrap()
        .opaque_payload
        .as_array_mut()
        .unwrap()
    {
        part["functionCall"].as_object_mut().unwrap().remove("id");
    }
    let body = render(
        vec![
            ChatMessage::user("read"),
            assistant,
            ChatMessage::tool("internal2", "b"),
            ChatMessage::tool("internal1", "a"),
        ],
        &route,
        true,
    );
    for part in body["contents"][1]["parts"].as_array().unwrap() {
        assert!(part["functionCall"].get("id").is_none());
    }
    let results = body["contents"][2]["parts"].as_array().unwrap();
    assert_eq!(results[0]["functionResponse"]["response"]["output"], "a");
    assert_eq!(results[1]["functionResponse"]["response"]["output"], "b");
    assert!(results
        .iter()
        .all(|p| p["functionResponse"].get("id").is_none()));
}

#[test]
fn old_dictionary_is_not_read_and_first_call_gets_sentinel_even_if_later_call_is_signed() {
    let route = gemini_route_a();
    let assistant = signed_assistant_calls(
        &route,
        vec![("c1", "read", json!({})), ("c2", "read", json!({}))],
        vec![("c2", "LATER")],
    );
    let mut legacy = assistant.clone();
    legacy
        .reasoning_continuation
        .as_mut()
        .unwrap()
        .opaque_payload = json!({"c1": "LEGACY"});
    for msg in [assistant, legacy] {
        let body = render(vec![ChatMessage::user("go"), msg], &route, true);
        assert_eq!(
            body["contents"][1]["parts"][0]["thoughtSignature"],
            SKIP_THOUGHT_SIGNATURE_VALIDATOR
        );
    }
}

#[test]
fn thinking_config_clamps_to_model_supported_tiers_and_omits_budget() {
    let route = gemini_route_a();
    let req = ChatRequest {
        model: "gemini-4-pro".to_string(),
        messages: vec![ChatMessage::user("hi")],
        thinking_level: Some(ThinkingLevel::Max),
        ..Default::default()
    };
    let thinking_cfg = ThinkingConfig {
        enabled: true,
        level: "medium".to_string(),
        ..Default::default()
    };
    let body = build_request_body(
        &req,
        &route,
        &ThinkingConfig {
            enabled: true,
            level: req.thinking_level.unwrap().as_str().to_string(),
            ..thinking_cfg.clone()
        },
        &["low".into(), "high".into()],
        true,
        &Capabilities {
            reasoning: true,
            ..Default::default()
        },
    );
    assert_eq!(
        body["generationConfig"]["thinkingConfig"],
        json!({ "includeThoughts": true, "thinkingLevel": "high" })
    );
    assert!(body["generationConfig"]["thinkingConfig"]
        .get("thinkingBudget")
        .is_none());

    // Medium on a [low, high]-only model clamps down to low.
    let clamped_body = build_request_body(
        &ChatRequest {
            model: "gemini-4-pro".to_string(),
            messages: vec![ChatMessage::user("hi")],
            ..Default::default()
        },
        &route,
        &thinking_cfg,
        &["low".into(), "high".into()],
        true,
        &Capabilities {
            reasoning: true,
            ..Default::default()
        },
    );
    assert_eq!(
        clamped_body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "low"
    );
}
