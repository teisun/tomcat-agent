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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    Timeout,
    BudgetExceeded,
}

struct ExecutionGuardState {
    started_at: Mutex<Instant>,
    interrupt_count: AtomicU64,
    reason: Mutex<Option<InterruptReason>>,
    timeout: Duration,
    budget: u64,
}

impl ExecutionGuardState {
    fn new(timeout: Duration, budget: u64) -> Self {
        Self {
            started_at: Mutex::new(Instant::now()),
            interrupt_count: AtomicU64::new(0),
            reason: Mutex::new(None),
            timeout,
            budget,
        }
    }

    fn reset(&self) {
        *self.started_at.lock() = Instant::now();
        self.interrupt_count.store(0, Ordering::SeqCst);
        *self.reason.lock() = None;
    }

    fn should_interrupt(&self) -> bool {
        if !self.timeout.is_zero() && self.started_at.lock().elapsed() >= self.timeout {
            *self.reason.lock() = Some(InterruptReason::Timeout);
            return true;
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

    pub fn run_script_file(&mut self, path: &Path) -> Result<serde_json::Value, AppError> {
        if !path.exists() {
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("script file not found: {}", path.display()),
            )));
        }

        let combined = self.build_combined_script(path, false)?;
        self.execute_script(&combined)?;
        Ok(serde_json::Value::Null)
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
        self.execute_script(&combined)
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

    fn execute_script(&self, code: &str) -> Result<(), AppError> {
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
        let guard = Arc::new(ExecutionGuardState::new(timeout, interrupt_budget));

        run_with_local_runtime(move || async move {
            guard.reset();
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

            Ok(())
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
    let wait_fn = Function::new(
        ctx.clone(),
        Async(move |timeout_ms: u64| {
            let wait_bridge = wait_bridge.clone();
            async move {
                wait_bridge
                    .wait_for_event(timeout_ms)
                    .await
                    .map_err(|e| js_runtime_error(e.to_string()))
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
    fn destroy_consumes_instance_without_side_effects() {
        let instance = PluginVmInstance::new(PluginEngineConfig::default(), "destroy".to_string())
            .expect("create quickjs instance");
        instance.destroy();
    }
}
