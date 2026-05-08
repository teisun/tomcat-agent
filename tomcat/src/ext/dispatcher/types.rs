use crate::core::{LlmProvider, PrimitiveExecutor, SessionManager, ToolRegistry};
use crate::ext::host_binding::HostResponse;
use crate::ext::vm_actor::EventEnvelope;
use crate::infra::event_bus::EventBus;
use crate::infra::AuditRecorder;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;

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
    pub(super) session: Option<Arc<SessionManager>>,
    pub(super) audit: Option<Arc<dyn AuditRecorder>>,
    pub(super) async_results: Arc<DashMap<String, AsyncCallStatus>>,
    /// instance_id -> [callId, ...] 映射，用于实例销毁时清理 pending 任务。
    pub(super) instance_calls: Arc<DashMap<String, Vec<String>>>,
    pub(super) tokio_handle: Option<Handle>,
    pub(super) async_timeout: Duration,
    pub(super) llm_semaphore: Arc<Semaphore>,
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
    /// 插件实例已注册的 slash 命令：(name, description)，handler 仅存于 JS `__pi_commands`。
    pub(super) plugin_commands: Arc<DashMap<String, Vec<(String, String)>>>,
}

impl HostApiDispatcher {
    /// 构造分发器；EventBus 必选，其余可选。
    /// Tokio Handle 默认通过 `Handle::try_current()` 自动获取；
    /// 可通过 `with_tokio_handle()` 显式注入。
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            event_bus,
            primitive: None,
            tools: None,
            llm: None,
            session: None,
            audit: None,
            async_results: Arc::new(DashMap::new()),
            instance_calls: Arc::new(DashMap::new()),
            tokio_handle: Handle::try_current().ok(),
            async_timeout: Duration::from_secs(30),
            llm_semaphore: Arc::new(Semaphore::new(5)),
            event_receivers: Arc::new(DashMap::new()),
            event_senders: Arc::new(DashMap::new()),
            ui_notify_count: None,
            command_completed_count: Arc::new(AtomicU32::new(0)),
            command_failed_count: Arc::new(AtomicU32::new(0)),
            plugin_commands: Arc::new(DashMap::new()),
        }
    }

    /// 返回某 Wasm 实例在宿主侧登记的 `registerCommand` 元数据（不含 JS handler）。
    pub fn registered_plugin_commands(&self, instance_id: &str) -> Vec<(String, String)> {
        self.plugin_commands
            .get(instance_id)
            .map(|e| e.value().clone())
            .unwrap_or_default()
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

    /// 注入 SessionManager（会话 API）。
    pub fn with_session(mut self, s: Arc<SessionManager>) -> Self {
        self.session = Some(s);
        self
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

    /// 设置异步 Hostcall 超时时长（默认 30s）。
    pub fn with_async_timeout(mut self, d: Duration) -> Self {
        self.async_timeout = d;
        self
    }

    /// 设置 LLM 最大并发请求数（默认 5）。
    pub fn with_llm_concurrency(mut self, max: usize) -> Self {
        self.llm_semaphore = Arc::new(Semaphore::new(max));
        self
    }
}
