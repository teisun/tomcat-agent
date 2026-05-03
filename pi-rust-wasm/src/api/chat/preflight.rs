//! `pi chat` 会话入口的非阻塞预检。
//!
//! 预检只负责尽早补齐 `search_files` Tier1 依赖（rg/fd）；失败不影响聊天，
//! `search_files` 仍会在运行时自动回落到 Tier2。

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;

use crate::infra::{wire, AppConfig, EventBus, EventContext};

const SKIP_ENV: &str = "PI_SKIP_SEARCH_TOOLS_PREFLIGHT";

pub(crate) fn start_search_tools_preflight(config: &AppConfig, event_bus: Arc<dyn EventBus>) {
    if should_skip_preflight(config) {
        return;
    }

    std::thread::spawn(move || {
        let started = Instant::now();
        let missing = missing_search_tools();
        if missing.is_empty() {
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
            emit_preflight(
                &*event_bus,
                "failed",
                "No supported package manager found for automatic search tool installation",
                json!({ "missing": missing_search_tools() }),
            );
            return;
        };

        emit_preflight(
            &*event_bus,
            "progress",
            &format!("running: {} {}", plan.program, plan.args.join(" ")),
            json!({ "program": plan.program, "args": plan.args }),
        );

        let output = StdCommand::new(plan.program).args(&plan.args).output();
        match output {
            Ok(output) if output.status.success() => {
                emit_preflight(
                    &*event_bus,
                    "success",
                    "search_files Tier1 tools installation finished",
                    json!({
                        "elapsedMs": started.elapsed().as_millis(),
                        "missingAfter": missing_search_tools(),
                    }),
                );
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                emit_preflight(
                    &*event_bus,
                    "failed",
                    "search_files Tier1 tools installation failed; Tier2 fallback remains available",
                    json!({
                        "exitCode": output.status.code(),
                        "stderr": trim_for_event(&stderr),
                        "elapsedMs": started.elapsed().as_millis(),
                    }),
                );
            }
            Err(err) => {
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

fn emit_preflight(bus: &dyn EventBus, status: &str, message: &str, extra: serde_json::Value) {
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

fn trim_for_event(value: &str) -> String {
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
