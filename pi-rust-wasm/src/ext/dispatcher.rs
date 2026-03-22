//! # 宿主 API 统一分发器 (HostApiDispatcher)
//!
//! 单入口多路复用：根据 HostRequest 的 module/method 路由到对应 Processor。
//! 与 Architecture 宿主API层（host-api-layer）3.3 一致；支持 4 原语、LLM、工具、事件、会话 API。
//!
//! ## 结构示意
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                        HostApiDispatcher                                     │
//! ├─────────────────────────────────────────────────────────────────────────────┤
//! │ 注入的 Processor（Option）                                                    │
//! │   event_bus ──────► 事件 on/off/emit/once                                     │
//! │   primitive ──────► 4 原语 readFile/writeFile/editFile/executeBash           │
//! │   tools ───────────► 工具 register/call/list/getActive/setActive             │
//! │   llm ─────────────► LLM createChatCompletion / createChatCompletionStream    │
//! │   session ─────────► 会话 getCurrent/getMessages/sendMessage                 │
//! │   audit ───────────► 每笔 Hostcall 记录                                       │
//! ├─────────────────────────────────────────────────────────────────────────────┤
//! │ 异步基础设施                                                                  │
//! │   async_results: DashMap<callId, AsyncCallStatus>   ► 异步任务结果缓存        │
//! │   instance_calls: DashMap<instance_id, [callId]>   ► 实例→callId 映射（清理） │
//! │   tokio_handle ───► 共享 Runtime，同步路径 block_on / 异步路径 spawn          │
//! │   llm_semaphore ──► 限制 LLM 并发（默认 5）                                   │
//! │   async_timeout ──► 异步任务超时（默认 30s）                                  │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 调用流
//!
//! ```text
//!   Wasm __pi_host_call(request_json)
//!            │
//!            ▼
//!   ┌─────────────────────┐
//!   │  dispatch(instance_id, request)  │  同步入口
//!   └──────────┬──────────┘
//!              │
//!              ├── module == "__async" ？ ──否──► 有 call_id ？
//!              │         │                            │
//!              │        是                           是
//!              │         │                            ▼
//!              │         │                  submit_async() ──► 写 Pending，spawn 任务
//!              │         │                            │      立即返回 { pending: true }
//!              │         │                            │
//!              │         │                            │  后台: timeout(dispatch_async)
//!              │         │                            │         │
//!              │         │                            │         ▼ 结果写入 async_results
//!              │         │                            │
//!              │         └────────────────────────────┘
//!              │
//!              └── 同步路径: block_on( dispatch_async(instance_id, request) )
//!                                    │
//!                                    ▼
//!              ┌─────────────────────────────────────────────────────────┐
//!              │  dispatch_async: 按 (module, method) 路由                │
//!              │    __async.poll ──► do_async_poll (查 async_results)     │
//!              │    fs.* ──────────► do_read_file / do_write_file / ...   │
//!              │    llm.* ─────────► do_chat / do_chat_stream (带 Semaphore) │
//!              │    tools.* ───────► do_register_tool / do_call_tool / ... │
//!              │    events.* ──────► do_events (on/off/emit/once)          │
//!              │    session.* ─────► do_get_current_session / getMessages  │
//!              │    context.* ─────► do_context_* (isIdle/abort/cwd/...)   │
//!              │    agent.log ─────► do_log                               │
//!              └─────────────────────────────────────────────────────────┘
//!                                    │
//!                                    ▼
//!              可选 audit.record_hostcall() ──► 返回 HostResponse
//! ```
//!
//! ## 异步 submit/poll 时序
//!
//! ```text
//!  插件(JS)                 dispatch()              async_results         Tokio 任务
//!     │                        │                         │                    │
//!     │  hostCall(..., callId)  │                         │                    │
//!     │───────────────────────►│                         │                    │
//!     │                        │ insert(Pending)         │                    │
//!     │                        │────────────────────────►│                    │
//!     │                        │ spawn( timeout(dispatch_async) )             │
//!     │                        │─────────────────────────────────────────────►│
//!     │  { pending: true }     │                         │                    │
//!     │◄───────────────────────│                         │                    │
//!     │                        │                         │                    │
//!     │  __async.poll(callId)  │                         │                    │ 完成
//!     │───────────────────────►│ get(callId)             │                    │
//!     │                        │◄────────────────────────│ insert(Done/Error) │
//!     │  { ready, result }     │                         │                    │
//!     │◄───────────────────────│ remove(callId)          │                    │
//! ```

use crate::core::{
    ChatMessage, ChatRequest, EditOperation, LlmProvider, PrimitiveExecutor, SessionManager,
    StreamEvent, Tool, ToolRegistry,
};
use crate::infra::error::AppError;
use crate::infra::event_bus::{EventBus, EventContext, EventListenerId};
use crate::infra::{AuditRecorder, HostcallAuditEntry};
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use futures_util::StreamExt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;

use super::host_binding::{HostRequest, HostResponse};
use super::vm_actor::EventEnvelope;

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
    event_bus: Arc<dyn EventBus>,
    primitive: Option<Arc<dyn PrimitiveExecutor>>,
    tools: Option<Arc<dyn ToolRegistry>>,
    llm: Option<Arc<dyn LlmProvider>>,
    session: Option<Arc<SessionManager>>,
    audit: Option<Arc<dyn AuditRecorder>>,
    async_results: Arc<DashMap<String, AsyncCallStatus>>,
    /// instance_id -> [callId, ...] 映射，用于实例销毁时清理 pending 任务。
    instance_calls: Arc<DashMap<String, Vec<String>>>,
    tokio_handle: Option<Handle>,
    async_timeout: Duration,
    llm_semaphore: Arc<Semaphore>,
    /// 长生命周期 VM 的事件队列：instance_id -> event Receiver（Mutex 保证 Sync）。
    /// waitForEvent 路由从此 channel 阻塞接收事件。
    event_receivers:
        Arc<DashMap<String, Arc<std::sync::Mutex<std::sync::mpsc::Receiver<EventEnvelope>>>>>,
    /// 事件发送端：宿主通过此端投递事件给 VM。
    event_senders: Arc<DashMap<String, std::sync::mpsc::SyncSender<EventEnvelope>>>,
    /// 可选：`context.uiNotify` 调用次数（测试断言用，与生产逻辑无关）。
    ui_notify_count: Option<Arc<AtomicU32>>,
    /// 插件实例已注册的 slash 命令：(name, description)，handler 仅存于 JS `__pi_commands`。
    plugin_commands: Arc<DashMap<String, Vec<(String, String)>>>,
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

    /// 同步分发入口：
    /// - `request.call_id` 非空且 module != "__async" → 异步提交（spawn Tokio 任务，立即返回 `{pending: true}`）
    /// - 否则 → 同步路径（block_on dispatch_async）
    ///
    /// # Errors
    /// * 与 [`dispatch_async`] 相同。
    pub fn dispatch(
        &self,
        instance_id: &str,
        request: HostRequest,
    ) -> Result<HostResponse, AppError> {
        // __session.waitForEvent：同步阻塞等待事件（在 spawn_blocking 线程内调用）
        if request.module == "__session" && request.method == "waitForEvent" {
            return self.do_wait_for_event(instance_id, &request.params);
        }

        // __async.poll 始终走同步路径，不管是否携带 callId
        let is_async_poll = request.module == "__async";

        if !is_async_poll {
            if let Some(call_id) = request.call_id.clone() {
                return self.submit_async(instance_id, &call_id, request);
            }
        }

        // 同步路径：优先使用共享 Handle，fallback 到 Runtime::new()
        match &self.tokio_handle {
            Some(h) => h.block_on(self.dispatch_async(instance_id, request)),
            None => {
                let rt = tokio::runtime::Runtime::new().expect("create runtime for sync dispatch");
                rt.block_on(self.dispatch_async(instance_id, request))
            }
        }
    }

    /// 异步提交：spawn 后台 Tokio 任务，立即返回 `{pending: true}`。
    fn submit_async(
        &self,
        instance_id: &str,
        call_id: &str,
        request: HostRequest,
    ) -> Result<HostResponse, AppError> {
        let handle = self.tokio_handle.as_ref().ok_or_else(|| {
            AppError::Plugin("async hostcall requires a Tokio runtime handle".into())
        })?;

        self.async_results
            .insert(call_id.to_string(), AsyncCallStatus::Pending);

        self.instance_calls
            .entry(instance_id.to_string())
            .or_default()
            .push(call_id.to_string());

        let dispatcher = self.clone();
        let inst_id = instance_id.to_string();
        let cid = call_id.to_string();
        let timeout = self.async_timeout;

        handle.spawn(async move {
            let result =
                tokio::time::timeout(timeout, dispatcher.dispatch_async(&inst_id, request)).await;
            let status = match result {
                Ok(Ok(resp)) => AsyncCallStatus::Done(resp),
                Ok(Err(e)) => AsyncCallStatus::Error(e.to_string()),
                Err(_) => AsyncCallStatus::Error(format!(
                    "async hostcall timeout ({}s)",
                    timeout.as_secs()
                )),
            };
            dispatcher.async_results.insert(cid, status);
        });

        Ok(HostResponse {
            ok: true,
            data: Some(serde_json::json!({"pending": true})),
            error: None,
            call_id: Some(call_id.to_string()),
        })
    }

    /// 清理指定实例的所有 pending 异步任务（插件卸载/实例销毁时调用）。
    ///
    /// 先通过 event channel 发送 `__shutdown` 事件，让 JS 侧 `for(;;)` 事件循环
    /// 正常退出，再移除 channel 和 pending 任务。仅依赖 channel disconnection
    /// 不可靠（receiver Arc 可能被 `do_wait_for_event` 持有）。
    pub fn cleanup_instance(&self, instance_id: &str) {
        use super::vm_actor::EventEnvelope;
        tracing::debug!("[cleanup_instance] {instance_id} start");
        if let Some(tx) = self.event_senders.get(instance_id) {
            let send_result = tx.try_send(EventEnvelope {
                event_type: "__shutdown".to_string(),
                data: serde_json::json!({}),
                context: serde_json::json!({}),
            });
            tracing::debug!(
                "[cleanup_instance] {instance_id} try_send __shutdown ok={}",
                send_result.is_ok()
            );
        } else {
            tracing::warn!("[cleanup_instance] no event_sender for {instance_id}");
        }
        if let Some((_, call_ids)) = self.instance_calls.remove(instance_id) {
            for cid in call_ids {
                self.async_results.remove(&cid);
            }
        }
        self.event_receivers.remove(instance_id);
        self.event_senders.remove(instance_id);
        tracing::debug!("[cleanup_instance] {instance_id} channels removed");
    }

    /// 为长生命周期 VM 注册事件 channel。
    /// `instance_id` 格式建议为 `{session_id}/{plugin_id}`（与 VmRuntimeKey::Display 一致）。
    pub fn register_event_channel(
        &self,
        instance_id: &str,
        capacity: usize,
    ) -> std::sync::mpsc::SyncSender<EventEnvelope> {
        let (tx, rx) = std::sync::mpsc::sync_channel(capacity);
        self.event_receivers
            .insert(instance_id.to_string(), Arc::new(std::sync::Mutex::new(rx)));
        let tx_clone = tx.clone();
        self.event_senders.insert(instance_id.to_string(), tx);
        tx_clone
    }

    /// 获取事件发送端（宿主向 VM 投递事件）。
    pub fn get_event_sender(
        &self,
        instance_id: &str,
    ) -> Option<std::sync::mpsc::SyncSender<EventEnvelope>> {
        self.event_senders
            .get(instance_id)
            .map(|r| r.value().clone())
    }

    /// 阻塞等待事件（在 spawn_blocking 线程内调用，不会阻塞 Tokio worker）。
    /// 默认无限阻塞；JS 侧可传 `timeout_ms` 参数设置超时。
    /// 注意：必须克隆 Arc 后立即释放 get() 的 Ref，否则在 recv() 阻塞期间持有 DashMap 的 shard 锁，
    /// 会导致 end_session → cleanup_instance → event_receivers.remove() 死锁。
    fn do_wait_for_event(
        &self,
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        use std::sync::mpsc::RecvTimeoutError;
        use std::time::Duration;

        let rx_arc: Arc<std::sync::Mutex<std::sync::mpsc::Receiver<EventEnvelope>>> = self
            .event_receivers
            .get(instance_id)
            .ok_or_else(|| {
                AppError::Plugin(format!(
                    "no event channel registered for instance '{instance_id}'"
                ))
            })?
            .clone();

        let rx = rx_arc
            .lock()
            .map_err(|_| AppError::Plugin("event channel mutex poisoned".into()))?;

        let timeout_ms = params
            .get("timeoutMs")
            .and_then(|v| v.as_u64())
            .filter(|&ms| ms > 0);

        match timeout_ms {
            Some(ms) => match rx.recv_timeout(Duration::from_millis(ms)) {
                Ok(envelope) => {
                    tracing::debug!(
                        "[waitForEvent {instance_id}] event type={}",
                        envelope.event_type
                    );
                    let data = serde_json::to_value(&envelope)
                        .unwrap_or_else(|_| serde_json::json!({"type": "__error"}));
                    Ok(HostResponse::ok(data))
                }
                Err(RecvTimeoutError::Timeout) => {
                    Ok(HostResponse::ok(serde_json::json!({"type": "__tick"})))
                }
                Err(RecvTimeoutError::Disconnected) => {
                    tracing::debug!(
                        "[waitForEvent {instance_id}] channel disconnected → __shutdown"
                    );
                    Ok(HostResponse::ok(serde_json::json!({"type": "__shutdown"})))
                }
            },
            None => match rx.recv() {
                Ok(envelope) => {
                    tracing::debug!(
                        "[waitForEvent {instance_id}] event type={}",
                        envelope.event_type
                    );
                    let data = serde_json::to_value(&envelope)
                        .unwrap_or_else(|_| serde_json::json!({"type": "__error"}));
                    Ok(HostResponse::ok(data))
                }
                Err(_) => {
                    tracing::debug!(
                        "[waitForEvent {instance_id}] channel disconnected → __shutdown"
                    );
                    Ok(HostResponse::ok(serde_json::json!({"type": "__shutdown"})))
                }
            },
        }
    }

    /// 投递事件到指定 VM 的事件 channel（带回压：channel 满时返回错误而非阻塞）。
    pub fn deliver_event(
        &self,
        instance_id: &str,
        envelope: EventEnvelope,
    ) -> Result<(), AppError> {
        let tx = self.event_senders.get(instance_id).ok_or_else(|| {
            AppError::Plugin(format!("no event channel for instance '{instance_id}'"))
        })?;

        tx.try_send(envelope).map_err(|e| match e {
            std::sync::mpsc::TrySendError::Full(_) => AppError::Plugin(format!(
                "event channel full for '{instance_id}' (backpressure)"
            )),
            std::sync::mpsc::TrySendError::Disconnected(_) => {
                AppError::Plugin(format!("event channel closed for '{instance_id}'"))
            }
        })
    }

    /// 异步分发入口：按 module/method 路由，每笔 Hostcall 可选记录审计。
    ///
    /// # Errors
    /// * 返回的 `HostResponse` 中 `ok == false` 表示业务错误；未注入对应 Processor 时返回明确错误信息（如 "005"）。
    pub async fn dispatch_async(
        &self,
        instance_id: &str,
        request: HostRequest,
    ) -> Result<HostResponse, AppError> {
        let module = request.module.clone();
        let method = request.method.clone();
        let params = request.params.clone();

        let result = match (request.module.as_str(), request.method.as_str()) {
            ("__async", "poll") => self.do_async_poll(&params),
            ("log" | "agent", "log")
            | ("agent", "info")
            | ("agent", "warn")
            | ("agent", "error")
            | ("agent", "debug") => self.do_log(&method, &params),
            ("fs" | "primitive", "readFile") => self.do_read_file(instance_id, &params).await,
            ("fs" | "primitive", "writeFile") => self.do_write_file(instance_id, &params).await,
            ("fs" | "primitive", "editFile") => self.do_edit_file(instance_id, &params).await,
            ("fs" | "primitive", "executeBash") => self.do_execute_bash(instance_id, &params).await,
            ("llm", "createChatCompletion") => self.do_chat(instance_id, &params).await,
            ("llm", "createChatCompletionStream") => {
                self.do_chat_stream(instance_id, &params).await
            }
            ("llm", "getModel") => Ok(Self::do_llm_get_model()),
            ("llm", "setModel") => Ok(Self::do_llm_set_model(&params)),
            ("tools", "registerTool") => self.do_register_tool(instance_id, &params).await,
            ("tools", "unregisterTool") => self.do_unregister_tool(instance_id, &params).await,
            ("tools", "getToolList") => self.do_list_tools(instance_id, &params).await,
            ("tools", "callTool") => self.do_call_tool(instance_id, &params).await,
            ("tools", "getActiveTools") => self.do_get_active_tools(instance_id, &params).await,
            ("tools", "setActiveTools") => self.do_set_active_tools(instance_id, &params).await,
            ("tools", "registerCommand") => self.do_register_command(instance_id, &params).await,
            ("events", "on")
            | ("events", "subscribe")
            | ("events", "once")
            | ("events", "off")
            | ("events", "emit") => {
                let effective_method = if method == "subscribe" { "on" } else { &method };
                self.do_events(instance_id, effective_method, &params).await
            }
            ("session" | "agent", "getCurrentSession") => {
                self.do_get_current_session(&params).await
            }
            ("session", "getMessages") => self.do_get_messages(&params).await,
            ("session", "sendMessage") => self.do_send_message(&params).await,
            ("agent", "sendMessage") => self.do_agent_send_message(&params),
            ("agent", "sendUserMessage") => self.do_agent_send_user_message(&params),
            ("context", "isIdle") => Ok(Self::do_context_is_idle()),
            ("context", "abort") => Ok(Self::do_context_abort()),
            ("context", "getCwd") => Ok(Self::do_context_get_cwd()),
            ("context", "getModel") => Ok(Self::do_context_get_model()),
            ("context", "uiNotify") => Ok(self.do_context_ui_notify(&params)),
            ("context", "uiSelect") => Ok(Self::do_context_ui_select(&params)),
            ("context", "uiConfirm") => Ok(Self::do_context_ui_confirm(&params)),
            ("context", "uiInput") => Ok(Self::do_context_ui_input(&params)),
            ("context", "uiSetStatus") => Ok(Self::do_context_ui_set_status(&params)),
            ("context", "getSystemPrompt") => Ok(Self::do_context_get_system_prompt()),
            ("context", "hasPendingMessages") => Ok(Self::do_context_has_pending()),
            ("context", "shutdown") => Ok(Self::do_context_shutdown()),
            ("context", "getContextUsage") => Ok(Self::do_context_usage()),
            ("context", "compact") => Ok(Self::do_context_compact()),
            _ => Ok(HostResponse::err(format!(
                "unknown API: {}.{}",
                module, method
            ))),
        };

        let success = result.is_ok();
        let detail = result.as_ref().err().map(|e| e.to_string());
        let response = match &result {
            Ok(r) => r.clone(),
            Err(e) => HostResponse::err(e.to_string()),
        };

        if let Some(audit) = &self.audit {
            audit.record_hostcall(HostcallAuditEntry {
                plugin_id: instance_id.to_string(),
                module,
                method,
                success,
                detail,
            });
        }

        Ok(response)
    }

    fn do_log(&self, _method: &str, params: &serde_json::Value) -> Result<HostResponse, AppError> {
        let msg = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
        tracing::debug!("[plugin log] {}", msg);
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    fn do_async_poll(&self, params: &serde_json::Value) -> Result<HostResponse, AppError> {
        let call_id = params
            .get("callId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("__async.poll: missing callId".into()))?;
        match self.async_results.get(call_id) {
            Some(entry) => match entry.value() {
                AsyncCallStatus::Pending => {
                    Ok(HostResponse::ok(serde_json::json!({"ready": false})))
                }
                AsyncCallStatus::Done(resp) => {
                    let data = resp.data.clone();
                    drop(entry);
                    self.async_results.remove(call_id);
                    Ok(HostResponse::ok(
                        serde_json::json!({"ready": true, "result": data}),
                    ))
                }
                AsyncCallStatus::Error(e) => {
                    let err = e.clone();
                    drop(entry);
                    self.async_results.remove(call_id);
                    Ok(HostResponse::err(err))
                }
            },
            None => Ok(HostResponse::err(format!("unknown callId: {call_id}"))),
        }
    }

    async fn do_read_file(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let p = match &self.primitive {
            None => return Ok(HostResponse::err("PrimitiveExecutor not configured (005)")),
            Some(exec) => exec,
        };
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("readFile: missing path".to_string()))?;
        let content = p.read_file(path, plugin_id).await?;
        Ok(HostResponse::ok(serde_json::json!({ "content": content })))
    }

    async fn do_write_file(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let p = match &self.primitive {
            None => return Ok(HostResponse::err("PrimitiveExecutor not configured (005)")),
            Some(exec) => exec,
        };
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("writeFile: missing path".to_string()))?;
        let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let overwrite = params
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let result = p.write_file(path, content, overwrite, plugin_id).await?;
        Ok(HostResponse::ok(
            serde_json::to_value(result).map_err(AppError::Serialize)?,
        ))
    }

    async fn do_edit_file(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let p = match &self.primitive {
            None => return Ok(HostResponse::err("PrimitiveExecutor not configured (005)")),
            Some(exec) => exec,
        };
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("editFile: missing path".to_string()))?;
        let edits: Vec<EditOperation> = params
            .get("edits")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let result = p.edit_file(path, edits, plugin_id).await?;
        Ok(HostResponse::ok(
            serde_json::to_value(result).map_err(AppError::Serialize)?,
        ))
    }

    async fn do_execute_bash(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let p = match &self.primitive {
            None => return Ok(HostResponse::err("PrimitiveExecutor not configured (005)")),
            Some(exec) => exec,
        };
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("executeBash: missing command".to_string()))?;
        let cwd = params.get("cwd").and_then(|v| v.as_str()).map(String::from);
        let argv_store: Option<Vec<String>> =
            params.get("args").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            });
        let argv_ref = argv_store.as_deref();
        let result = p
            .execute_bash(command, cwd.as_deref(), plugin_id, argv_ref)
            .await?;
        Ok(HostResponse::ok(
            serde_json::to_value(result).map_err(AppError::Serialize)?,
        ))
    }

    async fn do_chat(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let llm = match &self.llm {
            None => return Ok(HostResponse::err("LlmProvider not configured (004)")),
            Some(l) => l,
        };
        let _permit = self
            .llm_semaphore
            .acquire()
            .await
            .map_err(|_| AppError::Plugin("LLM semaphore closed".into()))?;
        let req = parse_chat_request(params)?;
        let resp = llm.chat(req).await?;
        Ok(HostResponse::ok(
            serde_json::to_value(resp).map_err(AppError::Serialize)?,
        ))
    }

    async fn do_chat_stream(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let llm = match &self.llm {
            None => return Ok(HostResponse::err("LlmProvider not configured (004)")),
            Some(l) => l,
        };
        let _permit = self
            .llm_semaphore
            .acquire()
            .await
            .map_err(|_| AppError::Plugin("LLM semaphore closed".into()))?;
        let req = parse_chat_request(params)?;
        let mut stream = llm.chat_stream(req).await?;
        let mut content = String::new();
        while let Some(ev) = stream.next().await {
            let ev = ev?;
            if let StreamEvent::ContentDelta { delta } = ev {
                content.push_str(&delta);
            }
        }
        Ok(HostResponse::ok(serde_json::json!({ "content": content })))
    }

    async fn do_register_tool(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let tool = parse_tool(params, plugin_id)?;
        tools.register_tool(tool, plugin_id).await?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    async fn do_unregister_tool(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let name = params
            .get("toolName")
            .or_else(|| params.get("tool_name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("unregisterTool: missing toolName".to_string()))?;
        tools.unregister_tool(name, plugin_id).await?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    async fn do_list_tools(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let filter_plugin = params.get("pluginId").and_then(|v| v.as_str());
        let list = tools.list_tools(filter_plugin).await?;
        Ok(HostResponse::ok(
            serde_json::to_value(list).map_err(AppError::Serialize)?,
        ))
    }

    async fn do_call_tool(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let name = params
            .get("toolName")
            .or_else(|| params.get("tool_name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("callTool: missing toolName".to_string()))?;
        let tool_params = params
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let result = tools.call_tool(name, tool_params, plugin_id).await?;
        Ok(HostResponse::ok(result))
    }

    /// 返回当前已启用的工具名列表（与 pi-mono getActiveTools 对齐）。
    async fn do_get_active_tools(
        &self,
        _plugin_id: &str,
        _params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let list = tools.list_tools(None).await?;
        let names: Vec<&str> = list.iter().map(|t| t.name.as_str()).collect();
        Ok(HostResponse::ok(
            serde_json::to_value(names).map_err(AppError::Serialize)?,
        ))
    }

    /// 设置活跃工具集（按名称过滤启用/禁用）。MVP 阶段仅返回确认，不实际变更状态。
    async fn do_set_active_tools(
        &self,
        _plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let _tools = match &self.tools {
            None => return Ok(HostResponse::err("ToolRegistry not configured (006)")),
            Some(t) => t,
        };
        let _tool_names = params
            .get("toolNames")
            .or_else(|| params.get("tool_names"))
            .and_then(|v| v.as_array());
        // MVP: 接受请求但不实际变更工具启用状态，后续迭代实现完整的 active/inactive 切换。
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    /// 注册命令（与 pi-mono ExtensionAPI.registerCommand 对齐）。宿主侧仅存元数据；handler 在 JS `__pi_commands`。
    async fn do_register_command(
        &self,
        plugin_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("registerCommand: missing name".to_string()))?;
        let description = params
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        tracing::debug!(
            "[registerCommand] plugin={} cmd={} desc={}",
            plugin_id,
            name,
            description
        );
        match self.plugin_commands.entry(plugin_id.to_string()) {
            Entry::Occupied(mut ent) => {
                let v = ent.get_mut();
                if let Some(i) = v.iter().position(|(n, _)| n == name) {
                    v[i] = (name.to_string(), description.to_string());
                } else {
                    v.push((name.to_string(), description.to_string()));
                }
            }
            Entry::Vacant(ent) => {
                ent.insert(vec![(name.to_string(), description.to_string())]);
            }
        }
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    async fn do_events(
        &self,
        plugin_id: &str,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let event_name = params
            .get("eventName")
            .or_else(|| params.get("event_name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Plugin("events: missing eventName".to_string()))?;
        match method {
            "on" => {
                // 宿主侧注册占位回调；实际 JS 回调由 __pi_dispatch_event 触发 pi_bridge.js 中的 __pi_hooks。
                // TODO: 长生命周期 VM 就绪后，此处应注入真实回调（通过 WasmInstance 回调到插件 JS）。
                let id = self.event_bus.on(event_name, Box::new(|_| Ok(())));
                Ok(HostResponse::ok(serde_json::json!({ "listenerId": id.0 })))
            }
            "once" => {
                let id = self.event_bus.once(event_name, Box::new(|_| Ok(())));
                Ok(HostResponse::ok(serde_json::json!({ "listenerId": id.0 })))
            }
            "off" => {
                let id = params
                    .get("listenerId")
                    .or_else(|| params.get("listener_id"))
                    .and_then(|v| v.as_u64())
                    .map(EventListenerId)
                    .ok_or_else(|| {
                        AppError::Plugin("events.off: missing listenerId".to_string())
                    })?;
                self.event_bus.off(id);
                Ok(HostResponse::ok(serde_json::Value::Null))
            }
            "emit" => {
                let payload = params
                    .get("payload")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let ctx = EventContext::new(event_name, payload).with_plugin_id(plugin_id);
                self.event_bus.emit_sync(event_name, ctx)?;
                Ok(HostResponse::ok(serde_json::Value::Null))
            }
            _ => Ok(HostResponse::err(format!(
                "events: unknown method {}",
                method
            ))),
        }
    }

    async fn do_get_current_session(
        &self,
        _params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let key = session.current_session_key();
        let entry = session.get_session(key)?;
        let data = match entry {
            Some(e) => serde_json::to_value(e).map_err(AppError::Serialize)?,
            None => serde_json::Value::Null,
        };
        Ok(HostResponse::ok(data))
    }

    async fn do_get_messages(&self, params: &serde_json::Value) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let cap = params.get("cap").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let entries = session.get_entries(cap)?;
        let list: Vec<serde_json::Value> = entries
            .into_iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();
        Ok(HostResponse::ok(serde_json::json!(list)))
    }

    // -- agent module: sendMessage / sendUserMessage -----------------------
    fn do_agent_send_message(&self, params: &serde_json::Value) -> Result<HostResponse, AppError> {
        let Some(session) = &self.session else {
            tracing::debug!(
                "[plugin sendMessage] no SessionManager, message={:?}",
                params.get("message")
            );
            return Ok(HostResponse::ok(serde_json::Value::Null));
        };
        if params
            .get("options")
            .and_then(|o| o.get("silent"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            tracing::debug!("[plugin sendMessage] silent=true, skip transcript append");
            return Ok(HostResponse::ok(serde_json::Value::Null));
        }
        let wire = agent_send_message_wire(params)?;
        session.append_message(wire)?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    fn do_agent_send_user_message(
        &self,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let Some(session) = &self.session else {
            tracing::debug!(
                "[plugin sendUserMessage] no SessionManager, content={:?}",
                params.get("content")
            );
            return Ok(HostResponse::ok(serde_json::Value::Null));
        };
        if params
            .get("options")
            .and_then(|o| o.get("silent"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(HostResponse::ok(serde_json::Value::Null));
        }
        let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let role = params
            .get("options")
            .and_then(|o| o.get("role"))
            .and_then(|v| v.as_str())
            .unwrap_or("user");
        session.append_message(serde_json::json!({ "role": role, "content": content }))?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    // -- context module (for pi_bridge.js ctx proxy) ----------------------
    fn do_context_is_idle() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "idle": true }))
    }

    fn do_context_abort() -> HostResponse {
        tracing::debug!("[context] abort requested by plugin");
        HostResponse::ok(serde_json::Value::Null)
    }

    fn do_context_get_cwd() -> HostResponse {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        HostResponse::ok(serde_json::json!({ "cwd": cwd }))
    }

    fn do_context_get_model() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "model": serde_json::Value::Null }))
    }

    /// Returns the currently configured LLM model name.
    /// MVP: per-instance model selection is not yet stored; returns null.
    /// Future: maintain a per-instance model override map.
    fn do_llm_get_model() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "model": serde_json::Value::Null }))
    }

    /// Acknowledges a model-switch request from a plugin.
    /// MVP: the request is logged but does not alter the active LlmProvider.
    /// Future: maintain a per-instance model override map.
    fn do_llm_set_model(params: &serde_json::Value) -> HostResponse {
        let model = params.get("model").and_then(|v| v.as_str()).unwrap_or("");
        tracing::debug!("[llm.setModel] plugin requested model={} (MVP stub)", model);
        HostResponse::ok(serde_json::json!({ "model": model }))
    }

    fn do_context_ui_notify(&self, params: &serde_json::Value) -> HostResponse {
        if let Some(c) = &self.ui_notify_count {
            c.fetch_add(1, Ordering::SeqCst);
        }
        let msg = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let kind = params
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("info");
        tracing::debug!("[context.ui.notify] [{}] {}", kind, msg);
        HostResponse::ok(serde_json::Value::Null)
    }

    /// pi-mono `ctx.ui.select`：无 TTY 时返回确定性默认（首项），便于扩展逻辑与 E2E 断言。
    fn do_context_ui_select(params: &serde_json::Value) -> HostResponse {
        let options = params
            .get("options")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let title = params.get("title").and_then(|v| v.as_str()).unwrap_or("");
        tracing::debug!(
            "[context.ui.select] title={} option_count={}",
            title,
            options.len()
        );
        let (selected_index, selected, cancelled) = if let Some(first) = options.first() {
            (0_i64, first.clone(), false)
        } else {
            (-1_i64, serde_json::Value::Null, true)
        };
        HostResponse::ok(serde_json::json!({
            "selectedIndex": selected_index,
            "selected": selected,
            "cancelled": cancelled
        }))
    }

    fn do_context_ui_confirm(params: &serde_json::Value) -> HostResponse {
        let title = params.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let message = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
        tracing::debug!(
            "[context.ui.confirm] title={} message_len={}",
            title,
            message.len()
        );
        HostResponse::ok(serde_json::json!({ "confirmed": true }))
    }

    fn do_context_ui_input(params: &serde_json::Value) -> HostResponse {
        let placeholder = params
            .get("placeholder")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        tracing::debug!("[context.ui.input] placeholder_len={}", placeholder.len());
        HostResponse::ok(serde_json::json!({ "value": "" }))
    }

    fn do_context_ui_set_status(params: &serde_json::Value) -> HostResponse {
        let message = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let details = params
            .get("details")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        tracing::debug!("[context.ui.setStatus] {} details={}", message, details);
        HostResponse::ok(serde_json::Value::Null)
    }

    fn do_context_get_system_prompt() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "prompt": "" }))
    }

    fn do_context_has_pending() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "pending": false }))
    }

    fn do_context_shutdown() -> HostResponse {
        tracing::warn!("[context] shutdown requested by plugin");
        HostResponse::ok(serde_json::Value::Null)
    }

    fn do_context_usage() -> HostResponse {
        HostResponse::ok(serde_json::json!({ "tokens": null, "contextWindow": 0, "percent": null }))
    }

    fn do_context_compact() -> HostResponse {
        tracing::debug!("[context] compact requested by plugin");
        HostResponse::ok(serde_json::Value::Null)
    }

    async fn do_send_message(&self, params: &serde_json::Value) -> Result<HostResponse, AppError> {
        let session = match &self.session {
            None => return Ok(HostResponse::err("SessionManager not configured")),
            Some(s) => s,
        };
        let message = params
            .get("message")
            .cloned()
            .ok_or_else(|| AppError::Plugin("sendMessage: missing message".to_string()))?;
        session.append_message(message)?;
        Ok(HostResponse::ok(serde_json::Value::Null))
    }
}

/// `agent.sendMessage` → 当前会话 transcript  wire 格式（role + content）。
fn agent_send_message_wire(params: &serde_json::Value) -> Result<serde_json::Value, AppError> {
    let opts = params.get("options").and_then(|v| v.as_object());
    let role_default = opts
        .and_then(|o| o.get("role"))
        .and_then(|v| v.as_str())
        .unwrap_or("user");
    let message = params
        .get("message")
        .ok_or_else(|| AppError::Plugin("sendMessage: missing message".into()))?;
    if let Some(obj) = message.as_object() {
        let role = obj
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or(role_default);
        let content = obj
            .get("content")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        return Ok(serde_json::json!({ "role": role, "content": content }));
    }
    if let Some(s) = message.as_str() {
        return Ok(serde_json::json!({ "role": role_default, "content": s }));
    }
    Ok(serde_json::json!({ "role": role_default, "content": message }))
}

/// 规整 TypeBox / 包装型 `parameters` 为 JSON Schema 风格，便于 LLM tools。
pub(crate) fn normalize_tool_parameters(params: &serde_json::Value) -> serde_json::Value {
    match params {
        serde_json::Value::Null => serde_json::json!({
            "type": "object",
            "properties": {}
        }),
        serde_json::Value::Object(map) => {
            if map.len() == 1 {
                if let Some(inner) = map.get("schema") {
                    return normalize_tool_parameters(inner);
                }
            }
            let mut out = params.clone();
            if let Some(o) = out.as_object_mut() {
                o.remove("default");
                let has_shape = o.contains_key("type")
                    || o.contains_key("properties")
                    || o.contains_key("anyOf")
                    || o.contains_key("oneOf")
                    || o.contains_key("allOf")
                    || o.contains_key("items")
                    || o.contains_key("enum")
                    || o.contains_key("const");
                if has_shape {
                    return out;
                }
                if o.is_empty() {
                    return serde_json::json!({ "type": "object", "properties": {} });
                }
                return serde_json::json!({ "type": "object", "properties": out.clone() });
            }
            serde_json::json!({ "type": "object", "properties": {} })
        }
        _ => serde_json::json!({ "type": "object", "properties": {} }),
    }
}

fn parse_chat_request(params: &serde_json::Value) -> Result<ChatRequest, AppError> {
    let messages: Vec<ChatMessage> = params
        .get("messages")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let model = params
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    Ok(ChatRequest {
        messages,
        model,
        temperature: params
            .get("temperature")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32),
        max_tokens: params
            .get("maxTokens")
            .or_else(|| params.get("max_tokens"))
            .and_then(|v| v.as_u64())
            .map(|u| u as u32),
        stream: params.get("stream").and_then(|v| v.as_bool()),
        model_override: None,
        tools: None,
    })
}

fn parse_tool(params: &serde_json::Value, plugin_id: &str) -> Result<Tool, AppError> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Plugin("registerTool: missing name".to_string()))?
        .to_string();
    let label = params
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or(&name)
        .to_string();
    let description = params
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let raw_params = params
        .get("parameters")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let parameters = normalize_tool_parameters(&raw_params);
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(Tool {
        name,
        label,
        description,
        parameters,
        plugin_id: plugin_id.to_string(),
        is_enabled: true,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        BashResult, ChatResponse, ChatResponseChoice, DirEntry, EditFileResult, EditOperation,
        PrimitiveOperation, WriteFileResult,
    };
    use crate::infra::wire;
    use crate::infra::DefaultEventBus;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn dispatch_unknown_api_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "unknown".to_string(),
            method: "foo".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("unknown API"));
    }

    #[tokio::test]
    async fn dispatch_log_succeeds() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "agent".to_string(),
            method: "log".to_string(),
            params: serde_json::json!({ "message": "hello" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_read_file_without_primitive_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "fs".to_string(),
            method: "readFile".to_string(),
            params: serde_json::json!({ "path": "/tmp/x", "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("005"));
    }

    #[tokio::test]
    async fn dispatch_session_get_current_without_session_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "session".to_string(),
            method: "getCurrentSession".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("SessionManager not configured"));
    }

    #[tokio::test]
    async fn dispatch_events_on_returns_listener_id() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "events".to_string(),
            method: "on".to_string(),
            params: serde_json::json!({ "eventName": "test_event" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
        let data = res.data.unwrap();
        assert!(data.get("listenerId").is_some());
    }

    #[tokio::test]
    async fn dispatch_events_emit_succeeds() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "events".to_string(),
            method: "emit".to_string(),
            params: serde_json::json!({ "eventName": "ev", "payload": {} }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_with_audit_records_hostcall() {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        struct CountAudit;
        impl AuditRecorder for CountAudit {
            fn record_primitive(&self, _: crate::infra::PrimitiveAuditEntry) {}
            fn record_tool_call(&self, _: crate::infra::ToolAuditEntry) {}
            fn record_hostcall(&self, _: crate::infra::HostcallAuditEntry) {
                COUNT.fetch_add(1, Ordering::SeqCst);
            }
            fn record_plugin_lifecycle(&self, _: crate::infra::PluginLifecycleAuditEntry) {}
        }
        let bus = Arc::new(DefaultEventBus::new());
        let audit = Arc::new(CountAudit);
        let d = HostApiDispatcher::new(bus).with_audit(audit);
        let req = HostRequest {
            module: "agent".to_string(),
            method: "log".to_string(),
            params: serde_json::json!({ "message": "audit test" }),
            call_id: None,
        };
        let _ = d.dispatch_async("inst-1", req).await.unwrap();
        assert_eq!(COUNT.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dispatch_tools_without_registry_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "tools".to_string(),
            method: "getToolList".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("006"));
    }

    #[tokio::test]
    async fn dispatch_llm_without_provider_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "llm".to_string(),
            method: "createChatCompletion".to_string(),
            params: serde_json::json!({ "messages": [], "model": "gpt-4" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("004"));
    }

    struct MockPrimitive;
    #[async_trait::async_trait]
    impl PrimitiveExecutor for MockPrimitive {
        async fn read_file(&self, _path: &str, _plugin_id: &str) -> Result<String, AppError> {
            Ok("mock_content".to_string())
        }
        async fn list_dir(&self, _path: &str, _plugin_id: &str) -> Result<Vec<DirEntry>, AppError> {
            Ok(vec![])
        }
        async fn write_file(
            &self,
            path: &str,
            _content: &str,
            _overwrite: bool,
            _plugin_id: &str,
        ) -> Result<WriteFileResult, AppError> {
            Ok(WriteFileResult {
                path: path.to_string(),
                written: true,
            })
        }
        async fn edit_file(
            &self,
            path: &str,
            _edits: Vec<EditOperation>,
            _plugin_id: &str,
        ) -> Result<EditFileResult, AppError> {
            Ok(EditFileResult {
                path: path.to_string(),
                applied: true,
            })
        }
        async fn execute_bash(
            &self,
            _command: &str,
            _cwd: Option<&str>,
            _plugin_id: &str,
            _argv: Option<&[String]>,
        ) -> Result<BashResult, AppError> {
            Ok(BashResult {
                stdout: "ok".to_string(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
        async fn require_user_confirmation(
            &self,
            _op: PrimitiveOperation,
            _preview: &str,
            _plugin_id: &str,
        ) -> Result<bool, AppError> {
            Ok(true)
        }
    }

    struct MockLlm;
    #[async_trait::async_trait]
    impl LlmProvider for MockLlm {
        fn provider_name(&self) -> &str {
            "mock"
        }
        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, AppError> {
            Ok(ChatResponse {
                id: Some("id".to_string()),
                choices: vec![ChatResponseChoice {
                    index: 0,
                    message: ChatMessage::assistant("hi"),
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            })
        }
        async fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> Result<
            Box<dyn futures_util::Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>,
            AppError,
        > {
            use futures_util::stream;
            Ok(Box::new(stream::iter(vec![Ok(
                StreamEvent::ContentDelta {
                    delta: "hi".to_string(),
                },
            )])))
        }
        fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
            Ok(0)
        }
    }

    struct MockToolRegistry;
    #[async_trait::async_trait]
    impl ToolRegistry for MockToolRegistry {
        async fn register_tool(&self, _tool: Tool, _plugin_id: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn unregister_tool(&self, _name: &str, _plugin_id: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn get_tool(&self, _name: &str) -> Result<Tool, AppError> {
            Err(AppError::Tool("not found".to_string()))
        }
        async fn list_tools(&self, _plugin_id: Option<&str>) -> Result<Vec<Tool>, AppError> {
            Ok(vec![])
        }
        async fn call_tool(
            &self,
            _name: &str,
            _params: serde_json::Value,
            _plugin_id: &str,
        ) -> Result<serde_json::Value, AppError> {
            Ok(serde_json::json!({ "content": "ok", "details": null }))
        }
        fn unregister_plugin_tools(&self, _plugin_id: &str) {}
    }

    #[tokio::test]
    async fn dispatch_read_file_with_primitive_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_primitive(Arc::new(MockPrimitive));
        let req = HostRequest {
            module: "fs".to_string(),
            method: "readFile".to_string(),
            params: serde_json::json!({ "path": "/tmp/x", "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
        assert_eq!(
            res.data
                .as_ref()
                .and_then(|d| d.get("content").and_then(|c| c.as_str())),
            Some("mock_content")
        );
    }

    #[tokio::test]
    async fn dispatch_write_file_with_primitive_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_primitive(Arc::new(MockPrimitive));
        let req = HostRequest {
            module: "fs".to_string(),
            method: "writeFile".to_string(),
            params: serde_json::json!({ "path": "/tmp/x", "content": "body", "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_edit_file_with_primitive_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_primitive(Arc::new(MockPrimitive));
        let req = HostRequest {
            module: "fs".to_string(),
            method: "editFile".to_string(),
            params: serde_json::json!({ "path": "/tmp/x", "edits": [], "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_execute_bash_with_primitive_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_primitive(Arc::new(MockPrimitive));
        let req = HostRequest {
            module: "fs".to_string(),
            method: "executeBash".to_string(),
            params: serde_json::json!({ "command": "echo x", "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_execute_bash_with_argv_calls_primitive() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let ran = Arc::new(AtomicBool::new(false));
        let ran2 = Arc::clone(&ran);
        #[derive(Clone)]
        struct ArgvPrimitive(Arc<AtomicBool>);
        #[async_trait::async_trait]
        impl PrimitiveExecutor for ArgvPrimitive {
            async fn read_file(&self, _p: &str, _id: &str) -> Result<String, AppError> {
                Ok(String::new())
            }
            async fn list_dir(&self, _p: &str, _id: &str) -> Result<Vec<DirEntry>, AppError> {
                Ok(vec![])
            }
            async fn write_file(
                &self,
                _p: &str,
                _c: &str,
                _o: bool,
                _id: &str,
            ) -> Result<WriteFileResult, AppError> {
                Ok(WriteFileResult {
                    path: String::new(),
                    written: false,
                })
            }
            async fn edit_file(
                &self,
                _p: &str,
                _e: Vec<EditOperation>,
                _id: &str,
            ) -> Result<EditFileResult, AppError> {
                Ok(EditFileResult {
                    path: String::new(),
                    applied: false,
                })
            }
            async fn execute_bash(
                &self,
                cmd: &str,
                _cwd: Option<&str>,
                _id: &str,
                argv: Option<&[String]>,
            ) -> Result<BashResult, AppError> {
                if cmd == "echo" {
                    if let Some(a) = argv {
                        if a.len() == 2 && a[0] == "a" && a[1] == "b" {
                            self.0.store(true, Ordering::SeqCst);
                        }
                    }
                }
                Ok(BashResult {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                })
            }
            async fn require_user_confirmation(
                &self,
                _op: PrimitiveOperation,
                _prev: &str,
                _id: &str,
            ) -> Result<bool, AppError> {
                Ok(true)
            }
        }
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_primitive(Arc::new(ArgvPrimitive(ran2)));
        let req = HostRequest {
            module: "fs".to_string(),
            method: "executeBash".to_string(),
            params: serde_json::json!({
                "command": "echo",
                "args": ["a", "b"],
                "pluginId": "p1"
            }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-argv", req).await.unwrap();
        assert!(res.ok);
        assert!(
            ran.load(Ordering::SeqCst),
            "execute_bash 应收到 argv 模式参数"
        );
    }

    #[tokio::test]
    async fn dispatch_register_command_records_metadata() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "tools".to_string(),
            method: "registerCommand".to_string(),
            params: serde_json::json!({ "name": "my-cmd", "description": "desc" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-rc", req).await.unwrap();
        assert!(res.ok);
        let cmds = d.registered_plugin_commands("inst-rc");
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].0, "my-cmd");
        assert_eq!(cmds[0].1, "desc");
    }

    #[test]
    fn normalize_tool_parameters_unwraps_schema() {
        let raw = serde_json::json!({
            "schema": {
                "type": "object",
                "properties": { "q": { "type": "string" } }
            }
        });
        let n = normalize_tool_parameters(&raw);
        assert_eq!(n.get("type").and_then(|v| v.as_str()), Some("object"));
        assert!(n.get("properties").is_some());
    }

    #[tokio::test]
    async fn dispatch_chat_with_llm_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_llm(Arc::new(MockLlm));
        let req = HostRequest {
            module: "llm".to_string(),
            method: "createChatCompletion".to_string(),
            params: serde_json::json!({ "messages": [], "model": "gpt-4" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_chat_stream_with_llm_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_llm(Arc::new(MockLlm));
        let req = HostRequest {
            module: "llm".to_string(),
            method: "createChatCompletionStream".to_string(),
            params: serde_json::json!({ "messages": [], "model": "gpt-4" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
        assert!(res
            .data
            .as_ref()
            .and_then(|d| d.get("content").and_then(|c| c.as_str()))
            .is_some());
    }

    #[tokio::test]
    async fn dispatch_register_tool_with_registry_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "registerTool".to_string(),
            params: serde_json::json!({ "name": "t1", "label": "T1", "description": "d", "parameters": {} }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_list_tools_with_registry_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "getToolList".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
        assert!(res.data.as_ref().map(|d| d.is_array()).unwrap_or(false));
    }

    #[tokio::test]
    async fn dispatch_call_tool_with_registry_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "callTool".to_string(),
            params: serde_json::json!({ "toolName": "t1", "params": {} }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_session_get_current_with_session_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let key = mgr.current_session_key();
        let _ = mgr.create_session(key, None).unwrap();
        let d = HostApiDispatcher::new(bus).with_session(Arc::new(mgr));
        let req = HostRequest {
            module: "session".to_string(),
            method: "getCurrentSession".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_get_messages_with_session_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let key = mgr.current_session_key();
        let _ = mgr.create_session(key, None).unwrap();
        let d = HostApiDispatcher::new(bus).with_session(Arc::new(mgr));
        let req = HostRequest {
            module: "session".to_string(),
            method: "getMessages".to_string(),
            params: serde_json::json!({ "cap": 5 }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
        assert!(res.data.as_ref().map(|d| d.is_array()).unwrap_or(false));
    }

    #[tokio::test]
    async fn dispatch_send_message_with_session_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let dir = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(dir.path().to_path_buf());
        let key = mgr.current_session_key();
        let _ = mgr.create_session(key, None).unwrap();
        let d = HostApiDispatcher::new(bus).with_session(Arc::new(mgr));
        let req = HostRequest {
            module: "session".to_string(),
            method: "sendMessage".to_string(),
            params: serde_json::json!({ "message": { "role": "user", "content": { "text": "hi" } } }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_unregister_tool_with_registry_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "unregisterTool".to_string(),
            params: serde_json::json!({ "toolName": "t1", "pluginId": "p1" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_events_once_returns_listener_id() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "events".to_string(),
            method: "once".to_string(),
            params: serde_json::json!({ "eventName": "test" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
        let id = res
            .data
            .as_ref()
            .and_then(|d| d.get("listenerId"))
            .and_then(|v| v.as_u64());
        assert!(id.is_some());
    }

    #[tokio::test]
    async fn dispatch_events_off_removes_listener() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let on_req = HostRequest {
            module: "events".to_string(),
            method: "on".to_string(),
            params: serde_json::json!({ "eventName": "e1" }),
            call_id: None,
        };
        let on_res = d.dispatch_async("inst-1", on_req).await.unwrap();
        assert!(on_res.ok);
        let listener_id = on_res
            .data
            .as_ref()
            .and_then(|d| d.get("listenerId"))
            .and_then(|v| v.as_u64())
            .expect("listenerId");
        let off_req = HostRequest {
            module: "events".to_string(),
            method: "off".to_string(),
            params: serde_json::json!({ "eventName": "e1", "listenerId": listener_id }),
            call_id: None,
        };
        let off_res = d.dispatch_async("inst-1", off_req).await.unwrap();
        assert!(off_res.ok);
    }

    #[tokio::test]
    async fn dispatch_chat_parses_max_tokens_and_temperature() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_llm(Arc::new(MockLlm));
        let req = HostRequest {
            module: "llm".to_string(),
            method: "createChatCompletion".to_string(),
            params: serde_json::json!({
                "messages": [],
                "model": "m",
                "maxTokens": 100,
                "temperature": 0.7
            }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_register_tool_missing_name_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "registerTool".to_string(),
            params: serde_json::json!({ "label": "L", "description": "d" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
        assert!(res
            .error
            .as_ref()
            .map(|e| e.contains("name"))
            .unwrap_or(false));
    }

    #[tokio::test]
    async fn dispatch_get_active_tools_with_registry_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "getActiveTools".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_set_active_tools_with_registry_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tools(Arc::new(MockToolRegistry));
        let req = HostRequest {
            module: "tools".to_string(),
            method: "setActiveTools".to_string(),
            params: serde_json::json!({ "toolNames": ["tool_a", "tool_b"] }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_register_command_returns_ok() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "tools".to_string(),
            method: "registerCommand".to_string(),
            params: serde_json::json!({ "name": "myCmd", "description": "test command" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(res.ok);
    }

    #[tokio::test]
    async fn dispatch_register_command_missing_name_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let req = HostRequest {
            module: "tools".to_string(),
            method: "registerCommand".to_string(),
            params: serde_json::json!({ "description": "no name" }),
            call_id: None,
        };
        let res = d.dispatch_async("inst-1", req).await.unwrap();
        assert!(!res.ok);
    }

    // ========== Async Hostcall Tests (8.4.8) ==========

    fn make_dispatcher_with_primitive() -> HostApiDispatcher {
        let bus = Arc::new(DefaultEventBus::new());
        HostApiDispatcher::new(bus)
            .with_tokio_handle(Handle::current())
            .with_primitive(Arc::new(MockPrimitive))
    }

    #[tokio::test]
    async fn async_submit_poll_full_roundtrip() {
        let d = make_dispatcher_with_primitive();
        let req = HostRequest {
            module: "fs".to_string(),
            method: "executeBash".to_string(),
            params: serde_json::json!({"command": "echo hi"}),
            call_id: Some("req-1".to_string()),
        };
        let submit = d.dispatch("inst-a", req).unwrap();
        assert!(submit.ok);
        assert_eq!(submit.call_id.as_deref(), Some("req-1"));
        assert!(submit
            .data
            .as_ref()
            .unwrap()
            .get("pending")
            .unwrap()
            .as_bool()
            .unwrap());

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let poll_req = HostRequest {
            module: "__async".to_string(),
            method: "poll".to_string(),
            params: serde_json::json!({"callId": "req-1"}),
            call_id: None,
        };
        let poll_res = d.dispatch_async("inst-a", poll_req).await.unwrap();
        assert!(poll_res.ok);
        let data = poll_res.data.unwrap();
        assert!(data.get("ready").unwrap().as_bool().unwrap());
        assert!(data.get("result").is_some());
    }

    #[tokio::test]
    async fn sync_path_unchanged_without_call_id() {
        let d = make_dispatcher_with_primitive();
        let res = tokio::task::spawn_blocking(move || {
            let req = HostRequest {
                module: "fs".to_string(),
                method: "executeBash".to_string(),
                params: serde_json::json!({"command": "echo hi"}),
                call_id: None,
            };
            d.dispatch("inst-a", req)
        })
        .await
        .unwrap()
        .unwrap();
        assert!(res.ok);
        assert!(res.data.as_ref().unwrap().get("stdout").is_some());
    }

    #[tokio::test]
    async fn async_poll_not_ready_immediately() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
        d.async_results
            .insert("pending-1".to_string(), AsyncCallStatus::Pending);
        let poll_req = HostRequest {
            module: "__async".to_string(),
            method: "poll".to_string(),
            params: serde_json::json!({"callId": "pending-1"}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
        assert!(res.ok);
        assert!(!res.data.unwrap().get("ready").unwrap().as_bool().unwrap());
    }

    #[tokio::test]
    async fn async_poll_ready_returns_result() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
        d.async_results.insert(
            "done-1".to_string(),
            AsyncCallStatus::Done(HostResponse::ok(serde_json::json!({"stdout": "hello"}))),
        );
        let poll_req = HostRequest {
            module: "__async".to_string(),
            method: "poll".to_string(),
            params: serde_json::json!({"callId": "done-1"}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
        assert!(res.ok);
        let data = res.data.unwrap();
        assert!(data.get("ready").unwrap().as_bool().unwrap());
        let result = data.get("result").unwrap();
        assert_eq!(result.get("stdout").unwrap().as_str().unwrap(), "hello");
    }

    #[tokio::test]
    async fn async_poll_error_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
        d.async_results.insert(
            "err-1".to_string(),
            AsyncCallStatus::Error("something broke".to_string()),
        );
        let poll_req = HostRequest {
            module: "__async".to_string(),
            method: "poll".to_string(),
            params: serde_json::json!({"callId": "err-1"}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("something broke"));
    }

    #[tokio::test]
    async fn async_timeout_produces_error() {
        let bus = Arc::new(DefaultEventBus::new());
        // Slow mock: sleeps longer than timeout
        struct SlowPrimitive;
        #[async_trait::async_trait]
        impl PrimitiveExecutor for SlowPrimitive {
            async fn read_file(&self, _: &str, _: &str) -> Result<String, AppError> {
                Ok(String::new())
            }
            async fn list_dir(&self, _: &str, _: &str) -> Result<Vec<DirEntry>, AppError> {
                Ok(vec![])
            }
            async fn write_file(
                &self,
                _: &str,
                _: &str,
                _: bool,
                _: &str,
            ) -> Result<WriteFileResult, AppError> {
                Ok(WriteFileResult {
                    path: String::new(),
                    written: false,
                })
            }
            async fn edit_file(
                &self,
                _: &str,
                _: Vec<EditOperation>,
                _: &str,
            ) -> Result<EditFileResult, AppError> {
                Ok(EditFileResult {
                    path: String::new(),
                    applied: false,
                })
            }
            async fn execute_bash(
                &self,
                _: &str,
                _: Option<&str>,
                _: &str,
                _: Option<&[String]>,
            ) -> Result<BashResult, AppError> {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                Ok(BashResult {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                })
            }
            async fn require_user_confirmation(
                &self,
                _: PrimitiveOperation,
                _: &str,
                _: &str,
            ) -> Result<bool, AppError> {
                Ok(true)
            }
        }
        let d = HostApiDispatcher::new(bus)
            .with_tokio_handle(Handle::current())
            .with_primitive(Arc::new(SlowPrimitive))
            .with_async_timeout(std::time::Duration::from_millis(100));
        let req = HostRequest {
            module: "fs".to_string(),
            method: "executeBash".to_string(),
            params: serde_json::json!({"command": "slow"}),
            call_id: Some("timeout-1".to_string()),
        };
        d.dispatch("inst-a", req).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let poll_req = HostRequest {
            module: "__async".to_string(),
            method: "poll".to_string(),
            params: serde_json::json!({"callId": "timeout-1"}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("timeout"));
    }

    #[tokio::test]
    async fn async_multiple_call_ids_concurrent() {
        let d = make_dispatcher_with_primitive();
        for i in 0..5 {
            let req = HostRequest {
                module: "fs".to_string(),
                method: "executeBash".to_string(),
                params: serde_json::json!({"command": format!("echo {i}")}),
                call_id: Some(format!("multi-{i}")),
            };
            let submit = d.dispatch("inst-a", req).unwrap();
            assert!(submit.ok);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        for i in 0..5 {
            let poll_req = HostRequest {
                module: "__async".to_string(),
                method: "poll".to_string(),
                params: serde_json::json!({"callId": format!("multi-{i}")}),
                call_id: None,
            };
            let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
            assert!(res.ok);
            assert!(res.data.unwrap().get("ready").unwrap().as_bool().unwrap());
        }
    }

    #[tokio::test]
    async fn async_cleanup_instance_removes_pending() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
        d.async_results
            .insert("ci-1".to_string(), AsyncCallStatus::Pending);
        d.async_results
            .insert("ci-2".to_string(), AsyncCallStatus::Pending);
        d.instance_calls
            .entry("inst-x".to_string())
            .or_default()
            .extend(["ci-1".to_string(), "ci-2".to_string()]);
        // Also add one for a different instance to ensure it's not removed
        d.async_results
            .insert("other-1".to_string(), AsyncCallStatus::Pending);
        d.instance_calls
            .entry("inst-y".to_string())
            .or_default()
            .push("other-1".to_string());

        d.cleanup_instance("inst-x");

        assert!(d.async_results.get("ci-1").is_none());
        assert!(d.async_results.get("ci-2").is_none());
        assert!(d.async_results.get("other-1").is_some());
    }

    #[tokio::test]
    async fn async_poll_cleans_up_after_ready() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
        d.async_results.insert(
            "once-1".to_string(),
            AsyncCallStatus::Done(HostResponse::ok(serde_json::json!({"v": 42}))),
        );
        let poll_req = || HostRequest {
            module: "__async".to_string(),
            method: "poll".to_string(),
            params: serde_json::json!({"callId": "once-1"}),
            call_id: None,
        };
        let res1 = d.dispatch_async("inst-a", poll_req()).await.unwrap();
        assert!(res1.ok);
        assert!(res1.data.unwrap().get("ready").unwrap().as_bool().unwrap());

        let res2 = d.dispatch_async("inst-a", poll_req()).await.unwrap();
        assert!(!res2.ok);
        assert!(res2.error.unwrap().contains("unknown callId"));
    }

    #[tokio::test]
    async fn async_poll_missing_call_id_returns_err() {
        let bus = Arc::new(DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus).with_tokio_handle(Handle::current());
        let poll_req = HostRequest {
            module: "__async".to_string(),
            method: "poll".to_string(),
            params: serde_json::json!({}),
            call_id: None,
        };
        let res = d.dispatch_async("inst-a", poll_req).await.unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("missing callId"));
    }

    #[test]
    fn register_event_channel_and_deliver() {
        let bus = Arc::new(crate::infra::DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        d.register_event_channel("s1/p1", 4);

        let envelope = EventEnvelope {
            event_type: "test_event".into(),
            data: serde_json::json!({"key": "val"}),
            context: serde_json::json!({}),
        };
        d.deliver_event("s1/p1", envelope).unwrap();
    }

    #[test]
    fn deliver_event_backpressure() {
        let bus = Arc::new(crate::infra::DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        d.register_event_channel("s1/p1", 2);

        for _ in 0..2 {
            d.deliver_event(
                "s1/p1",
                EventEnvelope {
                    event_type: "x".into(),
                    data: serde_json::json!(null),
                    context: serde_json::json!(null),
                },
            )
            .unwrap();
        }

        let r = d.deliver_event(
            "s1/p1",
            EventEnvelope {
                event_type: "overflow".into(),
                data: serde_json::json!(null),
                context: serde_json::json!(null),
            },
        );
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("backpressure"));
    }

    #[test]
    fn wait_for_event_receives_delivered_event() {
        let bus = Arc::new(crate::infra::DefaultEventBus::new());
        let d = Arc::new(HostApiDispatcher::new(bus));
        d.register_event_channel("s1/p1", 4);

        d.deliver_event(
            "s1/p1",
            EventEnvelope {
                event_type: wire::vm::WIRE_SESSION_START.into(),
                data: serde_json::json!({"sid": "s1"}),
                context: serde_json::json!({}),
            },
        )
        .unwrap();

        let resp = d
            .do_wait_for_event("s1/p1", &serde_json::json!({}))
            .unwrap();
        assert!(resp.ok);
        assert_eq!(
            resp.data.as_ref().unwrap()["type"].as_str().unwrap(),
            wire::vm::WIRE_SESSION_START
        );
    }

    #[test]
    fn wait_for_event_channel_closed_returns_shutdown() {
        let bus = Arc::new(crate::infra::DefaultEventBus::new());
        let d = Arc::new(HostApiDispatcher::new(bus));
        d.register_event_channel("s1/p1", 4);

        // Drop the sender side to close the channel
        d.event_senders.remove("s1/p1");

        let resp = d
            .do_wait_for_event("s1/p1", &serde_json::json!({}))
            .unwrap();
        assert!(resp.ok);
        assert_eq!(resp.data.as_ref().unwrap()["type"], "__shutdown");
    }

    #[test]
    fn cleanup_instance_removes_event_channels() {
        let bus = Arc::new(crate::infra::DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        d.register_event_channel("s1/p1", 4);
        assert!(d.get_event_sender("s1/p1").is_some());

        d.cleanup_instance("s1/p1");
        assert!(d.get_event_sender("s1/p1").is_none());
    }

    #[test]
    fn deliver_event_no_channel_returns_err() {
        let bus = Arc::new(crate::infra::DefaultEventBus::new());
        let d = HostApiDispatcher::new(bus);
        let r = d.deliver_event(
            "nonexistent",
            EventEnvelope {
                event_type: "x".into(),
                data: serde_json::json!(null),
                context: serde_json::json!(null),
            },
        );
        assert!(r.is_err());
    }
}
