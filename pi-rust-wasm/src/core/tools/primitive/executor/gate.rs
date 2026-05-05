//! # PermissionGate 桥接 + bash 命令决策
//!
//! `gate_check_path` / `gate_check_bash` 是所有原语进入业务执行前的「最后一公里」，
//! 把 [`PermissionGate`] 三层决策（Allow / Deny / NeedConfirm）翻译成
//! `Result<(PathBuf, scope, grant), AppError>`，并在 NeedConfirm 命中时通过
//! [`UserConfirmationProvider`] 与人交互、把用户的 `AllowOnce` /
//! `AllowAndPersistRoot` 落到 `SessionGrants` 后**重新走 gate**——直到拿到
//! 明确的 Allow / Deny 才返回。
//!
//! `run_search_command` 是 `search_files` Tier1 spawn 子进程的统一超时入口，
//! 与 gate 决策无直接关系，但放在同文件保持「权限/超时类外围流水」内聚。

use super::helpers::op_summary;
use super::DefaultPrimitiveExecutor;
use crate::core::permission::{
    GrantTrace, GrantTrigger, GrantType, PermissionDecision, PermissionScope,
};
use crate::core::tools::primitive::{ConfirmDecision, PrimitiveOperation};
use crate::infra::error::AppError;
use crate::infra::platform::normalize_path;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

impl DefaultPrimitiveExecutor {
    /// 经 gate 决定一个原语对路径的访问，必要时弹 confirm 完成 layer-2。
    ///
    /// 返回 `Ok((path_buf, scope, grant))` 表示放行；
    /// `Err(AppError::Permission)` 表示被 gate 拒绝或用户拒绝 confirm。
    pub(super) async fn gate_check_path(
        &self,
        op: PrimitiveOperation,
        path: &str,
        plugin_id: &str,
    ) -> Result<(PathBuf, PermissionScope, GrantTrace), AppError> {
        let gate = &self.gate;
        let normalized = normalize_path(path)?;
        loop {
            let decision = gate.check(op, &normalized.to_string_lossy())?;
            match decision {
                PermissionDecision::Allow { grant, scope } => {
                    return Ok((normalized, scope, grant))
                }
                PermissionDecision::Deny { reason } => {
                    return Err(AppError::Permission(reason));
                }
                PermissionDecision::NeedConfirm {
                    reason,
                    suggested_root,
                } => {
                    let preview = format!(
                        "[{:?}] {}\n路径: {}\n原因: {}",
                        op,
                        op_summary(op),
                        normalized.display(),
                        reason
                    );
                    let dec = self
                        .confirmation
                        .confirm_decision(op, &preview, plugin_id, suggested_root.clone())
                        .await?;
                    match dec {
                        ConfirmDecision::Deny => {
                            return Err(AppError::Permission(format!(
                                "用户拒绝授权: {}。下次工具再次访问该路径时会重新弹出 [s]/[w]/[c] 授权选项；也可以执行 `pi workspace add {}` 一次性永久授权。",
                                normalized.display(),
                                normalized.display()
                            )));
                        }
                        ConfirmDecision::AllowOnce => {
                            // 落 SessionGrants：AllowOnce 授权当前目标路径本身。
                            gate.grant_session(normalized.clone(), GrantTrigger::UserConfirm);
                            // 重新 check：现在应该 Allow。
                            continue;
                        }
                        ConfirmDecision::AllowAndPersistRoot { root } => {
                            // 1) 同时落 SessionGrants（本会话生效）。
                            gate.grant_session(root.clone(), GrantTrigger::UserConfirm);
                            // 2) 持久化由 caller（CLI confirm 实现）负责调用
                            //    `pi workspace add` 等价的 append_workspace_root_to_disk；
                            //    这里只标记会话授权，避免和 disk 写入耦合。
                            //    重新 check 应 Allow。
                            continue;
                        }
                    }
                }
            }
        }
    }

    /// 经 gate 决定一条 bash 命令是否放行；layer-2 命中弹 confirm。
    pub(super) async fn gate_check_bash(
        &self,
        command: &str,
        plugin_id: &str,
    ) -> Result<(PermissionScope, GrantTrace), AppError> {
        let gate = &self.gate;
        let decision = gate.check_bash(command)?;
        match decision {
            PermissionDecision::Allow { grant, scope } => Ok((scope, grant)),
            PermissionDecision::Deny { reason } => Err(AppError::Permission(reason)),
            PermissionDecision::NeedConfirm { reason, .. } => {
                let preview = format!(
                    "[Bash] 危险命令命中确认列表\n命令: {}\n原因: {}",
                    command, reason
                );
                let dec = self
                    .confirmation
                    .confirm_decision(PrimitiveOperation::Bash, &preview, plugin_id, None)
                    .await?;
                match dec {
                    ConfirmDecision::AllowOnce | ConfirmDecision::AllowAndPersistRoot { .. } => {
                        Ok((
                            PermissionScope::BashApproval,
                            GrantTrace::new(GrantType::BashPolicy, GrantTrigger::UserConfirm),
                        ))
                    }
                    ConfirmDecision::Deny => {
                        Err(AppError::Permission("用户拒绝 bash 确认".to_string()))
                    }
                }
            }
        }
    }

    pub(super) async fn run_search_command(
        &self,
        mut command: Command,
        timeout_secs: u64,
    ) -> Result<std::process::Output, AppError> {
        match tokio::time::timeout(Duration::from_secs(timeout_secs), command.output()).await {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(e)) => Err(AppError::Primitive(e.to_string())),
            Err(_) => Err(AppError::Primitive(format!(
                "search_files timed out after {}s. Narrow path/glob or lower head_limit.",
                timeout_secs
            ))),
        }
    }
}
