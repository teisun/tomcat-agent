//! 子 Agent 派发时必须经 LlmResolver 拿到「模型 + provider」配对。
//!
//! 锁住本次事故不变量：会话模型切到 `fcodex/gpt-5.6-sol` 后，子 Agent 不得
//! 继续使用启动默认 `deepseek-v4-flash` 对应的客户端。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::super::explorer::ExplorerTask;
use super::super::file_store::{
    plan_path_for_id, write_plan, PlanFile, PlanFileFrontmatter, PlanFileState, TodoItem,
    TodoStatus,
};
use super::super::prod_reviewer::{
    ProdCodeReviewerDispatcher, ProdExplorerDispatcher, ProdPlanReviewerDispatcher,
    ProdReviewerDeps,
};
use super::super::verify::{ProdVerifierDeps, ProdVerifierDispatcher};
use super::super::{
    CodeReviewerDispatcher, ExplorerDispatcher, PlanReviewerDispatcher, PlanRuntime,
    VerifierDispatcher,
};
use crate::core::agent_registry::{AgentRegistry, RegistrationGuard};
use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, LlmProvider, LlmResolver, LlmScene, ResolvedCall,
    SharedModelCatalog, StreamEvent,
};
use crate::core::permission::{BashAstChecker, DefaultPermissionGate, GateConfig, SessionGrants};
use crate::core::skill::SkillSet;
use crate::core::tools::primitive::HashlineSegment;
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::core::tools::primitive::PrimitiveOperation;
use crate::core::tools::web_fetch::WebFetchRuntime;
use crate::core::NoopStore;
use crate::infra::config::{AppConfig, ContextConfig, LlmFilesConfig};
use crate::infra::error::{llm_error, llm_http_status_error, AppError, LlmErrorStage};
use crate::infra::event_bus::DefaultEventBus;
use crate::infra::TracingAuditRecorder;
use crate::AllowAllConfirmation;
use crate::{
    BashResult, DirEntry, EditFileResult, EditOperation, ModelPrefsStore, ReadResult,
    SearchFilesArgs, SearchFilesOutput, ThinkingLevel, WriteFileResult,
};
use async_trait::async_trait;
use futures_util::{future, StreamExt};
use parking_lot::{Mutex, RwLock};
use tokio_stream::wrappers::IntervalStream;

fn home_lock() -> &'static crate::test_support::TestLock {
    crate::test_support::home_env_lock()
}

fn orig_home() -> &'static Option<String> {
    static O: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    O.get_or_init(|| std::env::var("HOME").ok())
}

struct HomeGuard {
    path: PathBuf,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
        match orig_home() {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}

fn setup_home() -> HomeGuard {
    let p = std::env::temp_dir().join(format!(
        "tomcat_subagent_binding_home_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(p.join(".tomcat/plans")).unwrap();
    let _ = orig_home();
    std::env::set_var("HOME", &p);
    HomeGuard { path: p }
}

fn write_planning_plan(plan_id: &str, body: &str) -> PathBuf {
    let path = plan_path_for_id(plan_id).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_plan(
        &path,
        &PlanFile {
            frontmatter: PlanFileFrontmatter {
                plan_id: plan_id.to_string(),
                goal: "goal".into(),
                state: PlanFileState::Planning,
                session_key: Some("session-a".into()),
                session_id: Some("sid-a".into()),
                created_at: "2026-07-27T00:00:00Z".into(),
                schema_version: 1,
                todos: vec![TodoItem {
                    id: "t1".into(),
                    content: "step 1".into(),
                    status: TodoStatus::Pending,
                    evidence: Vec::new(),
                    kind: Default::default(),
                }],
                green_build_pass: false,
                green_build_evidence: Vec::new(),
                code_review_pass: false,
                code_review_pass_at_ms: None,
                code_review_rounds: 0,
                code_review_baseline_ms: None,
                code_review_open_findings: Vec::new(),
                code_review_disputed_findings: Vec::new(),
                code_review_handoff: false,
                code_review_handoff_acknowledged: false,
                code_review_residual_findings: Vec::new(),
                completion_gate_cycles: 0,
                acceptance_commands: Vec::new(),
                unknown: Default::default(),
            },
            body: body.to_string(),
        },
        1000,
    )
    .unwrap();
    path
}

type ResolveCallLog = Vec<(LlmScene, Option<String>)>;

#[derive(Clone, Default)]
struct ResolveLog(Arc<Mutex<ResolveCallLog>>);

enum RecordingStreamPlan {
    Immediate(Vec<Result<StreamEvent, AppError>>),
    KeepaliveOnlyIdle { interval_ms: u64, timeout_sec: u64 },
}

struct RecordingProvider {
    name: String,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    streams: Mutex<Vec<RecordingStreamPlan>>,
}

impl RecordingProvider {
    fn success(name: &str, text: &str) -> (Arc<Self>, Arc<Mutex<Vec<ChatRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(Self {
            name: name.to_string(),
            requests: requests.clone(),
            streams: Mutex::new(vec![RecordingStreamPlan::Immediate(vec![
                Ok(StreamEvent::ContentDelta {
                    delta: text.to_string(),
                }),
                Ok(StreamEvent::FinishReason {
                    reason: "stop".to_string(),
                }),
            ])]),
        });
        (provider, requests)
    }

    fn fatal_400(name: &str) -> (Arc<Self>, Arc<Mutex<Vec<ChatRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(Self {
            name: name.to_string(),
            requests: requests.clone(),
            streams: Mutex::new(vec![RecordingStreamPlan::Immediate(vec![Err(
                llm_http_status_error(name, 400, "unsupported model"),
            )])]),
        });
        (provider, requests)
    }

    fn keepalive_only_idle(
        name: &str,
        timeout_sec: u64,
    ) -> (Arc<Self>, Arc<Mutex<Vec<ChatRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut streams = Vec::new();
        for _ in 0..10 {
            streams.push(RecordingStreamPlan::KeepaliveOnlyIdle {
                interval_ms: 200,
                timeout_sec,
            });
        }
        let provider = Arc::new(Self {
            name: name.to_string(),
            requests: requests.clone(),
            streams: Mutex::new(streams),
        });
        (provider, requests)
    }
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    fn provider_name(&self) -> &str {
        &self.name
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
        Err(AppError::Llm("recording provider chat unused".into()))
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<
        Box<dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
        AppError,
    > {
        self.requests.lock().push(req);
        let mut guard = self.streams.lock();
        if guard.is_empty() {
            return Err(AppError::Llm("no streams left".into()));
        }
        let stream = match guard.remove(0) {
            RecordingStreamPlan::Immediate(events) => Box::new(tokio_stream::iter(events))
                as Box<
                    dyn tokio_stream::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin,
                >,
            RecordingStreamPlan::KeepaliveOnlyIdle {
                interval_ms,
                timeout_sec,
            } => {
                let provider_name = self.name.clone();
                let timeout_ticks =
                    ((timeout_sec * 1000) + interval_ms.saturating_sub(1)) / interval_ms;
                let interval =
                    IntervalStream::new(tokio::time::interval(Duration::from_millis(interval_ms)))
                        .enumerate()
                        .map(move |(idx, _)| {
                            if (idx as u64) + 1 >= timeout_ticks {
                                Some(Err(llm_error(
                                    &provider_name,
                                    LlmErrorStage::IdleTimeout,
                                    format!("流式空闲超时: stream_timeout_sec={}s", timeout_sec),
                                )))
                            } else {
                                None
                            }
                        })
                        .filter_map(future::ready);
                Box::new(interval)
            }
        };
        Ok(stream)
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

struct FakeResolver {
    providers: HashMap<String, Arc<dyn LlmProvider>>,
    wire_names: HashMap<String, String>,
    default_model: String,
    fail_main: bool,
    log: ResolveLog,
}

impl FakeResolver {
    fn new(
        providers: HashMap<String, Arc<dyn LlmProvider>>,
        wire_names: HashMap<String, String>,
        default_model: impl Into<String>,
        log: ResolveLog,
    ) -> Self {
        Self {
            providers,
            wire_names,
            default_model: default_model.into(),
            fail_main: false,
            log,
        }
    }
}

impl LlmResolver for FakeResolver {
    fn resolve(
        &self,
        scene: LlmScene,
        session_override: Option<&str>,
    ) -> Result<ResolvedCall, AppError> {
        self.log
            .0
            .lock()
            .push((scene, session_override.map(str::to_string)));
        if self.fail_main && scene == LlmScene::Main {
            return Err(AppError::Config("missing API key for session model".into()));
        }
        let catalog_id = match scene {
            LlmScene::Main => session_override
                .filter(|m| !m.trim().is_empty())
                .unwrap_or(&self.default_model)
                .to_string(),
            LlmScene::Compaction => self.default_model.clone(),
            other => {
                return Err(AppError::Config(format!(
                    "fake resolver does not support scene {other:?}"
                )));
            }
        };
        let provider = self
            .providers
            .get(&catalog_id)
            .cloned()
            .ok_or_else(|| AppError::Config(format!("unknown model `{catalog_id}`")))?;
        let wire = self
            .wire_names
            .get(&catalog_id)
            .cloned()
            .unwrap_or_else(|| catalog_id.clone());
        Ok(ResolvedCall::from_parts_unchecked(
            provider, catalog_id, wire,
        ))
    }
}

struct UnusedPrimitive;

#[async_trait]
impl PrimitiveExecutor for UnusedPrimitive {
    async fn read(
        &self,
        _path: &str,
        _offset: Option<u64>,
        _limit: Option<u64>,
        _line_numbers: bool,
        _hashline: bool,
        _plugin_id: &str,
    ) -> Result<ReadResult, AppError> {
        unreachable!("binding test should not call read")
    }

    async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, AppError> {
        unreachable!("binding test should not call read_file")
    }

    async fn list_dir(&self, _path: &str, _plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
        unreachable!("binding test should not call list_dir")
    }

    async fn write_file(
        &self,
        _path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<WriteFileResult, AppError> {
        unreachable!("binding test should not call write_file")
    }

    async fn edit_file(
        &self,
        _path: &str,
        _edits: Vec<EditOperation>,
        _plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        unreachable!("binding test should not call edit_file")
    }

    async fn execute_bash(
        &self,
        _command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _foreground_wait_ms: Option<u64>,
    ) -> Result<BashResult, AppError> {
        unreachable!("binding test should not call bash")
    }

    async fn hashline_edit(
        &self,
        _path: &str,
        _segments: Vec<HashlineSegment>,
        _plugin_id: &str,
    ) -> Result<EditFileResult, AppError> {
        unreachable!("binding test should not call hashline_edit")
    }

    async fn search_files(
        &self,
        _args: SearchFilesArgs,
        _plugin_id: &str,
    ) -> Result<SearchFilesOutput, AppError> {
        unreachable!("binding test should not call search_files")
    }

    async fn require_user_confirmation(
        &self,
        _operation: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        unreachable!("binding test should not call require_user_confirmation")
    }
}

fn permission_gate(root: &Path) -> Arc<dyn crate::core::permission::PermissionGate> {
    DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: root.to_path_buf(),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
    .into_arc()
}

struct BindingFixture {
    home: HomeGuard,
    plan_runtime: Arc<PlanRuntime>,
    registry: Arc<AgentRegistry>,
    _root_guard: RegistrationGuard,
    agent_trail_dir: PathBuf,
    workspace: tempfile::TempDir,
    resolver: Arc<FakeResolver>,
    model_catalog: SharedModelCatalog,
    model_prefs: Arc<ModelPrefsStore>,
    resolve_log: ResolveLog,
    #[allow(dead_code)]
    fcodex_requests: Arc<Mutex<Vec<ChatRequest>>>,
    #[allow(dead_code)]
    deepseek_requests: Arc<Mutex<Vec<ChatRequest>>>,
}

fn build_fixture(
    fcodex_provider: Arc<dyn LlmProvider>,
    deepseek_provider: Arc<dyn LlmProvider>,
    fcodex_requests: Arc<Mutex<Vec<ChatRequest>>>,
    deepseek_requests: Arc<Mutex<Vec<ChatRequest>>>,
    fail_main: bool,
) -> BindingFixture {
    let home = setup_home();
    let plan_id = "binding_plan";
    let path = write_planning_plan(plan_id, "## Goal\nbinding\n");
    let plan_runtime = PlanRuntime::new("session-a");
    plan_runtime.bind_plan_file_for_test(path);
    plan_runtime.set_session_model("fcodex/gpt-5.6-sol");

    let agent_trail_dir = home.path.join(".tomcat/agents/main");
    std::fs::create_dir_all(&agent_trail_dir).unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let config = AppConfig::default();
    std::fs::write(
        home.path.join(".tomcat/models.toml"),
        r#"
[[models]]
id = "fcodex/gpt-5.6-sol"
api = "openai-responses"
provider = "fcodex"
supported_reasoning_levels = ["off", "high"]
"#,
    )
    .unwrap();
    let model_catalog = SharedModelCatalog::load(&config).unwrap();
    let model_prefs = Arc::new(
        ModelPrefsStore::load(
            home.path.join(".tomcat/model-thinking.json"),
            ThinkingLevel::High,
        )
        .unwrap(),
    );

    let mut providers: HashMap<String, Arc<dyn LlmProvider>> = HashMap::new();
    providers.insert("fcodex/gpt-5.6-sol".into(), fcodex_provider);
    providers.insert("deepseek-v4-flash".into(), deepseek_provider);
    let mut wire_names = HashMap::new();
    wire_names.insert("fcodex/gpt-5.6-sol".into(), "gpt-5.6-sol".into());
    wire_names.insert("deepseek-v4-flash".into(), "deepseek-v4-flash".into());
    let resolve_log = ResolveLog::default();
    let mut resolver = FakeResolver::new(
        providers,
        wire_names,
        "deepseek-v4-flash",
        resolve_log.clone(),
    );
    resolver.fail_main = fail_main;
    let resolver = Arc::new(resolver);

    let registry = AgentRegistry::new();
    let root_guard = registry.register_root("parent-binding").unwrap();

    BindingFixture {
        home,
        plan_runtime,
        registry,
        _root_guard: root_guard,
        agent_trail_dir,
        workspace,
        resolver,
        model_catalog,
        model_prefs,
        resolve_log,
        fcodex_requests,
        deepseek_requests,
    }
}

fn reviewer_deps(fx: &BindingFixture, model_override: Option<&str>) -> ProdReviewerDeps {
    ProdReviewerDeps {
        agent_registry: fx.registry.clone(),
        parent_session_id: "parent-binding".into(),
        llm_resolver: fx.resolver.clone(),
        model_catalog: fx.model_catalog.clone(),
        model_prefs: fx.model_prefs.clone(),
        primitive: Arc::new(UnusedPrimitive),
        event_bus: Arc::new(DefaultEventBus::new()),
        agent_trail_dir: fx.agent_trail_dir.to_string_lossy().to_string(),
        checkpoint_store: Arc::new(NoopStore),
        context_config: ContextConfig::default(),
        refresh_mutation_stamp: true,
        llm_files_config: LlmFilesConfig::default(),
        sessions_dir: fx.agent_trail_dir.join("sessions"),
        agent_workspace_dir: fx.workspace.path().to_path_buf(),
        skill_set: Arc::new(RwLock::new(SkillSet::default())),
        skills_config: AppConfig::default().skills,
        bash_config: AppConfig::default().tools.bash,
        gate: permission_gate(fx.workspace.path()),
        confirmation: Arc::new(AllowAllConfirmation),
        audit: Arc::new(TracingAuditRecorder),
        bash_ast: BashAstChecker::default(),
        plan_runtime: Arc::downgrade(&fx.plan_runtime),
        model_override: model_override.map(str::to_string),
        fallback_model: "deepseek-v4-flash".into(),
        max_turns: 4,
    }
}

fn verifier_deps(fx: &BindingFixture) -> ProdVerifierDeps {
    ProdVerifierDeps {
        agent_registry: fx.registry.clone(),
        parent_session_id: "parent-binding".into(),
        llm_resolver: fx.resolver.clone(),
        model_catalog: fx.model_catalog.clone(),
        model_prefs: fx.model_prefs.clone(),
        primitive: Arc::new(UnusedPrimitive),
        event_bus: Arc::new(DefaultEventBus::new()),
        agent_trail_dir: fx.agent_trail_dir.to_string_lossy().to_string(),
        checkpoint_store: Arc::new(NoopStore),
        context_config: ContextConfig::default(),
        refresh_mutation_stamp: true,
        llm_files_config: LlmFilesConfig::default(),
        sessions_dir: fx.agent_trail_dir.join("sessions"),
        web_fetch_runtime: Arc::new(
            WebFetchRuntime::new(
                &AppConfig::default(),
                fx.agent_trail_dir.join("tool-results"),
            )
            .unwrap(),
        ),
        agent_workspace_dir: fx.workspace.path().to_path_buf(),
        skill_set: Arc::new(RwLock::new(SkillSet::default())),
        skills_config: AppConfig::default().skills,
        bash_config: AppConfig::default().tools.bash,
        gate: permission_gate(fx.workspace.path()),
        confirmation: Arc::new(AllowAllConfirmation),
        audit: Arc::new(TracingAuditRecorder),
        bash_ast: BashAstChecker::default(),
        plan_runtime: Arc::downgrade(&fx.plan_runtime),
        model_override: None,
        fallback_model: "deepseek-v4-flash".into(),
    }
}

fn assert_main_resolve_used_session_model(log: &ResolveLog) {
    let calls = log.0.lock().clone();
    assert!(
        calls.iter().any(|(scene, override_)| {
            *scene == LlmScene::Main && override_.as_deref() == Some("fcodex/gpt-5.6-sol")
        }),
        "expected Main resolve with session model, got {calls:?}"
    );
}

#[tokio::test]
async fn subagent_uses_provider_matching_session_model_not_startup_default() {
    let _g = home_lock().lock().unwrap();
    let (fcodex, fcodex_requests) = RecordingProvider::success(
        "fcodex",
        r#"<review>
summary: ok
changes_summary: none
applied_changes: false
</review>"#,
    );
    let (deepseek, deepseek_requests) =
        RecordingProvider::success("deepseek", "should not be called");
    let fx = build_fixture(
        fcodex,
        deepseek,
        fcodex_requests.clone(),
        deepseek_requests.clone(),
        false,
    );
    fx.model_prefs
        .set_reasoning("fcodex/gpt-5.6-sol", ThinkingLevel::Off)
        .unwrap();
    let dispatcher = ProdPlanReviewerDispatcher::new("binding_test", reviewer_deps(&fx, None));
    let summary = dispatcher
        .dispatch("binding_plan", "## Goal\nbinding\n", true)
        .await;

    assert!(!summary.aborted, "summary={}", summary.summary);
    assert_main_resolve_used_session_model(&fx.resolve_log);
    assert!(
        deepseek_requests.lock().is_empty(),
        "startup default provider must not receive subagent traffic"
    );
    let reqs = fcodex_requests.lock().clone();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].model, "gpt-5.6-sol");
    assert_eq!(reqs[0].thinking_level, Some(ThinkingLevel::Off));
    let _ = fx.home;
}

#[tokio::test]
async fn all_four_subagents_resolve_main_with_session_model() {
    let _g = home_lock().lock().unwrap();
    let (fcodex_plan, _) = RecordingProvider::success(
        "fcodex",
        r#"<review>
summary: plan ok
changes_summary: none
applied_changes: false
</review>"#,
    );
    let (fcodex_code, _) = RecordingProvider::success(
        "fcodex",
        r#"<review>
verdict: pass
summary: code ok
changes_summary: none
applied_changes: false
</review>"#,
    );
    let (fcodex_explorer, _) = RecordingProvider::success(
        "fcodex",
        r#"<explorer>
findings: none
</explorer>"#,
    );
    let (fcodex_verifier, _) = RecordingProvider::success(
        "fcodex",
        r#"<verify>
checks: []
verdict: pass
summary: verify ok
</verify>"#,
    );
    // One shared FakeResolver should route all Main resolves by catalog id.
    // Reuse one fcodex provider with multiple scripted streams for four dispatches.
    let fcodex_requests = Arc::new(Mutex::new(Vec::new()));
    let deepseek_requests = Arc::new(Mutex::new(Vec::new()));
    let fcodex = Arc::new(RecordingProvider {
        name: "fcodex".into(),
        requests: fcodex_requests.clone(),
        streams: Mutex::new(vec![
            RecordingStreamPlan::Immediate(vec![
                Ok(StreamEvent::ContentDelta {
                    delta: r#"<review>
summary: plan ok
changes_summary: none
applied_changes: false
</review>"#
                        .into(),
                }),
                Ok(StreamEvent::FinishReason {
                    reason: "stop".into(),
                }),
            ]),
            RecordingStreamPlan::Immediate(vec![
                Ok(StreamEvent::ContentDelta {
                    delta: r#"<review>
verdict: pass
summary: code ok
changes_summary: none
applied_changes: false
</review>"#
                        .into(),
                }),
                Ok(StreamEvent::FinishReason {
                    reason: "stop".into(),
                }),
            ]),
            RecordingStreamPlan::Immediate(vec![
                Ok(StreamEvent::ContentDelta {
                    delta: "explorer found nothing critical".into(),
                }),
                Ok(StreamEvent::FinishReason {
                    reason: "stop".into(),
                }),
            ]),
            RecordingStreamPlan::Immediate(vec![
                Ok(StreamEvent::ContentDelta {
                    delta: r#"<verify>
checks:
  - name: smoke
    command: true
    result: pass
    output_excerpt: ok
verdict: pass
summary: verify ok
</verify>"#
                        .into(),
                }),
                Ok(StreamEvent::FinishReason {
                    reason: "stop".into(),
                }),
            ]),
        ]),
    });
    let deepseek = Arc::new(RecordingProvider {
        name: "deepseek".into(),
        requests: deepseek_requests.clone(),
        streams: Mutex::new(Vec::new()),
    });
    let _ = (fcodex_plan, fcodex_code, fcodex_explorer, fcodex_verifier);
    let fx = build_fixture(fcodex, deepseek, fcodex_requests, deepseek_requests, false);
    fx.model_prefs
        .set_reasoning("fcodex/gpt-5.6-sol", ThinkingLevel::Xhigh)
        .unwrap();
    let parent_session =
        crate::core::session::SessionManager::new(fx.agent_trail_dir.join("sessions"));
    parent_session
        .create_session("agent:main:main", None)
        .expect("create parent session");
    let parent_transcript = parent_session
        .current_transcript_path()
        .expect("current transcript path")
        .expect("parent transcript");
    let captured: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let captured = Arc::clone(&captured);
        let parent_session = parent_session.clone();
        fx.plan_runtime
            .attach_transcript_appender(Arc::new(move |extra| {
                captured.lock().push(extra.clone());
                parent_session.append_custom_entry(extra)
            }));
    }

    let plan = ProdPlanReviewerDispatcher::new("binding_test", reviewer_deps(&fx, None));
    let code = ProdCodeReviewerDispatcher::new("binding_test", reviewer_deps(&fx, None));
    let explorer = ProdExplorerDispatcher::new("binding_test", reviewer_deps(&fx, None));
    let verifier = ProdVerifierDispatcher::new("binding_test", verifier_deps(&fx));
    let review_state = super::sample_frontmatter();

    let plan_summary = plan
        .dispatch("binding_plan", "## Goal\nbinding\n", true)
        .await;
    let code_summary = code
        .dispatch(
            "binding_plan",
            "## Goal\nbinding\n",
            &review_state,
            &crate::core::plan_runtime::CodeReviewDispatchInfo {
                round: 1,
                review_attempt_id: "binding_plan:1".into(),
                tool_call_id: "tc-binding".into(),
                is_incremental: false,
                delta_file_count: 0,
            },
        )
        .await;
    let explorer_report = explorer
        .dispatch(&ExplorerTask {
            id: "e1".into(),
            prompt: "look around".into(),
        })
        .await;
    let verify_summary = verifier
        .dispatch("binding_plan", "## Goal\nbinding\n")
        .await;

    assert!(!plan_summary.aborted, "{}", plan_summary.summary);
    assert!(!code_summary.aborted, "{}", code_summary.summary);
    assert!(!explorer_report.aborted, "{}", explorer_report.report);
    assert_eq!(verify_summary.verdict, "pass");

    let main_calls: Vec<_> = fx
        .resolve_log
        .0
        .lock()
        .iter()
        .filter(|(scene, _)| *scene == LlmScene::Main)
        .cloned()
        .collect();
    assert_eq!(main_calls.len(), 4, "main_calls={main_calls:?}");
    for (_scene, override_) in &main_calls {
        assert_eq!(override_.as_deref(), Some("fcodex/gpt-5.6-sol"));
    }
    let requests = fx.fcodex_requests.lock().clone();
    assert_eq!(requests.len(), 4);
    assert!(
        requests
            .iter()
            .all(|request| request.thinking_level == Some(ThinkingLevel::High)),
        "unsupported Xhigh must clamp to high for every subagent: {requests:?}"
    );
    let events = captured.lock();
    let plan_started = events
        .iter()
        .find(|v| v["event"] == "plan.review.started")
        .expect("missing plan.review.started");
    assert_eq!(
        plan_started["child_session_id"],
        plan_summary.child_session_id
    );
    assert!(plan_started["transcript_path"]
        .as_str()
        .unwrap_or_default()
        .ends_with(".jsonl"));
    let code_started = events
        .iter()
        .find(|v| v["event"] == "plan.code_review.started")
        .expect("missing plan.code_review.started");
    assert_eq!(
        code_started["child_session_id"],
        code_summary.child_session_id
    );
    assert_eq!(code_started["review_attempt_id"], "binding_plan:1");
    let explorer_started = events
        .iter()
        .find(|v| v["event"] == "plan.explorer.started")
        .expect("missing plan.explorer.started");
    assert_eq!(
        explorer_started["child_session_id"],
        explorer_report.child_session_id
    );
    assert_eq!(explorer_started["task_id"], "e1");
    let transcript = std::fs::read_to_string(&parent_transcript).expect("read parent transcript");
    assert!(
        transcript.contains("\"event\":\"plan.review.started\""),
        "transcript={transcript}"
    );
    assert!(
        transcript.contains("\"event\":\"plan.code_review.started\""),
        "transcript={transcript}"
    );
    assert!(
        transcript.contains("\"event\":\"plan.explorer.started\""),
        "transcript={transcript}"
    );
    let _ = fx.home;
}

#[tokio::test]
async fn resolve_failure_aborts_with_model_unresolved_and_keeps_plan_file() {
    let _g = home_lock().lock().unwrap();
    let (fcodex, fcodex_requests) = RecordingProvider::success("fcodex", "unused");
    let (deepseek, deepseek_requests) = RecordingProvider::success("deepseek", "unused");
    let fx = build_fixture(fcodex, deepseek, fcodex_requests, deepseek_requests, true);
    let plan_path = plan_path_for_id("binding_plan").unwrap();
    assert!(plan_path.exists());

    let dispatcher = ProdPlanReviewerDispatcher::new("binding_test", reviewer_deps(&fx, None));
    let summary = dispatcher
        .dispatch("binding_plan", "## Goal\nbinding\n", true)
        .await;

    assert!(summary.aborted);
    assert_eq!(summary.reviewer_stop_reason, "model_unresolved");
    assert!(
        summary
            .summary
            .contains("模型 `fcodex/gpt-5.6-sol` 解析失败"),
        "summary={}",
        summary.summary
    );
    assert!(plan_path.exists(), "create_plan/disk plan must remain");
    let _ = fx.home;
}

#[tokio::test]
async fn first_llm_fatal_uses_no_transcript_hint_and_llm_error_stop_reason() {
    let _g = home_lock().lock().unwrap();
    let (fcodex, fcodex_requests) = RecordingProvider::fatal_400("fcodex");
    let (deepseek, deepseek_requests) = RecordingProvider::success("deepseek", "unused");
    let fx = build_fixture(fcodex, deepseek, fcodex_requests, deepseek_requests, false);

    let dispatcher = ProdPlanReviewerDispatcher::new("binding_test", reviewer_deps(&fx, None));
    let summary = dispatcher
        .dispatch("binding_plan", "## Goal\nbinding\n", true)
        .await;

    assert!(summary.aborted);
    assert_eq!(summary.reviewer_stop_reason, "llm_error");
    assert!(
        summary.summary.contains("[debug transcript]"),
        "summary={}",
        summary.summary
    );
    let path = fx
        .agent_trail_dir
        .join("subagent-sessions")
        .join(format!("{}.jsonl", summary.child_session_id));
    assert!(
        path.exists(),
        "eager transcript should exist even with zero messages"
    );
    let raw = std::fs::read_to_string(&path).expect("read eager transcript");
    let lines: Vec<_> = raw.lines().collect();
    assert_eq!(
        lines.len(),
        4,
        "seed system/user messages should follow header + meta"
    );
    assert!(
        raw.contains("\"event\":\"subagent.transcript.meta\""),
        "raw={raw}"
    );
    let _ = fx.home;
}

#[tokio::test]
async fn prod_code_reviewer_keepalive_only_provider_surfaces_idle_timeout() {
    let _g = home_lock().lock().unwrap();
    let (fcodex, fcodex_requests) = RecordingProvider::keepalive_only_idle("fcodex", 1);
    let (deepseek, deepseek_requests) = RecordingProvider::success("deepseek", "unused");
    let fx = build_fixture(fcodex, deepseek, fcodex_requests, deepseek_requests, false);

    let dispatcher = ProdCodeReviewerDispatcher::new("binding_test", reviewer_deps(&fx, None));
    let review_state = super::sample_frontmatter();
    let summary = dispatcher
        .dispatch(
            "binding_plan",
            "## Goal\nbinding\n",
            &review_state,
            &crate::core::plan_runtime::CodeReviewDispatchInfo {
                round: 1,
                review_attempt_id: "binding_plan:1".into(),
                tool_call_id: "tc-binding".into(),
                is_incremental: false,
                delta_file_count: 0,
            },
        )
        .await;

    assert!(summary.aborted, "summary={}", summary.summary);
    assert_eq!(summary.reviewer_stop_reason, "llm_error");
    assert!(
        summary.summary.contains("流式空闲超时"),
        "summary={}",
        summary.summary
    );
    assert!(
        summary.summary.contains("stream_timeout_sec=1s"),
        "summary={}",
        summary.summary
    );
    let _ = fx.home;
}
