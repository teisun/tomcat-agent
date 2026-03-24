//! # WasmInstance 真实实现（默认构建即包含）
//!
//! 每个插件独立 Vm；run_script / run_script_file 通过 wasmedge_quickjs.wasm 执行 JS；每次执行新建 Vm + 当次 WasiModule（argv + preopen），宿主导入 __pi_host_call 注册到 env 模块。

use crate::infra::error::AppError;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wasmedge_sdk::error::CoreExecutionError;
use wasmedge_sdk::{
    config::Config, error::CoreError, vm::SyncInst, wasi::WasiModule, CallingFrame,
    ImportObjectBuilder, Instance, Module, Store, Vm, WasmValue,
};

/// pi_bridge.js embedded at compile time — the primary JS bridge between host and QuickJS guest.
const EMBEDDED_BRIDGE_JS: &str = include_str!("../../assets/js/pi_bridge.js");

/// npm package shims embedded at compile time; injected between pi_bridge.js and user script.
const PI_TUI_SHIM: &str = include_str!("../../assets/js/pi_tui_shim.js");
const PI_CODING_AGENT_SHIM: &str = include_str!("../../assets/js/pi_coding_agent_shim.js");
const PI_AI_SHIM: &str = include_str!("../../assets/js/pi_ai_shim.js");
const PI_TYPEBOX_SHIM: &str = include_str!("../../assets/js/pi_typebox_shim.js");
const PI_NODE_SHIM: &str = include_str!("../../assets/js/pi_node_shim.js");
const PI_SANDBOX_RUNTIME_SHIM: &str = include_str!("../../assets/js/pi_sandbox_runtime_shim.js");
const PI_MS_SHIM: &str = include_str!("../../assets/js/pi_ms_shim.js");
/// Async main loop IIFE injected at the tail of long-lived VM scripts.
const PI_MAIN_LOOP: &str = include_str!("../../assets/js/pi_main_loop.js");

/// Hostcall 回调函数签名：接收 JSON 请求字符串，返回 JSON 响应字符串。
type HostInvokeFn = dyn Fn(&str) -> Result<String, AppError> + Send + Sync;

/// 宿主导入的 host data：供 __pi_host_call 使用。
struct HostData {
    plugin_id: String,
    host_invoke: Option<Arc<HostInvokeFn>>,
}
impl fmt::Debug for HostData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostData")
            .field("plugin_id", &self.plugin_id)
            .finish_non_exhaustive()
    }
}

/// 单插件独立 Wasm 实例（真实实现：每次 run_script/run_script_file 新建 Vm + 当次 WasiModule argv/preopen）。
pub struct WasmInstance {
    config: Config,
    plugin_id: String,
    /// 宿主导入回调：request_json -> response_json；在构建 Vm 时注册到 env.__pi_host_call。
    #[allow(clippy::type_complexity)]
    host_invoke: Option<Arc<dyn Fn(&str) -> Result<String, AppError> + Send + Sync>>,
    /// QuickJS wasm 路径；未设置时 run_script 返回错误提示设置 WASMEDGE_QUICKJS_PATH。
    quickjs_path: Option<PathBuf>,
    /// 懒创建：env 宿主导入模块（Store 需持有其引用）。
    import_object: Option<wasmedge_sdk::ImportObject<HostData>>,
    /// 当次执行的 WasiModule（每次 run_script/run_script_file 时新建并替换，含 argv + preopen）。
    wasi_module: Option<WasiModule>,
}
impl fmt::Debug for WasmInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WasmInstance")
            .field("config", &self.config)
            .field("plugin_id", &self.plugin_id)
            .field("quickjs_path", &self.quickjs_path)
            .finish_non_exhaustive()
    }
}

impl WasmInstance {
    /// 由 WasmEngine::create_instance 调用。QuickJS 路径：优先使用 engine 传入的配置（来自 AppConfig/环境变量 PI_WASM__WASM__QUICKJS_PATH），否则回退到 WASMEDGE_QUICKJS_PATH。
    ///
    /// # Errors
    /// * 当前实现不返回错误；路径未设置时在 [`run_script`] 中返回 [`AppError::QuickJS`]。
    pub fn new(
        config: Config,
        plugin_id: String,
        quickjs_path_from_engine: Option<PathBuf>,
    ) -> Result<Self, AppError> {
        let quickjs_path = quickjs_path_from_engine.filter(|p| p.exists()).or_else(|| {
            std::env::var("WASMEDGE_QUICKJS_PATH")
                .ok()
                .map(PathBuf::from)
                .filter(|p| p.exists())
        });
        Ok(Self {
            config,
            plugin_id,
            host_invoke: None,
            quickjs_path,
            import_object: None,
            wasi_module: None,
        })
    }

    /// 执行 JS 代码：写入临时文件后由 wasmedge_quickjs.wasm 执行；每次执行新建 Vm 与 WasiModule（argv + preopen），脚本会被真正执行。
    ///
    /// # Errors
    /// * [`AppError::QuickJS`] - quickjs_path 未设置或路径不存在时返回。
    /// * [`AppError::WasmEdge`] - 注册/执行 quickjs 模块失败时返回。
    /// * [`AppError::Io`] - 写入临时脚本文件失败时返回。
    pub fn run_script(&mut self, code: &str) -> Result<serde_json::Value, AppError> {
        let (_script_path, _guard) = temp_js_file(code)?;
        self.run_script_file_impl(&_script_path)
    }

    /// 执行指定路径的 .js 文件：由 wasmedge_quickjs.wasm 执行；每次执行新建 Vm 与 WasiModule（argv + preopen）。
    ///
    /// **注意**：此方法创建短生命周期 VM，仅用于基础引擎测试和 `load_plugin` 加载校验。
    /// 插件/扩展 E2E 测试必须使用 `init_vm` + `start_session_vm` 长生命周期路径。
    ///
    /// # Errors
    /// * [`AppError::QuickJS`] - quickjs_path 未设置或路径不存在时返回。
    /// * [`AppError::WasmEdge`] - 注册/执行 quickjs 模块失败时返回。
    /// * [`AppError::Io`] - 路径不存在或不可读时返回。
    pub fn run_script_file(&mut self, path: &Path) -> Result<serde_json::Value, AppError> {
        if !path.exists() {
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("script file not found: {}", path.display()),
            )));
        }
        self.run_script_file_impl(path)
    }

    fn run_script_file_impl(&mut self, script_path: &Path) -> Result<serde_json::Value, AppError> {
        let quickjs_path = self
            .quickjs_path
            .clone()
            .ok_or_else(|| {
                AppError::QuickJS(
                    "WASMEDGE_QUICKJS_PATH not set or path does not exist. Set it to wasmedge_quickjs.wasm path.".to_string(),
                )
            })?;

        let combined = self.build_combined_script(script_path)?;
        let (combined_path, _tmp_dir) = temp_js_file(&combined)?;

        let config = self.config.clone();
        let script_dir = combined_path.parent().ok_or_else(|| {
            AppError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "script path has no parent",
            ))
        })?;
        let script_name = combined_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                AppError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "script path has no file name",
                ))
            })?;
        let preopen_strings = build_wasi_preopens(script_dir)?;
        let preopen_refs: Vec<&str> = preopen_strings.iter().map(|s| s.as_str()).collect();
        let argv: Vec<&str> = vec!["quickjs", script_name];
        self.wasi_module = Some(
            WasiModule::create(Some(argv), None, Some(preopen_refs))
                .map_err(|e| AppError::WasmEdge(e.to_string()))?,
        );
        let mut vm = self.build_vm()?;
        let module = Module::from_file(Some(&config), &quickjs_path)
            .map_err(|e| AppError::WasmEdge(e.to_string()))?;
        vm.register_module(Some("quickjs"), module)
            .map_err(|e| AppError::WasmEdge(e.to_string()))?;
        match vm.run_func(Some("quickjs"), "_start", []) {
            Ok(_) => Ok(serde_json::Value::Null),
            Err(e) => {
                let msg = e.to_string();
                // QuickJS _start 正常退出时可能返回 "exit code 0" 类 CoreError，不视为失败
                if msg.contains("exit code 0") || msg.contains("success") {
                    Ok(serde_json::Value::Null)
                } else {
                    Err(AppError::QuickJS(format!(
                        "script execution failed: {}",
                        msg
                    )))
                }
            }
        }
    }

    /// Prepend pi_bridge.js + npm shims (if present) to the user script content.
    /// Falls back to the plain user script if bridge is absent.
    /// Injection order: pi_bridge → tui shim → coding-agent shim → ai shim → typebox shim → user script.
    fn build_combined_script(&self, user_script: &Path) -> Result<String, AppError> {
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
        let bridge_code = get_bridge_js_content();
        Ok(format!(
            "// --- pi_bridge.js (auto-injected) ---\n{bridge_code}\n\
             // --- pi_node_shim.js ---\n{PI_NODE_SHIM}\n\
             // --- pi_tui_shim.js ---\n{PI_TUI_SHIM}\n\
             // --- pi_coding_agent_shim.js ---\n{PI_CODING_AGENT_SHIM}\n\
             // --- pi_ai_shim.js ---\n{PI_AI_SHIM}\n\
             // --- pi_typebox_shim.js ---\n{PI_TYPEBOX_SHIM}\n\
             // --- pi_sandbox_runtime_shim.js ---\n{PI_SANDBOX_RUNTIME_SHIM}\n\
             // --- pi_ms_shim.js ---\n{PI_MS_SHIM}\n\
             // --- user script ---\n{user_code}"
        ))
    }

    /// 注册宿主导入并映射到 QuickJS 全局 agent；内部在 build_vm 时注册 env.__pi_host_call。
    ///
    /// # Errors
    /// * 当前实现不返回错误。
    pub fn register_host_binding(
        &mut self,
        invoke_fn: impl Fn(&str) -> Result<String, AppError> + Send + Sync + 'static,
    ) -> Result<(), AppError> {
        self.host_invoke = Some(Arc::new(invoke_fn));
        Ok(())
    }

    /// **[已废弃]** 短生命周期事件分发：组合脚本 + `__pi_dispatch_event` 模式。
    ///
    /// 长生命周期 VM 模式下，事件通过 `__session.waitForEvent` channel 投递，
    /// 无需重新加载脚本。请改用 `PluginManager::dispatch_session_event`。
    ///
    /// # Errors
    /// * [`AppError::QuickJS`] - 事件 JSON 序列化失败。
    /// * 其他错误同 [`run_script`]。
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
        let user_code = std::fs::read_to_string(plugin_script).map_err(AppError::Io)?;
        let envelope = serde_json::json!({
            "type": event_type,
            "data": event_data,
            "context": context,
        });
        let envelope_str = serde_json::to_string(&envelope)
            .map_err(|e| AppError::QuickJS(format!("event serialization: {e}")))?;
        let escaped = envelope_str
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n");
        let combined = format!("{user_code}\n__pi_dispatch_event('{escaped}');\n");
        self.run_script(&combined)
    }

    /// 构建持久 Vm 并加载 QuickJS 模块（不执行 _start）。
    /// 用于长生命周期 VM actor 模式：actor 线程拿到 Vm 后自行调用 run_func。
    ///
    /// `script_path` 为插件入口脚本路径，用于设置 WasiModule 的 argv 和 preopen。
    ///
    /// 返回 `(Vm, _tmp_dir)` — 调用方须持有 `_tmp_dir` 直到 Vm 不再使用，
    /// 否则临时文件会被提前清理。
    #[allow(clippy::type_complexity)]
    pub fn init_vm(
        &mut self,
        script_path: &Path,
    ) -> Result<(Vm<'_, dyn SyncInst>, PathBuf, tempfile::TempDir), AppError> {
        let quickjs_path = self.quickjs_path.clone().ok_or_else(|| {
            AppError::QuickJS("WASMEDGE_QUICKJS_PATH not set or path does not exist.".to_string())
        })?;

        let mut combined = self.build_combined_script(script_path)?;
        combined.push('\n');
        combined.push_str(PI_MAIN_LOOP);
        let (combined_path, tmp_dir) = temp_js_file(&combined)?;

        let config = self.config.clone();
        let script_dir = combined_path.parent().ok_or_else(|| {
            AppError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "script path has no parent",
            ))
        })?;
        let script_name = combined_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                AppError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "script path has no file name",
                ))
            })?;
        let preopen_strings = build_wasi_preopens(script_dir)?;
        let preopen_refs: Vec<&str> = preopen_strings.iter().map(|s| s.as_str()).collect();
        let argv: Vec<&str> = vec!["quickjs", script_name];
        self.wasi_module = Some(
            WasiModule::create(Some(argv), None, Some(preopen_refs))
                .map_err(|e| AppError::WasmEdge(e.to_string()))?,
        );

        let mut vm = self.build_vm()?;
        let module = Module::from_file(Some(&config), &quickjs_path)
            .map_err(|e| AppError::WasmEdge(e.to_string()))?;
        vm.register_module(Some("quickjs"), module)
            .map_err(|e| AppError::WasmEdge(e.to_string()))?;

        Ok((vm, combined_path, tmp_dir))
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// 销毁实例，释放资源。
    pub fn destroy(self) {}

    /// 构建 Vm：env（宿主导入）+ 当次 WasiModule（已在 run_script_file_impl 中设置）。Store 持有对 self 上 import_object 与 wasi_module 的引用。
    fn build_vm(&mut self) -> Result<Vm<'_, dyn SyncInst>, AppError> {
        if self.import_object.is_none() {
            let host_data = HostData {
                plugin_id: self.plugin_id.clone(),
                host_invoke: self.host_invoke.clone(),
            };
            let mut builder = ImportObjectBuilder::new("env", host_data)
                .map_err(|e| AppError::WasmEdge(e.to_string()))?;
            builder
                .with_func::<(i32, i32, i32), i32>("__pi_host_call", host_call_impl)
                .map_err(|e| AppError::WasmEdge(e.to_string()))?;
            self.import_object = Some(builder.build());
        }
        let mut instances: HashMap<String, &mut dyn SyncInst> = HashMap::new();
        let env = self.import_object.as_mut().unwrap();
        instances.insert("env".to_string(), env as &mut dyn SyncInst);
        let wasi = self.wasi_module.as_mut().unwrap();
        instances.insert(wasi.name().to_string(), wasi.as_mut() as &mut dyn SyncInst);
        let store = Store::new(Some(&self.config), instances)
            .map_err(|e| AppError::WasmEdge(e.to_string()))?;
        Ok(Vm::new(store))
    }
}

/// 宿主导入 __pi_host_call(buf_ptr, req_len, buf_cap) -> resp_len：
/// 从线性内存 buf_ptr 读取 req_len 字节请求 JSON，调用宿主回调，
/// 将响应写回 buf_ptr（不超过 buf_cap），返回实际响应长度。
fn host_call_impl(
    data: &mut HostData,
    _inst: &mut Instance,
    frame: &mut CallingFrame,
    args: Vec<WasmValue>,
) -> Result<Vec<WasmValue>, CoreError> {
    if args.len() < 3 {
        return Err(CoreError::Execution(CoreExecutionError::HostFuncFailed));
    }
    let buf_ptr = args[0].to_i32() as u32;
    let req_len = args[1].to_i32() as u32;
    let buf_cap = args[2].to_i32() as u32;
    let invoke = data
        .host_invoke
        .as_ref()
        .ok_or(CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
    let mut memory = frame
        .memory_mut(0)
        .ok_or(CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
    let buf = memory
        .get_data(buf_ptr, req_len)
        .map_err(|_| CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
    let request_json = String::from_utf8(buf)
        .map_err(|_| CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
    let response_json = invoke(&request_json)
        .map_err(|_| CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
    let resp_bytes = response_json.as_bytes();
    let out_len = resp_bytes.len() as u32;
    if out_len <= buf_cap {
        memory
            .set_data(resp_bytes, buf_ptr)
            .map_err(|_| CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
    }
    Ok(vec![WasmValue::from_i32(out_len as i32)])
}

fn temp_js_file(code: &str) -> Result<(PathBuf, tempfile::TempDir), AppError> {
    let dir = tempfile::tempdir().map_err(AppError::Io)?;
    let path = dir.path().join("script.js");
    std::fs::write(&path, code).map_err(AppError::Io)?;
    Ok((path, dir))
}

/// wasmedge-quickjs Node 兼容模块目录。
/// 优先级：`PI_WASM_QUICKJS_MODULES_PATH` → `~/.pi_/assets/modules/` → `CARGO_MANIFEST_DIR/assets/modules`（开发兜底）。
fn resolve_quickjs_modules_dir() -> Option<PathBuf> {
    std::env::var("PI_WASM_QUICKJS_MODULES_PATH")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| {
            let p = dirs::home_dir()?.join(".pi_").join("assets").join("modules");
            p.is_dir().then_some(p)
        })
        .or_else(|| {
            let p = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join("modules");
            p.is_dir().then_some(p)
        })
}

/// WASI preopen：`.` → 脚本目录；`./modules` → QuickJS 内置 Node polyfill（fs/path/…）。
fn build_wasi_preopens(script_host_dir: &Path) -> Result<Vec<String>, AppError> {
    let host_dir = script_host_dir
        .canonicalize()
        .map_err(AppError::Io)
        .unwrap_or_else(|_| script_host_dir.to_path_buf());
    let mut preopens = vec![format!(".:{}", host_dir.display())];
    if let Some(modules_dir) = resolve_quickjs_modules_dir() {
        let m = modules_dir.canonicalize().map_err(AppError::Io)?;
        preopens.push(format!("./modules:{}", m.display()));
    }
    Ok(preopens)
}

/// Return the pi_bridge.js content. Priority:
/// 1. `PI_BRIDGE_JS_PATH` env var (if set and file is readable)
/// 2. Compile-time embedded constant
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
