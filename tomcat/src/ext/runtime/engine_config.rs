//! # 插件引擎配置
//!
//! 为 rquickjs 插件引擎提供统一默认值与运行时预算配置。

/// 单实例 QuickJS 默认堆上限（MB）。
pub const DEFAULT_QUICKJS_HEAP_MB: u32 = 16;
/// 单次执行默认软超时（毫秒）。
pub const DEFAULT_PLUGIN_CALL_TIMEOUT_MS: u64 = 30_000;
/// 单次执行默认 interrupt budget。
pub const DEFAULT_PLUGIN_INTERRUPT_BUDGET: u64 = 5_000_000;
/// 长生命周期 VM 默认空闲回收阈值（毫秒）。
pub const DEFAULT_PLUGIN_IDLE_TTL_MS: u64 = 5 * 60 * 1000;

/// 进程级插件引擎配置。
#[derive(Debug, Clone)]
pub struct PluginEngineConfig {
    pub quickjs_heap_mb: u32,
    /// 单次 JS 执行片段的软超时。
    pub call_timeout_ms: u64,
    /// QuickJS interrupt handler 的预算计数；超出则中断当前执行片段。
    pub interrupt_budget: u64,
    /// 长生命周期 VM 空闲超时回收阈值。
    pub idle_ttl_ms: u64,
}

impl Default for PluginEngineConfig {
    fn default() -> Self {
        Self {
            quickjs_heap_mb: DEFAULT_QUICKJS_HEAP_MB,
            call_timeout_ms: DEFAULT_PLUGIN_CALL_TIMEOUT_MS,
            interrupt_budget: DEFAULT_PLUGIN_INTERRUPT_BUDGET,
            idle_ttl_ms: DEFAULT_PLUGIN_IDLE_TTL_MS,
        }
    }
}
