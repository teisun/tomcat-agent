//! # `bash` 工具实现（T2-P0-016 PR-E）
//!
//! ## 流程
//! 1. cwd 路径预检（走 `gate_check_path`，复用 read scope）；
//! 2. 拼装审计字符串 `audit_cmd`（命令 + 参数）；
//! 3. `gate_check_bash`（whitelist / approval 三层）→ `(scope, grant)`；
//! 4. 用 `bash_parser::extract_paths` 把命令里出现的路径逐一交回 gate；
//! 5. **`spawn`** 子进程（Unix `sh -c` + 注入 wasmedge env，Windows `cmd /C`）或显式 argv；
//! 6. **`tokio::time::timeout(timeout_ms, child.wait_with_output())` 等价**：
//!    本实现用 **手工分离**——`Child::stdout/stderr.take()` → 并行 reader 任务读管道，
//!    `tokio::time::timeout(_, child.wait())` 等退出，超时 `child.kill().await + child.wait()` 收口。
//!    **禁止** `tokio::time::timeout(_, child.wait_with_output())` 反模式：`wait_with_output`
//!    会消费 `Child`，超时分支拿不到句柄做 `kill`（bash.md §2.4.3 / §6.2 / §9.2）。
//! 7. 收集 stdout / stderr / exit_code，写审计并返回。
//!
//! ## 与 PR-现状的差异（T2-P0-016 PR-E）
//! - **MUST**：`Command::output()` → `spawn` + 并行 reader + `timeout(child.wait())`；
//! - **MUST**：超时走 `child.kill().await` + `wait` 收口，`BashResult.exit_code = -1`；
//! - **TBD（Phase-E.3）**：超长输出走 `EndTruncatingAccumulator` + `persisted_output_path`，
//!   `BashResult` 结构扩 `timed_out / truncated / persisted_output_path`。当前 Phase-E.2
//!   先用「头尾保留」简化截断（不写盘），保证 `BashResult` 字段不变。

use super::helpers::{grant_trigger_str, grant_type_str, permission_scope_str};
use super::DefaultPrimitiveExecutor;
use crate::core::tools::primitive::{BashResult, PrimitiveOperation};
use crate::infra::audit::{AuditPrimitiveOp, PrimitiveAuditEntry};
use crate::infra::error::AppError;
use crate::infra::{
    DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS, DEFAULT_TOOLS_BASH_TIMEOUT_MS, MAX_TOOLS_BASH_TIMEOUT_MS,
};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

/// 解析「最终生效超时」：调用方覆盖 → executor 注入 → 兜底默认；上限统一 clamp。
///
/// `tool_exec` 入口已 clamp 一次；这里再来一次防御，保证「直调 trait 方法」的路径
/// （dispatcher / extension / 测试 mock）也走同一上限。
fn resolve_timeout_ms(executor: &DefaultPrimitiveExecutor, override_ms: Option<u64>) -> u64 {
    let configured = executor.bash_timeout_ms;
    let raw = override_ms.unwrap_or(configured);
    let raw = if raw == 0 {
        DEFAULT_TOOLS_BASH_TIMEOUT_MS
    } else {
        raw
    };
    raw.min(MAX_TOOLS_BASH_TIMEOUT_MS)
}

/// 解析「最终输出字符上限」：直接使用 executor 注入值；当前 Phase-E.2 仅用于头尾保留
/// 的简化截断，Phase-E.3 接入 `output_accum.rs` 后扩为「超限落盘 + persisted_output_path」。
fn resolve_max_output_chars(executor: &DefaultPrimitiveExecutor) -> usize {
    let v = executor.bash_max_output_chars;
    if v == 0 {
        DEFAULT_TOOLS_BASH_MAX_OUTPUT_CHARS
    } else {
        v
    }
}

/// `spawn_pipe_readers` 的返回类型别名（避开 `clippy::type_complexity`）。
type PipeReader = tokio::task::JoinHandle<std::io::Result<Vec<u8>>>;

/// 启动并行 reader：把 `Child::stdout / stderr` 边读边落入两条 `Vec<u8>`。
///
/// 不在 reader 里做截断 / 落盘——先收齐字节再交给上层逻辑（Phase-E.3 会替换为
/// `EndTruncatingAccumulator`）。reader 任务内部不依赖 `Child`，仅持有 `take()` 出来
/// 的管道 `ChildStdout` / `ChildStderr`，因此 `Child` 仍可被外层 `kill()` 杀掉
/// （`wait_with_output` 反模式是 `Child` 与 reader 同寿命）。
fn spawn_pipe_readers(child: &mut Child) -> (PipeReader, PipeReader) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(s) = stdout {
            let mut reader = BufReader::new(s);
            reader.read_to_end(&mut buf).await?;
        }
        Ok(buf)
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(s) = stderr {
            let mut reader = BufReader::new(s);
            reader.read_to_end(&mut buf).await?;
        }
        Ok(buf)
    });
    (stdout_task, stderr_task)
}

/// 简易头尾保留截断（Phase-E.2 兜底，Phase-E.3 替换为 `EndTruncatingAccumulator` + 落盘）。
///
/// 为避免在多字节 UTF-8 中间切断，按 `char_indices` 逐字符切。当总长不超过 `max_chars`
/// 时直接返回原文；否则：保留前 `max_chars / 2` 与后 `max_chars / 2` 字符，中间插入
/// `... [N chars truncated] ...`。
fn truncate_head_tail(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars || max_chars < 16 {
        return s.to_string();
    }
    let half = max_chars / 2;
    let total: Vec<(usize, char)> = s.char_indices().collect();
    let head_end = total.get(half).map(|(i, _)| *i).unwrap_or_else(|| s.len());
    let tail_start = total
        .get(total.len().saturating_sub(half))
        .map(|(i, _)| *i)
        .unwrap_or(0);
    let truncated_chars = total.len().saturating_sub(max_chars);
    format!(
        "{}\n... [truncated {} chars; full output not persisted in PR-E.2] ...\n{}",
        &s[..head_end],
        truncated_chars,
        &s[tail_start..]
    )
}

pub(super) async fn execute_bash_impl(
    executor: &DefaultPrimitiveExecutor,
    command: &str,
    cwd: Option<&str>,
    plugin_id: &str,
    argv: Option<&[String]>,
    timeout_ms_override: Option<u64>,
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

    // 把命令里出现的路径逐一交给 gate.check 处理（layer-1 deny / layer-2 confirm）。
    // 仅作"尽力而为"——shell_words 解析失败的命令，依赖 forbidden regex 兜底。
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

    let timeout_ms = resolve_timeout_ms(executor, timeout_ms_override);
    let max_output_chars = resolve_max_output_chars(executor);

    // 构造命令并强制管道（默认 inherit 会把输出直接打到 agent 进程标准流）。
    let mut cmd = match argv {
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
            let mut c = Command::new(shell);
            c.arg(shell_arg).arg(&script);
            c
        }
        Some(args) => {
            let mut c = Command::new(command);
            c.args(args);
            c
        }
    };
    cmd.current_dir(&cwd_path)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());

    // 5. spawn —— 必须 spawn 才能在超时分支拿到 Child::kill 的句柄
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Primitive(e.to_string()))?;

    // 6. 并行 reader（与下面的 wait+timeout 解耦，便于超时分支独立 kill）
    let (stdout_task, stderr_task) = spawn_pipe_readers(&mut child);

    // 6'. tokio::time::timeout 包 child.wait()
    let wait_fut = child.wait();
    let timed_out;
    let exit_code: i32 = match timeout(Duration::from_millis(timeout_ms), wait_fut).await {
        Ok(Ok(status)) => {
            timed_out = false;
            status.code().unwrap_or(-1)
        }
        Ok(Err(e)) => {
            // wait 自身错误：杀不杀都没意义（进程已不可达），但保险起见抢救一次。
            let _ = child.kill().await;
            return Err(AppError::Primitive(e.to_string()));
        }
        Err(_elapsed) => {
            // Elapsed: 子进程仍在跑 → 必须 kill（句柄还在手里），再 wait 收口收尸。
            timed_out = true;
            let _ = child.kill().await;
            // 即便 kill 失败，wait 等到子进程消失也是必要的，避免僵尸。
            let _ = child.wait().await;
            -1
        }
    };

    // 取并行 reader 的结果（reader 在子进程退出 / kill 后会读到 EOF）。
    let stdout_bytes = stdout_task
        .await
        .unwrap_or_else(|_| Ok(Vec::new()))
        .unwrap_or_default();
    let stderr_bytes = stderr_task
        .await
        .unwrap_or_else(|_| Ok(Vec::new()))
        .unwrap_or_default();

    let mut stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let mut stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();

    // Phase-E.2 简化截断（Phase-E.3 接入 EndTruncatingAccumulator + 落盘）：分别对
    // stdout / stderr 头尾保留 `max_output_chars` 字符，避免单流溢出 LLM 上下文。
    stdout = truncate_head_tail(&stdout, max_output_chars);
    stderr = truncate_head_tail(&stderr, max_output_chars);

    if timed_out {
        let hint = format!(
            "(timed out after {} ms; child killed; partial output above)",
            timeout_ms
        );
        if stderr.is_empty() {
            stderr = hint;
        } else {
            stderr.push('\n');
            stderr.push_str(&hint);
        }
    }

    executor.audit.record_primitive(PrimitiveAuditEntry {
        operation: AuditPrimitiveOp::Bash,
        path_or_cmd: audit_cmd,
        plugin_id: plugin_id.to_string(),
        user_approved: true,
        success: !timed_out && exit_code == 0,
        detail: Some(format!(
            "exit_code={} timed_out={} stdout_len={} stderr_len={} timeout_ms={}",
            exit_code,
            timed_out,
            stdout.len(),
            stderr.len(),
            timeout_ms
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_head_tail_preserves_short_input() {
        let s = "hello world";
        assert_eq!(truncate_head_tail(s, 100), s);
    }

    #[test]
    fn truncate_head_tail_skips_when_max_chars_too_small() {
        // 阈值过小（< 16）直接返原文，避免「截后比原文更长」反例
        let s = "0123456789abcdef";
        assert_eq!(truncate_head_tail(s, 8), s);
    }

    #[test]
    fn truncate_head_tail_keeps_head_and_tail() {
        let s: String = (0..200).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        let out = truncate_head_tail(&s, 32);
        assert!(out.contains("[truncated"));
        // 头部前 16 字符仍在
        assert!(out.starts_with(&s[..16]));
        // 尾部最后 16 字符仍在
        assert!(out.ends_with(&s[s.len() - 16..]));
    }

    #[test]
    fn truncate_head_tail_handles_multibyte_safely() {
        let s = "中文a".repeat(100);
        let out = truncate_head_tail(&s, 32);
        assert!(out.contains("[truncated"));
        // 不应 panic（按 char_indices 切，不会切到字符中间）
    }
}
