//! `preflight` 私有项单元测试（`tests/preflight_test.rs`，与同目录 `suite_test` / `cwd_lazy_prompt_test` 的 `*_test` 命名一致；由 `preflight.rs` 末尾 `#[path]` 挂载）。

use std::path::Path;

use crate::infra::AppConfig;

use super::{should_skip_preflight, trim_for_event};

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

#[cfg(unix)]
#[test]
fn nohup_shell_quotes_log_path_with_spaces() {
    use super::{build_nohup_shell_command, InstallPlan};

    let plan = InstallPlan {
        program: "brew",
        args: vec!["install", "ripgrep", "fd"],
    };
    let log = Path::new("/tmp/fake preflight.log");
    let cmd = build_nohup_shell_command(&plan, log);
    assert!(cmd.starts_with("nohup "));
    assert!(cmd.contains(">>"));
    assert!(cmd.ends_with(" 2>&1 &"));
    assert!(
        cmd.contains("'") || !log.display().to_string().contains(' '),
        "path with spaces should be quoted: {cmd}"
    );
}
