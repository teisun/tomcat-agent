//! # `core::security`：跨工具安全策略
//!
//! Contains [`secrets`] for redaction and [`project_trust`] for user-approved
//! project identities; the latter has no connector-specific data.

pub mod project_trust;
pub mod secrets;

#[cfg(test)]
mod tests;
