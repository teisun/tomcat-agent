//! # `clap` parse 路径
//!
//! 仅验证 CLI 参数解析（不实际执行子命令逻辑）：
//!
//! - `init` 拒绝已下线的 `--config` 参数。
//! - `doctor` / `config get` / `session list` / `plugin list` / `audit list`
//!   能正确还原为各自的 `Commands` 枚举变体。
//! - 无参调用 `tomcat` 时 `command` 为 `None`，由 main 入口按默认 mode 解析为
//!   `claw` 或 `code`。

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
            sub: SessionSub::List { scope: None }
        }
    ));
}

#[test]
fn cli_parse_session_list_with_scope() {
    let cli = Cli::try_parse_from(["tomcat", "session", "list", "--scope", "claw"]).unwrap();
    let cmd = cli.command.unwrap();
    assert!(matches!(
        cmd,
        Commands::Session {
            sub: SessionSub::List {
                scope: Some(SessionScopeArg::Claw)
            }
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
fn cli_parse_plugin_build() {
    let cli = Cli::try_parse_from(["tomcat", "plugin", "build", "./plugin"]).unwrap();
    let cmd = cli.command.unwrap();
    assert!(matches!(
        cmd,
        Commands::Plugin {
            sub: PluginSub::Build { path }
        } if path == "./plugin"
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
fn skill_subcommand_parsed() {
    let list = Cli::try_parse_from(["tomcat", "skill", "list"]).unwrap();
    assert!(matches!(
        list.command,
        Some(Commands::Skill {
            sub: SkillSub::List
        })
    ));

    let reload = Cli::try_parse_from(["tomcat", "skill", "reload"]).unwrap();
    assert!(matches!(
        reload.command,
        Some(Commands::Skill {
            sub: SkillSub::Reload
        })
    ));
}

#[test]
fn cli_parse_model_add_and_key_set() {
    let add = Cli::try_parse_from([
        "tomcat",
        "model",
        "add",
        "claude-opus-gateway",
        "--api",
        "anthropic-messages",
        "--provider",
        "anthropic",
        "--model-name",
        "claude-opus-4-6",
        "--base-url",
        "https://api.example.test/v1",
        "--reasoning",
        "--tools",
        "--thinking-format",
        "anthropic",
    ])
    .unwrap();
    assert!(matches!(
        add.command,
        Some(Commands::Model {
            sub: ModelSub::Add {
                id,
                api,
                provider,
                model_name: Some(model_name),
                base_url: Some(base_url),
                reasoning: true,
                tools: true,
                thinking_format: Some(thinking_format),
                ..
            }
        }) if id == "claude-opus-gateway"
            && api == "anthropic-messages"
            && provider == "anthropic"
            && model_name == "claude-opus-4-6"
            && base_url == "https://api.example.test/v1"
            && thinking_format == "anthropic"
    ));

    let key = Cli::try_parse_from(["tomcat", "model", "key", "set", "anthropic", "secret-value"])
        .unwrap();
    assert!(matches!(
        key.command,
        Some(Commands::Model {
            sub: ModelSub::Key {
                sub: ModelKeySub::Set { provider, value }
            }
        }) if provider == "anthropic" && value.as_deref() == Some("secret-value")
    ));
}

#[test]
fn cli_parse_install_visibility_scope_root() {
    let cli = Cli::try_parse_from([
        "tomcat",
        "install",
        "./fixtures/pkg",
        "--visibility",
        "scope",
        "--scope-root",
        "/tmp/demo",
        "--force",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Install {
            source,
            visibility: Some(PackageVisibilityArg::Scope),
            scope_root: Some(scope_root),
            force: true,
        }) if source == "./fixtures/pkg" && scope_root == "/tmp/demo"
    ));
}

#[test]
fn cli_parse_packages_and_uninstall() {
    let uninstall = Cli::try_parse_from([
        "tomcat",
        "uninstall",
        "demo-package",
        "--visibility",
        "agent",
    ])
    .unwrap();
    assert!(matches!(
        uninstall.command,
        Some(Commands::Uninstall {
            package,
            visibility: Some(PackageVisibilityArg::Agent),
            scope_root: None,
        }) if package == "demo-package"
    ));

    let packages = Cli::try_parse_from(["tomcat", "packages"]).unwrap();
    assert!(matches!(
        packages.command,
        Some(Commands::Packages {
            visibility: None,
            scope_root: None
        })
    ));
}

#[test]
fn cli_parse_claw() {
    let cli = Cli::try_parse_from(["tomcat", "claw"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Claw { resume: false })
    ));
}

#[test]
fn cli_parse_code_resume() {
    let cli = Cli::try_parse_from(["tomcat", "code", "--resume"]).unwrap();
    assert!(matches!(cli.command, Some(Commands::Code { resume: true })));
}

#[test]
fn cli_parse_serve_stdio_and_print_schema() {
    let cli = Cli::try_parse_from(["tomcat", "serve", "--stdio", "--print-schema"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Serve {
            stdio: true,
            ws: false,
            print_schema: true
        })
    ));
}

#[test]
fn cli_parse_chat_alias_resume() {
    let cli = Cli::try_parse_from(["tomcat", "chat", "--resume"]).unwrap();
    assert!(matches!(cli.command, Some(Commands::Chat { resume: true })));
}

#[test]
fn cli_parse_default_command_is_none() {
    let cli = Cli::try_parse_from(["tomcat"]).unwrap();
    assert!(cli.command.is_none());
}
