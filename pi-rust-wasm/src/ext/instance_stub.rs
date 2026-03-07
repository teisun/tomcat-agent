//! # WasmInstance 桩实现（未启用 feature "wasm" 时使用）

use crate::infra::error::AppError;

/// 单插件独立 Wasm 实例（桩：无真实 VM）。
#[derive(Debug)]
pub struct WasmInstance {
    plugin_id: String,
}

impl WasmInstance {
    /// 桩：不允许直接构造，由 WasmEngine::create_instance 创建。
    #[allow(dead_code)]
    pub(crate) fn new(plugin_id: String) -> Self {
        Self { plugin_id }
    }

    /// 执行 JS 代码（桩实现返回错误；真实实现需 QuickJS wasm）。
    pub fn run_script(&mut self, _code: &str) -> Result<serde_json::Value, AppError> {
        Err(AppError::QuickJS(
            "WasmEdge QuickJS stub. Real implementation requires QuickJS wasm.".to_string(),
        ))
    }

    /// 注册宿主导入并映射到 QuickJS 全局 `agent`（桩：无操作，真实实现见 008）。
    pub fn register_host_binding(
        &mut self,
        _invoke_fn: impl Fn(&str) -> Result<String, AppError> + Send + Sync + 'static,
    ) -> Result<(), AppError> {
        let _ = self.plugin_id.as_str();
        Ok(())
    }

    /// 销毁实例，释放资源（桩：无操作）。
    pub fn destroy(self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_script_returns_err_in_stub() {
        let mut inst = WasmInstance::new("p1".to_string());
        let r = inst.run_script("1+1");
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("stub"));
    }

    #[test]
    fn register_host_binding_ok_in_stub() {
        let mut inst = WasmInstance::new("p1".to_string());
        let r = inst.register_host_binding(|_| Err(AppError::Config("x".to_string())));
        assert!(r.is_ok());
    }

    #[test]
    fn destroy_consumes_instance() {
        let inst = WasmInstance::new("p1".to_string());
        inst.destroy();
    }
}
