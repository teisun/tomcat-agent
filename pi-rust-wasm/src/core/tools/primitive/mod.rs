//! # Primitive tool executor
//!
//! Default implementation for read/write/edit/bash/list-dir primitives.

mod diff;
mod executor;
#[cfg(test)]
mod tests;

pub use executor::DefaultPrimitiveExecutor;
