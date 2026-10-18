//! `tomcat chat` 会话入口的非阻塞预检。
//!
//! 预检只负责尽早补齐 `search_files` Tier1 依赖（rg/fd）与 checkpoint 依赖的 git；失败不影响聊天，
//! `search_files` 仍会在运行时自动回落到 Tier2。
//!
//! **墙钟**：安装命令经 [`spawn_detached`] 脱离本进程在后台运行，tomcat 只负责启动，不等待包管理器
//! 完成，也不当场判断成败（看日志）。勿与环境变量 `PI_SEARCH_TIER2_DEADLINE_MS` 混淆（后者仅约束
//! `search_files` Tier2 兜底路径的墙钟）。

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;

use crate::core::SwitchingCheckpointStore;
use crate::infra::i18n::tr;
use crate::infra::platform::{
    find_executable, spawn_detached, system_package_manager, PackageManager,
    PATH_CHANGES_NEED_RESTART,
};
use crate::infra::{wire, AppConfig, ScopedEventEmitter};

const SKIP_ENV: &str = "PI_SKIP_SEARCH_TOOLS_PREFLIGHT";

/// 最近一次 detached 安装的日志路径（仅 UX；**不用作**「是否仍在安装」的判定）。
pub(crate) const DETACHED_LOG_MARKER_NAME: &str = "preflight-detached-log.marker";
const GIT_DETACHED_LOG_MARKER_NAME: &str = "preflight-detached-git-log.marker";

/// `tracing` target：仅开预检诊断时用 `RUST_LOG=tomcat_preflight=debug`。
pub(crate) const TRACE_TARGET: &str = "tomcat_preflight";

pub(crate) fn start_search_tools_preflight(config: &AppConfig, emitter: Arc<ScopedEventEmitter>) {
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
            remove_detached_log_marker_file(DETACHED_LOG_MARKER_NAME);
            emit_preflight(
                &emitter,
                "ready",
                &tr("preflight.searchReady", &[]),
                json!({ "missing": [] }),
            );
            return;
        }

        emit_preflight(
            &emitter,
            "start",
            &tr("preflight.searchStart", &[]),
            json!({ "missing": missing }),
        );

        let Some(manager) = system_package_manager() else {
            tracing::debug!(
                target: TRACE_TARGET,
                "no supported package manager for automatic install"
            );
            emit_preflight(
                &emitter,
                "failed",
                &tr("preflight.searchNoManager", &[]),
                json!({ "missing": missing_search_tools() }),
            );
            return;
        };
        let plan = install_plan(manager, InstallTarget::SearchTools);

        tracing::debug!(
            target: TRACE_TARGET,
            program = plan.program,
            args = ?plan.args,
            "search_tools preflight running package manager"
        );
        run_search_tools_install(&emitter, &plan, started);
    });
}

pub fn start_git_preflight(
    config: &AppConfig,
    emitter: Arc<ScopedEventEmitter>,
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
        if find_executable(&["git"]).is_some() {
            remove_detached_log_marker_file(GIT_DETACHED_LOG_MARKER_NAME);
            checkpoint_switcher.force_activate_shadow();
            emit_git_preflight(&emitter, "ready", &tr("preflight.gitReady", &[]), json!({}));
            return;
        }

        emit_git_preflight(&emitter, "start", &tr("preflight.gitStart", &[]), json!({}));

        let Some(manager) = system_package_manager() else {
            emit_git_preflight(
                &emitter,
                "failed",
                &tr("preflight.gitNoManager", &[]),
                json!({}),
            );
            return;
        };
        run_git_install(
            &emitter,
            &install_plan(manager, InstallTarget::Git),
            started,
        );
    });
}

fn run_search_tools_install(emitter: &ScopedEventEmitter, plan: &InstallPlan, started: Instant) {
    remove_detached_log_marker_if_homebrew_idle(DETACHED_LOG_MARKER_NAME);

    if brew_install_already_in_progress(plan) {
        let mut extra = json!({});
        if let Some(p) = read_valid_detached_marker_log_path(DETACHED_LOG_MARKER_NAME) {
            extra["logPath"] = json!(p.display().to_string());
        }
        emit_preflight(
            emitter,
            "already_installing",
            &tr("preflight.searchBusy", &[]),
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
            emitter,
            "failed",
            &tr("preflight.searchLogFailed", &[]),
            json!({
                "elapsedMs": started.elapsed().as_millis(),
            }),
        );
        return;
    };

    match spawn_detached_install(plan, &log_path) {
        Ok(()) => {
            write_detached_log_marker(DETACHED_LOG_MARKER_NAME, &log_path);
            let extra = json!({
                "logPath": log_path.display().to_string(),
                "elapsedMs": started.elapsed().as_millis(),
            });
            let detached_msg = tr(
                search_detached_message_key(plan.manager, PATH_CHANGES_NEED_RESTART),
                &[],
            );
            emit_preflight(emitter, "detached", &detached_msg, extra);
        }
        Err(err) => {
            tracing::warn!(
                target: TRACE_TARGET,
                error = %err,
                "search_tools preflight detached spawn failed"
            );
            emit_preflight(
                emitter,
                "failed",
                &tr("preflight.searchSpawnFailed", &[]),
                json!({
                    "error": err.to_string(),
                    "elapsedMs": started.elapsed().as_millis(),
                }),
            );
        }
    }
}

fn run_git_install(emitter: &ScopedEventEmitter, plan: &InstallPlan, started: Instant) {
    remove_detached_log_marker_if_homebrew_idle(GIT_DETACHED_LOG_MARKER_NAME);

    if brew_install_already_in_progress(plan) {
        let mut extra = json!({
            "elapsedMs": started.elapsed().as_millis(),
        });
        if let Some(p) = read_valid_detached_marker_log_path(GIT_DETACHED_LOG_MARKER_NAME) {
            extra["logPath"] = json!(p.display().to_string());
        }
        emit_git_preflight(
            emitter,
            "already_installing",
            &tr("preflight.gitBusy", &[]),
            extra,
        );
        return;
    }

    let Some(log_path) = new_preflight_log_file_path() else {
        emit_git_preflight(
            emitter,
            "failed",
            &tr("preflight.gitLogFailed", &[]),
            json!({
                "elapsedMs": started.elapsed().as_millis(),
            }),
        );
        return;
    };

    match spawn_detached_install(plan, &log_path) {
        Ok(()) => {
            write_detached_log_marker(GIT_DETACHED_LOG_MARKER_NAME, &log_path);
            emit_git_preflight(
                emitter,
                "detached",
                &tr(git_detached_message_key(PATH_CHANGES_NEED_RESTART), &[]),
                json!({
                    "elapsedMs": started.elapsed().as_millis(),
                    "logPath": log_path.display().to_string(),
                }),
            );
        }
        Err(err) => {
            emit_git_preflight(
                emitter,
                "failed",
                &tr("preflight.gitSpawnFailed", &[]),
                json!({
                    "elapsedMs": started.elapsed().as_millis(),
                    "error": err.to_string(),
                }),
            );
        }
    }
}

/// 装好的程序要新开终端才找得到时，detached 文案多提醒一句"重新打开终端"。
fn search_detached_message_key(manager: PackageManager, path_needs_restart: bool) -> &'static str {
    if manager == PackageManager::Brew {
        "preflight.searchDetachedBrew"
    } else if path_needs_restart {
        "preflight.searchDetachedRestart"
    } else {
        "preflight.searchDetached"
    }
}

fn git_detached_message_key(path_needs_restart: bool) -> &'static str {
    if path_needs_restart {
        "preflight.gitDetachedRestart"
    } else {
        "preflight.gitDetached"
    }
}

/// Homebrew 安装/编译是否可能在跑（**窄匹配**，仅供并发判定；不以 marker 为依据）。
/// 本机没有 brew 时直接返回 false，不去启动 pgrep。
fn homebrew_install_in_progress() -> bool {
    const PATTERNS: &[&str] = &[
        "[Hh]omebrew/.*brew\\.rb",
        "[Hh]omebrew/.*build\\.rb",
        "brew\\.rb.*install",
    ];
    find_executable(&["brew"]).is_some() && PATTERNS.iter().any(|pat| pgrep_f_matches(pat))
}

fn pgrep_f_matches(pattern: &str) -> bool {
    StdCommand::new("pgrep")
        .args(["-f", pattern])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn brew_install_already_in_progress(plan: &InstallPlan) -> bool {
    plan.manager == PackageManager::Brew && homebrew_install_in_progress()
}

/// Marker 仅在「当前没有检测到 Homebrew 活动」时清除，避免陈旧路径。
fn remove_detached_log_marker_if_homebrew_idle(marker_name: &str) {
    if homebrew_install_in_progress() {
        return;
    }
    remove_detached_log_marker_file(marker_name);
}

fn detached_log_marker_path(marker_name: &str) -> Option<PathBuf> {
    let dir = preflight_log_dir()?;
    Some(dir.join(marker_name))
}

fn remove_detached_log_marker_file(marker_name: &str) {
    if let Some(p) = detached_log_marker_path(marker_name) {
        let _ = std::fs::remove_file(p);
    }
}

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

fn new_preflight_log_file_path() -> Option<PathBuf> {
    let dir = preflight_log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = chrono::Local::now().format("%Y%m%dT%H%M%S%.6f");
    Some(dir.join(format!("preflight-file-log-{stamp}.log")))
}

fn spawn_detached_install(plan: &InstallPlan, log_path: &Path) -> std::io::Result<()> {
    let envs: &[(&str, &str)] = if plan.manager == PackageManager::Brew {
        &[("HOMEBREW_NO_BUILD_FROM_SOURCE", "1")]
    } else {
        &[]
    };
    spawn_detached(plan.program, &plan.args, envs, log_path)
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
    if find_executable(&["rg", "ripgrep"]).is_none() {
        missing.push("ripgrep");
    }
    if find_executable(&["fd", "fdfind"]).is_none() {
        missing.push("fd");
    }
    missing
}

#[derive(Debug, Clone, Copy)]
enum InstallTarget {
    SearchTools,
    Git,
}

#[derive(Debug, PartialEq, Eq)]
struct InstallPlan {
    manager: PackageManager,
    program: &'static str,
    args: Vec<&'static str>,
}

/// 用 `manager` 安装 `target` 的命令；各管理器下的包名是产品知识，只在这里维护。
fn install_plan(manager: PackageManager, target: InstallTarget) -> InstallPlan {
    use InstallTarget::{Git, SearchTools};
    use PackageManager::{AptGet, Brew, Dnf, Pacman, TermuxPkg, Winget};

    let (program, args) = match (manager, target) {
        (TermuxPkg, SearchTools) => ("pkg", vec!["install", "-y", "ripgrep", "fd"]),
        (TermuxPkg, Git) => ("pkg", vec!["install", "-y", "git"]),
        (Brew, SearchTools) => ("brew", vec!["install", "--force-bottle", "ripgrep", "fd"]),
        (Brew, Git) => ("brew", vec!["install", "--force-bottle", "git"]),
        (Winget, SearchTools) => (
            "cmd",
            vec![
                "/C",
                "winget install --id BurntSushi.ripgrep.MSVC --silent --accept-source-agreements --accept-package-agreements && winget install --id sharkdp.fd --silent --accept-source-agreements --accept-package-agreements",
            ],
        ),
        (Winget, Git) => (
            "cmd",
            vec![
                "/C",
                "winget install --id Git.Git --silent --accept-source-agreements --accept-package-agreements",
            ],
        ),
        (AptGet, SearchTools) => ("apt-get", vec!["install", "-y", "ripgrep", "fd-find"]),
        (AptGet, Git) => ("apt-get", vec!["install", "-y", "git"]),
        (Dnf, SearchTools) => ("dnf", vec!["install", "-y", "ripgrep", "fd-find"]),
        (Dnf, Git) => ("dnf", vec!["install", "-y", "git"]),
        (Pacman, SearchTools) => ("pacman", vec!["-S", "--noconfirm", "ripgrep", "fd"]),
        (Pacman, Git) => ("pacman", vec!["-S", "--noconfirm", "git"]),
    };
    InstallPlan {
        manager,
        program,
        args,
    }
}

fn preflight_log_dir() -> Option<PathBuf> {
    let home = crate::infra::platform::home_dir()?;
    Some(
        home.join(".tomcat")
            .join("agents")
            .join("main")
            .join("logs"),
    )
}

fn emit_preflight(
    emitter: &ScopedEventEmitter,
    status: &str,
    message: &str,
    extra: serde_json::Value,
) {
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
    let _ = emitter.emit_payload(wire::WIRE_SEARCH_TOOLS_PREFLIGHT, payload);
}

fn emit_git_preflight(
    emitter: &ScopedEventEmitter,
    status: &str,
    message: &str,
    extra: serde_json::Value,
) {
    let payload = json!({
        "status": status,
        "message": message,
        "extra": extra,
    });
    let _ = emitter.emit_payload(wire::WIRE_GIT_PREFLIGHT, payload);
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
