//! # WasmEngine 桩实现（未启用 feature "wasm" 时使用）
//!
//! 保证主库在无 WasmEdge 环境下可编译；创建实例会返回错误。

use crate::infra::error::AppError;
use std::sync::Arc;

/// Standard 模式默认：Wasm 最大页数 512 (32MB)，QuickJS 堆 16MB。
pub const DEFAULT_WASM_MAX_PAGES: u32 = 512;
pub const DEFAULT_QUICKJS_HEAP_MB: u32 = 16;

/// 引擎配置，预留内存上限等（MVP 使用固定 Standard 默认值）。
#[derive(Debug, Clone)]
pub struct WasmEngineConfig {
    pub wasm_max_pages: u32,
    pub quickjs_heap_mb: u32,
    /// wasmedge_quickjs.wasm 路径。
    #[allow(dead_code)]
    pub quickjs_path: Option<String>,
}

impl Default for WasmEngineConfig {
    fn default() -> Self {
        Self {
            wasm_max_pages: DEFAULT_WASM_MAX_PAGES,
            quickjs_heap_mb: DEFAULT_QUICKJS_HEAP_MB,
            quickjs_path: None,
        }
    }
}

/// 全局 Wasm 引擎（桩：未启用 wasm 时不可用）。
#[derive(Debug, Clone)]
pub struct WasmEngine {
    _config: WasmEngineConfig,
}

impl WasmEngine {
    /// 获取或初始化全局单例（桩实现返回错误；真实实现需接入 WasmEdge SDK）。
    pub fn global(_config: Option<WasmEngineConfig>) -> Result<Arc<Self>, AppError> {
        Err(AppError::WasmEdge(
            "WasmEdge runtime stub. Real implementation requires WasmEdge SDK integration."
                .to_string(),
        ))
    }

    /// 创建独立 Wasm 实例（桩实现返回错误）。
    pub fn create_instance(&self, _plugin_id: &str) -> Result<super::WasmInstance, AppError> {
        Err(AppError::WasmEdge(
            "WasmEdge runtime stub. Real implementation requires WasmEdge SDK.".to_string(),
        ))
    }

    /// 预留：从配置层动态设置内存上限（MVP 可传固定值）。
    #[allow(dead_code)]
    pub fn set_memory_limit(&self, _max_pages: u32) {
        // no-op in stub
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_global_returns_err_in_stub() {
        let r = WasmEngine::global(None);
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("stub"));
    }

    #[test]
    fn engine_create_instance_returns_err_in_stub() {
        let engine = WasmEngine {
            _config: WasmEngineConfig::default(),
        };
        let r = engine.create_instance("plugin-1");
        assert!(r.is_err());
    }

    #[test]
    fn config_default_standard_mode() {
        let c = WasmEngineConfig::default();
        assert_eq!(c.wasm_max_pages, DEFAULT_WASM_MAX_PAGES);
        assert_eq!(c.quickjs_heap_mb, DEFAULT_QUICKJS_HEAP_MB);
        assert!(c.quickjs_path.is_none());
    }
}
