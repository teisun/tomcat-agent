//! `pi chat` 会话入口的非阻塞预检。
//!
//! 预检只负责尽早补齐 `search_files` Tier1 依赖（rg/fd）；失败不影响聊天，
//! `search_files` 仍会在运行时自动回落到 Tier2。
//!
//! **墙钟**：包管理器子进程使用 [`std::process::Command::output`]，无 pi 侧超时；
//! 勿与环境变量 `PI_SEARCH_TIER2_DEADLINE_MS` 混淆（后者仅约束 `search_files` Tier2 兜底路径的墙钟）。

use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Output};
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;

use crate::infra::{wire, AppConfig, EventBus, EventContext};

const SKIP_ENV: &str = "PI_SKIP_SEARCH_TOOLS_PREFLIGHT";

/// `tracing` target：仅开预检诊断时用 `RUST_LOG=pi_wasm_preflight=debug`。
pub(crate) const TRACE_TARGET: &str = "pi_wasm_preflight";

pub(crate) fn start_search_tools_preflight(config: &AppConfig, event_bus: Arc<dyn EventBus>) {
    if should_skip_preflight(config) {
        tracing::debug!(
            target: TRACE_TARGET,
            "search_tools preflight skipped ({} set or preflight.auto_install_search_tools=false)",
            SKIP_ENV
        );
        return;
    }

    std::thread::spawn(move || {
        let started = Instant::now();
        tracing::debug!(target: TRACE_TARGET, "search_tools preflight thread started");

        let missing = missing_search_tools();
        if missing.is_empty() {
            tracing::debug!(
                target: TRACE_TARGET,
                "search_files Tier1 binaries already on PATH"
            );
            emit_preflight(
                &*event_bus,
                "ready",
                "search_files Tier1 tools are already available",
                json!({ "missing": [] }),
            );
            return;
        }

        emit_preflight(
            &*event_bus,
            "start",
            "search_files Tier1 tools missing; attempting background install",
            json!({ "missing": missing }),
        );

        let Some(plan) = install_plan() else {
            tracing::debug!(
                target: TRACE_TARGET,
                "no supported package manager for automatic install"
            );
            emit_preflight(
                &*event_bus,
                "failed",
                "No supported package manager found for automatic search tool installation",
                json!({ "missing": missing_search_tools() }),
            );
            return;
        };

        tracing::debug!(
            target: TRACE_TARGET,
            program = plan.program,
            args = ?plan.args,
            "search_tools preflight running package manager"
        );

        emit_preflight(
            &*event_bus,
            "progress",
            &format!("running: {} {}", plan.program, plan.args.join(" ")),
            json!({ "program": plan.program, "args": plan.args }),
        );

        let output = StdCommand::new(plan.program).args(&plan.args).output();
        match output {
            Ok(output) => {
                let log_path = write_install_log(&plan, &output);
                if let Some(ref p) = log_path {
                    tracing::debug!(
                        target: TRACE_TARGET,
                        path = %p.display(),
                        "search_tools preflight install log written"
                    );
                } else {
                    tracing::debug!(
                        target: TRACE_TARGET,
                        "search_tools preflight install log not written (I/O or home dir)"
                    );
                }
                let stderr_lossy = String::from_utf8_lossy(&output.stderr);
                tracing::debug!(
                    target: TRACE_TARGET,
                    elapsed_ms = started.elapsed().as_millis(),
                    exit_ok = output.status.success(),
                    stdout_len = output.stdout.len(),
                    stderr_len = output.stderr.len(),
                    stderr_tail_chars = trim_for_event(&stderr_lossy).chars().count(),
                    "search_tools preflight command finished"
                );
                if output.status.success() {
                    let mut extra = json!({
                        "elapsedMs": started.elapsed().as_millis(),
                        "missingAfter": missing_search_tools(),
                    });
                    if let Some(p) = log_path {
                        extra["logPath"] = json!(p.display().to_string());
                    }
                    emit_preflight(
                        &*event_bus,
                        "success",
                        "search_files Tier1 tools installation finished",
                        extra,
                    );
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let mut extra = json!({
                        "exitCode": output.status.code(),
                        "stderr": trim_for_event(&stderr),
                        "elapsedMs": started.elapsed().as_millis(),
                    });
                    if let Some(p) = log_path {
                        extra["logPath"] = json!(p.display().to_string());
                    }
                    emit_preflight(
                        &*event_bus,
                        "failed",
                        "search_files Tier1 tools installation failed; Tier2 fallback remains available",
                        extra,
                    );
                }
            }
            Err(err) => {
                tracing::debug!(
                    target: TRACE_TARGET,
                    error = %err,
                    "search_tools preflight could not spawn package manager"
                );
                emit_preflight(
                    &*event_bus,
                    "failed",
                    "search_files Tier1 tools installation could not be started; Tier2 fallback remains available",
                    json!({
                        "error": err.to_string(),
                        "elapsedMs": started.elapsed().as_millis(),
                    }),
                );
            }
        }
    });
}

fn should_skip_preflight(config: &AppConfig) -> bool {
    if std::env::var_os(SKIP_ENV).is_some() {
        return true;
    }
    !config.preflight.auto_install_search_tools
}

fn missing_search_tools() -> Vec<&'static str> {
    let mut missing = Vec::new();
    if find_binary(&["rg", "ripgrep"]).is_none() {
        missing.push("ripgrep");
    }
    if find_binary(&["fd", "fdfind"]).is_none() {
        missing.push("fd");
    }
    missing
}

struct InstallPlan {
    program: &'static str,
    args: Vec<&'static str>,
}

fn install_plan() -> Option<InstallPlan> {
    if is_termux() && find_binary(&["pkg"]).is_some() {
        return Some(InstallPlan {
            program: "pkg",
            args: vec!["install", "-y", "ripgrep", "fd"],
        });
    }

    #[cfg(target_os = "android")]
    {
        return None;
    }

    if cfg!(target_os = "macos") && find_binary(&["brew"]).is_some() {
        return Some(InstallPlan {
            program: "brew",
            args: vec!["install", "ripgrep", "fd"],
        });
    }

    if cfg!(target_os = "windows") && find_binary(&["winget"]).is_some() {
        return Some(InstallPlan {
            program: "cmd",
            args: vec![
                "/C",
                "winget install --id BurntSushi.ripgrep.MSVC --silent --accept-source-agreements --accept-package-agreements && winget install --id sharkdp.fd --silent --accept-source-agreements --accept-package-agreements",
            ],
        });
    }

    if find_binary(&["apt-get"]).is_some() {
        return Some(InstallPlan {
            program: "apt-get",
            args: vec!["install", "-y", "ripgrep", "fd-find"],
        });
    }
    if find_binary(&["dnf"]).is_some() {
        return Some(InstallPlan {
            program: "dnf",
            args: vec!["install", "-y", "ripgrep", "fd-find"],
        });
    }
    if find_binary(&["pacman"]).is_some() {
        return Some(InstallPlan {
            program: "pacman",
            args: vec!["-S", "--noconfirm", "ripgrep", "fd"],
        });
    }
    None
}

fn is_termux() -> bool {
    std::env::var_os("TERMUX_VERSION").is_some()
        || Path::new("/data/data/com.termux/files/usr/bin/pkg").exists()
}

fn find_binary(candidates: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for candidate in candidates {
            let path = dir.join(candidate);
            if path.is_file() {
                return Some(path);
            }
            #[cfg(windows)]
            {
                let exe = dir.join(format!("{}.exe", candidate));
                if exe.is_file() {
                    return Some(exe);
                }
            }
        }
    }
    None
}

fn preflight_log_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(home.join(".pi_").join("agents").join("main").join("logs"))
}

/// 将安装子进程完整 stdout/stderr 写入 `~/.pi_/agents/main/logs/preflight-file-log-<ts>.log`。
/// 失败时静默返回 `None`（不打断预检）。
fn write_install_log(plan: &InstallPlan, output: &Output) -> Option<PathBuf> {
    let dir = preflight_log_dir()?;
    if std::fs::create_dir_all(&dir).is_err() {
        tracing::warn!(
            target: TRACE_TARGET,
            "search_tools preflight: could not create log directory"
        );
        return None;
    }
    let stamp = chrono::Local::now().format("%Y%m%dT%H%M%S%.6f");
    let path = dir.join(format!("preflight-file-log-{stamp}.log"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut buf = String::new();
    buf.push_str(&format!(
        "timestamp={}\nos={}\nprogram={}\nargs={:?}\nexit_code={:?}\n",
        chrono::Local::now().to_rfc3339(),
        std::env::consts::OS,
        plan.program,
        plan.args,
        output.status.code()
    ));
    buf.push_str("\n--- stdout ---\n");
    buf.push_str(stdout.as_ref());
    buf.push_str("\n--- stderr ---\n");
    buf.push_str(stderr.as_ref());
    buf.push('\n');

    let mut f = std::fs::File::create(&path).ok()?;
    if f.write_all(buf.as_bytes()).is_err() {
        tracing::warn!(
            target: TRACE_TARGET,
            "search_tools preflight: could not write install log file"
        );
        return None;
    }
    Some(path)
}

fn emit_preflight(bus: &dyn EventBus, status: &str, message: &str, extra: serde_json::Value) {
    tracing::debug!(
        target: TRACE_TARGET,
        wire_status = %status,
        message = %message,
        "search_tools_preflight emit"
    );
    let payload = json!({
        "status": status,
        "message": message,
        "extra": extra,
    });
    let _ = bus.emit_sync(
        wire::WIRE_SEARCH_TOOLS_PREFLIGHT,
        EventContext::new(wire::WIRE_SEARCH_TOOLS_PREFLIGHT, payload),
    );
}

/// 与事件 `extra.stderr` 使用同一截断规则，供终端摘要展示。
pub(crate) fn trim_for_event(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() > 500 {
        trimmed.chars().take(500).collect::<String>() + "..."
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_skip_preflight_when_config_disables_auto_install() {
        let mut cfg = AppConfig::default();
        cfg.preflight.auto_install_search_tools = false;
        assert!(should_skip_preflight(&cfg));
    }

    #[test]
    fn trim_for_event_limits_long_messages() {
        let input = "x".repeat(600);
        let out = trim_for_event(&input);
        assert!(out.ends_with("..."));
        assert!(out.len() < input.len());
    }
}
