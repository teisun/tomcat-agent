use crate::core::{LlmProvider, LlmResolver, PrimitiveExecutor, SessionManager, ToolRegistry};
use crate::ext::host_binding::HostResponse;
use crate::ext::vm_actor::EventEnvelope;
use crate::ext::PluginManager;
use crate::infra::event_bus::{EventBus, EventListenerId};
use crate::infra::http_client::{
    build_outbound_client, default_connect_timeout_for, OutboundClientErrorKind,
    OutboundClientOptions,
};
use crate::infra::{
    AuditRecorder, DEFAULT_TOOLS_WEB_FETCH_MAX_HTTP_CONTENT_BYTES,
    DEFAULT_TOOLS_WEB_FETCH_TIMEOUT_MS,
};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::{oneshot, Semaphore};

/// 异步 Hostcall 任务状态。
pub enum AsyncCallStatus {
    Pending,
    Done(HostResponse),
    Error(String),
}

/// 宿主 API 分发器：Send + Sync，支持多 Agent 并发。
/// 各 Processor 以 Option 注入，未注入时返回明确错误。
/// Clone 为浅拷贝（内部均为 Arc），可安全传入 spawn 的 Future。
#[derive(Clone)]
pub struct HostApiDispatcher {
    pub(super) event_bus: Arc<dyn EventBus>,
    pub(super) primitive: Option<Arc<dyn PrimitiveExecutor>>,
    pub(super) tools: Option<Arc<dyn ToolRegistry>>,
    pub(super) llm: Option<Arc<dyn LlmProvider>>,
    pub(super) llm_resolver: Option<Arc<dyn LlmResolver>>,
    pub(super) session: Option<Arc<SessionManager>>,
    pub(super) session_registry: Arc<DashMap<String, Weak<SessionManager>>>,
    pub(super) audit: Option<Arc<dyn AuditRecorder>>,
    pub(super) async_results: Arc<DashMap<String, AsyncCallStatus>>,
    /// instance_id -> [callId, ...] 映射，用于实例销毁时清理 pending 任务。
    pub(super) instance_calls: Arc<DashMap<String, Vec<String>>>,
    pub(super) tokio_handle: Option<Handle>,
    pub(super) async_timeout: Duration,
    pub(super) llm_semaphore: Arc<Semaphore>,
    pub(super) fetch_client: reqwest::Client,
    pub(super) fetch_proxy_mode_label: &'static str,
    pub(super) fetch_timeout: Duration,
    pub(super) fetch_semaphore: Arc<Semaphore>,
    pub(super) fetch_max_body_bytes: usize,
    pub(super) plugin_manager: Option<Weak<PluginManager>>,
    /// 长生命周期 VM 的事件队列：instance_id -> event Receiver（Mutex 保证 Sync）。
    /// waitForEvent 路由从此 channel 阻塞接收事件。
    pub(super) event_receivers:
        Arc<DashMap<String, Arc<std::sync::Mutex<std::sync::mpsc::Receiver<EventEnvelope>>>>>,
    /// 事件发送端：宿主通过此端投递事件给 VM。
    pub(super) event_senders: Arc<DashMap<String, std::sync::mpsc::SyncSender<EventEnvelope>>>,
    /// 可选：`context.uiNotify` 调用次数（测试断言用，与生产逻辑无关）。
    pub(super) ui_notify_count: Option<Arc<AtomicU32>>,
    /// `context.commandCompleted` 调用次数（测试断言用）。
    pub(super) command_completed_count: Arc<AtomicU32>,
    /// `context.commandFailed` 调用次数（测试断言用）。
    pub(super) command_failed_count: Arc<AtomicU32>,
    /// 插件已注册的 slash 命令：(name, description)，handler 仅存于 JS `__pi_commands`。
    pub(super) plugin_commands: Arc<DashMap<String, Vec<(String, String)>>>,
    /// 插件已注册的工具名（按 plugin_id 聚合）。
    pub(super) plugin_tools: Arc<DashMap<String, Vec<String>>>,
    /// 插件已注册的宿主事件监听 ID（按 plugin_id 聚合）。
    pub(super) plugin_event_listeners: Arc<DashMap<String, Vec<EventListenerId>>>,
    /// command/tool invoke 的宿主侧结果等待者：call_id -> oneshot sender。
    pub(super) command_waiters:
        Arc<DashMap<String, oneshot::Sender<Result<serde_json::Value, String>>>>,
}

impl HostApiDispatcher {
    /// 构造分发器；EventBus 必选，其余可选。
    /// Tokio Handle 默认通过 `Handle::try_current()` 自动获取；
    /// 可通过 `with_tokio_handle()` 显式注入。
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        let timeout = Duration::from_millis(DEFAULT_TOOLS_WEB_FETCH_TIMEOUT_MS);
        let mut options = OutboundClientOptions::new(None);
        options.use_public_ip_dns_resolver = true;
        options.redirect_policy = Some(reqwest::redirect::Policy::none());
        options.timeout = Some(timeout);
        options.connect_timeout = Some(default_connect_timeout_for(timeout));
        let fetch_client = build_outbound_client(
            options,
            OutboundClientErrorKind::Tool,
            "创建默认 net.fetch HTTP 客户端失败",
        )
        .expect("create default net.fetch client");
        Self {
            event_bus,
            primitive: None,
            tools: None,
            llm: None,
            llm_resolver: None,
            session: None,
            session_registry: Arc::new(DashMap::new()),
            audit: None,
            async_results: Arc::new(DashMap::new()),
            instance_calls: Arc::new(DashMap::new()),
            tokio_handle: Handle::try_current().ok(),
            async_timeout: Duration::from_secs(120),
            llm_semaphore: Arc::new(Semaphore::new(5)),
            fetch_client,
            fetch_proxy_mode_label: "system",
            fetch_timeout: timeout,
            fetch_semaphore: Arc::new(Semaphore::new(5)),
            fetch_max_body_bytes: DEFAULT_TOOLS_WEB_FETCH_MAX_HTTP_CONTENT_BYTES,
            plugin_manager: None,
            event_receivers: Arc::new(DashMap::new()),
            event_senders: Arc::new(DashMap::new()),
            ui_notify_count: None,
            command_completed_count: Arc::new(AtomicU32::new(0)),
            command_failed_count: Arc::new(AtomicU32::new(0)),
            plugin_commands: Arc::new(DashMap::new()),
            plugin_tools: Arc::new(DashMap::new()),
            plugin_event_listeners: Arc::new(DashMap::new()),
            command_waiters: Arc::new(DashMap::new()),
        }
    }

    /// 返回某插件在宿主侧登记的 `registerCommand` 元数据（不含 JS handler）。
    pub fn registered_plugin_commands(&self, instance_or_plugin_id: &str) -> Vec<(String, String)> {
        let plugin_id = instance_or_plugin_id
            .rsplit_once('/')
            .map(|(_, plugin_id)| plugin_id)
            .unwrap_or(instance_or_plugin_id);
        self.plugin_commands
            .get(plugin_id)
            .map(|e| e.value().clone())
            .unwrap_or_default()
    }

    pub fn registered_plugin_tools(&self, instance_or_plugin_id: &str) -> Vec<String> {
        let plugin_id = instance_or_plugin_id
            .rsplit_once('/')
            .map(|(_, plugin_id)| plugin_id)
            .unwrap_or(instance_or_plugin_id);
        self.plugin_tools
            .get(plugin_id)
            .map(|e| e.value().clone())
            .unwrap_or_default()
    }

    pub fn registered_plugin_listener_ids(
        &self,
        instance_or_plugin_id: &str,
    ) -> Vec<EventListenerId> {
        let plugin_id = instance_or_plugin_id
            .rsplit_once('/')
            .map(|(_, plugin_id)| plugin_id)
            .unwrap_or(instance_or_plugin_id);
        self.plugin_event_listeners
            .get(plugin_id)
            .map(|e| e.value().clone())
            .unwrap_or_default()
    }

    pub fn register_command_waiter(
        &self,
        call_id: &str,
    ) -> oneshot::Receiver<Result<serde_json::Value, String>> {
        let (tx, rx) = oneshot::channel();
        self.command_waiters.insert(call_id.to_string(), tx);
        rx
    }

    pub fn drop_command_waiter(&self, call_id: &str) {
        self.command_waiters.remove(call_id);
    }

    #[cfg(test)]
    pub(crate) fn command_waiter_count(&self) -> usize {
        self.command_waiters.len()
    }

    /// 是否在构造时捕获到 Tokio runtime handle。异步 hostcall（pi.fetch /
    /// createChatCompletion）依赖它；若为 false，插件后端会在发起请求前抛
    /// "async hostcall requires a Tokio runtime handle"。回归测试用。
    #[cfg(test)]
    pub(crate) fn has_tokio_handle(&self) -> bool {
        self.tokio_handle.is_some()
    }

    /// 注入 `uiNotify` 调用计数器（E2E / 集成测试）。
    pub fn with_ui_notify_counter(mut self, counter: Arc<AtomicU32>) -> Self {
        self.ui_notify_count = Some(counter);
        self
    }

    /// `commandCompleted` 累计调用次数（测试断言用）。
    pub fn command_completed_count(&self) -> u32 {
        self.command_completed_count.load(Ordering::SeqCst)
    }

    /// `commandFailed` 累计调用次数（测试断言用）。
    pub fn command_failed_count(&self) -> u32 {
        self.command_failed_count.load(Ordering::SeqCst)
    }

    /// 注入 4 原语执行器。
    pub fn with_primitive(mut self, p: Arc<dyn PrimitiveExecutor>) -> Self {
        self.primitive = Some(p);
        self
    }

    /// 注入工具注册中心。
    pub fn with_tools(mut self, t: Arc<dyn ToolRegistry>) -> Self {
        self.tools = Some(t);
        self
    }

    /// 注入 LLM Provider。
    pub fn with_llm(mut self, l: Arc<dyn LlmProvider>) -> Self {
        self.llm = Some(l);
        self
    }

    /// 注入 LLM Resolver（按显式 model 路由 provider）。
    pub fn with_llm_resolver(mut self, resolver: Arc<dyn LlmResolver>) -> Self {
        self.llm_resolver = Some(resolver);
        self
    }

    /// 注入 SessionManager（会话 API）。
    pub fn with_session(mut self, s: Arc<SessionManager>) -> Self {
        self.session = Some(s);
        self
    }

    pub fn bind_session(&self, session_id: &str, session: Weak<SessionManager>) {
        self.session_registry
            .insert(session_id.to_string(), session);
    }

    /// 注入审计记录器（每笔 Hostcall 记录）。
    pub fn with_audit(mut self, a: Arc<dyn AuditRecorder>) -> Self {
        self.audit = Some(a);
        self
    }

    /// 显式注入 Tokio Handle（覆盖构造时自动获取的值）。
    pub fn with_tokio_handle(mut self, h: Handle) -> Self {
        self.tokio_handle = Some(h);
        self
    }

    /// 设置异步 Hostcall 超时时长（默认 120s）。
    pub fn with_async_timeout(mut self, d: Duration) -> Self {
        self.async_timeout = d;
        self
    }

    /// 设置 LLM 最大并发请求数（默认 5）。
    pub fn with_llm_concurrency(mut self, max: usize) -> Self {
        self.llm_semaphore = Arc::new(Semaphore::new(max));
        self
    }

    pub fn with_fetch_http_client(mut self, client: reqwest::Client) -> Self {
        self.fetch_client = client;
        self
    }

    pub fn with_fetch_transport_diagnostics(
        mut self,
        timeout: Duration,
        explicit_proxy: bool,
        ambient_proxy: bool,
    ) -> Self {
        self.fetch_timeout = timeout;
        self.fetch_proxy_mode_label = if explicit_proxy {
            "explicit"
        } else if ambient_proxy {
            "env"
        } else {
            "direct"
        };
        self
    }

    pub fn with_fetch_concurrency(mut self, max: usize) -> Self {
        self.fetch_semaphore = Arc::new(Semaphore::new(max));
        self
    }

    pub fn with_fetch_max_body_bytes(mut self, max: usize) -> Self {
        self.fetch_max_body_bytes = max;
        self
    }

    pub fn with_plugin_manager(mut self, manager: Weak<PluginManager>) -> Self {
        self.plugin_manager = Some(manager);
        self
    }
}
