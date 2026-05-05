//! # `execute_bash` 实现
//!
//! 流程（与 mod.rs 头注释 ③ 对应）：
//! 1. cwd 路径预检（走 `gate_check_path`，复用 read scope）；
//! 2. 拼装审计字符串 `audit_cmd`（命令 + 参数）；
//! 3. `gate_check_bash`（whitelist / approval 三层）→ `(scope, grant)`；
//! 4. 用 `bash_parser::extract_paths` 把命令里出现的路径逐一交回 gate；
//! 5. spawn shell（unix `sh -c` 注入 wasmedge env，windows `cmd /C`）或显式 argv；
//! 6. 收集 stdout / stderr / exit_code，写审计并返回。

use super::helpers::{grant_trigger_str, grant_type_str, permission_scope_str};
use super::DefaultPrimitiveExecutor;
use crate::core::tools::primitive::{BashResult, PrimitiveOperation};
use crate::infra::audit::{AuditPrimitiveOp, PrimitiveAuditEntry};
use crate::infra::error::AppError;
use std::path::PathBuf;
use tokio::process::Command;

/// bash 执行默认超时（预留，后续可配合 tokio::time::timeout 使用）。
#[allow(dead_code)]
const BASH_TIMEOUT_SECS: u64 = 30;

pub(super) async fn execute_bash_impl(
    executor: &DefaultPrimitiveExecutor,
    command: &str,
    cwd: Option<&str>,
    plugin_id: &str,
    argv: Option<&[String]>,
) -> Result<BashResult, AppError> {
    let cwd_path = if let Some(c) = cwd {
        let (p, _l, _s) = executor
            .gate_check_path(PrimitiveOperation::Read, c, plugin_id)
            .await?;
        p
    } else {
        PathBuf::from(".")
    };

    let audit_cmd = match argv {
        None => command.to_string(),
        Some(args) => {
            let mut s = command.to_string();
            for a in args {
                s.push(' ');
                s.push_str(a);
            }
            s
        }
    };

    // bash 决策来源（whitelist / approval）—— 走 gate 三层。
    let (bash_scope, bash_grant) = match executor.gate_check_bash(&audit_cmd, plugin_id).await {
        Ok((scope, grant)) => (scope, grant),
        Err(e) => {
            executor.audit.record_primitive(PrimitiveAuditEntry {
                operation: AuditPrimitiveOp::Bash,
                path_or_cmd: audit_cmd.clone(),
                plugin_id: plugin_id.to_string(),
                user_approved: false,
                success: false,
                detail: Some(e.to_string()),
                ..Default::default()
            });
            return Err(e);
        }
    };

    // 然后把命令里出现的路径逐一交给 gate.check 处理（layer-1 deny / layer-2 confirm）。
    // 仅作"尽力而为"——shell_words 解析失败的命令，依赖 forbidden regex 兜底。
    // 见 docs/TODOS.md `T-147`：动态路径访问的静态预检盲区由后续提示词注入防御方向覆盖。
    for raw in crate::core::permission::bash_parser::extract_paths(&audit_cmd) {
        if let Err(e) = executor
            .gate_check_path(PrimitiveOperation::Bash, &raw, plugin_id)
            .await
        {
            executor.audit.record_primitive(PrimitiveAuditEntry {
                operation: AuditPrimitiveOp::Bash,
                path_or_cmd: audit_cmd.clone(),
                plugin_id: plugin_id.to_string(),
                user_approved: false,
                success: false,
                detail: Some(format!("路径未授权: {} ({})", raw, e)),
                ..Default::default()
            });
            return Err(e);
        }
    }

    let output = match argv {
        None => {
            #[cfg(unix)]
            let script = {
                let env_path = executor
                    .config
                    .wasmedge_env_path
                    .as_deref()
                    .unwrap_or(r#"$HOME/.wasmedge/env"#);
                format!(r#"[ -f "{0}" ] && . "{0}"; {1}"#, env_path, command)
            };
            #[cfg(windows)]
            let script = command.to_string();
            #[cfg(unix)]
            let (shell, shell_arg) = ("sh", "-c");
            #[cfg(windows)]
            let (shell, shell_arg) = ("cmd", "/C");
            Command::new(shell)
                .arg(shell_arg)
                .arg(&script)
                .current_dir(&cwd_path)
                .kill_on_drop(true)
                .output()
                .await
        }
        Some(args) => {
            let mut cmd = Command::new(command);
            cmd.args(args)
                .current_dir(&cwd_path)
                .kill_on_drop(true)
                .output()
                .await
        }
    }
    .map_err(|e| AppError::Primitive(e.to_string()))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit_code = output.status.code().unwrap_or(-1);

    executor.audit.record_primitive(PrimitiveAuditEntry {
        operation: AuditPrimitiveOp::Bash,
        path_or_cmd: audit_cmd,
        plugin_id: plugin_id.to_string(),
        user_approved: true,
        success: exit_code == 0,
        detail: Some(format!(
            "exit_code={} stdout_len={} stderr_len={}",
            exit_code,
            stdout.len(),
            stderr.len()
        )),
        permission_scope: Some(permission_scope_str(bash_scope)),
        grant_type: Some(grant_type_str(bash_grant.grant_type)),
        grant_trigger: Some(grant_trigger_str(bash_grant.trigger)),
    });
    Ok(BashResult {
        stdout,
        stderr,
        exit_code,
    })
}
