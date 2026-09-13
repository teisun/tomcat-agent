use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::BackgroundCompletionRoutes;
use crate::core::connector::ConnectorRegistry;
use crate::core::llm::openai_files::OpenAiFilesRuntime;
use crate::core::llm::SharedModelCatalog;
use crate::core::plan_runtime;
use crate::core::tools::contract::registry::ToolRegistry;
use crate::core::tools::primitive::{BashTaskId, BashTaskRegistry, PrimitiveExecutor};
use crate::core::tools::web_fetch::WebFetchRuntime;
use crate::core::tools::web_search::WebSearchRuntime;
use crate::core::{CheckpointStore, LlmResolver, ModelPrefsStore, SessionManager};
use crate::ext::{FunctionRegistry, HostApiDispatcher, PluginFunctionInvoker, PluginManager};
use crate::infra::{AuditRecorder, EventBus};

/// Monotonic, process-local signal that a committed package installation changed a resource
/// inventory. It deliberately is not persisted: every new process discovers the filesystem anew.
static RESOURCE_INVENTORY_EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn publish_resource_inventory_change() {
    RESOURCE_INVENTORY_EPOCH.fetch_add(1, Ordering::Release);
}

pub fn current_resource_inventory_epoch() -> u64 {
    RESOURCE_INVENTORY_EPOCH.load(Ordering::Acquire)
}

pub struct GlobalServices {
    pub model_catalog: SharedModelCatalog,
    pub llm_resolver: Arc<dyn LlmResolver>,
    pub model_prefs: Arc<ModelPrefsStore>,
    pub primitive: Arc<dyn PrimitiveExecutor>,
    pub tool_registry: Arc<dyn ToolRegistry>,
    pub function_registry: Arc<FunctionRegistry>,
    pub event_bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditRecorder>,
    pub gate: Arc<dyn crate::core::permission::PermissionGate>,
    pub config_backend: Option<crate::core::agent_loop::SharedConfigBackend>,
    pub package_install_backend: Option<crate::core::agent_loop::SharedPackageInstallBackend>,
    pub web_fetch_runtime: Arc<WebFetchRuntime>,
    pub web_search_runtime: Arc<WebSearchRuntime>,
    pub plugin_manager: Option<Arc<PluginManager>>,
    pub plugin_function_invoker: Option<Arc<PluginFunctionInvoker>>,
    pub connector_registry: Option<Arc<ConnectorRegistry>>,
}

pub struct ScopeContainer {
    pub event_bus: Arc<dyn EventBus>,
    pub tool_registry: Arc<dyn ToolRegistry>,
    pub function_registry: Arc<FunctionRegistry>,
    pub plugin_manager: Option<Arc<PluginManager>>,
    pub plugin_function_invoker: Option<Arc<PluginFunctionInvoker>>,
    pub connector_registry: Option<Arc<ConnectorRegistry>>,
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
    /// Root used to discover project-local skills, plugins, and functions. For legacy
    /// sessions without an explicit project root this preserves the prior workspace fallback.
    pub resource_root: PathBuf,
    /// Explicit project root captured when the session was created. `None` means the
    /// session may still run tools in its cwd, but it cannot install project-scoped resources.
    pub session_project_root: Option<PathBuf>,
    pub agent_definition_dir: PathBuf,
    pub agent_trail_dir: PathBuf,
    pub cfg_path: PathBuf,
    pub skill_set: Arc<RwLock<crate::core::skill::SkillSet>>,
    pub skill_discovery_handle:
        Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<crate::core::skill::SkillSet>>>>,
}

type OpenAiFilesRuntimeSlot = Arc<Mutex<Option<(String, Arc<OpenAiFilesRuntime>)>>>;

pub struct SessionRuntime {
    pub session: SessionManager,
    pub message_append_sink: Arc<dyn crate::core::session::manager::MessageAppendSink>,
    pub cancel_token: Arc<Mutex<CancellationToken>>,
    pub last_interrupt_at: Arc<Mutex<Option<Instant>>>,
    pub hard_exit_requested: Arc<AtomicBool>,
    /// Last committed inventory generation this session has incorporated into its prompt cache.
    pub seen_resource_inventory_epoch: Arc<AtomicU64>,
    pub session_grants: crate::core::permission::SessionGrants,
    pub bash_task_registry: Arc<BashTaskRegistry>,
    pub follow_up_queue: Arc<Mutex<Vec<crate::core::llm::ChatMessage>>>,
    pub steering_queue: Arc<Mutex<Vec<crate::core::llm::ChatMessage>>>,
    pub completion_routes: BackgroundCompletionRoutes,
    pub delivered_completion: Arc<Mutex<HashSet<BashTaskId>>>,
    pub completion_subscriber_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Checkpoint writes stay off the turn's hot path, but graceful shutdown
    /// awaits this bounded set so a completed final turn is never discarded.
    pub checkpoint_record_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    pub read_file_state: Arc<crate::core::tools::pipeline::read_state::ReadFileState>,
    /// Files cleanup queue is session-scoped. Reusing this runtime lets uploads
    /// and session-end cleanup observe the same in-memory queue.
    pub openai_files_runtime: OpenAiFilesRuntimeSlot,
    pub thinking_display: Arc<std::sync::atomic::AtomicU8>,
    pub todos_runtime: Arc<plan_runtime::todo_runtime::TodosRuntime>,
    pub plan_runtime: Arc<plan_runtime::PlanRuntime>,
    pub suppress_cli_output: bool,
}

pub type SessionRuntimeRegistry = Arc<Mutex<HashMap<String, Arc<SessionRuntime>>>>;
