mod common;

use async_trait::async_trait;
use serial_test::serial;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use tomcat::core::llm::system_prompt::{WorkspaceContext, WorkspaceState};
use tomcat::core::llm::{ChatRequest, ChatResponse, LlmProvider, LlmResolver, LlmScene};
use tomcat::{
    init_context_state, run_chat_turn, AppConfig, AppError, BashResult, Capabilities, ChatContext,
    ChatMessage, DirEntry, EditFileResult, EditOperation, PrimitiveExecutor, PrimitiveOperation,
    ResolvedCall, StreamEvent, WriteFileResult,
};

struct EnvGuard {
    key: &'static str,
    old: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let old = std::env::var_os(key);
        unsafe { std::env::set_var(key, value.into()) };
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.old {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

struct CurrentDirGuard {
    previous: std::path::PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(path).expect("set_current_dir");
        Self { previous }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

struct DeterministicMockLlm {
    streams: Mutex<VecDeque<Vec<Result<StreamEvent, AppError>>>>,
}

impl DeterministicMockLlm {
    fn new(streams: Vec<Vec<Result<StreamEvent, AppError>>>) -> Self {
        Self {
            streams: Mutex::new(streams.into()),
        }
    }
}

#[async_trait]
impl LlmProvider for DeterministicMockLlm {
    fn provider_name(&self) -> &str {
        "mock"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("mock chat not used".to_string()))
    }

    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        let mut guard = self.streams.lock().unwrap();
        let events = guard
            .pop_front()
            .ok_or_else(|| AppError::Llm("DeterministicMockLlm: no more streams".to_string()))?;
        drop(guard);
        Ok(Box::new(tokio_stream::iter(events)))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

struct FixedResolver {
    provider: Arc<dyn LlmProvider>,
    default_model: String,
}

impl FixedResolver {
    fn new(provider: Arc<dyn LlmProvider>, default_model: impl Into<String>) -> Self {
        Self {
            provider,
            default_model: default_model.into(),
        }
    }

    fn resolved_call(&self, model: &str) -> ResolvedCall {
        ResolvedCall {
            provider_impl: self.provider.clone(),
            model: model.to_string(),
            api: "openai-responses".to_string(),
            provider: "openai".to_string(),
            base_url: Some("https://api.openai.com".to_string()),
            key_source: "OPENAI_API_KEY".to_string(),
            thinking_format: tomcat::core::llm::thinking_policy::thinking_format_for_model(model),
            capabilities: Capabilities {
                vision: false,
                files: false,
                tools: true,
                reasoning: true,
                web_search: false,
            },
        }
    }
}

impl LlmResolver for FixedResolver {
    fn resolve(
        &self,
        _scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        let model = session_override
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .unwrap_or(&self.default_model);
        Ok(self.resolved_call(model))
    }
}

fn install_fixed_resolver(
    ctx: &mut ChatContext,
    provider: Arc<dyn LlmProvider>,
    default_model: &str,
) {
    ctx.llm = provider.clone();
    ctx.llm_resolver = Arc::new(FixedResolver::new(provider, default_model));
}

struct SkillReadPrimitive;

#[async_trait]
impl PrimitiveExecutor for SkillReadPrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        std::fs::read_to_string(path).map_err(AppError::Io)
    }

    async fn list_dir(&self, _path: &str, _plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        Ok(vec![])
    }

    async fn write_file(
        &self,
        _path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        unreachable!()
    }

    async fn edit_file(
        &self,
        _path: &str,
        _edits: Vec<EditOperation>,
        _plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        unreachable!()
    }

    async fn execute_bash(
        &self,
        _command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
        _timeout_ms: Option<u64>,
    ) -> Result<BashResult, AppError> {
        unreachable!()
    }

    async fn require_user_confirmation(
        &self,
        _operation: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        unreachable!()
    }
}

fn cli_text_stream(text: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ContentDelta {
            delta: text.to_string(),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "stop".to_string(),
        }),
    ]
}

fn cli_tool_call_stream(id: &str, name: &str, args: &str) -> Vec<Result<StreamEvent, AppError>> {
    vec![
        Ok(StreamEvent::ToolCallDelta {
            index: 0,
            id: Some(id.to_string()),
            name: Some(name.to_string()),
            arguments_delta: Some(args.to_string()),
        }),
        Ok(StreamEvent::FinishReason {
            reason: "tool_calls".to_string(),
        }),
    ]
}

fn write_skill_fixture(workspace: &Path, name: &str, description: &str, user_only: bool) {
    let skill_dir = workspace.join(".tomcat").join("skills").join(name);
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    let mut content = format!("---\nname: {name}\ndescription: {description}\n");
    if user_only {
        content.push_str("disable-model-invocation: true\n");
    }
    content.push_str("---\n# Commit\n1. Run git status.\n");
    std::fs::write(skill_dir.join("SKILL.md"), content).expect("write skill");
}

fn write_live_skill_fixture(workspace: &Path, name: &str, description: &str, secret_token: &str) {
    let skill_dir = workspace.join(".tomcat").join("skills").join(name);
    std::fs::create_dir_all(&skill_dir).expect("create live skill dir");
    let content = format!(
        "---\nname: {name}\ndescription: {description}\n---\n# Live Commit Skill\nsecret-token: {secret_token}\n1. Run git status.\n"
    );
    std::fs::write(skill_dir.join("SKILL.md"), content).expect("write live skill");
}

fn build_system_text(ctx: &ChatContext, skill_set: &tomcat::core::skill::SkillSet) -> String {
    let budget = tomcat::infra::compute_context_budget_chars(&ctx.config.context);
    tomcat::core::llm::system_prompt::build_system_prompt_with_state_and_skills(
        WorkspaceContext {
            agent_workspace_dir: ctx.agent_workspace_dir.to_string_lossy().to_string(),
            agent_definition_dir: ctx.agent_definition_dir.to_string_lossy().to_string(),
            agent_plans_dir: "~/.tomcat/plans".to_string(),
            agent_trail_dir: ctx.agent_trail_dir.to_string_lossy().to_string(),
            tool_lines: Some(
                tomcat::core::tools::contract::catalog::render_core_identity_tool_lines_with_policy(
                    true,
                ),
            ),
        },
        WorkspaceState {
            read_write: vec![],
            read_only: vec![],
            path_rules: vec![],
        },
        Some(skill_set),
        Some(&ctx.config.skills),
        budget,
    )
}

#[tokio::test]
#[serial(env_lock)]
async fn test_chat_skill_discovery_disclosure_and_load_skill_roundtrip() {
    const ENV_KEY: &str = "TOMCAT_SKILL_TOOL_TEST_KEY";

    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(ENV_KEY, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    write_skill_fixture(workspace.path(), "commit", "Create a git commit", false);
    write_skill_fixture(
        workspace.path(),
        "hidden-review",
        "Only for manual reviewer usage",
        true,
    );

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(ENV_KEY.to_string());
    let mut ctx = ChatContext::from_config(cfg).expect("chat context should be created");

    let skill_set = tomcat::core::skill::discover(&ctx.config, &ctx.agent_workspace_dir);
    *ctx.skill_set.write() = skill_set.clone();
    let system_text = build_system_text(&ctx, &skill_set);
    assert!(system_text.contains("<available_skills>"));
    assert!(system_text.contains("<skill name=\"commit\">Create a git commit</skill>"));
    assert!(!system_text.contains("hidden-review"));
    assert!(!system_text.contains("# Commit"));

    ctx.primitive = Arc::new(SkillReadPrimitive);
    let mock_llm = Arc::new(DeterministicMockLlm::new(vec![
        cli_tool_call_stream("call_skill", "load_skill", r#"{"name":"commit"}"#),
        cli_text_stream("SKILL_OK"),
    ]));
    install_fixed_resolver(&mut ctx, mock_llm, "gpt-5.4");

    let mut state = init_context_state(&ctx.session, &ctx.config.context, &system_text).unwrap();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_chat_turn(
            &ctx,
            "Please use the commit skill.",
            &system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("run_chat_turn timeout 5s")
    .expect("run_chat_turn result");

    let result = match outcome {
        tomcat::AgentRunOutcome::Completed(result) => result,
        other => panic!("skill chat path should complete, got: {other:?}"),
    };
    assert!(
        result.final_text.contains("SKILL_OK"),
        "assistant should continue after load_skill, got: {:?}",
        result.final_text
    );
    let tool_msg = result
        .new_messages
        .iter()
        .find(|msg| {
            msg.role == tomcat::core::llm::ChatMessageRole::Tool
                && msg.tool_call_id.as_deref() == Some("call_skill")
        })
        .expect("tool result should exist");
    let tool_text = tool_msg.text_content().expect("tool result should be text");
    assert!(tool_text.contains("<skill name=\"commit\""));
    assert!(tool_text.contains("# Commit"));
    assert!(!tool_text.contains("description: Create a git commit"));
}

#[tokio::test]
#[serial(env_lock)]
async fn live_skill_load_roundtrip_with_real_llm() {
    if std::env::var("PI_LIVE_SKILL").ok().as_deref() != Some("1") {
        return;
    }
    common::load_openai_test_env();
    let _api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!("live_skill_load_roundtrip_with_real_llm 要求设置 OPENAI_API_KEY（环境变量或 tomcat/.env）")
    });

    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    const SECRET_TOKEN: &str = "LIVE_SKILL_TOKEN_42";
    write_live_skill_fixture(
        workspace.path(),
        "commit",
        "Use when the task is to create a git commit.",
        SECRET_TOKEN,
    );

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work.path().to_string_lossy().to_string());
    cfg.llm.default_model =
        std::env::var("TOMCAT_E2E_LLM_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());
    cfg.workspace.workspace_roots = vec![workspace.path().to_string_lossy().to_string()];
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");

    let skill_set = tomcat::core::skill::discover(&ctx.config, &ctx.agent_workspace_dir);
    *ctx.skill_set.write() = skill_set.clone();
    let system_text = build_system_text(&ctx, &skill_set);

    let mut state = init_context_state(&ctx.session, &ctx.config.context, &system_text).unwrap();
    let prompt = format!(
        "Choose the available skill whose description matches creating a git commit. \
Before answering, you MUST load that skill via the load_skill tool and read its full body. \
After the tool returns, reply with exactly two lines: first the secret token from the loaded skill body, then `SKILL_LIVE_OK`. \
Do not use any tool other than load_skill."
    );

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(180),
        run_chat_turn(
            &ctx,
            &prompt,
            &system_text,
            &mut state,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("run_chat_turn timeout 180s")
    .expect("run_chat_turn result");

    let result = match outcome {
        tomcat::AgentRunOutcome::Completed(result) => result,
        other => panic!("live skill path should complete, got: {other:?}"),
    };
    assert!(
        result.final_text.contains(SECRET_TOKEN),
        "assistant should surface the body-only token after load_skill, got: {:?}",
        result.final_text
    );
    assert!(
        result.final_text.contains("SKILL_LIVE_OK"),
        "assistant should confirm live skill completion, got: {:?}",
        result.final_text
    );
    let tool_msg = result
        .new_messages
        .iter()
        .find(|msg| {
            msg.role == tomcat::core::llm::ChatMessageRole::Tool
                && msg
                    .text_content()
                    .is_some_and(|text| text.contains("<skill name=\"commit\""))
        })
        .expect("tool result should exist");
    let tool_text = tool_msg.text_content().expect("tool result should be text");
    assert!(tool_text.contains("<skill name=\"commit\""));
    assert!(tool_text.contains(SECRET_TOKEN));
}
