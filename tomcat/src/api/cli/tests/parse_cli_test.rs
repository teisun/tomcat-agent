//! # `clap` parse 路径
//!
//! 仅验证 CLI 参数解析（不实际执行子命令逻辑）：
//!
//! - `init` 拒绝已下线的 `--config` 参数。
//! - `doctor` / `config get` / `session list` / `plugin list` / `audit list`
//!   能正确还原为各自的 `Commands` 枚举变体。
//! - 无参调用 `tomcat` 时 `command` 为 `None`，由 main 入口落到默认 chat 流程。

use super::super::*;

#[test]
fn cli_parse_init() {
    let cli = Cli::try_parse_from(["tomcat", "init"]).unwrap();
    let cmd = cli.command.expect("subcommand");
    assert!(matches!(cmd, Commands::Init));
}

#[test]
fn cli_parse_init_rejects_config_flag() {
    let r = Cli::try_parse_from(["tomcat", "init", "--config", "/tmp/tomcat.config.toml"]);
    assert!(r.is_err(), "--config should be rejected after removal");
}

#[test]
fn cli_parse_doctor() {
    let cli = Cli::try_parse_from(["tomcat", "doctor"]).unwrap();
    assert!(matches!(cli.command, Some(Commands::Doctor)));
}

#[test]
fn cli_parse_config_get() {
    let cli = Cli::try_parse_from(["tomcat", "config", "get"]).unwrap();
    let cmd = cli.command.unwrap();
    if let Commands::Config { sub } = cmd {
        assert!(matches!(sub, ConfigSub::Get { key: None }));
    }
}

#[test]
fn cli_parse_session_list() {
    let cli = Cli::try_parse_from(["tomcat", "session", "list"]).unwrap();
    let cmd = cli.command.unwrap();
    assert!(matches!(
        cmd,
        Commands::Session {
            sub: SessionSub::List
        }
    ));
}

#[test]
fn cli_parse_plugin_list() {
    let cli = Cli::try_parse_from(["tomcat", "plugin", "list"]).unwrap();
    let cmd = cli.command.unwrap();
    assert!(matches!(
        cmd,
        Commands::Plugin {
            sub: PluginSub::List
        }
    ));
}

#[test]
fn cli_parse_audit_list() {
    let cli = Cli::try_parse_from(["tomcat", "audit", "list"]).unwrap();
    let cmd = cli.command.unwrap();
    assert!(matches!(
        cmd,
        Commands::Audit {
            sub: AuditSub::List { limit: None }
        }
    ));
}

#[test]
fn cli_parse_default_chat() {
    let cli = Cli::try_parse_from(["tomcat"]).unwrap();
    assert!(cli.command.is_none());
}
