//! # `core::security`：跨工具安全策略
//!
//! 当前仅含 [`secrets`]（写盘前敏感信息扫描，T2-P0-017 PR-M+T3-K）。
//! 后续若新增 `policies` / `gating` 等独立模块在此目录扩展。

pub mod secrets;
