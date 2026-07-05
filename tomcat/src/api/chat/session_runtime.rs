use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::BackgroundCompletionRoutes;
use crate::core::llm::openai_files::OpenAiFilesRuntime;
use crate::core::llm::ModelCatalog;
use crate::core::plan_runtime;
use crate::core::tools::contract::registry::ToolRegistry;
use crate::core::tools::primitive::{BashTaskId, BashTaskRegistry, PrimitiveExecutor};
use crate::core::tools::web_fetch::WebFetchRuntime;
use crate::core::tools::web_search::WebSearchRuntime;
use crate::core::{CheckpointStore, LlmProvider, LlmResolver, ModelThinkingStore, SessionManager};
use crate::ext::{FunctionRegistry, HostApiDispatcher, PluginFunctionInvoker, PluginManager};
use crate::infra::{AuditRecorder, EventBus};

pub struct GlobalServices {
    pub llm: Arc<dyn LlmProvider>,
    pub model_catalog: Arc<ModelCatalog>,
    pub llm_resolver: Arc<dyn LlmResolver>,
    pub model_thinking: Arc<ModelThinkingStore>,
    pub primitive: Arc<dyn PrimitiveExecutor>,
    pub tool_registry: Arc<dyn ToolRegistry>,
    pub function_registry: Arc<FunctionRegistry>,
    pub event_bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditRecorder>,
    pub gate: Arc<dyn crate::core::permission::PermissionGate>,
    pub config_backend: Option<crate::core::agent_loop::SharedConfigBackend>,
    pub web_fetch_runtime: Arc<WebFetchRuntime>,
    pub web_search_runtime: Arc<WebSearchRuntime>,
    pub plugin_manager: Option<Arc<PluginManager>>,
    pub plugin_function_invoker: Option<Arc<PluginFunctionInvoker>>,
}

pub struct ScopeContainer {
    pub event_bus: Arc<dyn EventBus>,
    pub tool_registry: Arc<dyn ToolRegistry>,
    pub function_registry: Arc<FunctionRegistry>,
    pub plugin_manager: Option<Arc<PluginManager>>,
    pub plugin_function_invoker: Option<Arc<PluginFunctionInvoker>>,
    pub dispatcher: Arc<HostApiDispatcher>,
    pub skill_set: Arc<RwLock<crate::core::skill::SkillSet>>,
    pub skill_discovery_handle:
        Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<crate::core::skill::SkillSet>>>>,
}

pub struct ScopeServices {
    pub scope_container: Arc<ScopeContainer>,
    pub checkpoint_switcher: Arc<crate::core::SwitchingCheckpointStore>,
    pub checkpoint_store: Arc<dyn CheckpointStore>,
    pub agent_workspace_dir: PathBuf,
    pub agent_definition_dir: PathBuf,
    pub agent_trail_dir: PathBuf,
    pub cfg_path: PathBuf,
    pub skill_set: Arc<RwLock<crate::core::skill::SkillSet>>,
    pub skill_discovery_handle:
        Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<crate::core::skill::SkillSet>>>>,
}

pub struct SessionRuntime {
    pub session: SessionManager,
    pub message_append_sink: Arc<dyn crate::core::session::manager::MessageAppendSink>,
    pub cancel_token: Arc<Mutex<CancellationToken>>,
    pub last_interrupt_at: Arc<Mutex<Option<Instant>>>,
    pub hard_exit_requested: Arc<AtomicBool>,
    pub session_grants: crate::core::permission::SessionGrants,
    pub bash_task_registry: Arc<BashTaskRegistry>,
    pub follow_up_queue: Arc<Mutex<Vec<crate::core::llm::ChatMessage>>>,
    pub steering_queue: Arc<Mutex<Vec<crate::core::llm::ChatMessage>>>,
    pub completion_routes: BackgroundCompletionRoutes,
    pub delivered_completion: Arc<Mutex<HashSet<BashTaskId>>>,
    pub completion_subscriber_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pub read_file_state: Arc<crate::core::tools::pipeline::read_state::ReadFileState>,
    pub thinking_display: Arc<std::sync::atomic::AtomicU8>,
    pub openai_files_runtime: Option<Arc<OpenAiFilesRuntime>>,
    pub todos_runtime: Arc<plan_runtime::todo_runtime::TodosRuntime>,
    pub plan_runtime: Arc<plan_runtime::PlanRuntime>,
    pub suppress_cli_output: bool,
}

pub type SessionRuntimeRegistry = Arc<Mutex<HashMap<String, Arc<SessionRuntime>>>>;
