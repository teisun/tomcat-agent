//! # `config_get` / `config_set` LLM 工具实现（plan §6）
//!
//! 让 Agent 能够"用自然语言改配置"，但同时通过键级双向白名单 + 硬黑名单
//! 防止 Agent 自我提权或泄漏敏感信息：
//!
//! - **读路径**：`CONFIG_READ_ALLOWLIST` ∪ 否定 `CONFIG_HARDCODED_READ_DENY`。
//! - **写路径**：`CONFIG_WRITE_ALLOWLIST` ∪ 否定 `CONFIG_HARDCODED_WRITE_DENY`。
//! - **数组语义**：单元素追加 only；删除 / 整数组替换返回错误并引导 `tomcat config edit`。
//! - **二次 confirm**：每次 `config_set` 都强制走 `UserConfirmationProvider::confirm`
//!   并展示 unified diff，用户拒绝直接返回 `applied=false`。
//!
//! ## 与 CLI 的职责区分
//!
//! `tomcat config get/set/edit` 是用户特权通道，**不**受这里的白名单约束；本模块仅约束
//! LLM 通过 `config_get` / `config_set` 工具的访问。两条通道共享底层 `append_*_to_disk`
//! 落盘函数 + `with_config_lock` 文件锁，保证一致性。
//!
//! ## 测试位置
//!
//! 见本模块 `tests/config_tool_test.rs`：
//! - 白名单 / hardcoded deny 矩阵
//! - 数组单元素追加正反案例
//! - confirm AllowOnce / Deny / AllowAndPersistRoot 分支

pub mod allowlist;
pub mod get;
pub mod set;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::core::agent_loop::ConfigBackend;
use crate::core::permission::PermissionGate;
use crate::core::tools::contract::confirmation::UserConfirmationProvider;
use crate::infra::config::load_config;
use crate::infra::error::AppError;

pub use allowlist::{is_array_field, is_readable, is_writable};
pub use get::config_get_impl;
pub use set::{config_set_impl, ConfigSetOutcome};

/// `config_get` / `config_set` 工具运行所需的上下文。
///
/// chat 启动时由 `ChatContext::from_config` 构造一次；之后每次工具调用都重新
/// 从 `config_path` 读取最新配置（避免内存中的 `AppConfig` 与磁盘漂移）。
pub struct ConfigToolContext {
    /// `tomcat.config.toml` 绝对路径；写盘 / 读盘均经由 `with_config_lock` 串行化。
    pub config_path: PathBuf,
    /// 二次 confirm 提供方；与 primitive 层共享同一 `CliConfirmation` 实例。
    pub confirmation: Arc<dyn UserConfirmationProvider>,
    /// 当前 chat 共享的权限 gate；用于阻止 config_set 绕过 deny，并让 path_rules 热生效。
    pub gate: Option<Arc<dyn PermissionGate>>,
}

impl ConfigToolContext {
    pub fn new(config_path: PathBuf, confirmation: Arc<dyn UserConfirmationProvider>) -> Self {
        Self {
            config_path,
            confirmation,
            gate: None,
        }
    }

    pub fn with_gate(mut self, gate: Arc<dyn PermissionGate>) -> Self {
        self.gate = Some(gate);
        self
    }
}

/// `core::agent_loop` 注入用的 `ConfigBackend` 适配器。
///
/// 与 [`ConfigToolContext`] 1:1：把 `config_path` + `confirmation` 包装为
/// trait 对象，方便 `AgentLoop::with_config_backend(Arc::new(ChatConfigBackend{...}))`。
/// `config_get` 每次都重新 `load_config`，避免内存视图与磁盘漂移；写盘走
/// `with_config_lock` 串行化。
pub struct ChatConfigBackend {
    pub ctx: ConfigToolContext,
}

#[async_trait]
impl ConfigBackend for ChatConfigBackend {
    async fn config_get(&self, key: &str) -> Result<serde_json::Value, AppError> {
        let cfg = load_config(Some(&self.ctx.config_path))?;
        config_get_impl(key, &cfg)
    }

    async fn config_set(&self, key: &str, value: &str) -> Result<serde_json::Value, AppError> {
        let outcome = config_set_impl(key, value, &self.ctx).await?;
        serde_json::to_value(&outcome)
            .map_err(|e| AppError::Config(format!("序列化 config_set 结果失败: {}", e)))
    }

    async fn package_install(
        &self,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        use crate::core::package::{PackageManager, PackageVisibility};
        use crate::core::permission::PermissionDecision;
        use crate::core::tools::contract::confirmation::ConfirmDecision;
        use crate::core::tools::primitive::PrimitiveOperation;

        let source = args["source"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppError::Config("package_install.source 不能为空".to_string()))?;
        let visibility = match args["scope"].as_str().unwrap_or("scope") {
            "scope" => PackageVisibility::Scope,
            "agent" => PackageVisibility::Agent,
            "global" => PackageVisibility::Global,
            other => {
                return Err(AppError::Config(format!(
                    "package_install.scope 无效: {other}"
                )))
            }
        };
        let force = args["force"].as_bool().unwrap_or(false);
        let source_path = std::fs::canonicalize(source).map_err(AppError::Io)?;
        let workspace = std::env::current_dir().map_err(AppError::Io)?;
        let check_gate = |operation, path: &std::path::Path| -> Result<(), AppError> {
            let Some(gate) = self.ctx.gate.as_ref() else {
                return Ok(());
            };
            match gate.check(operation, &path.to_string_lossy())? {
                PermissionDecision::Deny { reason } => Err(AppError::Permission(format!(
                    "package_install 被路径策略拒绝: {} ({reason})",
                    path.display()
                ))),
                _ => Ok(()),
            }
        };
        check_gate(PrimitiveOperation::Read, &source_path)?;
        let mut pending_reads = vec![source_path.clone()];
        while let Some(path) = pending_reads.pop() {
            check_gate(PrimitiveOperation::Read, &path)?;
            if path.is_dir() {
                for entry in std::fs::read_dir(&path).map_err(AppError::Io)? {
                    pending_reads.push(entry.map_err(AppError::Io)?.path());
                }
            }
        }

        let cfg = load_config(Some(&self.ctx.config_path))?;
        let manager = PackageManager::new(&cfg);
        let prepared = manager.prepare_install(source, visibility, Some(&workspace), force)?;
        for path in [
            &prepared.layer_paths.layer_root,
            &prepared.layer_paths.plugins_dir,
            &prepared.layer_paths.skills_dir,
            &prepared.layer_paths.package_registry_path,
            &prepared.layer_paths.plugin_registry_path,
        ] {
            check_gate(PrimitiveOperation::Write, path)?;
        }
        for resource in &prepared.resources {
            check_gate(PrimitiveOperation::Write, &resource.destination_dir)?;
        }
        let decision = self
            .ctx
            .confirmation
            .confirm_decision(
                PrimitiveOperation::Write,
                &format!("Install local package {source} into {visibility}"),
                "package_install",
                Some(workspace.clone()),
            )
            .await?;
        if matches!(decision, ConfirmDecision::Deny) {
            return Err(AppError::Permission(
                "用户取消 package_install；未写入文件".to_string(),
            ));
        }
        let resources = prepared
            .resources
            .iter()
            .map(|resource| serde_json::json!({"kind": resource.kind.as_str(), "id": resource.id}))
            .collect::<Vec<_>>();
        let outcome = manager.install(prepared)?;
        Ok(serde_json::json!({
            "installed": true,
            "package": outcome.record.name,
            "version": outcome.record.version,
            "resources": resources,
            "warnings": outcome.warnings,
            "inventory_dirty": true,
        }))
    }
}

#[cfg(test)]
mod tests;
