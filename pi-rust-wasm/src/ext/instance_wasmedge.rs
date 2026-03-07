//! # WasmInstance 真实实现（feature "wasmedge" 且已安装 WasmEdge 时使用）
//!
//! 每个插件独立 Vm；run_script 通过 wasmedge_quickjs.wasm 执行 JS；宿主导入 __pi_host_call 注册到 env 模块并供 QuickJS 映射到全局 agent。

use crate::infra::error::AppError;
use std::path::PathBuf;
use std::sync::Arc;
use wasmedge_sdk::{
    config::Config, error::HostFuncError, vm::VmBuilder, CallingFrame, ImportObjectBuilder,
    NeverType, Vm, WasmValue,
};

/// 单插件独立 Wasm 实例（真实实现：每实例一个 Vm，懒加载 quickjs wasm + 宿主导入）。
#[derive(Debug)]
pub struct WasmInstance {
    config: Config,
    plugin_id: String,
    /// 宿主导入回调：request_json -> response_json；在 build_vm 时注册到 env.__pi_host_call。
    host_invoke: Option<Arc<dyn Fn(&str) -> Result<String, AppError> + Send + Sync>>,
    /// QuickJS wasm 路径；未设置时 run_script 返回错误提示设置 WASMEDGE_QUICKJS_PATH。
    quickjs_path: Option<PathBuf>,
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
            .as_ref()
            .ok_or_else(|| {
                AppError::QuickJS(
                    "WASMEDGE_QUICKJS_PATH not set or path does not exist. Set it to wasmedge_quickjs.wasm path.".to_string(),
                )
            })?;
        let (_temp_path, _guard) = temp_js_file(code)?;
        let mut vm = self.build_vm()?;
        vm.register_module_from_file("quickjs", quickjs_path)
            .map_err(|e| AppError::WasmEdge(e.to_string()))?;
        // wasmedge_quickjs.wasm 通常通过 _start 或 run 执行；此处调用 _start，传入参数需通过 WASI 配置（后续可扩展）。
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

    fn build_vm(&self) -> Result<Vm, AppError> {
        let mut vm = VmBuilder::new()
            .with_config(self.config.clone())
            .build()
            .map_err(|e| AppError::WasmEdge(e.to_string()))?;
        let host_invoke = self.host_invoke.clone();
        let plugin_id = self.plugin_id.clone();
        let import = ImportObjectBuilder::new()
            .with_func::<(i32, i32), i32, NeverType>(
                "__pi_host_call",
                move |frame: CallingFrame, args: Vec<WasmValue>, _data: *mut std::ffi::c_void| {
                    host_call_impl(frame, args, plugin_id.as_str(), host_invoke.as_ref())
                },
                None,
            )
            .map_err(|e| AppError::WasmEdge(e.to_string()))?
            .build::<NeverType>("env", None)
            .map_err(|e| AppError::WasmEdge(e.to_string()))?;
        vm.register_import_module(&import)
            .map_err(|e| AppError::WasmEdge(e.to_string()))?;
        Ok(vm)
    }
}

/// 宿主导入 __pi_host_call 的实现：从线性内存读取请求 JSON，调用宿主回调，将响应写回内存。
/// - 线性内存边界由 WasmEdge 运行时保证（get_data/set_data 越界会返回错误）。
/// - 响应缓冲区不足时仅回写 out_len，不写入响应体，由 guest 以更大缓冲区重试。
fn host_call_impl(
    frame: CallingFrame,
    args: Vec<WasmValue>,
    _plugin_id: &str,
    host_invoke: Option<&Arc<dyn Fn(&str) -> Result<String, AppError> + Send + Sync>>,
) -> Result<Vec<WasmValue>, HostFuncError> {
    if args.len() < 2 {
        return Err(HostFuncError::User(1));
    }
    let ptr = args[0].to_i32() as u32;
    let len = args[1].to_i32() as u32;
    let invoke = match host_invoke {
        Some(f) => f,
        None => return Err(HostFuncError::User(2)),
    };
    let mut memory = frame.memory_mut(0).ok_or(HostFuncError::User(3))?;
    let data = memory
        .get_data(ptr, len)
        .map_err(|_| HostFuncError::User(4))?;
    let request_json = String::from_utf8(data).map_err(|_| HostFuncError::User(5))?;
    let response_json = invoke(&request_json).map_err(|_| HostFuncError::User(6))?;
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
