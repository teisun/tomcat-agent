//! # Primitive tool executor
//!
//! Default implementation for read/write/edit/bash/list-dir primitives.

pub mod confirmation;
mod diff;
mod executor;
#[cfg(test)]
mod tests;
mod types;

pub use confirmation::{
    AllowAllConfirmation, ConfirmDecision, DenyAllConfirmation, UserConfirmationProvider,
};
pub use executor::DefaultPrimitiveExecutor;
pub use types::{
    BashResult, DirEntry, EditFileResult, EditOperation, EditOperationType, PrimitiveExecutor,
    PrimitiveOperation, WriteFileResult,
};
