#![allow(clippy::field_reassign_with_default)]

mod common;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serial_test::serial;
use tempfile::TempDir;
use tomcat::core::agent_loop::build_collapse_summary_artifacts_for_test;
use tomcat::core::llm::MessageKind;
use tomcat::core::plan_runtime::file_store::{
    plan_path_for_id, read_plan, write_plan, PlanFile, PlanFileFrontmatter, PlanFileState,
    TodoItem, TodoStatus, PLAN_FILE_SCHEMA_VERSION,
};
use tomcat::core::plan_runtime::state::PlanState;
use tomcat::core::plan_runtime::PlanRuntime;
use tomcat::core::session::transcript::append_entry;
use tomcat::core::session::{PlanEventKind, PlanEventRef};
use tomcat::core::tools::contract::catalog::builtin_tool_by_name;
use tomcat::core::tools::plan_tool::update_plan::{self, UpdatePlanArgs};
use tomcat::{
    init_context_state, resolve_llm, ChatMessage, ChatRequest, ContextConfig, LlmConfig,
    SessionManager,
};

const COMPACTION_MODEL: &str = "deepseek-v4-pro";
const SESSION_KEY: &str = "keepalive-real-llm";
const SESSION_ID: &str = "keepalive-real-llm-session";
const LLM_TIMEOUT: Duration = Duration::from_secs(120);

fn require_api_key() {
    let _ = common::require_deepseek_api_key("current_tail_guard_real_llm_tests");
}

fn default_model() -> String {
    common::deepseek_test_model()
}

fn real_llm_config() -> LlmConfig {
    let mut cfg = LlmConfig::default();
    common::apply_deepseek_llm_config(&mut cfg);
    cfg
}

fn real_llm() -> Arc<dyn tomcat::LlmProvider> {
    resolve_llm(&real_llm_config())
        .expect("resolve_llm 失败：请检查 DEEPSEEK_API_KEY / DeepSeek 配置")
}

struct HomeGuard {
    _temp: TempDir,
    old_home: Option<OsString>,
}

impl HomeGuard {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("创建临时 HOME 失败");
        std::fs::create_dir_all(temp.path().join(".tomcat").join("plans"))
            .expect("创建 ~/.tomcat/plans 失败");
        let old_home = std::env::var_os("HOME");
        std::env::set_var("HOME", temp.path());
        Self {
            _temp: temp,
            old_home,
        }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        if let Some(ref old_home) = self.old_home {
            std::env::set_var("HOME", old_home);
        } else {
            std::env::remove_var("HOME");
        }
    }
}

#[derive(Clone)]
struct PlanFixture {
    plan_id: String,
    plan_path: PathBuf,
    plan_runtime: Arc<PlanRuntime>,
    latest_plan_event: PlanEventRef,
    active_id: String,
    active_content: String,
    next_id: String,
    next_content: String,
}

fn unique_plan_id(label: &str) -> String {
    format!(
        "keepalive-{}-{}-{}",
        label,
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

fn build_plan_fixture(label: &str) -> PlanFixture {
    let plan_id = unique_plan_id(label);
    let plan_path = plan_path_for_id(&plan_id).expect("plan_path_for_id 失败");
    let active_id = "t_active".to_string();
    let next_id = "t_next".to_string();
    let active_content = "Extract collapse summary helper".to_string();
    let next_content = "Add real LLM keepalive checks".to_string();
    let plan = PlanFile {
        frontmatter: PlanFileFrontmatter {
            plan_id: plan_id.clone(),
            goal: "Verify keepalive continuation after collapse".to_string(),
            state: PlanFileState::Executing,
            session_key: Some(SESSION_KEY.to_string()),
            session_id: Some(SESSION_ID.to_string()),
            created_at: Utc::now().to_rfc3339(),
            schema_version: PLAN_FILE_SCHEMA_VERSION,
            todos: vec![
                TodoItem {
                    id: active_id.clone(),
                    content: active_content.clone(),
                    status: TodoStatus::InProgress,
                },
                TodoItem {
                    id: next_id.clone(),
                    content: next_content.clone(),
                    status: TodoStatus::Pending,
                },
            ],
            unknown: Default::default(),
        },
        body: "\
## Goal
Verify that the model can continue execution after keepalive collapse.

## Plan
- Preserve the execution keepalive snapshot
- Use the keepalive snapshot to advance the next todo
"
        .to_string(),
    };
    write_plan(&plan_path, &plan, 2_000).expect("write_plan 失败");

    let plan_runtime = PlanRuntime::new_with_session_id(SESSION_KEY, SESSION_ID);
    plan_runtime.set_max_code_review_rounds(0);
    plan_runtime.set_executing_for_test(plan_id.clone());

    PlanFixture {
        latest_plan_event: PlanEventRef {
            kind: PlanEventKind::Build,
            plan_id: plan_id.clone(),
            path: plan_path.clone(),
        },
        plan_id,
        plan_path,
        plan_runtime,
        active_id,
        active_content,
        next_id,
        next_content,
    }
}

fn make_working_messages(fixture: &PlanFixture) -> Vec<ChatMessage> {
    vec![
        ChatMessage::user(format!(
            "The plan is still executing. The active step is `{}` and the next step is `{}`.",
            fixture.active_content, fixture.next_content
        )),
        ChatMessage::assistant(format!(
            "I confirmed the active plan is still executing. Finish `{}` first, then promote `{}` to the next in-progress step.",
            fixture.active_content, fixture.next_content
        )),
    ]
}

fn assign_manual_message_ids(mut messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    for (idx, msg) in messages.iter_mut().enumerate() {
        msg.msg_id = Some(format!("m{}", idx + 1));
    }
    messages
}

fn make_update_plan_tool_definition() -> serde_json::Value {
    let entry = builtin_tool_by_name("update_plan").expect("catalog 缺少 update_plan");
    serde_json::json!({
        "type": "function",
        "function": {
            "name": entry.name,
            "description": entry.description,
            "parameters": (entry.parameters)(),
        }
    })
}

fn keepalive_resume_system_prompt() -> String {
    "\
You are resuming an executing plan after context collapse.
The only available tool is update_plan.
Do not answer in prose.
Treat the `## Execution Keepalive` block inside the compaction summary as the source of truth for:
- which step is currently active
- which remaining step should become next in progress
Use the provided plan file text only to recover the matching todo ids.
Call update_plan exactly once.
Do not add, remove, or rewrite todo contents.
Use only `kind`, `id`, and `status` inside each op.
Do not include `content`, `path`, or `replace` unless absolutely required.
"
    .to_string()
}

fn keepalive_resume_user_prompt(fixture: &PlanFixture) -> String {
    let plan_text =
        std::fs::read_to_string(&fixture.plan_path).expect("读取 plan file 失败（resume prompt）");
    format!(
        "\
Current active plan file: `{path}`

Use the Execution Keepalive block to identify the current step and the next pending step.
Then map those contents back to todo ids in the plan file below and call `update_plan` exactly once:

```md
{plan_text}
```

Expected transition:
- mark the keepalive current step as completed
- mark the next pending step as in_progress

Return a single strict JSON tool call.
Do not reply with plain text.
",
        path = fixture.plan_path.display(),
        plan_text = plan_text.trim()
    )
}

async fn request_update_plan_args(
    llm: &Arc<dyn tomcat::LlmProvider>,
    context_messages: Vec<ChatMessage>,
    fixture: &PlanFixture,
) -> UpdatePlanArgs {
    let mut messages = vec![ChatMessage::system(keepalive_resume_system_prompt())];
    messages.extend(context_messages);
    messages.push(ChatMessage::user(keepalive_resume_user_prompt(fixture)));
    let tool_definitions = vec![make_update_plan_tool_definition()];
    let mut last_error = String::new();

    for attempt in 1..=2 {
        let request = ChatRequest {
            messages: messages.clone(),
            model: default_model(),
            temperature: None,
            max_tokens: Some(512),
            stream: Some(false),
            model_override: None,
            tools: Some(tool_definitions.clone()),
        };
        let response = tokio::time::timeout(LLM_TIMEOUT, llm.chat(request))
            .await
            .expect("真实 LLM resume 请求超时")
            .expect("真实 LLM resume 请求失败");
        let assistant = response
            .choices
            .first()
            .expect("真实 LLM 响应 choices 为空")
            .message
            .clone();
        match parse_update_plan_args_from_message(&assistant) {
            Ok(args) => return args,
            Err(err) => {
                last_error = err;
                if attempt == 2 {
                    break;
                }
                messages.push(assistant);
                messages.push(ChatMessage::user(
                    "You must call update_plan now. Return exactly one valid JSON tool call. Use only `ops` with `kind`, `id`, and `status`. Do not answer with plain text.",
                ));
            }
        }
    }

    panic!("真实 LLM 两次尝试后仍未给出合法 update_plan tool call: {last_error}")
}

fn parse_update_plan_args_from_message(assistant: &ChatMessage) -> Result<UpdatePlanArgs, String> {
    let Some(tool_calls) = assistant.tool_calls.as_ref() else {
        return Err(format!(
            "missing tool_calls; assistant_text={:?}",
            assistant.text_content()
        ));
    };
    let Some(tool_call) = tool_calls
        .iter()
        .find(|call| call["function"]["name"].as_str() == Some("update_plan"))
    else {
        return Err(format!(
            "missing update_plan tool call; tool_calls={tool_calls:?}"
        ));
    };
    let Some(raw_args) = tool_call["function"]["arguments"].as_str() else {
        return Err(format!("missing arguments string; tool_call={tool_call:?}"));
    };
    let parsed_args: serde_json::Value = serde_json::from_str(raw_args)
        .map_err(|err| format!("invalid JSON args: {err}; raw={raw_args}"))?;
    UpdatePlanArgs::from_json(&parsed_args)
        .map_err(|err| format!("invalid update_plan args: {err}; raw={raw_args}"))
}

fn assert_update_plan_targets_current_and_next(args: &UpdatePlanArgs, fixture: &PlanFixture) {
    let mut statuses = HashMap::new();
    for op in &args.ops {
        match op {
            update_plan::UpdateOp::SetStatus { id, status, .. } => {
                statuses.insert(id.clone(), *status);
            }
            update_plan::UpdateOp::Upsert { .. } | update_plan::UpdateOp::Remove { .. } => {}
        }
    }
    assert_eq!(
        statuses.get(&fixture.active_id),
        Some(&TodoStatus::Completed),
        "应把 keepalive 当前步骤对应的 todo 标成 completed，实际 args={:?}",
        args.ops
    );
    assert_eq!(
        statuses.get(&fixture.next_id),
        Some(&TodoStatus::InProgress),
        "应把下一条 pending todo 提成 in_progress，实际 args={:?}",
        args.ops
    );
    assert!(
        args.plan_id.as_deref().is_none()
            || args.plan_id.as_deref() == Some(fixture.plan_id.as_str()),
        "plan_id 应为空（走 active plan）或显式指向当前 plan，实际={:?}",
        args.plan_id
    );
    if let Some(path) = &args.path {
        assert!(
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".plan.md")),
            "path 若存在，应指向 plan 文件，实际={path}"
        );
    }
}

async fn execute_update_plan_and_assert(
    fixture: &PlanFixture,
    args: UpdatePlanArgs,
) -> serde_json::Value {
    let result = update_plan::execute(&fixture.plan_runtime, args)
        .await
        .expect("执行真实 LLM 产出的 update_plan 失败");
    assert_eq!(
        result["plan_state_after"].as_str(),
        Some("executing"),
        "推进当前步骤后不应提前 completed"
    );
    assert_eq!(
        result["active_in_progress"].as_str(),
        Some(fixture.next_id.as_str()),
        "active_in_progress 应切到下一条 pending"
    );
    let plan = read_plan(&fixture.plan_path).expect("执行后读取 plan 失败");
    let status_by_id: HashMap<_, _> = plan
        .frontmatter
        .todos
        .iter()
        .map(|todo| (todo.id.as_str(), todo.status))
        .collect();
    assert_eq!(
        status_by_id.get(fixture.active_id.as_str()),
        Some(&TodoStatus::Completed)
    );
    assert_eq!(
        status_by_id.get(fixture.next_id.as_str()),
        Some(&TodoStatus::InProgress)
    );
    result
}

#[tokio::test]
#[serial]
async fn real_llm_collapse_summary_includes_programmatic_keepalive() {
    require_api_key();
    let _home_guard = HomeGuard::new();
    let fixture = build_plan_fixture("case-a");
    let llm = real_llm();
    let working = assign_manual_message_ids(make_working_messages(&fixture));

    let artifacts = build_collapse_summary_artifacts_for_test(
        &working,
        &*llm,
        COMPACTION_MODEL,
        Some(&fixture.plan_runtime),
        Some(&fixture.latest_plan_event),
    )
    .await
    .expect("构建 collapse summary artifacts 失败");

    assert_eq!(
        artifacts.summary_message.kind,
        MessageKind::CompactionSummary
    );
    assert!(artifacts
        .summary_text
        .starts_with("## Structured Summary\n"));
    assert!(artifacts
        .summary_text
        .contains("\n\n## Execution Keepalive\n"));
    assert!(artifacts.summary_text.contains("- mode: executing"));
    assert!(artifacts.summary_text.contains(&format!(
        "- active_plan_path: {}",
        fixture.plan_path.display()
    )));
    assert!(artifacts
        .summary_text
        .contains(&format!("- current_step: {}", fixture.active_content)));
    assert!(artifacts.summary_text.contains(&fixture.next_content));
    assert!(artifacts.summary_text.contains(&format!(
        "latest_plan_event: build:{}:{}",
        fixture.plan_id,
        fixture.plan_path.display()
    )));
    match &artifacts.transcript_entry {
        tomcat::TranscriptEntry::BranchSummary(entry) => {
            assert_eq!(entry.covered_start_id.as_deref(), Some("m1"));
            assert_eq!(entry.covered_end_id.as_deref(), Some("m2"));
            assert_eq!(
                entry.summary.as_deref(),
                Some(artifacts.summary_text.as_str())
            );
        }
        other => panic!("期望 branch_summary entry，实际={other:?}"),
    }
}

#[tokio::test]
#[serial]
async fn real_llm_reads_keepalive_and_calls_update_plan() {
    require_api_key();
    let _home_guard = HomeGuard::new();
    let fixture = build_plan_fixture("case-b");
    let llm = real_llm();
    let working = assign_manual_message_ids(make_working_messages(&fixture));
    let artifacts = build_collapse_summary_artifacts_for_test(
        &working,
        &*llm,
        COMPACTION_MODEL,
        Some(&fixture.plan_runtime),
        Some(&fixture.latest_plan_event),
    )
    .await
    .expect("A 产物构建失败");

    let args =
        request_update_plan_args(&llm, vec![artifacts.summary_message.clone()], &fixture).await;
    assert_update_plan_targets_current_and_next(&args, &fixture);
    let result = execute_update_plan_and_assert(&fixture, args).await;
    assert_eq!(result["plan_id"].as_str(), Some(fixture.plan_id.as_str()));
}

#[tokio::test]
#[serial]
async fn real_llm_after_reload_reads_keepalive_and_calls_update_plan() {
    require_api_key();
    let _home_guard = HomeGuard::new();
    let fixture = build_plan_fixture("case-c");
    let llm = real_llm();

    let sessions_dir = tempfile::tempdir().expect("创建 sessions tempdir 失败");
    let mgr = SessionManager::new(sessions_dir.path().to_path_buf());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).expect("创建 session 失败");

    let mut working = make_working_messages(&fixture);
    for msg in &mut working {
        let row_id = mgr
            .append_message(serde_json::to_value(&*msg).expect("序列化 transcript message 失败"))
            .expect("append_message 失败");
        msg.msg_id = Some(row_id);
    }
    mgr.append_custom_entry(serde_json::json!({
        "event": tomcat::infra::wire::WIRE_PLAN_BUILD,
        "plan_id": fixture.plan_id.clone(),
        "path": fixture.plan_path.to_string_lossy(),
        "state": "executing",
    }))
    .expect("append_custom_entry(plan.build) 失败");

    let artifacts = build_collapse_summary_artifacts_for_test(
        &working,
        &*llm,
        COMPACTION_MODEL,
        Some(&fixture.plan_runtime),
        Some(&fixture.latest_plan_event),
    )
    .await
    .expect("构建 reload 前 collapse artifacts 失败");
    let transcript_path = mgr
        .current_transcript_path()
        .expect("读取当前 transcript path 失败")
        .expect("当前 transcript path 不存在");
    append_entry(&transcript_path, &artifacts.transcript_entry)
        .expect("append branch_summary 失败");

    let state = init_context_state(&mgr, &ContextConfig::default(), "keepalive reload sys")
        .expect("init_context_state(reload) 失败");
    assert!(
        state
            .messages
            .iter()
            .any(|msg| msg.kind == MessageKind::CompactionSummary
                && msg
                    .text_content()
                    .is_some_and(|text| text.contains("## Execution Keepalive"))),
        "reload 后应仍保留 execution keepalive"
    );

    let restored_runtime = PlanRuntime::new_with_session_id(SESSION_KEY, SESSION_ID);
    restored_runtime
        .attach_from_event(state.latest_plan_event.clone())
        .expect("attach_from_event 失败");
    assert_eq!(
        restored_runtime.mode(),
        PlanState::Executing {
            plan_id: fixture.plan_id.clone()
        }
    );

    let mut reloaded_messages = state.messages.clone();
    reloaded_messages.retain(|msg| msg.kind == MessageKind::CompactionSummary);
    let args = request_update_plan_args(&llm, reloaded_messages, &fixture).await;
    assert_update_plan_targets_current_and_next(&args, &fixture);
    let result = update_plan::execute(&restored_runtime, args)
        .await
        .expect("执行 reload 后的 update_plan 失败");
    assert_eq!(result["plan_state_after"].as_str(), Some("executing"));
    assert_eq!(
        restored_runtime.mode(),
        PlanState::Executing {
            plan_id: fixture.plan_id.clone()
        }
    );
    let plan = read_plan(&fixture.plan_path).expect("读取 reload 后 plan 失败");
    let active = plan
        .frontmatter
        .todos
        .iter()
        .find(|todo| todo.status == TodoStatus::InProgress)
        .expect("reload 后应仍有一条 in_progress");
    assert_eq!(active.id, fixture.next_id);
}
