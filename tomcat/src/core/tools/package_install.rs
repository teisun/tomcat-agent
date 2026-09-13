//! Native `package_install` backend.
//!
//! The LLM-facing request is parsed before this module is called.  This backend owns
//! one immutable chat-session snapshot: configuration, the explicitly selected project
//! root, the normal session working directory, and the session permission collaborators.
//! It never derives an installation root from the process working directory.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use async_trait::async_trait;

use crate::core::agent_loop::{
    PackageInstallBackend, PackageInstallRequest, PackageInstallStatus, PackageInstallToolResource,
    PackageInstallToolResult,
};
use crate::core::package::PackageManager;
use crate::core::permission::{GrantTrigger, PermissionDecision, PermissionGate};
use crate::core::tools::contract::confirmation::{ConfirmDecision, UserConfirmationProvider};
use crate::core::tools::primitive::PrimitiveOperation;
use crate::infra::config::AppConfig;
use crate::infra::error::AppError;

static NEXT_INSTALL_ATTEMPT_ID: AtomicU64 = AtomicU64::new(1);

/// Immutable values captured when the chat context is constructed.  Configuration
/// tools deliberately reread their file; package installation deliberately does not,
/// because the result must match this session's prompt and resource discovery snapshot.
pub struct PackageInstallContext {
    pub config: AppConfig,
    pub session_workspace_dir: PathBuf,
    pub session_project_root: Option<PathBuf>,
    pub confirmation: Arc<dyn UserConfirmationProvider>,
    pub gate: Option<Arc<dyn PermissionGate>>,
    pub audit_store: Option<Arc<crate::infra::AuditStore>>,
    plan_runtime: Option<Weak<crate::core::plan_runtime::PlanRuntime>>,
}

impl PackageInstallContext {
    pub fn new(
        config: AppConfig,
        session_workspace_dir: PathBuf,
        session_project_root: Option<PathBuf>,
        confirmation: Arc<dyn UserConfirmationProvider>,
    ) -> Self {
        Self {
            config,
            session_workspace_dir,
            session_project_root,
            confirmation,
            gate: None,
            audit_store: None,
            plan_runtime: None,
        }
    }

    pub fn with_gate(mut self, gate: Arc<dyn PermissionGate>) -> Self {
        self.gate = Some(gate);
        self
    }

    pub fn with_audit_store(mut self, store: Arc<crate::infra::AuditStore>) -> Self {
        self.audit_store = Some(store);
        self
    }

    /// Binds this backend to the current session's mode. The weak reference avoids
    /// making the install service own the session runtime, while keeping PLAN-mode
    /// rejection correct if the user changes mode after the backend was created.
    pub fn with_plan_runtime(
        mut self,
        runtime: &Arc<crate::core::plan_runtime::PlanRuntime>,
    ) -> Self {
        self.plan_runtime = Some(Arc::downgrade(runtime));
        self
    }
}

/// Chat implementation of the typed package-install contract.
pub struct ChatPackageInstallBackend {
    pub ctx: PackageInstallContext,
}

struct InstallAuditEvent<'a> {
    attempt_id: &'a str,
    status: &'a str,
    success: bool,
    request: &'a PackageInstallRequest,
    source_path: &'a Path,
    target_path: &'a Path,
    error: Option<&'a AppError>,
}

impl ChatPackageInstallBackend {
    fn ensure_session_allows_install(&self) -> Result<(), AppError> {
        let Some(runtime) = self.ctx.plan_runtime.as_ref().and_then(Weak::upgrade) else {
            return Ok(());
        };
        if runtime.mode() == crate::core::session::manager::AgentMode::Plan {
            return Err(AppError::Permission(
                "package_install 在 PLAN 模式不可用；请先退出计划模式再安装资源".to_string(),
            ));
        }
        Ok(())
    }

    /// Checks a managed destination. `NeedConfirm` does not widen the user's
    /// filesystem permissions: package layers are known tool-owned targets, while
    /// explicit deny/readonly rules remain a hard stop.
    fn check_managed_target(&self, path: &Path) -> Result<(), AppError> {
        let Some(gate) = self.ctx.gate.as_ref() else {
            return Ok(());
        };
        match gate.check(PrimitiveOperation::Write, &path.to_string_lossy())? {
            PermissionDecision::Deny { reason } => Err(AppError::Permission(format!(
                "package_install 目标被路径策略拒绝: {} ({reason})",
                path.display()
            ))),
            PermissionDecision::NeedConfirm { .. } | PermissionDecision::Allow { .. } => Ok(()),
        }
    }

    /// Runs the same three-way Read authorization as a regular file read. A
    /// package-install approval can never stand in for source access approval.
    fn record_install_audit(&self, event: InstallAuditEvent<'_>) -> Result<(), AppError> {
        let store = self.ctx.audit_store.as_ref().ok_or_else(|| {
            AppError::Audit(
                "package_install requires a writable audit store for agent/global".to_string(),
            )
        })?;
        let detail = serde_json::json!({
            "attempt_id": event.attempt_id,
            "status": event.status,
            "scope": event.request.visibility.as_str(),
            "source": event.source_path,
            "target": event.target_path,
            "error": event.error.map(ToString::to_string),
        });
        store.append(&crate::infra::audit_store::AuditEntryRow {
            timestamp: chrono::Utc::now().to_rfc3339(),
            payload: crate::infra::audit_store::AuditKindPayload::ToolCall {
                tool_name: "package_install".to_string(),
                plugin_id: "__agent__".to_string(),
                caller_plugin_id: "__agent__".to_string(),
                success: event.success,
                detail: Some(detail.to_string()),
            },
        })
    }

    async fn authorize_source_read(&self, path: &Path) -> Result<(), AppError> {
        let Some(gate) = self.ctx.gate.as_ref() else {
            return Ok(());
        };
        loop {
            match gate.check(PrimitiveOperation::Read, &path.to_string_lossy())? {
                PermissionDecision::Allow { .. } => return Ok(()),
                PermissionDecision::Deny { reason } => {
                    return Err(AppError::Permission(format!(
                        "package_install 来源被路径策略拒绝: {} ({reason})",
                        path.display()
                    )));
                }
                PermissionDecision::NeedConfirm {
                    reason,
                    suggested_root,
                } => {
                    let preview = format!(
                        "[Read] Read package installation source\n路径: {}\n原因: {}",
                        path.display(),
                        reason
                    );
                    match self
                        .ctx
                        .confirmation
                        .confirm_decision(
                            PrimitiveOperation::Read,
                            &preview,
                            "package_install_source",
                            suggested_root,
                        )
                        .await?
                    {
                        ConfirmDecision::Deny => {
                            return Err(AppError::Permission(format!(
                                "用户拒绝读取 package_install 来源: {}",
                                path.display()
                            )));
                        }
                        ConfirmDecision::AllowOnce => {
                            gate.grant_session(path.to_path_buf(), GrantTrigger::UserConfirm);
                        }
                        ConfirmDecision::AllowAndPersistRoot { root } => {
                            if !path.starts_with(&root) {
                                return Err(AppError::Permission(format!(
                                    "来源授权根不包含请求路径: {}",
                                    root.display()
                                )));
                            }
                            gate.grant_session(root, GrantTrigger::UserConfirm);
                        }
                    }
                }
            }
        }
    }

    async fn check_source_tree(&self, source: &Path) -> Result<(), AppError> {
        let mut pending_reads = vec![source.to_path_buf()];
        while let Some(path) = pending_reads.pop() {
            let metadata = std::fs::symlink_metadata(&path).map_err(AppError::Io)?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(AppError::Config(format!(
                    "package installation source tree must not contain symbolic links: {}",
                    path.display()
                )));
            }
            if !file_type.is_file() && !file_type.is_dir() {
                return Err(AppError::Config(format!(
                    "package installation source tree contains an unsupported special file: {}",
                    path.display()
                )));
            }
            self.authorize_source_read(&path).await?;
            if file_type.is_dir() {
                for entry in std::fs::read_dir(&path).map_err(AppError::Io)? {
                    pending_reads.push(entry.map_err(AppError::Io)?.path());
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl PackageInstallBackend for ChatPackageInstallBackend {
    async fn install(
        &self,
        request: PackageInstallRequest,
    ) -> Result<PackageInstallToolResult, AppError> {
        self.ensure_session_allows_install()?;
        let source_metadata = std::fs::symlink_metadata(&request.source).map_err(AppError::Io)?;
        if source_metadata.file_type().is_symlink() {
            return Err(AppError::Config(
                "package installation source must not be a symbolic link".to_string(),
            ));
        }
        let source_path = std::fs::canonicalize(&request.source).map_err(AppError::Io)?;
        self.check_source_tree(&source_path).await?;

        let scope_root = match request.visibility {
            crate::core::package::PackageVisibility::Scope => self
                .ctx
                .session_project_root
                .as_deref()
                .ok_or_else(|| {
                    AppError::Config(
                        "package_install.scope 需要创建会话时提供一个存在的绝对项目目录；当前会话没有项目根"
                            .to_string(),
                    )
                })?,
            crate::core::package::PackageVisibility::Agent
            | crate::core::package::PackageVisibility::Global => self.ctx.session_workspace_dir.as_path(),
        };
        let manager = PackageManager::new(&self.ctx.config);
        let prepared =
            manager.prepare_install(&source_path, request.visibility, Some(scope_root), false)?;
        for path in [
            &prepared.layer_paths.layer_root,
            &prepared.layer_paths.plugins_dir,
            &prepared.layer_paths.skills_dir,
            &prepared.layer_paths.package_registry_path,
            &prepared.layer_paths.plugin_registry_path,
        ] {
            self.check_managed_target(path)?;
        }
        for resource in &prepared.resources {
            self.check_managed_target(&resource.destination_dir)?;
        }

        let requires_install_confirmation = matches!(
            request.visibility,
            crate::core::package::PackageVisibility::Agent
                | crate::core::package::PackageVisibility::Global
        );
        let attempt_id = requires_install_confirmation.then(|| {
            format!(
                "package-install-{}",
                NEXT_INSTALL_ATTEMPT_ID.fetch_add(1, Ordering::Relaxed)
            )
        });
        if let Some(attempt_id) = attempt_id.as_deref() {
            // Agent/global writes must be auditable before asking the user to approve them.
            if self.ctx.audit_store.is_none() {
                return Err(AppError::Audit(
                    "package_install agent/global 需要可写的审计日志；未写入文件".to_string(),
                ));
            }
            let confirmed = match self
                .ctx
                .confirmation
                .confirm(
                    PrimitiveOperation::Write,
                    &format!(
                        "Install local package {} into {}",
                        source_path.display(),
                        request.visibility
                    ),
                    "package_install",
                )
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    let _ = self.record_install_audit(InstallAuditEvent {
                        attempt_id,
                        status: "cancelled",
                        success: false,
                        request: &request,
                        source_path: &source_path,
                        target_path: &prepared.layer_paths.layer_root,
                        error: Some(&error),
                    });
                    return Err(error);
                }
            };
            if !confirmed {
                self.record_install_audit(InstallAuditEvent {
                    attempt_id,
                    status: "cancelled",
                    success: false,
                    request: &request,
                    source_path: &source_path,
                    target_path: &prepared.layer_paths.layer_root,
                    error: None,
                })?;
                return Err(AppError::Permission(
                    "用户取消 package_install；未写入文件".to_string(),
                ));
            }
            self.record_install_audit(InstallAuditEvent {
                attempt_id,
                status: "approved",
                success: true,
                request: &request,
                source_path: &source_path,
                target_path: &prepared.layer_paths.layer_root,
                error: None,
            })?;
        }

        let resources = prepared
            .resources
            .iter()
            .map(|resource| PackageInstallToolResource {
                kind: resource.kind.as_str().to_string(),
                id: resource.id.clone(),
            })
            .collect();
        let target_path = prepared
            .layer_paths
            .layer_root
            .to_string_lossy()
            .to_string();
        let outcome = match manager.install(prepared) {
            Ok(outcome) => outcome,
            Err(error) => {
                if let Some(attempt_id) = attempt_id.as_deref() {
                    let _ = self.record_install_audit(InstallAuditEvent {
                        attempt_id,
                        status: "install_failed",
                        success: false,
                        request: &request,
                        source_path: &source_path,
                        target_path: Path::new(&target_path),
                        error: Some(&error),
                    });
                }
                return Err(error);
            }
        };
        if let Some(attempt_id) = attempt_id.as_deref() {
            self.record_install_audit(InstallAuditEvent {
                attempt_id,
                status: "installed",
                success: true,
                request: &request,
                source_path: &source_path,
                target_path: Path::new(&target_path),
                error: None,
            })
            .map_err(|error| AppError::Audit(format!("资源已安装，但审计记录失败: {error}")))?;
        }
        crate::api::chat::publish_resource_inventory_change();
        Ok(PackageInstallToolResult {
            status: PackageInstallStatus::Installed,
            package: outcome.record.name,
            version: outcome.record.version,
            resources,
            warnings: outcome.warnings,
            scope: request.visibility.as_str().to_string(),
            target_path,
            inventory_dirty: true,
        })
    }
}
