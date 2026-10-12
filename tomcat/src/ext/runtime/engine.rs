//! # rquickjs 引擎实现
//!
//! 迁移后插件 VM 运行于进程内 QuickJS（`rquickjs`）而非 WasmEdge。

use super::engine_config::PluginEngineConfig;
use crate::infra::error::AppError;
use std::sync::Arc;

/// 进程级插件引擎配置。
#[derive(Debug, Clone)]
pub struct PluginEngine {
    config: PluginEngineConfig,
}

impl PluginEngine {
    /// 获取引擎单例配置。
    ///
    /// 当前 rquickjs 后端无需外部二进制资产，保留 `PluginEngineConfig`
    /// 仅为兼容上层注入接口与后续内存预算参数。
    pub fn global(config: Option<PluginEngineConfig>) -> Result<Arc<Self>, AppError> {
        Ok(Arc::new(Self {
            config: config.unwrap_or_default(),
        }))
    }

    /// 为指定插件/实例创建独立 VM 壳对象。
    pub fn create_instance(
        &self,
        plugin_id: &str,
    ) -> Result<crate::ext::PluginVmInstance, AppError> {
        super::instance::PluginVmInstance::new(self.config.clone(), plugin_id.to_string())
    }

    pub(crate) fn config(&self) -> &PluginEngineConfig {
        &self.config
    }

    /// 预留：后续可接 quickjs heap / budget 动态配置。
    #[allow(dead_code)]
    pub fn set_memory_limit(&self, _max_pages: u32) {}
}

#[cfg(test)]
#[path = "tests/engine_test.rs"]
mod tests;
