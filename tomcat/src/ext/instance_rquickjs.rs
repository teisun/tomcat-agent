//! # rquickjs 实例实现
//!
//! 每个插件实例持有独立 QuickJS 上下文；短生命周期脚本使用
//! `run_script` / `run_script_file`，长生命周期会话 VM 使用
//! `run_session_script` 持续运行到宿主发出 `__shutdown`。

use crate::ext::HostRequest;
use crate::infra::error::AppError;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use getrandom::fill as fill_random;
use parking_lot::Mutex;
use rquickjs::function::{Async, Func};
use rquickjs::{AsyncContext, AsyncRuntime, Ctx, Function, Object};
use sha2::{Digest, Sha256, Sha384, Sha512};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

use super::engine_stub::WasmEngineConfig;

const PI_RUNTIME_PRELUDE: &str = include_str!("../../assets/js/pi_runtime_prelude.js");
const PI_CRYPTO_SHIM: &str = include_str!("../../assets/js/pi_crypto_shim.js");
const EMBEDDED_BRIDGE_JS: &str = include_str!("../../assets/js/pi_bridge.js");
const PI_TUI_SHIM: &str = include_str!("../../assets/js/pi_tui_shim.js");
const PI_CODING_AGENT_SHIM: &str = include_str!("../../assets/js/pi_coding_agent_shim.js");
const PI_AI_SHIM: &str = include_str!("../../assets/js/pi_ai_shim.js");
const PI_TYPEBOX_SHIM: &str = include_str!("../../assets/js/pi_typebox_shim.js");
const PI_NODE_SHIM: &str = include_str!("../../assets/js/pi_node_shim.js");
const PI_SANDBOX_RUNTIME_SHIM: &str = include_str!("../../assets/js/pi_sandbox_runtime_shim.js");
const PI_MS_SHIM: &str = include_str!("../../assets/js/pi_ms_shim.js");
const PI_MAIN_LOOP: &str = include_str!("../../assets/js/pi_main_loop.js");

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
pub struct WasmInstance {
    #[allow(dead_code)]
    config: WasmEngineConfig,
    plugin_id: String,
    host_invoke: Option<Arc<HostInvokeFn>>,
}

impl fmt::Debug for WasmInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WasmInstance")
            .field("plugin_id", &self.plugin_id)
            .finish_non_exhaustive()
    }
}

impl WasmInstance {
    pub fn new(config: WasmEngineConfig, plugin_id: String) -> Result<Self, AppError> {
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
        let heap_limit_bytes = self.config.quickjs_heap_mb as usize * 1024 * 1024;
        let guard = Arc::new(ExecutionGuardState::new(timeout, interrupt_budget));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(AppError::Io)?;

        runtime.block_on(async move {
            guard.reset();
            let js_runtime = AsyncRuntime::new().map_err(to_app_js_error)?;
            js_runtime.set_memory_limit(heap_limit_bytes).await;
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
             // --- pi_tui_shim.js ---\n{PI_TUI_SHIM}\n\
             // --- pi_coding_agent_shim.js ---\n{PI_CODING_AGENT_SHIM}\n\
             // --- pi_ai_shim.js ---\n{PI_AI_SHIM}\n\
             // --- pi_typebox_shim.js ---\n{PI_TYPEBOX_SHIM}\n\
             // --- pi_sandbox_runtime_shim.js ---\n{PI_SANDBOX_RUNTIME_SHIM}\n\
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

fn register_crypto_globals<'js>(globals: &Object<'js>) -> rquickjs::Result<()> {
    globals.set(
        "__pi_crypto_hash_native",
        Func::from(
            move |algo: String, data_base64: String| -> rquickjs::Result<String> {
                let data = BASE64_STANDARD.decode(data_base64).map_err(|error| {
                    js_runtime_error(format!("invalid base64 input for crypto hash: {error}"))
                })?;
                hash_bytes_hex(&algo, &data).map_err(js_runtime_error)
            },
        ),
    )?;
    globals.set(
        "__pi_crypto_random_uuid_native",
        Func::from(move || -> rquickjs::Result<String> { Ok(Uuid::new_v4().to_string()) }),
    )?;
    globals.set(
        "__pi_crypto_random_bytes_native",
        Func::from(move |size: u32| -> rquickjs::Result<String> {
            let size = size as usize;
            if size > 10 * 1024 * 1024 {
                return Err(js_runtime_error(format!(
                    "randomBytes size exceeds limit: {size}"
                )));
            }
            let mut bytes = vec![0_u8; size];
            fill_random(&mut bytes)
                .map_err(|error| js_runtime_error(format!("fill random bytes failed: {error}")))?;
            Ok(bytes_to_hex(&bytes))
        }),
    )?;
    Ok(())
}

fn hash_bytes_hex(algo: &str, bytes: &[u8]) -> Result<String, String> {
    let normalized = algo.trim().to_ascii_lowercase();
    let digest = match normalized.as_str() {
        "sha256" => Sha256::digest(bytes).to_vec(),
        "sha384" => Sha384::digest(bytes).to_vec(),
        "sha512" => Sha512::digest(bytes).to_vec(),
        unsupported => {
            return Err(format!(
                "unsupported hash algorithm '{unsupported}', expected sha256/sha384/sha512"
            ))
        }
    };
    Ok(bytes_to_hex(&digest))
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
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
