//! # rquickjs 实例实现
//!
//! 每个插件实例持有独立 QuickJS 上下文；短生命周期脚本使用
//! `run_script` / `run_script_file`，长生命周期会话 VM 使用
//! `run_session_script` 持续运行到宿主发出 `__shutdown`。

use crate::ext::HostRequest;
use crate::infra::error::AppError;
use parking_lot::Mutex;
use rquickjs::function::{Async, Func};
use rquickjs::{AsyncContext, AsyncRuntime, Ctx, Function};
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use super::crypto_native::register_crypto_globals;
use super::engine_config::PluginEngineConfig;

const PI_RUNTIME_PRELUDE: &str = include_str!("../../../assets/js/pi_runtime_prelude.js");
const PI_CRYPTO_SHIM: &str = include_str!("../../../assets/js/pi_crypto_shim.js");
const EMBEDDED_BRIDGE_JS: &str = include_str!("../../../assets/js/pi_bridge.js");
const PI_TYPEBOX_SHIM: &str = include_str!("../../../assets/js/pi_typebox_shim.js");
const PI_NODE_SHIM: &str = include_str!("../../../assets/js/pi_node_shim.js");
const PI_MS_SHIM: &str = include_str!("../../../assets/js/pi_ms_shim.js");
const PI_MAIN_LOOP: &str = include_str!("../../../assets/js/pi_main_loop.js");

type HostInvokeFn = dyn Fn(&str) -> Result<String, AppError> + Send + Sync;

#[derive(Debug, Clone, Copy)]
enum InterruptReason {
    Cancelled,
    Timeout,
    BudgetExceeded,
}

#[derive(Clone)]
struct AgentExecutionControl {
    cancel: CancellationToken,
    waiting_on_host: Arc<dyn Fn() -> bool + Send + Sync>,
}

struct ExecutionGuardState {
    started_at: Mutex<Instant>,
    interrupt_count: AtomicU64,
    reason: Mutex<Option<InterruptReason>>,
    timeout: Duration,
    budget: u64,
    timeout_paused: AtomicBool,
    agent: Option<AgentExecutionControl>,
    host_wait_since: Mutex<Option<Instant>>,
}

impl ExecutionGuardState {
    fn new(timeout: Duration, budget: u64, agent: Option<AgentExecutionControl>) -> Self {
        Self {
            started_at: Mutex::new(Instant::now()),
            interrupt_count: AtomicU64::new(0),
            reason: Mutex::new(None),
            timeout,
            budget,
            timeout_paused: AtomicBool::new(false),
            agent,
            host_wait_since: Mutex::new(None),
        }
    }

    fn reset(&self) {
        *self.started_at.lock() = Instant::now();
        self.interrupt_count.store(0, Ordering::SeqCst);
        *self.reason.lock() = None;
    }

    fn pause_timeout(&self) {
        self.timeout_paused.store(true, Ordering::SeqCst);
    }

    fn resume_timeout(&self) {
        self.reset();
        self.timeout_paused.store(false, Ordering::SeqCst);
    }

    fn pause_for_idle_wait(&self) {
        self.pause_timeout();
        self.reset();
    }

    fn resume_after_idle_wait(&self) {
        self.resume_timeout();
    }

    // Only the agent-code runner calls these at Rust poll boundaries. Sleeping
    // while real host work is pending is not JS execution; runnable JS is never
    // exempted, even when a sibling host call remains pending.
    fn resume_from_host_wait(&self) {
        if let Some(since) = self.host_wait_since.lock().take() {
            let mut started = self.started_at.lock();
            if let Some(adjusted) = started.checked_add(since.elapsed()) {
                *started = adjusted;
            }
        }
    }

    fn pause_for_host_work(&self) {
        if self
            .agent
            .as_ref()
            .is_some_and(|agent| (agent.waiting_on_host)())
        {
            *self.host_wait_since.lock() = Some(Instant::now());
        }
    }

    fn stop_requested(&self) -> bool {
        if self
            .agent
            .as_ref()
            .is_some_and(|agent| agent.cancel.is_cancelled())
        {
            *self.reason.lock() = Some(InterruptReason::Cancelled);
            return true;
        }
        if self.timeout_paused.load(Ordering::SeqCst) {
            return false;
        }
        let now = self.host_wait_since.lock().unwrap_or_else(Instant::now);
        if !self.timeout.is_zero()
            && now.saturating_duration_since(*self.started_at.lock()) >= self.timeout
        {
            *self.reason.lock() = Some(InterruptReason::Timeout);
            return true;
        }
        false
    }

    fn should_interrupt(&self) -> bool {
        if self.stop_requested() {
            return true;
        }
        if self.timeout_paused.load(Ordering::SeqCst) {
            return false;
        }
        if self.budget > 0 && self.interrupt_count.fetch_add(1, Ordering::SeqCst) + 1 > self.budget
        {
            *self.reason.lock() = Some(InterruptReason::BudgetExceeded);
            return true;
        }
        false
    }

    fn reason_message(&self) -> Option<String> {
        match *self.reason.lock() {
            Some(InterruptReason::Cancelled) => Some("execution was cancelled".into()),
            Some(InterruptReason::Timeout) => Some(format!(
                "execution exceeded {}ms timeout",
                self.timeout.as_millis()
            )),
            Some(InterruptReason::BudgetExceeded) => Some(format!(
                "execution exceeded interrupt budget {}",
                self.budget
            )),
            None => None,
        }
    }
}

#[derive(Clone)]
struct HostBridge {
    plugin_id: String,
    host_invoke: Option<Arc<HostInvokeFn>>,
}

impl HostBridge {
    fn invoke(&self, request_json: &str) -> Result<String, AppError> {
        if let Some(invoke) = &self.host_invoke {
            return invoke(request_json);
        }

        let response = crate::ext::invoke_host_func(&self.plugin_id, request_json)?;
        serde_json::to_string(&response).map_err(AppError::from)
    }

    async fn wait_for_event(&self, timeout_ms: u64) -> Result<String, AppError> {
        let request = HostRequest {
            module: "__session".to_string(),
            method: "waitForEvent".to_string(),
            params: serde_json::json!({ "timeoutMs": timeout_ms }),
            call_id: None,
        };
        let request_json = serde_json::to_string(&request)?;
        let bridge = self.clone();
        tokio::task::spawn_blocking(move || bridge.invoke(&request_json))
            .await
            .map_err(|e| AppError::QuickJS(format!("waitForEvent join failed: {e}")))?
    }
}

/// 单插件独立 QuickJS 实例。
pub struct PluginVmInstance {
    #[allow(dead_code)]
    config: PluginEngineConfig,
    plugin_id: String,
    host_invoke: Option<Arc<HostInvokeFn>>,
}

impl fmt::Debug for PluginVmInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginVmInstance")
            .field("plugin_id", &self.plugin_id)
            .finish_non_exhaustive()
    }
}

impl PluginVmInstance {
    pub fn new(config: PluginEngineConfig, plugin_id: String) -> Result<Self, AppError> {
        Ok(Self {
            config,
            plugin_id,
            host_invoke: None,
        })
    }

    pub fn run_script(&mut self, code: &str) -> Result<serde_json::Value, AppError> {
        let (script_path, _guard) = temp_js_file(code)?;
        self.run_script_file(&script_path)
    }

    /// Executes one short-lived agent-authored JS snippet through the same QuickJS runtime,
    /// bridge, heap limit, wall timeout and interrupt budget used by plugins. The caller must
    /// install a host binding that implements `connector.callTool`.
    ///
    /// The snippet body runs inside an async function, so agent code may use `await callTool`.
    /// Its returned value is serialized as structured JSON rather than flattened into text;
    /// this preserves MCP image blocks for the agent-loop media splitter.
    pub fn run_agent_code(&mut self, code: &str) -> Result<serde_json::Value, AppError> {
        self.run_agent_code_with_cancel(code, CancellationToken::new(), || false)
    }

    /// Agent-only cancellation and host-wait ownership. Ordinary plugin runs
    /// keep their existing limits and event-loop behavior.
    pub(crate) fn run_agent_code_with_cancel(
        &mut self,
        code: &str,
        cancel: CancellationToken,
        waiting_on_host: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<serde_json::Value, AppError> {
        let wrapped = format!(
            r#"
globalThis.callTool = async function (name, arguments_) {{
  var response = await globalThis.__pi_hostCallAsync("connector", "callTool", {{
    name: name,
    arguments: arguments_ || {{}}
  }});
  if (!response || !response.ok) {{
    throw new Error((response && response.error) || "connector callTool failed");
  }}
  return response.data;
}};

(async function () {{
  try {{
    var value = await (async function () {{
{code}
    }})();
    globalThis.__tomcat_agent_code_result = JSON.stringify({{ ok: true, value: value }});
  }} catch (error) {{
    globalThis.__tomcat_agent_code_result = JSON.stringify({{
      ok: false,
      error: String((error && error.message) || error)
    }});
  }}
}})();
"#
        );
        let value = self.execute_script_with_control(
            &wrapped,
            Some(AgentExecutionControl {
                cancel,
                waiting_on_host: Arc::new(waiting_on_host),
            }),
        )?;
        let envelope = value.as_object().ok_or_else(|| {
            AppError::QuickJS("agent code returned invalid result envelope".into())
        })?;
        if envelope.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
            return Ok(envelope
                .get("value")
                .cloned()
                .unwrap_or(serde_json::Value::Null));
        }
        Err(AppError::QuickJS(
            envelope
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("agent code failed without an error message")
                .to_string(),
        ))
    }

    pub fn run_script_file(&mut self, path: &Path) -> Result<serde_json::Value, AppError> {
        if !path.exists() {
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("script file not found: {}", path.display()),
            )));
        }

        let combined = self.build_combined_script(path, false)?;
        self.execute_script(&combined)
    }

    /// 长生命周期会话 VM：持续运行直到宿主 `cleanup_instance` 注入 `__shutdown`。
    pub fn run_session_script(&mut self, path: &Path) -> Result<(), AppError> {
        if !path.exists() {
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("script file not found: {}", path.display()),
            )));
        }

        let combined = self.build_combined_script(path, true)?;
        self.execute_script(&combined).map(|_| ())
    }

    pub fn register_host_binding(
        &mut self,
        invoke_fn: impl Fn(&str) -> Result<String, AppError> + Send + Sync + 'static,
    ) -> Result<(), AppError> {
        self.host_invoke = Some(Arc::new(invoke_fn));
        Ok(())
    }

    #[deprecated(
        note = "Use PluginManager::dispatch_session_event with long-lived VM actor instead"
    )]
    pub fn dispatch_event(
        &mut self,
        plugin_script: &Path,
        event_type: &str,
        event_data: &serde_json::Value,
        context: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let envelope = serde_json::json!({
            "type": event_type,
            "data": event_data,
            "context": context,
        });
        let escaped = serde_json::to_string(&envelope)
            .map_err(|e| AppError::QuickJS(format!("event serialization: {e}")))?
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n");
        let mut combined = self.build_combined_script(plugin_script, false)?;
        combined.push_str("\n__pi_dispatch_event('");
        combined.push_str(&escaped);
        combined.push_str("');\n");
        self.execute_script(&combined)?;
        Ok(serde_json::Value::Null)
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn destroy(self) {}

    fn execute_script(&self, code: &str) -> Result<serde_json::Value, AppError> {
        self.execute_script_with_control(code, None)
    }

    fn execute_script_with_control(
        &self,
        code: &str,
        control: Option<AgentExecutionControl>,
    ) -> Result<serde_json::Value, AppError> {
        let bridge = HostBridge {
            plugin_id: self.plugin_id.clone(),
            host_invoke: self.host_invoke.clone(),
        };
        let plugin_id = self.plugin_id.clone();
        let code = code.to_string();
        let timeout = Duration::from_millis(self.config.call_timeout_ms);
        let interrupt_budget = self.config.interrupt_budget;
        let heap_limit_bytes = if self.config.quickjs_heap_mb == 0 {
            None
        } else {
            Some(self.config.quickjs_heap_mb as usize * 1024 * 1024)
        };
        let guard = Arc::new(ExecutionGuardState::new(
            timeout,
            interrupt_budget,
            control.clone(),
        ));

        run_with_local_runtime(move || async move {
            guard.reset();
            // Built-in bridge/shim bootstrap is host-owned overhead, not user/plugin work.
            guard.pause_timeout();
            let js_runtime = AsyncRuntime::new().map_err(to_app_js_error)?;
            if let Some(heap_limit_bytes) = heap_limit_bytes {
                js_runtime.set_memory_limit(heap_limit_bytes).await;
            }
            js_runtime
                .set_interrupt_handler(Some(Box::new({
                    let guard = guard.clone();
                    move || guard.should_interrupt()
                })))
                .await;

            let context = AsyncContext::full(&js_runtime)
                .await
                .map_err(to_app_js_error)?;

            if let Some(control) = control {
                let eval_guard = guard.clone();
                // Await only the snippet's top-level promise, not every timer or
                // unawaited call it ever spawned. Dropping this runtime clears
                // its scheduler; the Rust owner separately drains MCP children.
                let evaluation = context.async_with(async |ctx| {
                    install_host_globals(ctx.clone(), bridge, plugin_id, eval_guard.clone())?;
                    // Parse trusted bootstrap separately: guest declarations must not run
                    // before Rust starts the budget, or receive a way to reset it.
                    ctx.eval::<(), _>(build_runtime_prelude())?;
                    eval_guard.resume_timeout();
                    ctx.eval::<rquickjs::Promise<'_>, _>(code.as_str())?
                        .into_future::<()>()
                        .await
                });
                tokio::pin!(evaluation);
                let execution = std::future::poll_fn(|cx| {
                    guard.resume_from_host_wait();
                    let result = evaluation.as_mut().poll(cx);
                    if result.is_pending() {
                        guard.pause_for_host_work();
                    }
                    result
                });
                tokio::pin!(execution);
                let mut watchdog = tokio::time::interval(Duration::from_millis(25));
                loop {
                    tokio::select! {
                        biased;
                        _ = control.cancel.cancelled() => {
                            return Err(AppError::QuickJS("agent code execution was cancelled".into()));
                        }
                        result = &mut execution => {
                            result.map_err(|err| to_guarded_app_error(err, &guard))?;
                            break;
                        }
                        _ = watchdog.tick() => {
                            if guard.stop_requested() {
                                return Err(AppError::QuickJS(guard.reason_message().unwrap_or_else(|| "agent code interrupted".into())));
                            }
                        }
                    }
                }
            } else {
                context
                    .with(|ctx| -> rquickjs::Result<()> {
                        install_host_globals(
                            ctx.clone(),
                            bridge.clone(),
                            plugin_id.clone(),
                            guard.clone(),
                        )?;
                        ctx.eval::<(), _>(code.as_str())
                    })
                    .await
                    .map_err(|err| to_guarded_app_error(err, &guard))?;
                js_runtime.idle().await;
            }

            if let Some(reason) = guard.reason_message() {
                return Err(AppError::QuickJS(format!(
                    "plugin execution interrupted: {reason}"
                )));
            }

            let fatal_error = context
                .with(|ctx| {
                    ctx.globals()
                        .get::<_, Option<String>>("__pi_last_fatal_error")
                })
                .await
                .map_err(to_app_js_error)?;
            if let Some(fatal_error) = fatal_error.filter(|msg| !msg.is_empty()) {
                return Err(AppError::QuickJS(fatal_error));
            }

            let agent_code_result = context
                .with(|ctx| {
                    ctx.globals()
                        .get::<_, Option<String>>("__tomcat_agent_code_result")
                })
                .await
                .map_err(to_app_js_error)?;
            agent_code_result
                .map(|result| {
                    serde_json::from_str(&result).map_err(|error| {
                        AppError::QuickJS(format!(
                            "agent code result serialization failed: {error}"
                        ))
                    })
                })
                .transpose()
                .map(|result| result.unwrap_or(serde_json::Value::Null))
        })
    }

    fn build_combined_script(
        &self,
        user_script: &Path,
        include_main_loop: bool,
    ) -> Result<String, AppError> {
        let raw = std::fs::read_to_string(user_script).map_err(AppError::Io)?;
        let ext = user_script
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase());
        let user_code = match ext.as_deref() {
            Some("ts") | Some("tsx") => {
                let fname = user_script
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("plugin.ts");
                crate::ext::ts_compiler::transpile_pi_plugin_for_quickjs(&raw, fname)?
            }
            _ => raw,
        };

        let mut script = format!(
            "{prelude}// --- begin user budget window ---\nif (typeof __pi_resume_timeout === 'function') __pi_resume_timeout();\n\
             // --- user script ---\n{user_code}",
            prelude = build_runtime_prelude()
        );

        if include_main_loop && !user_code.contains("__pi_start_event_loop(") {
            script.push_str("\n// --- pi_main_loop.js ---\n");
            script.push_str(PI_MAIN_LOOP);
        }

        Ok(script)
    }
}

fn build_runtime_prelude() -> String {
    format!(
        "// --- pi_runtime_prelude.js (auto-injected) ---\n{PI_RUNTIME_PRELUDE}\n\
         // --- pi_crypto_shim.js ---\n{PI_CRYPTO_SHIM}\n\
         // --- pi_bridge.js (auto-injected) ---\n{bridge}\n\
         // --- pi_node_shim.js ---\n{PI_NODE_SHIM}\n\
         // --- pi_typebox_shim.js ---\n{PI_TYPEBOX_SHIM}\n\
         // --- pi_ms_shim.js ---\n{PI_MS_SHIM}\n",
        bridge = get_bridge_js_content()
    )
}

fn install_host_globals<'js>(
    ctx: Ctx<'js>,
    bridge: HostBridge,
    plugin_id: String,
    guard: Arc<ExecutionGuardState>,
) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    let print_plugin_id = plugin_id.clone();
    globals.set(
        "print",
        Func::from(move |text: String| -> rquickjs::Result<()> {
            tracing::info!(target: "plugin_vm", plugin_id = %print_plugin_id, "{text}");
            Ok(())
        }),
    )?;

    let sync_bridge = bridge.clone();
    globals.set(
        "__pi_host_call",
        Func::from(move |request_json: String| -> rquickjs::Result<String> {
            sync_bridge
                .invoke(&request_json)
                .map_err(|e| js_runtime_error(e.to_string()))
        }),
    )?;

    let sleep_fn = Function::new(
        ctx.clone(),
        Async(move |ms: u64| async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok::<(), rquickjs::Error>(())
        }),
    )?;
    globals.set("__pi_sleep", sleep_fn)?;

    register_crypto_globals(&globals)?;

    // Plugin event/timer ticks keep their per-event budget. Agent snippets instead
    // use the existing Rust poll-boundary accounting for their whole execution.
    if guard.agent.is_none() {
        let wait_bridge = bridge.clone();
        let wait_guard = guard.clone();
        let wait_fn = Function::new(
            ctx.clone(),
            Async(move |timeout_ms: u64| {
                let wait_bridge = wait_bridge.clone();
                let wait_guard = wait_guard.clone();
                async move {
                    // Waiting on the host event queue is idle time, not plugin execution.
                    wait_guard.pause_for_idle_wait();
                    let result = wait_bridge.wait_for_event(timeout_ms).await;
                    wait_guard.resume_after_idle_wait();
                    result.map_err(|e| js_runtime_error(e.to_string()))
                }
            }),
        )?;
        globals.set("__pi_wait_for_event", wait_fn)?;

        let budget_reset_guard = guard.clone();
        globals.set(
            "__pi_budget_reset",
            Func::from(move || -> rquickjs::Result<()> {
                budget_reset_guard.reset();
                Ok(())
            }),
        )?;

        let resume_timeout_guard = guard.clone();
        globals.set(
            "__pi_resume_timeout",
            Func::from(move || -> rquickjs::Result<()> {
                resume_timeout_guard.resume_timeout();
                Ok(())
            }),
        )?;
    }

    let interrupt_reason_guard = guard.clone();
    globals.set(
        "__pi_interrupt_reason",
        Func::from(move || -> rquickjs::Result<Option<String>> {
            Ok(interrupt_reason_guard.reason_message())
        }),
    )?;

    Ok(())
}

fn to_app_js_error(err: rquickjs::Error) -> AppError {
    AppError::QuickJS(err.to_string())
}

fn to_guarded_app_error(err: rquickjs::Error, guard: &ExecutionGuardState) -> AppError {
    if let Some(reason) = guard.reason_message() {
        return AppError::QuickJS(format!("plugin execution interrupted: {reason}"));
    }
    to_app_js_error(err)
}

fn js_runtime_error(message: impl Into<String>) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message("RustHost", "QuickJsHost", message.into())
}

fn temp_js_file(code: &str) -> Result<(PathBuf, tempfile::TempDir), AppError> {
    let dir = tempfile::tempdir().map_err(AppError::Io)?;
    let path = dir.path().join("script.js");
    std::fs::write(&path, code).map_err(AppError::Io)?;
    Ok((path, dir))
}

fn run_with_local_runtime<C, F, T>(build_future: C) -> Result<T, AppError>
where
    C: FnOnce() -> F + Send + 'static,
    F: Future<Output = Result<T, AppError>> + 'static,
    T: Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(AppError::Io)?;
            runtime.block_on(build_future())
        })
        .join()
        .map_err(|_| AppError::QuickJS("quickjs runtime worker panicked".to_string()))?;
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(AppError::Io)?;
    runtime.block_on(build_future())
}

fn get_bridge_js_content() -> std::borrow::Cow<'static, str> {
    if let Ok(path) = std::env::var("PI_BRIDGE_JS_PATH") {
        match std::fs::read_to_string(&path) {
            Ok(content) => return std::borrow::Cow::Owned(content),
            Err(e) => {
                tracing::warn!(
                    path = %path,
                    error = %e,
                    "PI_BRIDGE_JS_PATH set but file unreadable, falling back to embedded bridge"
                );
            }
        }
    }
    std::borrow::Cow::Borrowed(EMBEDDED_BRIDGE_JS)
}

#[cfg(test)]
#[path = "tests/instance_test.rs"]
mod tests;
