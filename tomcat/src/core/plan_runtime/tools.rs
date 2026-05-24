//! 兼容层：在 import 全量 rewiring 完成前，保留旧的 plan-runtime tools 别名，
//! 指向新的 `core::tools::plan_tool::*`。

pub use crate::core::tools::plan_tool::*;
