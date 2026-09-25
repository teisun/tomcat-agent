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
        let (path, _file_guard) = temp_js_file(&wrapped)?;
        let combined = self.build_combined_script(&path, false)?;
        let value = self.execute_script_with_control(
            &combined,
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
                    install_host_globals(ctx.clone(), bridge, plugin_id, eval_guard)?;
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
            "// --- pi_runtime_prelude.js (auto-injected) ---\n{PI_RUNTIME_PRELUDE}\n\
             // --- pi_crypto_shim.js ---\n{PI_CRYPTO_SHIM}\n\
             // --- pi_bridge.js (auto-injected) ---\n{bridge}\n\
             // --- pi_node_shim.js ---\n{PI_NODE_SHIM}\n\
             // --- pi_typebox_shim.js ---\n{PI_TYPEBOX_SHIM}\n\
             // --- pi_ms_shim.js ---\n{PI_MS_SHIM}\n\
             // --- begin user budget window ---\nif (typeof __pi_resume_timeout === 'function') __pi_resume_timeout();\n\
             // --- user script ---\n{user_code}",
            bridge = get_bridge_js_content()
        );

        if include_main_loop && !user_code.contains("__pi_start_event_loop(") {
            script.push_str("\n// --- pi_main_loop.js ---\n");
            script.push_str(PI_MAIN_LOOP);
        }

        Ok(script)
    }
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
    register_crypto_globals(&globals)?;

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
mod tests {
    use super::{PluginEngineConfig, PluginVmInstance};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn run_script_file_reports_missing_path() {
        let mut instance =
            PluginVmInstance::new(PluginEngineConfig::default(), "missing-script".to_string())
                .expect("create quickjs instance");
        let missing = std::path::Path::new("/definitely/missing/plugin.js");
        let err = instance
            .run_script_file(missing)
            .expect_err("missing script should fail");
        assert!(
            err.to_string().contains("script file not found"),
            "missing script error should mention path resolution: {err}"
        );
    }

    #[test]
    fn run_agent_code_returns_structured_final_value() {
        let mut instance =
            PluginVmInstance::new(PluginEngineConfig::default(), "agent-code".to_string())
                .expect("create quickjs instance");
        let result = instance
            .run_agent_code("return { kept: [1, 2], discarded: null };")
            .expect("agent code should return JSON");
        assert_eq!(result["kept"], serde_json::json!([1, 2]));
        assert!(result["discarded"].is_null());
    }

    #[test]
    fn build_combined_script_appends_main_loop_for_session_vm() {
        let instance =
            PluginVmInstance::new(PluginEngineConfig::default(), "combined-script".to_string())
                .expect("create quickjs instance");
        let dir = tempfile::tempdir().expect("create tempdir");
        let script_path = dir.path().join("main.js");
        std::fs::write(&script_path, "pi.log('ready');\n").expect("write user script");

        let combined = instance
            .build_combined_script(&script_path, true)
            .expect("build combined script");
        assert!(
            combined.contains("// --- pi_main_loop.js ---"),
            "session VM should inject main loop when plugin does not start it explicitly"
        );
        assert!(
            combined.contains("__pi_start_event_loop"),
            "combined script should include event loop bootstrap"
        );
        assert!(
            combined.contains("__pi_resume_timeout"),
            "combined script should resume the timeout budget at the user-code boundary"
        );
    }

    #[test]
    fn agent_result_does_not_wait_for_unawaited_timers() {
        let mut vm =
            PluginVmInstance::new(PluginEngineConfig::default(), "agent-final".into()).unwrap();
        let start = std::time::Instant::now();
        assert_eq!(
            vm.run_agent_code("setTimeout(() => {}, 60000); return 42;")
                .unwrap(),
            42
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn agent_external_cancel_stops_busy_and_idle_execution() {
        use std::time::{Duration, Instant};
        use tokio_util::sync::CancellationToken;
        for body in ["while (true) {}", "await new Promise(() => {});"] {
            let mut vm = PluginVmInstance::new(
                PluginEngineConfig {
                    call_timeout_ms: 500,
                    interrupt_budget: 0,
                    ..Default::default()
                },
                "agent-stop".into(),
            )
            .unwrap();
            let cancel = CancellationToken::new();
            let stop = cancel.clone();
            let (ready, seen) = std::sync::mpsc::channel();
            vm.register_host_binding(move |_| {
                ready.send(()).unwrap();
                Ok(serde_json::json!({"ok":true,"data":null}).to_string())
            })
            .unwrap();
            let stopper = std::thread::spawn(move || {
                seen.recv_timeout(Duration::from_secs(2)).unwrap();
                std::thread::sleep(Duration::from_millis(20));
                stop.cancel();
            });
            let start = Instant::now();
            let error = vm
                .run_agent_code_with_cancel(
                    &format!("__pi_host_call('{{}}'); {body}"),
                    cancel,
                    move || start.elapsed() < Duration::from_secs(2),
                )
                .unwrap_err();
            stopper.join().unwrap();
            assert!(error.to_string().contains("cancelled"), "{error}");
            assert!(start.elapsed() < Duration::from_secs(2));
        }
    }

    #[test]
    fn agent_host_wait_exceeds_short_vm_window_but_local_work_still_times_out() {
        use std::sync::atomic::AtomicBool;
        use std::time::{Duration, Instant};
        use tokio_util::sync::CancellationToken;
        for tail in ["return result;", "while (true) {}"] {
            let mut vm = PluginVmInstance::new(
                PluginEngineConfig {
                    call_timeout_ms: 20,
                    interrupt_budget: 0,
                    ..Default::default()
                },
                "agent-host-wait".into(),
            )
            .unwrap();
            let pending = Arc::new(AtomicBool::new(false));
            let binding_pending = pending.clone();
            let started = parking_lot::Mutex::new(None::<Instant>);
            vm.register_host_binding(move |request| {
                let request: serde_json::Value = serde_json::from_str(request).unwrap();
                let response = if request["module"] == "connector" {
                    binding_pending.store(true, Ordering::SeqCst);
                    *started.lock() = Some(Instant::now());
                    serde_json::json!({"ok":true,"data":{"pending":true},"callId":request["callId"]})
                } else if started.lock().unwrap().elapsed() >= Duration::from_millis(350) {
                    binding_pending.store(false, Ordering::SeqCst);
                    serde_json::json!({"ok":true,"data":{"ready":true,"response":{"ok":true,"data":42}}})
                } else { serde_json::json!({"ok":true,"data":{"ready":false}}) };
                Ok(response.to_string())
            }).unwrap();
            let value = vm.run_agent_code_with_cancel(
                &format!("const result = await callTool('fixture', {{}}); {tail}"),
                CancellationToken::new(),
                move || pending.load(Ordering::SeqCst),
            );
            if tail.starts_with("return") {
                assert_eq!(value.unwrap(), 42);
            } else {
                assert!(value.unwrap_err().to_string().contains("20ms timeout"));
            }
        }
        let mut vm = PluginVmInstance::new(
            PluginEngineConfig {
                call_timeout_ms: 50,
                interrupt_budget: 0,
                ..Default::default()
            },
            "agent-empty-promise".into(),
        )
        .unwrap();
        assert!(vm
            .run_agent_code("await new Promise(() => {});")
            .unwrap_err()
            .to_string()
            .contains("50ms timeout"));
    }

    #[test]
    fn run_script_reaches_registered_host_binding() {
        let mut instance =
            PluginVmInstance::new(PluginEngineConfig::default(), "host-binding".to_string())
                .expect("create quickjs instance");
        let call_count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&call_count);
        instance
            .register_host_binding(move |_request_json| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({ "ok": true, "data": null }).to_string())
            })
            .expect("register host binding");

        instance
            .run_script("pi.log('hello from inline test');")
            .expect("run inline script");
        assert!(
            call_count.load(Ordering::SeqCst) >= 1,
            "pi.log should route through the host binding"
        );
    }

    #[test]
    fn heap_limit_rejects_large_allocation() {
        let script = r#"
globalThis.__hold = new Uint8Array(4 * 1024 * 1024);
"#;
        let mut tight = PluginVmInstance::new(
            PluginEngineConfig {
                quickjs_heap_mb: 1,
                call_timeout_ms: 1_000,
                interrupt_budget: 1_000_000,
                ..Default::default()
            },
            "heap-limit".to_string(),
        )
        .expect("create quickjs instance");

        let err = tight
            .run_script(script)
            .expect_err("allocation above heap limit should fail");
        let mut roomy = PluginVmInstance::new(
            PluginEngineConfig {
                quickjs_heap_mb: 8,
                call_timeout_ms: 1_000,
                interrupt_budget: 1_000_000,
                ..Default::default()
            },
            "heap-roomy".to_string(),
        )
        .expect("create roomy quickjs instance");
        roomy
            .run_script(script)
            .expect("same allocation should fit once heap budget is raised");

        let message = err.to_string();
        assert!(
            message.contains("QuickJS") || message.contains("JS执行错误"),
            "heap guard should surface a QuickJS-side failure, got: {err}"
        );
    }

    #[test]
    fn heap_limit_zero_allows_large_allocation() {
        let script = r#"
globalThis.__hold = new Uint8Array(4 * 1024 * 1024);
"#;
        let mut unbounded = PluginVmInstance::new(
            PluginEngineConfig {
                quickjs_heap_mb: 0,
                call_timeout_ms: 1_000,
                interrupt_budget: 1_000_000,
                ..Default::default()
            },
            "heap-unbounded".to_string(),
        )
        .expect("create unbounded quickjs instance");

        unbounded
            .run_script(script)
            .expect("heap_mb=0 should skip the explicit memory limit");
    }

    #[test]
    fn call_timeout_interrupts_long_sync_script() {
        let mut instance = PluginVmInstance::new(
            PluginEngineConfig {
                quickjs_heap_mb: 8,
                call_timeout_ms: 50,
                interrupt_budget: 0,
                ..Default::default()
            },
            "call-timeout".to_string(),
        )
        .expect("create timeout-scoped quickjs instance");

        let err = instance
            .run_script("while (true) {}")
            .expect_err("call_timeout_ms should interrupt a runaway synchronous loop");
        assert!(
            err.to_string().contains("50ms timeout"),
            "timeout error should mention the configured 50ms budget, got: {err}"
        );
    }

    #[test]
    fn wait_for_event_refreshes_timeout_budget_between_idle_ticks() {
        let mut instance = PluginVmInstance::new(
            PluginEngineConfig {
                quickjs_heap_mb: 8,
                // Four 100ms idle waits exceed this total budget, so the test still proves
                // that each wait resets the timer. The per-tick headroom avoids scheduler
                // jitter turning a timing-contract test into a flaky 50ms race.
                call_timeout_ms: 200,
                interrupt_budget: 0,
                ..Default::default()
            },
            "idle-budget-reset".to_string(),
        )
        .expect("create quickjs instance");
        instance
            .register_host_binding(|request_json| {
                let request: serde_json::Value =
                    serde_json::from_str(request_json).expect("host request should be JSON");
                if request["module"] == "__session" && request["method"] == "waitForEvent" {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    return Ok(
                        serde_json::json!({ "ok": true, "data": { "type": "__tick" } }).to_string(),
                    );
                }
                Ok(serde_json::json!({ "ok": true, "data": null }).to_string())
            })
            .expect("register host binding");
        instance
            .run_script(
                r#"
(async function () {
  globalThis.__idleBudgetSink = 0;
  for (var i = 0; i < 4; i += 1) {
    await __pi_wait_for_event(100);
    for (var j = 0; j < 5000; j += 1) {
      globalThis.__idleBudgetSink += j;
    }
  }
})();
"#,
            )
            .expect("idle wait ticks should reset timeout budget instead of tripping it");
    }

    #[test]
    fn wait_for_event_can_block_past_call_timeout_without_tripping_idle_vm() {
        let mut instance = PluginVmInstance::new(
            PluginEngineConfig {
                quickjs_heap_mb: 8,
                call_timeout_ms: 50,
                interrupt_budget: 0,
                ..Default::default()
            },
            "idle-timeout-pause".to_string(),
        )
        .expect("create quickjs instance");
        instance
            .register_host_binding(|request_json| {
                let request: serde_json::Value =
                    serde_json::from_str(request_json).expect("host request should be JSON");
                if request["module"] == "__session" && request["method"] == "waitForEvent" {
                    std::thread::sleep(std::time::Duration::from_millis(80));
                    return Ok(
                        serde_json::json!({ "ok": true, "data": { "type": "__tick" } }).to_string(),
                    );
                }
                Ok(serde_json::json!({ "ok": true, "data": null }).to_string())
            })
            .expect("register host binding");
        instance
            .run_script(
                r#"
(async function () {
  await __pi_wait_for_event(80);
  globalThis.__idleWaitCompleted = true;
})();
"#,
            )
            .expect("idle wait should not spend call_timeout_ms while no plugin code is running");
    }

    #[test]
    fn destroy_consumes_instance_without_side_effects() {
        let instance = PluginVmInstance::new(PluginEngineConfig::default(), "destroy".to_string())
            .expect("create quickjs instance");
        instance.destroy();
    }
}
