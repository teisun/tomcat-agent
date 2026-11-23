use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serial_test::serial;

use super::super::cmd_compact::{compact_session, run};
use super::super::parse::ChatCommandOutcome;
use crate::api::chat::ChatContext;
use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider, LlmResolver, LlmScene,
    ResolvedCall,
};
use crate::core::session::user_message_sidecar::user_message_sidecar_path;
use crate::core::session::TranscriptEntry;

use crate::{init_context_state, AppConfig, AppError};

/// `/compact` should collect a summary, not start a streamed main turn.
#[derive(Default)]
struct CompactOnlyProvider {
    requests: Mutex<Vec<ChatRequest>>,
}

#[async_trait]
impl LlmProvider for CompactOnlyProvider {
    fn provider_name(&self) -> &str {
        "compact-only"
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AppError> {
        self.requests.lock().unwrap().push(request);
        Ok(ChatResponse {
            id: Some("compact-cli-summary".to_string()),
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant(
                    "CLI compacted summary.\n\n## Progress\nMODEL CLAIM: everything completed",
                ),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        _request: ChatRequest,
    ) -> Result<
        Box<
            dyn tokio_stream::Stream<Item = Result<crate::core::llm::StreamEvent, AppError>>
                + Send
                + Unpin,
        >,
        AppError,
    > {
        Err(AppError::Llm(
            "the /compact CLI command must not start a streamed main turn".to_string(),
        ))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

struct FixedResolver {
    catalog: Arc<crate::core::llm::ModelCatalog>,
    provider: Arc<dyn LlmProvider>,
    compaction_input_budget: Option<usize>,
}

impl LlmResolver for FixedResolver {
    fn resolve(
        &self,
        scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        let model = self
            .catalog
            .lookup_explicit(session_override.unwrap_or("gpt-5.6"))?;
        let mut call = ResolvedCall::from_parts_unchecked(
            Arc::clone(&self.provider),
            model.id.clone(),
            model.request_model_name().to_string(),
        );
        if scene == LlmScene::Compaction {
            if let Some(budget) = self.compaction_input_budget {
                call.limits.input_budget_tokens = budget;
            }
        }
        Ok(call)
    }
}

fn install_compactor(ctx: &mut ChatContext) {
    ctx.global_services.llm_resolver = Arc::new(FixedResolver {
        catalog: ctx.global_services.model_catalog.snapshot(),
        provider: Arc::new(CompactOnlyProvider::default()),
        compaction_input_budget: None,
    });
}

fn latest_summary(ctx: &ChatContext) -> String {
    ctx.session_runtime
        .session
        .get_entries(100)
        .unwrap()
        .into_iter()
        .rev()
        .find_map(|entry| match entry {
            TranscriptEntry::BranchSummary(summary) if summary.is_boundary == Some(true) => {
                summary.summary
            }
            _ => None,
        })
        .expect("durable summary")
}

#[tokio::test]
#[serial]
async fn compact_restores_todos_before_first_turn_survives_recompaction_and_switches_to_plan() {
    use crate::core::plan_runtime::file_store::{self, PlanFile};
    use crate::core::tools::plan_tool::todos;
    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = super::test_support::Fixture::new();
    let ctx = fixture.ctx();
    let session_id = ctx
        .session_runtime
        .session
        .current_session_id()
        .unwrap()
        .unwrap();
    let body = "# 工作笔记\n\n```text\n---\n```\n  原样保留  \n";
    let proof = "long-evidence-".repeat(2000);
    let initial = todos::execute(
        &ctx.session_runtime.todos_runtime,
        todos::TodosArgs::from_json(
            &serde_json::json!({"title":"cold restore", "draft":body, "ops":[
                {"kind":"upsert", "id":"saved", "content":"unfinished work"},
                {"kind":"upsert", "id":"done", "content":"done work"},
                {"kind":"set_status", "id":"done", "status":"completed", "evidence":[proof]}
            ]}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        initial.to_string().len() > 10_000,
        "exercise Layer1 oversized result replacement"
    );
    let saved = ctx.session_runtime.todos_runtime.snapshot().unwrap();
    let path = ctx.session_runtime.todos_runtime.path().to_path_buf();
    ctx.session_runtime
        .session
        .append_message(serde_json::json!({"role":"user", "content":"continue this work"}))
        .unwrap();
    ctx.session_runtime.session.append_message(serde_json::json!({"role":"assistant", "tool_calls":[{"id":"seed", "type":"function", "function":{"name":"todos", "arguments":"{}"}}]})).unwrap();
    ctx.session_runtime.session.append_message(serde_json::json!({"role":"tool", "tool_call_id":"seed", "content":initial.to_string()})).unwrap();
    drop(ctx);

    // A real new ChatContext opens the v2 file; no turn or old TodosRuntime is reused.
    let mut cold = fixture.ctx();
    cold.config.context.keep_recent_turns = 0;
    let provider = Arc::new(CompactOnlyProvider::default());
    cold.global_services.llm_resolver = Arc::new(FixedResolver {
        catalog: cold.global_services.model_catalog.snapshot(),
        provider: provider.clone(),
        compaction_input_budget: None,
    });
    assert_eq!(
        cold.session_runtime
            .session
            .current_session_id()
            .unwrap()
            .unwrap(),
        session_id
    );
    assert_eq!(
        cold.session_runtime.todos_runtime.snapshot().unwrap(),
        saved
    );
    let history =
        init_context_state(&cold.session_runtime.session, &cold.config.context, "").unwrap();
    let result: serde_json::Value = history
        .messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("seed"))
        .and_then(ChatMessage::text_content)
        .map(|text| serde_json::from_str(text).expect("saved Todos tool result is JSON"))
        .expect("hydrated Todos tool result");
    assert_eq!(result["todos_path"].as_str(), path.to_str());
    compact_session(&cold).await.unwrap();
    assert!(provider.requests.lock().unwrap()[0]
        .messages
        .iter()
        .any(|message| {
            message
                .text_content()
                .is_some_and(|text| text.contains(crate::core::compaction::TOOL_RESULT_PLACEHOLDER))
        }));
    let first = latest_summary(&cold);
    assert!(first.contains(&format!("todos_path: {}", path.display())));
    assert!(!first.contains("plan_path:"));
    assert!(first.contains("saved: unfinished work"));
    assert!(!first.contains("MODEL CLAIM"));
    assert!(!first.contains(body));
    let restored_context =
        init_context_state(&cold.session_runtime.session, &cold.config.context, "").unwrap();
    assert!(restored_context.messages.iter().any(|message| message
        .text_content()
        .is_some_and(|text| text.contains(path.to_str().unwrap()))));
    drop(cold);
    let mut cold = fixture.ctx();
    install_compactor(&mut cold);
    assert_eq!(
        cold.session_runtime.todos_runtime.snapshot().unwrap(),
        saved
    );
    todos::execute(
        &cold.session_runtime.todos_runtime,
        todos::TodosArgs::from_json(&serde_json::json!({"ops":[
            {"kind":"set_status", "id":"saved", "status":"completed", "evidence":["test:cold"]}
        ]}))
        .unwrap(),
    )
    .unwrap();
    let updated = cold.session_runtime.todos_runtime.snapshot().unwrap();
    assert_eq!(updated.draft, body);
    assert_eq!(updated.title, saved.title);
    assert_eq!(updated.created_at, saved.created_at);
    assert_eq!(updated.todos_id, saved.todos_id);
    assert_eq!(updated.items[1].evidence, saved.items[1].evidence);
    compact_session(&cold).await.unwrap();
    let second = latest_summary(&cold);
    assert!(second.contains(&format!("todos_path: {}", path.display())));
    assert_eq!(second.matches("## Progress").count(), 1);
    assert!(second.contains("[completed] saved: unfinished work"));
    assert!(!second.contains("MODEL CLAIM"));

    let plan_path = fixture.workspace.path().join("bound.plan.md");
    let plan = PlanFile {
        frontmatter: serde_json::from_value(serde_json::json!({
            "plan_id":"bound", "goal":"formal work", "state":"planning", "created_at":"now", "schema_version":1,
            "todos":[{"id":"formal", "content":"formal plan work", "status":"pending"}]
        })).unwrap(),
        body: "formal document".into(),
    };
    file_store::write_plan(&plan_path, &plan, 2000).unwrap();
    cold.session_runtime
        .plan_runtime
        .attach_from_resume_state(crate::core::session::manager::ResumeControlState {
            mode: Some(crate::core::session::AgentMode::Chat),
            seen_plan_current: true,
            active_plan: Some(crate::core::plan_runtime::ActivePlan::from_file(
                plan_path.clone(),
                &plan,
            )),
        })
        .unwrap();
    let plan_before = std::fs::read(&plan_path).unwrap();
    todos::execute(
        &cold.session_runtime.todos_runtime,
        todos::TodosArgs::from_json(&serde_json::json!({"title":"still independent"})).unwrap(),
    )
    .unwrap();
    compact_session(&cold).await.unwrap();
    let third = latest_summary(&cold);
    assert!(third.contains(&format!("plan_path: {}", plan_path.display())));
    assert!(!third.contains("todos_path:"));
    assert!(third.contains("formal: formal plan work"));
    assert!(!third.contains("saved: unfinished work"));
    assert_eq!(std::fs::read(&plan_path).unwrap(), plan_before);
    assert!(std::fs::read_to_string(path).unwrap().ends_with(body));
}

#[tokio::test]
#[serial]
async fn completed_plan_compaction_uses_newer_todos_even_after_reopen() {
    use crate::core::plan_runtime::file_store::{self, PlanFile, PlanFileState};
    use crate::core::tools::plan_tool::update_plan::{self, UpdateOp, UpdatePlanArgs};
    use std::time::{Duration, UNIX_EPOCH};

    let _lock = crate::test_support::home_env_lock().lock().unwrap();
    let fixture = super::test_support::Fixture::new();
    let mut ctx = fixture.ctx();
    install_compactor(&mut ctx);
    let plan_path = fixture.workspace.path().join("completed-a.plan.md");
    let plan: PlanFile = PlanFile {
        frontmatter: serde_json::from_value(serde_json::json!({
            "plan_id":"completed-a", "goal":"formal work", "state":"planning", "created_at":"now", "schema_version":1,
            "todos":[{"id":"formal", "content":"OLD PLAN WORK", "status":"pending"}]
        })).unwrap(),
        body: String::new(),
    };
    file_store::write_plan(&plan_path, &plan, 2000).unwrap();
    let todos = &ctx.session_runtime.todos_runtime;
    todos
        .commit(false, |doc| {
            doc.items.push(file_store::TodoItem {
                id: "scratch".into(),
                content: "OLDER TODOS WORK".into(),
                status: file_store::TodoStatus::Pending,
                evidence: vec![],
                kind: Default::default(),
            });
            Ok::<_, AppError>(())
        })
        .unwrap();
    let todos_path = todos.path().to_path_buf();
    let set_time = |path: &std::path::Path, seconds| {
        crate::test_support::set_mtime(path, UNIX_EPOCH + Duration::from_secs(seconds));
    };
    set_time(&todos_path, 100);
    ctx.session_runtime
        .session
        .append_message(serde_json::json!({"role":"user","content":"Finish A"}))
        .unwrap();
    let session_id = ctx
        .session_runtime
        .session
        .current_session_id()
        .unwrap()
        .unwrap();
    ctx.session_runtime
        .plan_runtime
        .build_plan(plan_path.to_str().unwrap(), Some(session_id), true)
        .unwrap();
    let outcome = update_plan::execute(
        &ctx.session_runtime.plan_runtime,
        UpdatePlanArgs {
            plan_id: None,
            path: Some(plan_path.to_string_lossy().into_owned()),
            replace: false,
            ops: vec![UpdateOp::SetStatus {
                id: "formal".into(),
                content: None,
                status: file_store::TodoStatus::Completed,
            }],
        },
    )
    .await
    .unwrap();
    assert_eq!(outcome["plan_state_after"], "completed");
    set_time(&plan_path, 200);
    compact_session(&ctx).await.unwrap();
    let first = latest_summary(&ctx);
    assert!(first.contains("## Progress\nThe plan is complete."));
    assert!(first.contains(" (200000)\n") && first.contains(" (100000)\n"));
    assert!(!first.contains("OLDER TODOS WORK"));

    todos
        .commit(false, |doc| {
            doc.items[0].content = "NEW TODOS WORK".into();
            Ok::<_, AppError>(())
        })
        .unwrap();
    set_time(&todos_path, 300);
    ctx.session_runtime
        .session
        .append_message(serde_json::json!({"role":"user","content":"Move on to the newer Todos"}))
        .unwrap();
    compact_session(&ctx).await.unwrap();
    let second = latest_summary(&ctx);
    drop(ctx);

    let mut reopened = fixture.ctx();
    install_compactor(&mut reopened);
    // The CLI and Serve attach the recovered control state after context construction.
    let hydrated = init_context_state(
        &reopened.session_runtime.session,
        &reopened.config.context,
        "",
    )
    .unwrap();
    reopened
        .session_runtime
        .plan_runtime
        .attach_from_resume_state(hydrated.resume_control)
        .unwrap();
    assert_eq!(
        reopened
            .session_runtime
            .plan_runtime
            .active_plan()
            .unwrap()
            .state,
        PlanFileState::Completed
    );
    reopened
        .session_runtime
        .session
        .append_message(serde_json::json!({"role":"user","content":"Continue after reopening"}))
        .unwrap();
    compact_session(&reopened).await.unwrap();
    let third = latest_summary(&reopened);
    for summary in [&second, &third] {
        assert!(summary.contains("NEW TODOS WORK") && summary.contains(" (300000)\n"));
        assert!(!summary.contains("The plan is complete."));
    }
    for (index, summary) in [&first, &second, &third].into_iter().enumerate() {
        assert!(summary.contains("plan_file_state: completed"));
        assert!(!summary.contains("OLD PLAN WORK") && !summary.contains("MODEL CLAIM"));
        assert_eq!(summary.matches("## Progress").count(), 1);
        let control = summary.split("</control_state>").next().unwrap();
        let progress = summary.split("## Progress").nth(1).unwrap();
        println!(
            "summary {}:\n{}\n## Progress{}",
            index + 1,
            control,
            progress
        );
    }
}

#[tokio::test]
#[serial]
async fn compact_command_persists_boundary_and_rehydrates_cli_context() {
    const API_KEY_ENV: &str = "TOMCAT_CMD_COMPACT_TEST_KEY";
    let old_key = std::env::var_os(API_KEY_ENV);
    unsafe { std::env::set_var(API_KEY_ENV, "test-key") };

    let work_dir = tempfile::tempdir().expect("work dir");
    let mut config = AppConfig::default();
    config.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    config.llm.api_key_env = Some(API_KEY_ENV.to_string());
    config.llm.default_model = "gpt-5.6".to_string();
    let mut ctx = ChatContext::from_config(config).expect("chat context");
    let provider = Arc::new(CompactOnlyProvider::default());
    ctx.global_services.llm_resolver = Arc::new(FixedResolver {
        catalog: ctx.global_services.model_catalog.snapshot(),
        provider: provider.clone(),
        compaction_input_budget: Some(1),
    });

    ctx.session_runtime
        .session
        .append_message(serde_json::json!({
            "role": "user",
            "content": format!("first CLI message: {}", "historical detail ".repeat(2_000)),
        }))
        .expect("append user");
    for message in [
        serde_json::json!({"role":"assistant", "tool_calls":[{"id":"read-1", "type":"function", "function":{"name":"read", "arguments":"{}"}}]}),
        serde_json::json!({"role":"tool", "tool_call_id":"read-1", "content":"compact-budget-tool-body"}),
    ] {
        ctx.session_runtime.session.append_message(message).unwrap();
    }
    ctx.session_runtime
        .session
        .append_message(serde_json::json!({
            "role": "assistant",
            "content": format!("first CLI reply: {}", "historical detail ".repeat(2_000)),
        }))
        .expect("append assistant");
    let mut context_state = init_context_state(
        &ctx.session_runtime.session,
        &ctx.config.context,
        "system prompt",
    )
    .expect("hydrate CLI context");
    let before_ratio = context_state.usage_ratio();

    assert!(context_state
        .messages
        .iter()
        .any(|message| message.text_content() == Some("compact-budget-tool-body")));
    let outcome = run(&ctx, &mut context_state, "system prompt").await;

    assert!(matches!(outcome, ChatCommandOutcome::Handled));
    {
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let body = requests[0].messages[1].text_content().unwrap();
        assert!(body.contains("first CLI message"));
        assert!(body.contains("first CLI reply"));
        assert!(!body.contains("compact-budget-tool-body"));
    }
    assert!(
        ctx.session_runtime
            .session
            .get_entries(16)
            .expect("transcript entries")
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::BranchSummary(summary) if summary.is_boundary == Some(true))),
        "/compact must persist a durable boundary"
    );
    assert!(
        context_state
            .messages
            .iter()
            .any(|message| message.kind == crate::core::llm::MessageKind::CompactionSummary),
        "/compact must rehydrate the CLI's in-memory context before accepting its next input"
    );
    assert!(
        context_state.usage_ratio() < before_ratio,
        "/compact 的验收不能只看 boundary 落盘，重载后的上下文占用必须严格下降: before={before_ratio}, after={}",
        context_state.usage_ratio()
    );
    let transcript = ctx
        .session_runtime
        .session
        .current_transcript_path()
        .expect("current transcript query")
        .expect("current transcript");
    assert!(std::fs::read_to_string(&transcript)
        .unwrap()
        .contains("compact-budget-tool-body"));
    let sidecar_path = user_message_sidecar_path(&transcript);
    assert!(
        sidecar_path.is_file(),
        "/compact must materialize the user-message sidecar"
    );
    assert!(
        std::fs::read_to_string(&sidecar_path)
            .expect("read sidecar")
            .contains("first CLI message"),
        "sidecar must preserve the original user JSON"
    );
    let summary = context_state
        .messages
        .iter()
        .find(|message| message.kind == crate::core::llm::MessageKind::CompactionSummary)
        .and_then(|message| message.text_content())
        .expect("rehydrated boundary summary");
    assert!(
        summary.contains(&sidecar_path.display().to_string()),
        "/compact summary must point to its readable sidecar"
    );
    // 真实跨 boundary 回归：reload 后内存只有旧摘要，第二次 /compact 的 verbatim 可以为空，
    // 但 sidecar 必须仍从权威 transcript 给出 boundary 前的 Normal user 原文。
    let second = compact_session(&ctx).await.expect("reload 后再次 /compact");
    assert!(second.covered_count > 0);
    let latest_summary = ctx
        .session_runtime
        .session
        .get_entries(32)
        .unwrap()
        .into_iter()
        .rev()
        .find_map(|entry| match entry {
            TranscriptEntry::BranchSummary(summary) if summary.is_boundary == Some(true) => {
                summary.summary
            }
            _ => None,
        })
        .expect("second boundary summary");
    assert!(
        latest_summary.contains("(none)"),
        "reload 后 snapshot 只有旧 summary，不应伪造 verbatim user"
    );
    assert!(latest_summary.contains(&sidecar_path.display().to_string()));
    assert!(
        std::fs::read_to_string(&sidecar_path)
            .unwrap()
            .contains("first CLI message"),
        "sidecar 必须保留 boundary 前的 Normal user"
    );

    match old_key {
        Some(value) => unsafe { std::env::set_var(API_KEY_ENV, value) },
        None => unsafe { std::env::remove_var(API_KEY_ENV) },
    }
}
