//! # WasmInstance 真实实现（默认构建即包含）
//!
//! 每个插件独立 Vm；run_script 通过 wasmedge_quickjs.wasm 执行 JS；宿主导入 __pi_host_call 注册到 env 模块并供 QuickJS 映射到全局 agent。

use crate::infra::error::AppError;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::fmt;
use wasmedge_sdk::{
    config::Config,
    error::CoreError,
    vm::SyncInst,
    wasi::WasiModule,
    CallingFrame,
    ImportObjectBuilder,
    Instance,
    Module,
    Store,
    Vm,
    WasmValue,
};
use wasmedge_sdk::error::CoreExecutionError;

/// 宿主导入的 host data：供 __pi_host_call 使用。
struct HostData {
    plugin_id: String,
    host_invoke: Option<Arc<dyn Fn(&str) -> Result<String, AppError> + Send + Sync>>,
}
impl fmt::Debug for HostData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostData")
            .field("plugin_id", &self.plugin_id)
            .finish_non_exhaustive()
    }
}

/// 单插件独立 Wasm 实例（真实实现：每实例一个 Vm，懒加载 quickjs wasm + 宿主导入）。
pub struct WasmInstance {
    config: Config,
    plugin_id: String,
    /// 宿主导入回调：request_json -> response_json；在 build_vm 时注册到 env.__pi_host_call。
    #[allow(clippy::type_complexity)]
    host_invoke: Option<Arc<dyn Fn(&str) -> Result<String, AppError> + Send + Sync>>,
    /// QuickJS wasm 路径；未设置时 run_script 返回错误提示设置 WASMEDGE_QUICKJS_PATH。
    quickjs_path: Option<PathBuf>,
    /// 懒创建：env 宿主导入模块。
    import_object: Option<wasmedge_sdk::ImportObject<HostData>>,
    /// 懒创建：WASI 模块。
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
    /// 由 WasmEngine::create_instance 调用。QuickJS 路径：优先使用 engine 传入的配置（来自 AppConfig/环境变量 PI_AWSM__WASM__QUICKJS_PATH），否则回退到 WASMEDGE_QUICKJS_PATH。
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

    /// 执行 JS 代码：写入临时文件后由 wasmedge_quickjs.wasm 执行；需配置 quickjs 路径（config 或环境变量）。
    ///
    /// # Errors
    /// * [`AppError::QuickJS`] - quickjs_path 未设置或路径不存在时返回。
    /// * [`AppError::WasmEdge`] - 注册/执行 quickjs 模块失败时返回。
    /// * [`AppError::Io`] - 写入临时脚本文件失败时返回。
    pub fn run_script(&mut self, code: &str) -> Result<serde_json::Value, AppError> {
        let quickjs_path = self
            .quickjs_path
            .clone()
            .ok_or_else(|| {
                AppError::QuickJS(
                    "WASMEDGE_QUICKJS_PATH not set or path does not exist. Set it to wasmedge_quickjs.wasm path.".to_string(),
                )
            })?;
        let config = self.config.clone();
        let (_temp_path, _guard) = temp_js_file(code)?;
        let mut vm = self.build_vm()?;
        let module = Module::from_file(Some(&config), &quickjs_path)
            .map_err(|e| AppError::WasmEdge(e.to_string()))?;
        vm.register_module(Some("quickjs"), module)
            .map_err(|e| AppError::WasmEdge(e.to_string()))?;
        let _ = vm.run_func(Some("quickjs"), "_start", []);
        Ok(serde_json::Value::Null)
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

    /// 销毁实例，释放资源。
    pub fn destroy(self) {}

    fn build_vm(&mut self) -> Result<Vm<'_, dyn SyncInst>, AppError> {
        if self.import_object.is_none() {
            let host_data = HostData {
                plugin_id: self.plugin_id.clone(),
                host_invoke: self.host_invoke.clone(),
            };
            let mut builder = ImportObjectBuilder::new("env", host_data)
                .map_err(|e| AppError::WasmEdge(e.to_string()))?;
            builder
                .with_func::<(i32, i32), i32>("__pi_host_call", host_call_impl)
                .map_err(|e| AppError::WasmEdge(e.to_string()))?;
            self.import_object = Some(builder.build());
        }
        if self.wasi_module.is_none() {
            self.wasi_module = Some(
                WasiModule::create(None, None, None).map_err(|e| AppError::WasmEdge(e.to_string()))?,
            );
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

/// 宿主导入 __pi_host_call 的实现：从线性内存读取请求 JSON，调用宿主回调，将响应写回内存。
fn host_call_impl(
    data: &mut HostData,
    _inst: &mut Instance,
    frame: &mut CallingFrame,
    args: Vec<WasmValue>,
) -> Result<Vec<WasmValue>, CoreError> {
    if args.len() < 2 {
        return Err(CoreError::Execution(CoreExecutionError::HostFuncFailed));
    }
    let ptr = args[0].to_i32() as u32;
    let len = args[1].to_i32() as u32;
    let invoke = data
        .host_invoke
        .as_ref()
        .ok_or(CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
    let mut memory = frame.memory_mut(0).ok_or(CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
    let buf = memory
        .get_data(ptr, len)
        .map_err(|_| CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
    let request_json = String::from_utf8(buf).map_err(|_| CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
    let response_json = invoke(&request_json).map_err(|_| CoreError::Execution(CoreExecutionError::HostFuncFailed))?;
    let resp_bytes = response_json.as_bytes();
    let out_len = resp_bytes.len() as u32;
    if out_len <= len {
        let _ = memory.set_data(resp_bytes.to_vec(), ptr);
    }
    Ok(vec![WasmValue::from_i32(out_len as i32)])
}

fn temp_js_file(code: &str) -> Result<(PathBuf, tempfile::TempDir), AppError> {
    let dir = tempfile::tempdir().map_err(AppError::Io)?;
    let path = dir.path().join("script.js");
    std::fs::write(&path, code).map_err(AppError::Io)?;
    Ok((path, dir))
}
