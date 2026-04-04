//! # 4 原语执行引擎默认实现
//!
//! 路径白名单、用户确认、备份、原子写入与审计；与 design CODE_BLOCK_P1_006 一致。

mod diff;
mod primitives;
#[cfg(test)]
mod tests;

pub use primitives::DefaultPrimitiveExecutor;
