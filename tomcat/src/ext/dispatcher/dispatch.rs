//! # HostApiDispatcher 同步/异步分发主入口
//!
//! 宿主侧（Rust）对 WASM 插件 hostcall 的 **唯一入口**。同一个 `HostRequest` 进来，
//! 这里决定它是"立刻 block_on 出结果"还是"spawn 后台 task + 返回 pending 票据"，
//! 然后按 `(module, method)` 路由到 50+ 个 `do_*` 方法。
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────────────┐
//! │  HostApiDispatcher::dispatch(instance_id, HostRequest)  ← 唯一入口      │
//! └────────────────────────────────────────────────────────────────────────┘
//!    │
//!    │ ① 特例：(module="__session", method="waitForEvent") ──► do_wait_for_event
//!    │
//!    │ ② is_async = (call_id.is_some() && module != "__async")
//!    ▼
//!  ┌─ is_async? ──────────────────────────────────────────────────────────┐
//!  │   YES                                          NO                     │
//!  │   │                                            │                      │
//!  │   ▼                                            ▼                      │
//!  │ submit_async                          tokio_handle.block_on(           │
//!  │  spawn(dispatch_async)                  dispatch_async(...))           │
//!  │  async_results[call_id]=Pending                                        │
//!  │  instance_calls[inst].push                                             │
//!  │  立返 HostResponse::ok({pending:true})                                 │
//!  └───────────────────────────────────────────────────────────────────────┘
//!
//! ┌── dispatch_async：(module, method) → do_* 路由大表 ───────────────────┐
//! │                                                                        │
//! │  __async    poll                ─► do_async_poll                       │
//! │  log/agent  log|info|warn|...   ─► do_log                              │
//! │  fs|prim    readFile/writeFile/ ─► do_read_file / do_write_file /      │
//! │             editFile/executeBash    do_edit_file / do_execute_bash     │
//! │  llm        createChatCompletion─► do_chat / do_chat_stream /          │
//! │             [Stream] / get/setMod   do_llm_get_model / do_llm_set_model│
//! │  tools      register/unregister ─► do_register_tool / do_unregister_   │
//! │             /list/call/active/cmd   tool / do_list_tools / do_call_    │
//! │                                     tool / do_get_active_tools / ...   │
//! │  events     on/once/subscribe/  ─► do_events（subscribe → on 重写）    │
//! │             off/emit                                                   │
//! │  session    getCurrent/getMsgs/ ─► do_get_current_session /            │
//! │             getBranch/getLeaf/      do_get_messages /                  │
//! │             getEntry/sendMessage    do_session_get_branch / ...        │
//! │  agent      sendMessage/sendUser─► do_agent_send_message /             │
//! │                                     do_agent_send_user_message         │
//! │  context    isIdle/abort/getCwd ─► do_context_*（22 个 UI/状态方法）   │
//! │             getModel/uiNotify/                                         │
//! │             uiSelect/uiConfirm/...                                     │
//! │  其他       _                   ─► HostResponse::err("unknown API")    │
//! │                                                                        │
//! └────────────────────────────────────────────────────────────────────────┘
//!    │
//!    ▼ 旁路（无论 Ok/Err 都执行）
//! ┌──────────────────────────────────────────────────────────────────────┐
//! │  audit.record_hostcall(plugin_id, module, method, success, detail)   │
//! └──────────────────────────────────────────────────────────────────────┘
//!    │
//!    ▼
//!   Result<HostResponse, AppError>
//! ```
//!
//! ## 异步票据生命周期
//!
//! `submit_async` 在 `async_results: DashMap<call_id, AsyncCallStatus>` 上登记
//! `Pending`，并把 `call_id` push 到 `instance_calls[instance_id]`。后台任务完成后
//! 写回 `Completed/Failed`。插件侧通过 `__async.poll` 轮询取结果；插件卸载时由
//! `cleanup_instance` 按 `instance_calls` 反查全部 abort + 清理。
//!
//! ## 与同族子模块的边界
//!
//! - **本文件**：路由分发与异步票据账本。
//! - `ops.rs`：4 原语 `do_read_file / do_write_file / do_edit_file / do_execute_bash` 实现。
//! - `session_ops.rs`：会话/agent/上下文/事件类 `do_*` 实现。
//! - `helpers.rs`：审计 / async_results 辅助。
//! - `types.rs`：`HostApiDispatcher` 与 `AsyncCallStatus` 数据结构。

use super::types::{AsyncCallStatus, HostApiDispatcher};
use crate::ext::host_binding::{HostRequest, HostResponse};
use crate::ext::vm_actor::EventEnvelope;
use crate::infra::error::AppError;
use crate::infra::HostcallAuditEntry;
use std::collections::BTreeSet;
use std::sync::Arc;

impl HostApiDispatcher {
    pub(crate) fn session_instance_ids(&self, session_id: &str) -> Vec<String> {
        let prefix = format!("{session_id}/");
        let mut ids = BTreeSet::new();

        for entry in self.event_senders.iter() {
            let instance_id = entry.key();
            if instance_id.starts_with(&prefix) {
                ids.insert(instance_id.clone());
            }
        }
        for entry in self.event_receivers.iter() {
            let instance_id = entry.key();
            if instance_id.starts_with(&prefix) {
                ids.insert(instance_id.clone());
            }
        }
        for entry in self.instance_calls.iter() {
            let instance_id = entry.key();
            if instance_id.starts_with(&prefix) {
                ids.insert(instance_id.clone());
            }
        }

        ids.into_iter().collect()
    }

    /// 同步分发入口：
    /// - `request.call_id` 非空且 module != "__async" → 异步提交（spawn Tokio 任务，立即返回 `{pending: true}`）
    /// - 否则 → 同步路径（block_on dispatch_async）
    pub fn dispatch(
        &self,
        instance_id: &str,
        request: HostRequest,
    ) -> Result<HostResponse, AppError> {
        if request.module == "__session" && request.method == "waitForEvent" {
            return self.do_wait_for_event(instance_id, &request.params);
        }

        let is_async_poll = request.module == "__async";

        if !is_async_poll {
            if let Some(call_id) = request.call_id.clone() {
                return self.submit_async(instance_id, &call_id, request);
            }
        }

        if tokio::runtime::Handle::try_current().is_ok() {
            let dispatcher = self.clone();
            let inst_id = instance_id.to_string();
            return std::thread::spawn(move || dispatcher.block_on_dispatch(inst_id, request))
                .join()
                .map_err(|_| AppError::Plugin("sync hostcall worker panicked".into()))?;
        }

        self.block_on_dispatch(instance_id.to_string(), request)
    }

    fn block_on_dispatch(
        &self,
        instance_id: String,
        request: HostRequest,
    ) -> Result<HostResponse, AppError> {
        match &self.tokio_handle {
            Some(h) => h.block_on(self.dispatch_async(&instance_id, request)),
            None => {
                let rt = tokio::runtime::Runtime::new().expect("create runtime for sync dispatch");
                rt.block_on(self.dispatch_async(&instance_id, request))
            }
        }
    }

    /// 异步提交：spawn 后台 Tokio 任务，立即返回 `{pending: true}`。
    pub(super) fn submit_async(
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
    pub fn cleanup_instance(&self, instance_id: &str) {
        tracing::debug!("[cleanup_instance] {instance_id} start");
        if let Some(tx) = self.event_senders.get(instance_id) {
            let send_result = tx.try_send(EventEnvelope {
                event_type: "__shutdown".to_string(),
                data: serde_json::json!({}),
                context: serde_json::json!({}),
            });
            match &send_result {
                Ok(()) => {
                    tracing::debug!("[cleanup_instance] {instance_id} try_send __shutdown ok")
                }
                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                    tracing::warn!(
                        "[cleanup_instance] {instance_id} try_send __shutdown failed: channel full"
                    );
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    tracing::warn!(
                        "[cleanup_instance] {instance_id} try_send __shutdown failed: disconnected"
                    );
                }
            }
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

    /// 清理某插件在宿主侧登记的能力镜像（tools / commands / listeners 元数据）。
    pub fn cleanup_plugin_capabilities(&self, plugin_id: &str) {
        self.plugin_commands.remove(plugin_id);
        self.plugin_tools.remove(plugin_id);
        self.plugin_event_listeners.remove(plugin_id);
    }

    /// 为长生命周期 VM 注册事件 channel。
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
    pub(super) fn do_wait_for_event(
        &self,
        instance_id: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        use std::sync::mpsc::RecvTimeoutError;
        use std::time::Duration;

        let timeout_ms = params
            .get("timeoutMs")
            .and_then(|v| v.as_u64())
            .filter(|&ms| ms > 0);
        tracing::debug!(
            "[waitForEvent {instance_id}] enter timeout_ms={:?}",
            timeout_ms
        );

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
            ("fs" | "primitive", "taskOutput") => self.do_task_output(instance_id, &params).await,
            ("fs" | "primitive", "taskStop") => self.do_task_stop(instance_id, &params).await,
            ("net", "fetch") => self.do_fetch(instance_id, &params).await,
            ("llm", "createChatCompletion") => self.do_chat(instance_id, &params).await,
            ("llm", "createChatCompletionStream") => {
                self.do_chat_stream(instance_id, &params).await
            }
            ("llm", "getModel") => Ok(self.do_llm_get_model(instance_id)),
            ("llm", "setModel") => Ok(Self::do_llm_set_model(&params)),
            ("tools", "registerTool") => self.do_register_tool(instance_id, &params).await,
            ("tools", "unregisterTool") => self.do_unregister_tool(instance_id, &params).await,
            ("tools", "getToolList") => self.do_list_tools(instance_id, &params).await,
            ("tools", "callTool") => self.do_call_tool(instance_id, &params).await,
            ("tools", "getActiveTools") => self.do_get_active_tools(instance_id, &params).await,
            ("tools", "setActiveTools") => self.do_set_active_tools(instance_id, &params).await,
            ("tools", "registerCommand") | ("commands", "registerCommand") => {
                self.do_register_command(instance_id, &params).await
            }
            ("tools", "registerFlag") | ("tools", "registerShortcut") | ("tools", "getFlag") => {
                Ok(HostResponse::ok(serde_json::Value::Null))
            }
            ("session", "getSessionName") => Ok(HostResponse::ok(serde_json::json!({"name": ""}))),
            ("session", "setSessionName") | ("session", "appendEntry") => {
                Ok(HostResponse::ok(serde_json::Value::Null))
            }
            ("llm", "setThinkingLevel") => Ok(HostResponse::ok(serde_json::Value::Null)),
            ("events", "on")
            | ("events", "subscribe")
            | ("events", "once")
            | ("events", "off")
            | ("events", "emit") => {
                let effective_method = if method == "subscribe" { "on" } else { &method };
                self.do_events(instance_id, effective_method, &params).await
            }
            ("session" | "agent", "getCurrentSession") => {
                self.do_get_current_session(instance_id, &params).await
            }
            ("session", "getMessages") => self.do_get_messages(instance_id, &params).await,
            ("session", "getBranch") => self.do_session_get_branch(instance_id, &params),
            ("session", "getLeafEntry") => self.do_session_get_leaf_entry(instance_id),
            ("session", "getLeafId") => self.do_session_get_leaf_id(instance_id),
            ("session", "getEntry") => self.do_session_get_entry(instance_id, &params),
            ("session", "getHeader") => self.do_session_get_header(instance_id),
            ("session", "getEntries") => self.do_session_get_entries(instance_id, &params),
            ("session", "sendMessage") => self.do_send_message(instance_id, &params).await,
            ("agent", "sendMessage") => self.do_agent_send_message(instance_id, &params),
            ("agent", "sendUserMessage") => self.do_agent_send_user_message(instance_id, &params),
            ("context", "isIdle") => Ok(Self::do_context_is_idle()),
            ("context", "abort") => Ok(Self::do_context_abort()),
            ("context", "getCwd") => Ok(self.do_context_get_cwd(instance_id)),
            ("context", "getModel") => Ok(self.do_context_get_model(instance_id)),
            ("context", "uiNotify") => Ok(self.do_context_ui_notify(&params)),
            ("context", "uiSelect") => Ok(Self::do_context_ui_select(&params)),
            ("context", "uiConfirm") => Ok(Self::do_context_ui_confirm(&params)),
            ("context", "uiInput") => Ok(Self::do_context_ui_input(&params)),
            ("context", "uiSetStatus") => Ok(Self::do_context_ui_set_status(&params)),
            ("context", "commandCompleted") => Ok(self.do_command_completed(&params)),
            ("context", "commandFailed") => Ok(self.do_command_failed(&params)),
            ("context", "uiCustom") => Ok(Self::do_context_ui_custom(&params)),
            ("context", "uiSetWidget") => Ok(Self::do_context_ui_stub("uiSetWidget", &params)),
            ("context", "uiSetFooter") => Ok(Self::do_context_ui_stub("uiSetFooter", &params)),
            ("context", "uiSetHeader") => Ok(Self::do_context_ui_stub("uiSetHeader", &params)),
            ("context", "uiEditor") => Ok(Self::do_context_ui_editor(&params)),
            ("context", "getSystemPrompt") => Ok(Self::do_context_get_system_prompt()),
            ("context", "hasPendingMessages") => Ok(Self::do_context_has_pending()),
            ("context", "shutdown") => Ok(Self::do_context_shutdown()),
            ("context", "getContextUsage") => Ok(Self::do_context_usage()),
            ("context", "compact") => Ok(Self::do_context_compact()),
            ("context", "listModels") => Ok(Self::do_context_list_models()),
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

    pub(super) fn do_log(
        &self,
        _method: &str,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
        let msg = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
        tracing::debug!("[plugin log] {}", msg);
        Ok(HostResponse::ok(serde_json::Value::Null))
    }

    pub(super) fn do_async_poll(
        &self,
        params: &serde_json::Value,
    ) -> Result<HostResponse, AppError> {
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
                    let full_response = resp.clone();
                    drop(entry);
                    self.async_results.remove(call_id);
                    Ok(HostResponse::ok(serde_json::json!({
                        "ready": true,
                        "result": data,
                        "response": full_response,
                    })))
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
}
