use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::super::current_tail_guard::{self, GuardRoute, GuardRouteReason};
use super::super::{AgentLoop, AgentLoopConfig};
use super::mocks::{test_binding, MockPrimitiveExecutor};
use crate::core::compaction::preheat::Preheat;
use crate::core::compaction::TOOL_RESULT_PLACEHOLDER;
use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider, MessageKind,
    StreamEvent,
};
use crate::core::session::manager::{estimate_msg_chars, ApiUsage, CompactionResult, ContextState};
use crate::infra::config::ContextConfig;
use crate::infra::error::AppError;
use crate::infra::event_bus::DefaultEventBus;

struct ChatOnlyMockLlm {
    summary_text: String,
}

#[async_trait]
impl LlmProvider for ChatOnlyMockLlm {
    fn provider_name(&self) -> &str {
        "chat_only_mock"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Ok(ChatResponse {
            id: None,
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant(&self.summary_text),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        Ok(Box::new(tokio_stream::iter(Vec::<
            Result<StreamEvent, AppError>,
        >::new())))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

#[test]
fn build_precheck_decision_covers_fit_reduce_and_collapse_routes() {
    let mut agent = make_agent(ContextConfig {
        current_tail_compactable_min_chars: 1,
        ..Default::default()
    });
    let fit = current_tail_guard::build_precheck_decision(&agent, &[], 900, 1_000);
    assert_eq!(fit.route, GuardRoute::Fits);
    assert_eq!(fit.route_reason, GuardRouteReason::Fits);
    assert!(fit.yellow_lamp_only);

    let mut reduce_state = context_state(8_000, 1_000);
    reduce_state
        .preheat
        .restore_completed(dummy_compaction_result("old", "new"));
    agent.set_context_state(Some(reduce_state));
    let reduce = current_tail_guard::build_precheck_decision(&agent, &[], 2_000, 1_000);
    assert_eq!(reduce.route, GuardRoute::Reduce);
    assert_eq!(reduce.route_reason, GuardRouteReason::PreheatShortcut);

    let collapse_messages = vec![
        ChatMessage::user("u"),
        assistant_with_tool_calls(&[("tc1", "read")]),
        ChatMessage::tool("tc1", &"x".repeat(200)),
    ];
    agent.start_idx = 1;
    agent.set_context_state(Some(context_state(8_000, 1_000)));
    let collapse =
        current_tail_guard::build_precheck_decision(&agent, &collapse_messages, 3_000, 1_000);
    assert_eq!(collapse.route, GuardRoute::Collapse);
    assert_eq!(collapse.route_reason, GuardRouteReason::NotEnoughReducible);
    assert_eq!(collapse.candidate_count, 1);
    assert!(collapse.max_reducible < collapse.overflow_tokens + 256);
}

#[test]
fn build_precheck_decision_respects_compactable_min_chars_override() {
    let mut agent = make_agent(ContextConfig {
        current_tail_compactable_min_chars: 5_000,
        ..Default::default()
    });
    let messages = vec![
        ChatMessage::user("u"),
        assistant_with_tool_calls(&[("tc1", "read")]),
        ChatMessage::tool("tc1", &"x".repeat(4_000)),
    ];
    agent.start_idx = 1;
    agent.set_context_state(Some(context_state(8_000, 1_000)));

    let decision = current_tail_guard::build_precheck_decision(&agent, &messages, 3_000, 1_000);
    assert_eq!(decision.route, GuardRoute::Collapse);
    assert_eq!(decision.candidate_count, 0);
    assert_eq!(decision.max_reducible, 0);
}

#[test]
fn build_precheck_decision_collapses_when_history_only_room_is_not_enough() {
    let mut agent = make_agent(ContextConfig {
        keep_recent_turns: 0,
        layer0_placeholder_threshold_chars: 1,
        current_tail_compactable_min_chars: 1,
        ..Default::default()
    });
    let messages = vec![
        ChatMessage::user("old turn"),
        ChatMessage::tool("old_tc", &"h".repeat(4_000)),
        ChatMessage::user("current turn"),
        assistant_with_tool_calls(&[("tc1", "write")]),
        ChatMessage::tool("tc1", &"w".repeat(8_000)),
    ];
    agent.start_idx = 2;
    agent.set_context_state(Some(ContextState {
        messages: vec![
            ChatMessage::user("old turn"),
            ChatMessage::tool("old_tc", &"h".repeat(4_000)),
        ],
        estimate_context_chars: 16_000,
        context_budget_chars: 8_000,
        context_budget_tokens: 1_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let decision = current_tail_guard::build_precheck_decision(&agent, &messages, 4_000, 1_000);
    assert_eq!(decision.route, GuardRoute::Collapse);
    assert_eq!(decision.candidate_count, 0);
    assert!(
        decision.max_reducible > 0,
        "history should still offer some room"
    );
    assert!(decision.max_reducible < decision.overflow_tokens + 256);
}

#[tokio::test]
async fn mid_turn_guard_fits_is_noop() {
    let mut agent = make_agent(ContextConfig {
        current_tail_compactable_min_chars: 1,
        ..Default::default()
    });
    let user = ChatMessage::user("read one file");
    let assistant = assistant_with_tool_calls(&[("tc1", "read")]);
    let tool = ChatMessage::tool("tc1", &"x".repeat(2_000));
    let mut messages = vec![user, assistant, tool];
    let original = snapshot_messages(&messages);
    let tail_chars: usize = messages.iter().map(estimate_msg_chars).sum();

    agent.start_idx = 0;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
        context_budget_chars: 40_000,
        context_budget_tokens: 10_000,
        last_api_usage: Some(ApiUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
        }),
        post_usage_appended_chars: tail_chars,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    assert_eq!(
        snapshot_messages(&messages),
        original,
        "fits route should not rewrite messages"
    );
    let state = agent.context_state.as_ref().unwrap();
    assert_eq!(state.session_obs.compaction_count, 0);
}

#[tokio::test]
async fn over_budget_without_preheat_still_collapses() {
    let mut agent = make_agent(ContextConfig {
        current_tail_compactable_min_chars: 1,
        ..Default::default()
    });
    let user = ChatMessage::user("apply a write");
    let assistant = assistant_with_tool_calls(&[("tc1", "write")]);
    let tool = ChatMessage::tool("tc1", &"x".repeat(8_000));
    let mut messages = vec![user, assistant, tool];
    let total_chars: usize = messages.iter().map(estimate_msg_chars).sum();

    agent.start_idx = 0;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: total_chars,
        context_budget_chars: 4_000,
        context_budget_tokens: 1_000,
        last_api_usage: None,
        post_usage_appended_chars: total_chars,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let decision = current_tail_guard::maybe_reduce_before_next_llm_capture_decision(
        &mut agent,
        &mut messages,
    )
    .await
    .unwrap()
    .expect("guard should emit a decision");

    assert_eq!(decision.route, GuardRoute::Collapse);
    assert_eq!(decision.route_reason, GuardRouteReason::NotEnoughReducible);
    assert!(decision.after_collapse.is_some());
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, MessageKind::CompactionSummary);
}

#[tokio::test]
async fn mid_turn_guard_stops_after_history_compaction_without_touching_tail() {
    let mut agent = make_agent(ContextConfig {
        keep_recent_turns: 1,
        layer0_placeholder_threshold_chars: 1,
        current_tail_compactable_min_chars: 1,
        ..Default::default()
    });
    let old_user = ChatMessage::user("old turn");
    let old_tool = ChatMessage::tool("old_tc", &"h".repeat(16_000));
    let recent_user = ChatMessage::user("current turn");
    let assistant = assistant_with_tool_calls(&[("tc1", "read"), ("tc2", "read")]);
    let current_a = ChatMessage::tool("tc1", &"a".repeat(2_000));
    let current_b = ChatMessage::tool("tc2", &"b".repeat(2_000));
    let mut messages = vec![
        old_user.clone(),
        old_tool.clone(),
        recent_user.clone(),
        assistant,
        current_a.clone(),
        current_b.clone(),
    ];
    let total_chars: usize = messages.iter().map(estimate_msg_chars).sum();

    agent.start_idx = 3;
    agent.set_context_state(Some(ContextState {
        messages: vec![old_user, old_tool, recent_user],
        estimate_context_chars: total_chars,
        context_budget_chars: 24_000,
        context_budget_tokens: 1_500,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let decision = current_tail_guard::maybe_reduce_before_next_llm_capture_decision(
        &mut agent,
        &mut messages,
    )
    .await
    .unwrap()
    .expect("guard should emit a decision");

    let tool_texts: Vec<_> = messages
        .iter()
        .filter(|msg| msg.role == crate::core::llm::ChatMessageRole::Tool)
        .map(|msg| msg.text_content().unwrap_or("").to_string())
        .collect();
    assert_eq!(tool_texts[0], TOOL_RESULT_PLACEHOLDER);
    assert_eq!(tool_texts[1], current_a.text_content().unwrap_or(""));
    assert_eq!(tool_texts[2], current_b.text_content().unwrap_or(""));
    assert_eq!(decision.route, GuardRoute::Reduce);
    assert!(decision.after_each_wave.is_empty());
    assert_eq!(decision.after_collapse, None);
}

#[tokio::test]
async fn mid_turn_guard_boundary_runs_layer0_before_rebuilding_messages() {
    let mut agent = make_agent(ContextConfig {
        keep_recent_turns: 1,
        layer0_single_result_max_chars: 1_000,
        layer0_placeholder_threshold_chars: 100,
        current_tail_compactable_min_chars: 1,
        ..Default::default()
    });
    let mut covered_start = ChatMessage::user("covered start");
    covered_start.msg_id = Some("covered-start".to_string());
    let mut covered_end = ChatMessage::user("covered end");
    covered_end.msg_id = Some("covered-end".to_string());
    let mut old_turn = ChatMessage::user("old tool turn");
    old_turn.msg_id = Some("old-tool-turn".to_string());
    let mut latest_turn = ChatMessage::user("latest tool turn");
    latest_turn.msg_id = Some("latest-tool-turn".to_string());
    let old_tool_content = "o".repeat(500);
    let latest_tool_content = "n".repeat(2_000);
    let old_tool = ChatMessage::tool("old-tool-call", &old_tool_content);
    let latest_tool = ChatMessage::tool("latest-tool-call", &latest_tool_content);
    let state_messages = vec![
        covered_start,
        covered_end,
        old_turn,
        old_tool,
        latest_turn,
        latest_tool,
    ];
    let total_chars: usize = state_messages.iter().map(estimate_msg_chars).sum();
    let mut state = ContextState {
        messages: Vec::new(),
        estimate_context_chars: total_chars,
        context_budget_chars: 800,
        context_budget_tokens: 200,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    state.preheat.restore_completed(CompactionResult {
        summary_text: "summary".to_string(),
        covered_start_id: "covered-start".to_string(),
        covered_end_id: "covered-end".to_string(),
        covered_count: 2,
        transcript_compaction_entry_id: Some("cmp-boundary".to_string()),
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    });
    let mut messages = state_messages;
    agent.start_idx = messages.len();
    agent.set_context_state(Some(state));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    assert_eq!(
        messages[0].kind,
        MessageKind::CompactionSummary,
        "the working list must observe the applied boundary"
    );
    assert!(
        messages
            .iter()
            .any(|message| { message.text_content() == Some(TOOL_RESULT_PLACEHOLDER) }),
        "the working list must observe L0's placeholder, proving L0 ran during apply"
    );
    assert!(
        messages.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|text| text.starts_with("[Tool result persisted:"))
        }),
        "the mid-turn path must also receive L0-A persistence"
    );
    let after_first_reduction = snapshot_messages(&messages);
    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();
    assert_eq!(
        snapshot_messages(&messages),
        after_first_reduction,
        "the follow-up compact_tool_results safety pass must be idempotent after boundary-owned L0"
    );
}

#[tokio::test]
async fn mid_turn_guard_runs_first_tail_wave_before_recheck() {
    let mut agent = make_agent(ContextConfig {
        current_tail_compactable_min_chars: 1,
        current_tail_single_result_max_chars: 10_000,
        ..Default::default()
    });
    let user = ChatMessage::user("read everything");
    let assistant = assistant_with_tool_calls(&[
        ("tc1", "read"),
        ("tc2", "read"),
        ("tc3", "read"),
        ("tc4", "read"),
        ("tc5", "read"),
    ]);
    let mut messages = vec![
        user,
        assistant,
        ChatMessage::tool("tc1", &"x".repeat(12_000)),
        ChatMessage::tool("tc2", &"y".repeat(3_000)),
        ChatMessage::tool("tc3", &"z".repeat(3_000)),
        ChatMessage::tool("tc4", &"p".repeat(3_000)),
        ChatMessage::tool("tc5", &"q".repeat(3_000)),
    ];
    let tail_chars: usize = messages.iter().map(estimate_msg_chars).sum();

    agent.start_idx = 0;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
        context_budget_chars: 20_000,
        context_budget_tokens: 5_000,
        last_api_usage: Some(ApiUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
        }),
        post_usage_appended_chars: tail_chars,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let decision = current_tail_guard::maybe_reduce_before_next_llm_capture_decision(
        &mut agent,
        &mut messages,
    )
    .await
    .unwrap()
    .expect("guard should emit a decision");

    let tool_texts: Vec<_> = messages
        .iter()
        .filter(|msg| msg.role == crate::core::llm::ChatMessageRole::Tool)
        .map(|msg| msg.text_content().unwrap_or("").to_string())
        .collect();
    assert!(
        tool_texts
            .iter()
            .any(|text| text.starts_with("[Tool result persisted:")),
        "step 0 should still persist the large result"
    );
    assert!(
        tool_texts
            .iter()
            .any(|text| text == TOOL_RESULT_PLACEHOLDER),
        "first placeholder wave should still happen before the first recheck"
    );
    assert_eq!(decision.route, GuardRoute::Reduce);
    assert_eq!(decision.after_each_wave.len(), 1);
    assert!(decision.after_each_wave[0] < decision.working_tokens);
    assert_eq!(decision.after_collapse, None);
}

#[tokio::test]
async fn tail_placeholder_waves_still_run_after_a_preheat_boundary() {
    let mut agent = make_agent(ContextConfig {
        current_tail_compactable_min_chars: 1,
        current_tail_single_result_max_chars: 20_000,
        ..Default::default()
    });
    let mut historical = ChatMessage::user("old context");
    historical.msg_id = Some("historical".to_string());
    let tail_user = ChatMessage::user("read the active files");
    let tail_assistant = assistant_with_tool_calls(&[
        ("tc1", "read"),
        ("tc2", "read"),
        ("tc3", "read"),
        ("tc4", "read"),
        ("tc5", "read"),
    ]);
    let mut messages = vec![historical.clone(), tail_user];
    messages.push(tail_assistant);
    for index in 1..=5 {
        messages.push(ChatMessage::tool(&format!("tc{index}"), &"x".repeat(3_000)));
    }
    let total_chars: usize = messages.iter().map(estimate_msg_chars).sum();
    let mut preheat = Preheat::new();
    preheat.restore_completed(CompactionResult {
        summary_text: "summary".to_string(),
        covered_start_id: "historical".to_string(),
        covered_end_id: "historical".to_string(),
        covered_count: 1,
        transcript_compaction_entry_id: Some("cmp-historical".to_string()),
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    });
    agent.start_idx = 1;
    agent.set_context_state(Some(ContextState {
        messages: vec![historical],
        estimate_context_chars: total_chars,
        context_budget_chars: 12_000,
        context_budget_tokens: 3_000,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat,
        session_obs: Default::default(),
        live: Default::default(),
    }));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    assert_eq!(messages[0].kind, MessageKind::CompactionSummary);
    assert_eq!(
        messages[agent.start_idx].text_content(),
        Some("read the active files"),
        "the surviving active turn remains the tail after the history boundary"
    );
    assert!(
        messages[agent.start_idx..]
            .iter()
            .any(|message| message.text_content() == Some(TOOL_RESULT_PLACEHOLDER)),
        "when the boundary alone is insufficient, normal tail placeholder waves still run"
    );
}

#[tokio::test]
async fn mid_turn_guard_runs_second_tail_wave_when_first_is_not_enough() {
    let mut agent = make_agent(ContextConfig {
        current_tail_compactable_min_chars: 1,
        current_tail_single_result_max_chars: 20_000,
        ..Default::default()
    });
    let user = ChatMessage::user("read more");
    let assistant = assistant_with_tool_calls(&[
        ("tc1", "read"),
        ("tc2", "read"),
        ("tc3", "read"),
        ("tc4", "read"),
        ("tc5", "read"),
        ("tc6", "read"),
        ("tc7", "read"),
        ("tc8", "read"),
    ]);
    let mut messages = vec![user, assistant];
    for (idx, ch) in ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h']
        .into_iter()
        .enumerate()
    {
        let tool_id = format!("tc{}", idx + 1);
        let tool_text = ch.to_string().repeat(4_000);
        messages.push(ChatMessage::tool(&tool_id, &tool_text));
    }
    let tail_chars: usize = messages.iter().map(estimate_msg_chars).sum();

    agent.start_idx = 0;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
        context_budget_chars: 14_000,
        context_budget_tokens: 2_800,
        last_api_usage: Some(ApiUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
        }),
        post_usage_appended_chars: tail_chars,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let decision = current_tail_guard::maybe_reduce_before_next_llm_capture_decision(
        &mut agent,
        &mut messages,
    )
    .await
    .unwrap()
    .expect("guard should emit a decision");

    let placeholder_count = messages
        .iter()
        .filter(|msg| msg.text_content() == Some(TOOL_RESULT_PLACEHOLDER))
        .count();
    assert_eq!(
        placeholder_count, 6,
        "two waves should rewrite 6 of 8 candidates"
    );
    assert_eq!(decision.after_each_wave.len(), 2);
    assert!(decision.after_each_wave[1] < decision.after_each_wave[0]);
    assert_eq!(decision.after_collapse, None);
}

#[tokio::test]
async fn current_tail_guard_catches_a_single_turn_that_jumps_from_low_watermark() {
    let mut agent = make_agent(ContextConfig {
        current_tail_compactable_min_chars: 1,
        current_tail_single_result_max_chars: 20_000,
        ..Default::default()
    });
    let user = ChatMessage::user("read the three generated artifacts");
    let assistant = assistant_with_tool_calls(&[("tc1", "read"), ("tc2", "read"), ("tc3", "read")]);
    let mut messages = vec![
        user,
        assistant,
        ChatMessage::tool("tc1", &"a".repeat(50_000)),
        ChatMessage::tool("tc2", &"b".repeat(50_000)),
        ChatMessage::tool("tc3", &"c".repeat(50_000)),
    ];
    let initial_chars = 128_000usize;
    let context_budget_chars = 320_000usize;
    let appended_tool_chars: usize = messages.iter().skip(2).map(estimate_msg_chars).sum();
    assert!(
        initial_chars as f64 / (context_budget_chars as f64) < 0.50,
        "fixture must begin below the Layer 0 pressure gate"
    );
    assert!(
        (initial_chars + appended_tool_chars) as f64 / (context_budget_chars as f64) > 0.85,
        "three large tool results must cross the high-water mark in one turn"
    );

    agent.start_idx = 0;
    let mut state = ContextState {
        messages: vec![],
        estimate_context_chars: initial_chars,
        context_budget_chars,
        context_budget_tokens: 60_000,
        last_api_usage: Some(ApiUsage {
            prompt_tokens: 32_000,
            completion_tokens: 0,
        }),
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    };
    for message in messages.iter().skip(2) {
        state.on_message_appended(estimate_msg_chars(message));
    }
    agent.set_context_state(Some(state));

    let decision = current_tail_guard::maybe_reduce_before_next_llm_capture_decision(
        &mut agent,
        &mut messages,
    )
    .await
    .unwrap()
    .expect("the overflow guard must make a reduction decision");

    assert_eq!(decision.route, GuardRoute::Reduce);
    assert!(
        messages
            .iter()
            .filter(|message| message.role == crate::core::llm::ChatMessageRole::Tool)
            .all(|message| {
                message
                    .text_content()
                    .is_some_and(|text| text.starts_with("[Tool result persisted:"))
            }),
        "the current-tail guard must retain a bounded disk reference instead of sending the 150K payload onward"
    );
    assert_eq!(decision.after_collapse, None);
}

#[tokio::test]
async fn mid_turn_guard_rewrites_oldest_whitelisted_tools_and_preserves_noncompactable_raw() {
    let mut agent = make_agent(ContextConfig {
        current_tail_compactable_min_chars: 1,
        current_tail_single_result_max_chars: 20_000,
        ..Default::default()
    });
    let user = ChatMessage::user("inspect mixed tools");
    let assistant = assistant_with_tool_calls(&[
        ("tc1", "search_files"),
        ("tc2", "bash"),
        ("tc3", "task_output"),
        ("tc4", "read"),
        ("tc5", "read"),
        ("tc6", "read"),
        ("tc7", "write"),
    ]);
    let mut messages = vec![
        user,
        assistant,
        ChatMessage::tool("tc1", &"a".repeat(4_000)),
        ChatMessage::tool("tc2", &"b".repeat(4_000)),
        ChatMessage::tool("tc3", &"c".repeat(4_000)),
        ChatMessage::tool("tc4", &"d".repeat(4_000)),
        ChatMessage::tool("tc5", &"e".repeat(4_000)),
        ChatMessage::tool("tc6", &"f".repeat(4_000)),
        ChatMessage::tool("tc7", &"g".repeat(4_000)),
    ];
    let tail_chars: usize = messages.iter().map(estimate_msg_chars).sum();

    agent.start_idx = 0;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
        context_budget_chars: 20_000,
        context_budget_tokens: 4_500,
        last_api_usage: Some(ApiUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
        }),
        post_usage_appended_chars: tail_chars,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let decision = current_tail_guard::maybe_reduce_before_next_llm_capture_decision(
        &mut agent,
        &mut messages,
    )
    .await
    .unwrap()
    .expect("guard should emit a decision");

    let tool_texts: Vec<_> = messages
        .iter()
        .filter(|msg| msg.role == crate::core::llm::ChatMessageRole::Tool)
        .map(|msg| msg.text_content().unwrap_or("").to_string())
        .collect();
    assert_eq!(decision.candidate_count, 6);
    assert_eq!(tool_texts[0], TOOL_RESULT_PLACEHOLDER);
    assert_eq!(tool_texts[1], TOOL_RESULT_PLACEHOLDER);
    assert_eq!(tool_texts[2], TOOL_RESULT_PLACEHOLDER);
    assert_eq!(tool_texts[3], "d".repeat(4_000));
    assert_eq!(tool_texts[4], "e".repeat(4_000));
    assert_eq!(tool_texts[5], "f".repeat(4_000));
    assert_eq!(
        tool_texts[6],
        "g".repeat(4_000),
        "non-compactable tool output must stay raw"
    );
}

#[tokio::test]
async fn mid_turn_guard_respects_single_result_threshold_override() {
    let mut agent = make_agent(ContextConfig {
        current_tail_compactable_min_chars: 1,
        current_tail_single_result_max_chars: 20_000,
        ..Default::default()
    });
    let user = ChatMessage::user("read everything");
    let assistant = assistant_with_tool_calls(&[
        ("tc1", "read"),
        ("tc2", "read"),
        ("tc3", "read"),
        ("tc4", "read"),
        ("tc5", "read"),
    ]);
    let mut messages = vec![
        user,
        assistant,
        ChatMessage::tool("tc1", &"x".repeat(12_000)),
        ChatMessage::tool("tc2", &"y".repeat(3_000)),
        ChatMessage::tool("tc3", &"z".repeat(3_000)),
        ChatMessage::tool("tc4", &"p".repeat(3_000)),
        ChatMessage::tool("tc5", &"q".repeat(3_000)),
    ];
    let tail_chars: usize = messages.iter().map(estimate_msg_chars).sum();

    agent.start_idx = 0;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
        context_budget_chars: 20_000,
        context_budget_tokens: 5_000,
        last_api_usage: Some(ApiUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
        }),
        post_usage_appended_chars: tail_chars,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    let tool_texts: Vec<_> = messages
        .iter()
        .filter(|msg| msg.role == crate::core::llm::ChatMessageRole::Tool)
        .map(|msg| msg.text_content().unwrap_or("").to_string())
        .collect();
    assert!(
        tool_texts
            .iter()
            .all(|text| !text.starts_with("[Tool result persisted:")),
        "raised threshold should disable step0 persistence"
    );
    assert!(
        tool_texts
            .iter()
            .any(|text| text == TOOL_RESULT_PLACEHOLDER),
        "tail reduction should still happen even when step0 persistence is disabled"
    );
}

#[tokio::test]
async fn mid_turn_guard_collapses_when_two_candidates_remain() {
    let mut agent = make_agent(ContextConfig {
        current_tail_compactable_min_chars: 1,
        current_tail_single_result_max_chars: 20_000,
        ..Default::default()
    });
    let user = ChatMessage::user("read four files");
    let assistant = assistant_with_tool_calls(&[
        ("tc1", "read"),
        ("tc2", "read"),
        ("tc3", "read"),
        ("tc4", "read"),
    ]);
    let mut messages = vec![
        user,
        assistant,
        ChatMessage::tool("tc1", &"a".repeat(8_000)),
        ChatMessage::tool("tc2", &"b".repeat(8_000)),
        ChatMessage::tool("tc3", &"c".repeat(8_000)),
        ChatMessage::tool("tc4", &"d".repeat(8_000)),
    ];
    let tail_chars: usize = messages.iter().map(estimate_msg_chars).sum();

    agent.start_idx = 0;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
        context_budget_chars: 6_000,
        context_budget_tokens: 1_500,
        last_api_usage: Some(ApiUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
        }),
        post_usage_appended_chars: tail_chars,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let decision = current_tail_guard::maybe_reduce_before_next_llm_capture_decision(
        &mut agent,
        &mut messages,
    )
    .await
    .unwrap()
    .expect("guard should emit a decision");

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, MessageKind::CompactionSummary);
    assert!(decision.after_collapse.is_some());
}

#[tokio::test]
async fn collapse_post_weigh_keeps_going_even_if_summary_is_still_over_budget() {
    let mut agent = make_agent_with_summary(ContextConfig::default(), "s".repeat(6_000));
    let user = ChatMessage::user("u".repeat(4_000));
    let assistant = ChatMessage::assistant("a".repeat(4_000));
    let mut messages = vec![user, assistant];
    let tail_chars: usize = messages.iter().map(estimate_msg_chars).sum();

    agent.start_idx = 0;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
        context_budget_chars: 200,
        context_budget_tokens: 50,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    let decision = current_tail_guard::maybe_reduce_before_next_llm_capture_decision(
        &mut agent,
        &mut messages,
    )
    .await
    .unwrap()
    .expect("guard should emit a decision");

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, MessageKind::CompactionSummary);
    assert!(
        agent.context_state.as_ref().unwrap().is_over_budget(),
        "test fixture expects collapse summary to remain overweight"
    );
    assert!(
        decision
            .after_collapse
            .expect("collapse should record post-weigh")
            > decision.budget_tokens
    );
}

#[tokio::test]
async fn collapse_handles_missing_msg_ids_without_sink() {
    let mut agent = make_agent(ContextConfig::default());
    let user = ChatMessage::user("u".repeat(4_000));
    let assistant = ChatMessage::assistant("a".repeat(4_000));
    let mut messages = vec![user, assistant];
    let tail_chars: usize = messages.iter().map(estimate_msg_chars).sum();

    agent.start_idx = 0;
    agent.set_context_state(Some(ContextState {
        messages: vec![],
        estimate_context_chars: tail_chars,
        context_budget_chars: 200,
        context_budget_tokens: 50,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }));

    current_tail_guard::maybe_reduce_before_next_llm(&mut agent, &mut messages)
        .await
        .unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, MessageKind::CompactionSummary);
    assert!(
        messages[0].msg_id.is_some(),
        "summary should still get a stable anchor id"
    );
}

fn make_agent(context_config: ContextConfig) -> AgentLoop {
    make_agent_with_summary(context_config, "summary".to_string())
}

fn make_agent_with_summary(context_config: ContextConfig, summary_text: String) -> AgentLoop {
    let config = AgentLoopConfig {
        session_id: "sess-guard-behavior".to_string(),
        agent_trail_dir: std::env::temp_dir().to_string_lossy().to_string(),
        context_config,
        ..Default::default()
    };
    AgentLoop::new(
        test_binding(Arc::new(ChatOnlyMockLlm { summary_text }), "gpt-4"),
        Arc::new(MockPrimitiveExecutor),
        Arc::new(DefaultEventBus::new()),
        config,
        CancellationToken::new(),
    )
}

fn context_state(estimate_context_chars: usize, context_budget_tokens: usize) -> ContextState {
    ContextState {
        messages: vec![],
        estimate_context_chars,
        context_budget_chars: estimate_context_chars.saturating_mul(4),
        context_budget_tokens,
        last_api_usage: None,
        post_usage_appended_chars: 0,
        transcript_path: PathBuf::new(),
        latest_plan_event: None,
        resume_control: Default::default(),
        preheat: Preheat::new(),
        session_obs: Default::default(),
        live: Default::default(),
    }
}

fn assistant_with_tool_calls(calls: &[(&str, &str)]) -> ChatMessage {
    let tool_calls: Vec<_> = calls
        .iter()
        .map(|(id, name)| {
            serde_json::json!({
                "id": id,
                "type": "function",
                "function": {"name": name, "arguments": "{}"},
            })
        })
        .collect();
    ChatMessage::assistant_with_tool_calls(Some("tools"), tool_calls)
}

fn dummy_compaction_result(start_id: &str, end_id: &str) -> CompactionResult {
    CompactionResult {
        summary_text: "summary".to_string(),
        covered_start_id: start_id.to_string(),
        covered_end_id: end_id.to_string(),
        covered_count: 1,
        transcript_compaction_entry_id: Some("cmp".to_string()),
        estimated_covered_tokens_before: None,
        estimated_summary_tokens: None,
        estimated_tokens_saved: None,
        preheat_elapsed_ms: 0,
    }
}

fn snapshot_messages(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(|msg| serde_json::to_value(msg).expect("chat message should serialize"))
        .collect()
}
