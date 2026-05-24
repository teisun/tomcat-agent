//! `tomcat chat` 会话入口的非阻塞预检。
//!
//! 预检只负责尽早补齐 `search_files` Tier1 依赖（rg/fd）；失败不影响聊天，
//! `search_files` 仍会在运行时自动回落到 Tier2。
//!
//! **墙钟**：Windows 上包管理器仍用 [`std::process::Command::output`]（阻塞至结束），无宿主侧超时。
//! **Unix**：安装命令通过 `nohup … >> 日志 &` 脱离会话后台运行，tomcat 仅 `spawn` shell，不等待 brew/apt 完成。
//! 勿与环境变量 `PI_SEARCH_TIER2_DEADLINE_MS` 混淆（后者仅约束 `search_files` Tier2 兜底路径的墙钟）。

#[cfg(windows)]
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
#[cfg(windows)]
use std::process::Output;
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use shell_words;

use crate::core::SwitchingCheckpointStore;
use crate::infra::{wire, AppConfig, EventBus, EventContext};

const SKIP_ENV: &str = "PI_SKIP_SEARCH_TOOLS_PREFLIGHT";

/// 最近一次 detached 安装的日志路径（仅 UX；**不用作**「是否仍在安装」的判定）。
pub(crate) const DETACHED_LOG_MARKER_NAME: &str = "preflight-detached-log.marker";
#[cfg(unix)]
const GIT_DETACHED_LOG_MARKER_NAME: &str = "preflight-detached-git-log.marker";

/// `tracing` target：仅开预检诊断时用 `RUST_LOG=tomcat_preflight=debug`。
pub(crate) const TRACE_TARGET: &str = "tomcat_preflight";

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
            #[cfg(unix)]
            remove_detached_log_marker_file(DETACHED_LOG_MARKER_NAME);
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

        #[cfg(unix)]
        {
            run_unix_preflight_install(&*event_bus, &plan, started);
        }

        #[cfg(windows)]
        {
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
        }
    });
}

pub fn start_git_preflight(
    config: &AppConfig,
    event_bus: Arc<dyn EventBus>,
    checkpoint_switcher: Arc<SwitchingCheckpointStore>,
) {
    if should_skip_git_preflight(config) {
        tracing::debug!(
            target: TRACE_TARGET,
            "git preflight skipped (preflight.auto_install_git=false)"
        );
        return;
    }

    std::thread::spawn(move || {
        let started = Instant::now();
        if find_binary(&["git"]).is_some() {
            #[cfg(unix)]
            remove_detached_log_marker_file(GIT_DETACHED_LOG_MARKER_NAME);
            checkpoint_switcher.force_activate_shadow();
            emit_git_preflight(
                &*event_bus,
                "ready",
                "git 已可用，checkpoint 将使用影子仓库",
                json!({}),
            );
            return;
        }

        emit_git_preflight(
            &*event_bus,
            "start",
            "git 缺失，正在尝试后台安装以启用 checkpoint",
            json!({}),
        );

        let Some(plan) = git_install_plan() else {
            emit_git_preflight(
                &*event_bus,
                "failed",
                "未找到可用于自动安装 git 的包管理器",
                json!({}),
            );
            return;
        };

        #[cfg(unix)]
        {
            run_unix_git_preflight_install(&*event_bus, &plan, started);
        }

        #[cfg(windows)]
        {
            let output = StdCommand::new(plan.program).args(&plan.args).output();
            match output {
                Ok(output) if output.status.success() && find_binary(&["git"]).is_some() => {
                    checkpoint_switcher.force_activate_shadow();
                    emit_git_preflight(
                        &*event_bus,
                        "success",
                        "git 安装完成，后续 checkpoint 将自动启用",
                        json!({
                            "elapsedMs": started.elapsed().as_millis(),
                        }),
                    );
                }
                Ok(output) => {
                    emit_git_preflight(
                        &*event_bus,
                        "failed",
                        "git 安装未成功完成，checkpoint 仍将退化为 Noop",
                        json!({
                            "elapsedMs": started.elapsed().as_millis(),
                            "stderr": trim_for_event(&String::from_utf8_lossy(&output.stderr)),
                            "exitCode": output.status.code(),
                        }),
                    );
                }
                Err(err) => {
                    emit_git_preflight(
                        &*event_bus,
                        "failed",
                        "git 安装命令无法启动，checkpoint 仍将退化为 Noop",
                        json!({
                            "elapsedMs": started.elapsed().as_millis(),
                            "error": err.to_string(),
                        }),
                    );
                }
            }
        }
    });
}

#[cfg(unix)]
fn run_unix_preflight_install(bus: &dyn EventBus, plan: &InstallPlan, started: Instant) {
    remove_detached_log_marker_if_homebrew_idle(DETACHED_LOG_MARKER_NAME);

    if brew_install_already_in_progress(plan) {
        let mut extra = json!({});
        if let Some(p) = read_valid_detached_marker_log_path(DETACHED_LOG_MARKER_NAME) {
            extra["logPath"] = json!(p.display().to_string());
        }
        emit_preflight(
            bus,
            "already_installing",
            "search_files Tier1 安装已在后台进行中",
            extra,
        );
        return;
    }

    let Some(log_path) = new_preflight_log_file_path() else {
        tracing::warn!(
            target: TRACE_TARGET,
            "search_tools preflight: could not allocate log path for detached install"
        );
        emit_preflight(
            bus,
            "failed",
            "search_files Tier1 tools installation could not prepare log file; Tier2 fallback remains available",
            json!({
                "elapsedMs": started.elapsed().as_millis(),
            }),
        );
        return;
    };

    match spawn_unix_detached_install(plan, &log_path) {
        Ok(()) => {
            write_detached_log_marker(DETACHED_LOG_MARKER_NAME, &log_path);
            let extra = json!({
                "logPath": log_path.display().to_string(),
                "elapsedMs": started.elapsed().as_millis(),
            });
            let detached_msg = if plan.program == "brew" {
                "search_files Tier1 安装已在后台继续（Homebrew 仅 bottle、禁止源码编译）；退出 chat 不影响"
            } else {
                "search_files Tier1 安装已在后台继续，退出 chat 不影响"
            };
            emit_preflight(bus, "detached", detached_msg, extra);
        }
        Err(err) => {
            tracing::warn!(
                target: TRACE_TARGET,
                error = %err,
                "search_tools preflight detached spawn failed"
            );
            emit_preflight(
                bus,
                "failed",
                "search_files Tier1 tools installation could not be started; Tier2 fallback remains available",
                json!({
                    "error": err.to_string(),
                    "elapsedMs": started.elapsed().as_millis(),
                }),
            );
        }
    }
}

#[cfg(unix)]
fn run_unix_git_preflight_install(bus: &dyn EventBus, plan: &InstallPlan, started: Instant) {
    remove_detached_log_marker_if_homebrew_idle(GIT_DETACHED_LOG_MARKER_NAME);

    if brew_install_already_in_progress(plan) {
        let mut extra = json!({
            "elapsedMs": started.elapsed().as_millis(),
        });
        if let Some(p) = read_valid_detached_marker_log_path(GIT_DETACHED_LOG_MARKER_NAME) {
            extra["logPath"] = json!(p.display().to_string());
        }
        emit_git_preflight(
            bus,
            "already_installing",
            "git 安装已在后台进行中；安装完成后下次 checkpoint 操作会自动启用影子仓库",
            extra,
        );
        return;
    }

    let Some(log_path) = new_preflight_log_file_path() else {
        emit_git_preflight(
            bus,
            "failed",
            "git 安装无法创建日志文件，checkpoint 将继续退化为 Noop",
            json!({
                "elapsedMs": started.elapsed().as_millis(),
            }),
        );
        return;
    };

    match spawn_unix_detached_install(plan, &log_path) {
        Ok(()) => {
            write_detached_log_marker(GIT_DETACHED_LOG_MARKER_NAME, &log_path);
            emit_git_preflight(
                bus,
                "detached",
                "git 安装已在后台继续；安装完成后下次 checkpoint 操作会自动启用影子仓库",
                json!({
                    "elapsedMs": started.elapsed().as_millis(),
                    "logPath": log_path.display().to_string(),
                }),
            );
        }
        Err(err) => {
            emit_git_preflight(
                bus,
                "failed",
                "git 安装无法启动，checkpoint 将继续退化为 Noop",
                json!({
                    "elapsedMs": started.elapsed().as_millis(),
                    "error": err.to_string(),
                }),
            );
        }
    }
}

/// Homebrew 安装/编译是否可能在跑（**窄匹配**，仅供并发判定；不以 marker 为依据）。
#[cfg(unix)]
fn homebrew_install_in_progress() -> bool {
    const PATTERNS: &[&str] = &[
        "[Hh]omebrew/.*brew\\.rb",
        "[Hh]omebrew/.*build\\.rb",
        "brew\\.rb.*install",
    ];
    for pat in PATTERNS {
        if pgrep_f_matches(pat) {
            return true;
        }
    }
    false
}

#[cfg(unix)]
fn pgrep_f_matches(pattern: &str) -> bool {
    StdCommand::new("pgrep")
        .args(["-f", pattern])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(unix)]
fn brew_install_already_in_progress(plan: &InstallPlan) -> bool {
    if plan.program != "brew" {
        return false;
    }
    homebrew_install_in_progress()
}

/// Marker 仅在「当前没有检测到 Homebrew 活动」时清除，避免陈旧路径。
#[cfg(unix)]
fn remove_detached_log_marker_if_homebrew_idle(marker_name: &str) {
    if homebrew_install_in_progress() {
        return;
    }
    remove_detached_log_marker_file(marker_name);
}

#[cfg(unix)]
fn detached_log_marker_path(marker_name: &str) -> Option<PathBuf> {
    let dir = preflight_log_dir()?;
    Some(dir.join(marker_name))
}

#[cfg(unix)]
fn remove_detached_log_marker_file(marker_name: &str) {
    if let Some(p) = detached_log_marker_path(marker_name) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(unix)]
fn read_valid_detached_marker_log_path(marker_name: &str) -> Option<PathBuf> {
    let marker = detached_log_marker_path(marker_name)?;
    let raw = std::fs::read_to_string(&marker).ok()?;
    let line = raw.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    let path = PathBuf::from(line);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

#[cfg(unix)]
fn write_detached_log_marker(marker_name: &str, log_path: &Path) {
    let Some(marker_path) = detached_log_marker_path(marker_name) else {
        return;
    };
    if let Some(parent) = marker_path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let tmp = marker_path.with_extension("marker.tmp");
    if std::fs::write(&tmp, format!("{}\n", log_path.display())).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, &marker_path);
}

#[cfg(unix)]
fn new_preflight_log_file_path() -> Option<PathBuf> {
    let dir = preflight_log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = chrono::Local::now().format("%Y%m%dT%H%M%S%.6f");
    Some(dir.join(format!("preflight-file-log-{stamp}.log")))
}

/// 构造 `nohup … >> log 2>&1 &` 并 `spawn`（不经由 [`write_install_log`]）。
#[cfg(unix)]
fn spawn_unix_detached_install(plan: &InstallPlan, log_path: &Path) -> std::io::Result<()> {
    let shell_cmd = build_nohup_shell_command(plan, log_path);
    tracing::debug!(
        target: TRACE_TARGET,
        shell_cmd_len = shell_cmd.len(),
        "search_tools preflight detached shell command"
    );
    StdCommand::new("/bin/sh")
        .arg("-c")
        .arg(&shell_cmd)
        .spawn()
        .map(|_| ())
}

#[cfg(unix)]
fn build_nohup_shell_command(plan: &InstallPlan, log_path: &Path) -> String {
    let joined = shell_words::join(std::iter::once(plan.program).chain(plan.args.iter().copied()));
    let log_lossy = log_path.to_string_lossy();
    let quoted_log = shell_words::quote(log_lossy.as_ref());
    let body = format!("nohup {joined} >> {quoted_log} 2>&1 &");
    if plan.program == "brew" {
        format!("HOMEBREW_NO_BUILD_FROM_SOURCE=1 {body}")
    } else {
        body
    }
}

fn should_skip_preflight(config: &AppConfig) -> bool {
    if std::env::var_os(SKIP_ENV).is_some() {
        return true;
    }
    !config.preflight.auto_install_search_tools
}

fn should_skip_git_preflight(config: &AppConfig) -> bool {
    !config.preflight.auto_install_git
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
            args: vec!["install", "--force-bottle", "ripgrep", "fd"],
        });
    }

    // TODO: implement detached install for Windows (PowerShell Start-Process -NoWait); v1 仍为阻塞 output()
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

fn git_install_plan() -> Option<InstallPlan> {
    if is_termux() && find_binary(&["pkg"]).is_some() {
        return Some(InstallPlan {
            program: "pkg",
            args: vec!["install", "-y", "git"],
        });
    }

    #[cfg(target_os = "android")]
    {
        return None;
    }

    if cfg!(target_os = "macos") && find_binary(&["brew"]).is_some() {
        return Some(InstallPlan {
            program: "brew",
            args: vec!["install", "--force-bottle", "git"],
        });
    }

    if cfg!(target_os = "windows") && find_binary(&["winget"]).is_some() {
        return Some(InstallPlan {
            program: "cmd",
            args: vec![
                "/C",
                "winget install --id Git.Git --silent --accept-source-agreements --accept-package-agreements",
            ],
        });
    }

    if find_binary(&["apt-get"]).is_some() {
        return Some(InstallPlan {
            program: "apt-get",
            args: vec!["install", "-y", "git"],
        });
    }
    if find_binary(&["dnf"]).is_some() {
        return Some(InstallPlan {
            program: "dnf",
            args: vec!["install", "-y", "git"],
        });
    }
    if find_binary(&["pacman"]).is_some() {
        return Some(InstallPlan {
            program: "pacman",
            args: vec!["-S", "--noconfirm", "git"],
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
    Some(
        home.join(".tomcat")
            .join("agents")
            .join("main")
            .join("logs"),
    )
}

/// 将安装子进程完整 stdout/stderr 写入 `~/.tomcat/agents/main/logs/preflight-file-log-<ts>.log`。
/// **Windows** 阻塞路径使用；Unix detached 由 shell 重定向实时写入，不经此函数汇总。
#[cfg(windows)]
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

fn emit_git_preflight(bus: &dyn EventBus, status: &str, message: &str, extra: serde_json::Value) {
    let payload = json!({
        "status": status,
        "message": message,
        "extra": extra,
    });
    let _ = bus.emit_sync(
        wire::WIRE_GIT_PREFLIGHT,
        EventContext::new(wire::WIRE_GIT_PREFLIGHT, payload),
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
#[path = "tests/preflight_test.rs"]
mod tests;
