//! `tomcat pathrules` 子命令实现：add / list（plan §9 / PR-10）。
//!
//! 首版仅提供 `add` 与 `list`：
//!
//! - `tomcat pathrules add <path> --mode deny|readonly`：调
//!   [`crate::infra::config::append_path_rule_to_disk`] 原子追加一条规则到
//!   `~/.tomcat/tomcat.config.toml` 的 `[[primitive.path_rules]]` 数组；和
//!   `tomcat config set primitive.path_rules <json>` 等价但更人友好。
//! - `tomcat pathrules list`：渲染三层合并视图（`[builtin]` / `[user]` / `[session]`）。
//!   首版无运行中 chat 实例，session 段恒为空——保留分组样式，方便 PR-Doc 文档承诺
//!   "三层来源可见"。
//!
//! `remove` 与 `clear-session` 已记入 [TODOS T-148](tomcat/docs/TODOS.md)，
//! 当前版本用 `tomcat config edit` 手编替代。
//!
//! 输入校验与 `tomcat workspace add` 对齐：路径不存在时仅输出警告但仍允许写入
//! （path_rules 可针对将来出现的路径，例如 `~/未来项目/secrets`）。

use crate::core::permission::{builtin_default_rules, PathRule, PathRuleMode};
use crate::infra::config::append_path_rule_to_disk;
use crate::infra::error::AppError;
use crate::infra::i18n::tr;
use crate::infra::platform::normalize_path;
use crate::AppConfig;

use super::{config_file_path, PathRulesSub};

pub(crate) fn run_pathrules(sub: PathRulesSub, cfg: &AppConfig) -> Result<(), AppError> {
    let config_path = config_file_path()?;

    match sub {
        PathRulesSub::Add { path, mode } => {
            let mode_enum = parse_mode(&mode)?;

            // 输入校验：路径规范化（展开 ~），不存在仅警告。
            let normalized = normalize_path(&path)?;
            let path_str = normalized.to_string_lossy().to_string();
            if !normalized.exists() {
                eprintln!(
                    "{}",
                    tr(
                        "cli.pathrules.absentWarning",
                        &[("path", &normalized.display().to_string())]
                    )
                );
            }

            if !config_path.exists() {
                println!(
                    "{}",
                    tr(
                        "cli.config.fileMissing",
                        &[("path", &config_path.display().to_string())]
                    )
                );
                return Ok(());
            }

            // 调共享 helper（内部走 with_config_lock + dedupe + validate_config）。
            append_path_rule_to_disk(&config_path, PathRule::new(path_str.clone(), mode_enum))?;
            println!(
                "{}",
                tr(
                    "cli.pathrules.added",
                    &[("path", &path_str), ("mode", mode_str(mode_enum))]
                )
            );
        }
        PathRulesSub::List => {
            // 三层合并视图：builtin / user TOML / session（首版固定空）。
            // builtin 与 PermissionGate 内部用同一份 `builtin_default_rules()`，
            // 保证 list 与 gate 实际生效一致。
            println!("{}", tr("cli.pathrules.builtin", &[]));
            for r in builtin_default_rules() {
                print_rule(&r);
            }

            println!();
            println!("{}", tr("cli.pathrules.user", &[]));
            if cfg.primitive.path_rules.is_empty() {
                println!("{}", tr("cli.pathrules.empty", &[]));
            } else {
                for r in &cfg.primitive.path_rules {
                    print_rule(r);
                }
            }

            println!();
            println!("{}", tr("cli.pathrules.session", &[]));
            println!("{}", tr("cli.pathrules.empty", &[]));
            println!();
            println!("{}", tr("cli.pathrules.editHint", &[]));
        }
    }
    Ok(())
}

fn parse_mode(s: &str) -> Result<PathRuleMode, AppError> {
    match s.trim().to_lowercase().as_str() {
        "deny" => Ok(PathRuleMode::Deny),
        "readonly" | "ro" | "read-only" => Ok(PathRuleMode::Readonly),
        other => Err(AppError::Config(tr(
            "cli.pathrules.invalidMode",
            &[("mode", other)],
        ))),
    }
}

fn mode_str(m: PathRuleMode) -> &'static str {
    match m {
        PathRuleMode::Deny => "deny",
        PathRuleMode::Readonly => "readonly",
    }
}

fn print_rule(r: &PathRule) {
    println!("  [{}]  {}", mode_str(r.mode), r.path);
}

#[cfg(test)]
#[path = "tests/pathrules_cmd_test.rs"]
mod tests;
