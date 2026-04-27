//! `pi pathrules` 子命令实现：add / list（plan §9 / PR-10）。
//!
//! 首版仅提供 `add` 与 `list`：
//!
//! - `pi pathrules add <path> --mode deny|readonly`：调
//!   [`crate::infra::config::append_path_rule_to_disk`] 原子追加一条规则到
//!   `~/.pi_/pi.config.toml` 的 `[[primitive.path_rules]]` 数组；和
//!   `pi config set primitive.path_rules <json>` 等价但更人友好。
//! - `pi pathrules list`：渲染三层合并视图（`[builtin]` / `[user]` / `[session]`）。
//!   首版无运行中 chat 实例，session 段恒为空——保留分组样式，方便 PR-Doc 文档承诺
//!   "三层来源可见"。
//!
//! `remove` 与 `clear-session` 已记入 [TODOS T-148](pi-rust-wasm/docs/TODOS.md)，
//! 当前版本用 `pi config edit` 手编替代。
//!
//! 输入校验与 `pi workspace add` 对齐：路径不存在时仅输出警告但仍允许写入
//! （path_rules 可针对将来出现的路径，例如 `~/未来项目/secrets`）。

use crate::core::permission::{builtin_default_rules, PathRule, PathRuleMode};
use crate::infra::config::append_path_rule_to_disk;
use crate::infra::error::AppError;
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
                    "警告：路径当前不存在: {} —— 仍记录该规则（路径未来出现时生效）",
                    normalized.display()
                );
            }

            if !config_path.exists() {
                println!(
                    "配置文件不存在: {}。请先运行: pi init",
                    config_path.display()
                );
                return Ok(());
            }

            // 调共享 helper（内部走 with_config_lock + dedupe + validate_config）。
            append_path_rule_to_disk(&config_path, PathRule::new(path_str.clone(), mode_enum))?;
            println!(
                "已追加 [primitive] path_rules: path=\"{}\" mode=\"{}\"",
                path_str,
                mode_str(mode_enum)
            );
        }
        PathRulesSub::List => {
            // 三层合并视图：builtin / user TOML / session（首版固定空）。
            // builtin 与 PermissionGate 内部用同一份 `builtin_default_rules()`，
            // 保证 list 与 gate 实际生效一致。
            println!("[builtin]  （内置默认；不可移除）");
            for r in builtin_default_rules() {
                print_rule(&r);
            }

            println!();
            println!("[user]     （来自 ~/.pi_/pi.config.toml [primitive.path_rules]）");
            if cfg.primitive.path_rules.is_empty() {
                println!("  (无)");
            } else {
                for r in &cfg.primitive.path_rules {
                    print_rule(r);
                }
            }

            println!();
            println!("[session]  （chat 运行时拖拽追加；CLI 进程不可见）");
            println!("  (无)");
            println!();
            println!(
                "提示：编辑/移除使用 `pi config edit`（手编 TOML）；首版未实现 \
                 `pi pathrules remove` 与 `clear-session`。"
            );
        }
    }
    Ok(())
}

fn parse_mode(s: &str) -> Result<PathRuleMode, AppError> {
    match s.trim().to_lowercase().as_str() {
        "deny" => Ok(PathRuleMode::Deny),
        "readonly" | "ro" | "read-only" => Ok(PathRuleMode::Readonly),
        other => Err(AppError::Config(format!(
            "未识别的 path_rule mode: '{}'（仅支持 deny / readonly）",
            other
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
mod tests {
    use super::*;

    #[test]
    fn parse_mode_recognises_canonical_forms() {
        assert!(matches!(parse_mode("deny"), Ok(PathRuleMode::Deny)));
        assert!(matches!(parse_mode("DENY"), Ok(PathRuleMode::Deny)));
        assert!(matches!(parse_mode("readonly"), Ok(PathRuleMode::Readonly)));
        assert!(matches!(
            parse_mode("read-only"),
            Ok(PathRuleMode::Readonly)
        ));
        assert!(matches!(parse_mode("ro"), Ok(PathRuleMode::Readonly)));
    }

    #[test]
    fn parse_mode_rejects_unknown() {
        match parse_mode("allow") {
            Err(AppError::Config(msg)) => assert!(msg.contains("未识别")),
            other => panic!("expected Config error, got {:?}", other),
        }
    }
}
