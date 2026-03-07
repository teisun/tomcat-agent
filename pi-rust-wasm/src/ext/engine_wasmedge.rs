//! # WasmEngine 真实实现（feature "wasmedge" 且已安装 WasmEdge 时使用）
//!
//! 依赖 WasmEdge C 库与 wasmedge-sdk；构建时需已安装 WasmEdge（见 https://wasmedge.org/docs/start/install）。

use super::engine_stub::{WasmEngineConfig, DEFAULT_QUICKJS_HEAP_MB, DEFAULT_WASM_MAX_PAGES};
use crate::infra::error::AppError;
use std::path::PathBuf;
use std::sync::Arc;
use wasmedge_sdk::{
    config::{
        CommonConfigOptions, Config, ConfigBuilder, HostRegistrationConfigOptions,
        RuntimeConfigOptions, StatisticsConfigOptions,
    },
    vm::VmBuilder,
};

/// 全局 Wasm 引擎（真实实现：WasmEdge Config + 单例）。
#[derive(Debug)]
pub struct WasmEngine {
    config: Config,
    /// 已规范化的 QuickJS wasm 路径；None 时 instance 回退到环境变量 WASMEDGE_QUICKJS_PATH。
    quickjs_path: Option<PathBuf>,
}

impl WasmEngine {
    /// 获取或初始化全局单例；开启 WASI、统计与内存上限。
    ///
    /// # Errors
    /// * [`AppError::WasmEdge`] - WasmEdge Config 构建失败时返回。
    pub fn global(config: Option<WasmEngineConfig>) -> Result<Arc<Self>, AppError> {
        let cfg = config.unwrap_or_default();
        let common = CommonConfigOptions::default()
            .bulk_memory_operations(true)
            .multi_value(true)
            .reference_types(true);
        let stat = StatisticsConfigOptions::default()
            .count_instructions(true)
            .measure_cost(true)
            .measure_time(true);
        let runtime = RuntimeConfigOptions::default().max_memory_pages(cfg.wasm_max_pages);
        let host = HostRegistrationConfigOptions::default().wasi(true);
        let config = ConfigBuilder::new(common)
            .with_statistics_config(stat)
            .with_runtime_config(runtime)
            .with_host_registration_config(host)
            .build()
            .map_err(|e| AppError::WasmEdge(e.to_string()))?;
        let quickjs_path = cfg
            .quickjs_path
            .filter(|s| !s.trim().is_empty())
            .and_then(|s| crate::normalize_path(s.trim()).ok())
            .filter(|p| p.exists());
        Ok(Arc::new(Self {
            config,
            quickjs_path,
        }))
    }

    /// 创建独立 Wasm 实例（每插件一个实例，实例间隔离）。
    ///
    /// # Errors
    /// * [`AppError::WasmEdge`] - 创建 Vm 或实例失败时返回。
    pub fn create_instance(&self, plugin_id: &str) -> Result<crate::ext::WasmInstance, AppError> {
        super::instance_wasmedge::WasmInstance::new(
            self.config.clone(),
            plugin_id.to_string(),
            self.quickjs_path.clone(),
        )
    }

    /// 预留：从配置层动态设置内存上限（MVP 可传固定值）。
    #[allow(dead_code)]
    pub fn set_memory_limit(&self, _max_pages: u32) {
        // 当前使用 Config 构建时已设 max_memory_pages；后续可从配置层动态更新
    }
}
