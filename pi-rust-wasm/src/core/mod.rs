//! # 宿主核心能力层
//!
//! 4 原语、工具注册等核心引擎，仅在宿主层运行。

pub mod confirmation;
pub mod executor;
pub mod primitives;
pub mod tools;

pub use confirmation::{AllowAllConfirmation, DenyAllConfirmation, UserConfirmationProvider};
pub use executor::DefaultPrimitiveExecutor;
pub use primitives::{
    BashResult, DirEntry, EditFileResult, EditOperation, EditOperationType, PrimitiveExecutor,
    PrimitiveOperation, WriteFileResult,
};
pub use tools::{DefaultToolRegistry, Tool, ToolExecutor, ToolRegistry};
