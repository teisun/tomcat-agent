//! # `ConfigBackend` and `PackageInstallBackend` contracts.
//!
//! Configuration access and package installation have deliberately separate backends:
//! configuration tools reread their file by design, while installation receives an
//! immutable session snapshot so its scope cannot drift to the process working directory.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use crate::infra::error::AppError;

/// 把 `config_get` / `config_set` 工具调用透传到具体后端的契约。
///
/// 实现见 [`crate::core::tools::config_tool::ChatConfigBackend`]。
#[async_trait]
pub trait ConfigBackend: Send + Sync + 'static {
    /// 读取一个配置项；返回值会被工具直接序列化给 LLM。
    async fn config_get(&self, key: &str) -> Result<serde_json::Value, AppError>;

    /// 写入（或追加）一个配置项；返回结构化 JSON（至少含 `applied` / `message`），
    /// 由工具直接序列化给 LLM；CLI 展示提示由 `tool_exec` 额外补充。
    async fn config_set(&self, key: &str, value: &str) -> Result<serde_json::Value, AppError>;
}

/// Fully validated local package-install request.  `source` is always an absolute
/// path; callers cannot use the tool to make a relative path depend on process cwd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInstallRequest {
    pub source: PathBuf,
    pub visibility: crate::core::package::PackageVisibility,
}

impl PackageInstallRequest {
    pub fn parse(value: &serde_json::Value) -> Result<Self, AppError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawRequest {
            source: String,
            #[serde(default)]
            scope: Option<crate::core::package::PackageVisibility>,
        }

        let raw: RawRequest = serde_json::from_value(value.clone())
            .map_err(|error| AppError::Config(format!("package_install 参数无效: {error}")))?;
        if raw.source.trim().is_empty() {
            return Err(AppError::Config(
                "package_install.source 不能为空".to_string(),
            ));
        }
        let source = PathBuf::from(raw.source);
        if !source.is_absolute() {
            return Err(AppError::Config(
                "package_install.source 必须是绝对路径".to_string(),
            ));
        }
        Ok(Self {
            source,
            visibility: raw
                .scope
                .unwrap_or(crate::core::package::PackageVisibility::Scope),
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackageInstallToolResource {
    pub kind: String,
    pub id: String,
}

/// Stable machine-readable terminal state for `package_install`.
///
/// Successful calls currently return `Installed`; the other variants reserve explicit
/// values for backends that can complete a declined or failed operation without
/// surfacing a transport/tool error.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageInstallStatus {
    Installed,
    Cancelled,
    Denied,
    Failed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackageInstallToolResult {
    pub status: PackageInstallStatus,
    pub package: String,
    pub version: String,
    pub resources: Vec<PackageInstallToolResource>,
    pub warnings: Vec<String>,
    pub scope: String,
    pub target_path: String,
    pub inventory_dirty: bool,
}

#[async_trait]
pub trait PackageInstallBackend: Send + Sync + 'static {
    async fn install(
        &self,
        request: PackageInstallRequest,
    ) -> Result<PackageInstallToolResult, AppError>;
}

/// Native package installation follows the same injection pattern, but uses its own
/// immutable session context so package paths cannot inherit process cwd.
pub type SharedPackageInstallBackend = Arc<dyn PackageInstallBackend>;

/// Type alias used by the config-tool path.
pub type SharedConfigBackend = Arc<dyn ConfigBackend>;
