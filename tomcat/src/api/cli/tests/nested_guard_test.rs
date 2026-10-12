use std::path::PathBuf;

use serial_test::serial;

use super::super::*;
use crate::infra::error::AppError;

fn blocked_commands() -> Vec<Commands> {
    vec![
        Commands::Init,
        Commands::Session {
            sub: SessionSub::New { scope: None },
        },
        Commands::Session {
            sub: SessionSub::Switch {
                session_id: "session-1".to_string(),
                scope: None,
            },
        },
        Commands::Session {
            sub: SessionSub::Delete {
                session_id: "session-1".to_string(),
                scope: None,
            },
        },
        Commands::Session {
            sub: SessionSub::Archive {
                session_id: "session-1".to_string(),
                scope: None,
            },
        },
        Commands::Install {
            source: "/tmp/pkg".to_string(),
            visibility: None,
            scope_root: None,
            force: false,
        },
        Commands::Uninstall {
            package: "pkg".to_string(),
            visibility: None,
            scope_root: None,
        },
        Commands::Plugin {
            sub: PluginSub::Load {
                path: "/tmp/plugin".to_string(),
            },
        },
        Commands::Plugin {
            sub: PluginSub::Unload {
                id: "plugin-id".to_string(),
            },
        },
        Commands::Plugin {
            sub: PluginSub::Enable {
                id: "plugin-id".to_string(),
            },
        },
        Commands::Plugin {
            sub: PluginSub::Disable {
                id: "plugin-id".to_string(),
            },
        },
        Commands::Config {
            sub: ConfigSub::Set {
                key: "log.level".to_string(),
                value: "debug".to_string(),
            },
        },
        Commands::Config {
            sub: ConfigSub::Edit,
        },
        Commands::Model {
            sub: ModelSub::Add {
                id: "custom".to_string(),
                api: "openai-responses".to_string(),
                provider: "openai".to_string(),
                model_name: Some("gpt-5.4".to_string()),
                api_key_env: None,
                base_url: None,
                vision: false,
                files: false,
                tools: false,
                reasoning: false,
                web_search: false,
                context_window: None,
                context_window_options: Vec::new(),
                max_output_tokens: None,
                description: None,
                thinking_format: None,
            },
        },
        Commands::Model {
            sub: ModelSub::Default {
                model: "gpt-5.4".to_string(),
            },
        },
        Commands::Workspace {
            sub: WorkspaceSub::Add {
                path: Some("/tmp/ws".to_string()),
                cwd: false,
            },
        },
        Commands::Workspace {
            sub: WorkspaceSub::Remove {
                path: "/tmp/ws".to_string(),
            },
        },
        Commands::Pathrules {
            sub: PathRulesSub::Add {
                path: "/tmp/ws".to_string(),
                mode: "readonly".to_string(),
            },
        },
        Commands::Claw { resume: false },
        Commands::Code { resume: false },
        Commands::Serve {
            stdio: true,
            ws: false,
            print_schema: false,
        },
        Commands::Chat { resume: false },
    ]
}

fn allowed_commands() -> Vec<Commands> {
    vec![
        Commands::Doctor,
        Commands::Serve {
            stdio: false,
            ws: false,
            print_schema: true,
        },
        Commands::Session {
            sub: SessionSub::List { scope: None },
        },
        Commands::Session {
            sub: SessionSub::Search {
                query: Some("needle".to_string()),
                scope: None,
            },
        },
        Commands::Plugin {
            sub: PluginSub::List,
        },
        Commands::Plugin {
            sub: PluginSub::Info {
                id: "plugin-id".to_string(),
            },
        },
        Commands::Plugin {
            sub: PluginSub::Build {
                path: "/tmp/plugin".to_string(),
            },
        },
        Commands::Config {
            sub: ConfigSub::Get {
                key: Some("log.level".to_string()),
            },
        },
        Commands::Model {
            sub: ModelSub::List,
        },
        Commands::Model {
            sub: ModelSub::Key {
                sub: ModelKeySub::List,
            },
        },
        Commands::Packages {
            visibility: None,
            scope_root: None,
        },
        Commands::Audit {
            sub: AuditSub::List { limit: Some(5) },
        },
        Commands::Audit {
            sub: AuditSub::Show {
                id: "1".to_string(),
            },
        },
        Commands::Audit {
            sub: AuditSub::Export {
                path: PathBuf::from("audit.json"),
            },
        },
        Commands::Skill {
            sub: SkillSub::List,
        },
        Commands::Skill {
            sub: SkillSub::Reload,
        },
        Commands::Workspace {
            sub: WorkspaceSub::List,
        },
        Commands::Pathrules {
            sub: PathRulesSub::List,
        },
    ]
}

fn assert_blocked(cmd: Commands) {
    match guard_nested_invocation_for(true, Some(&cmd)) {
        Err(AppError::Config(message)) => {
            assert!(
                message.contains(
                    "Refusing to run this Tomcat command inside an active Tomcat agent session"
                ),
                "guard message should stay English and actionable, got: {message}"
            );
            assert!(
                message.contains("Use the agent's tool calls instead"),
                "guard message should include recovery hint, got: {message}"
            );
        }
        other => panic!("expected nested guard rejection, got: {:?}", other),
    }
}

fn assert_allowed(cmd: Commands) {
    let result = guard_nested_invocation_for(true, Some(&cmd));
    assert!(
        result.is_ok(),
        "command should stay allowed under nested guard, got: {:?}",
        result
    );
}

#[test]
fn nested_guard_blocks_mutating_commands_when_active() {
    for cmd in blocked_commands() {
        assert_blocked(cmd);
    }
}

#[test]
fn nested_guard_allows_readonly_commands_when_active() {
    for cmd in allowed_commands() {
        assert_allowed(cmd);
    }
}

#[test]
fn nested_guard_allows_serve_print_schema_when_active() {
    assert_allowed(Commands::Serve {
        stdio: false,
        ws: false,
        print_schema: true,
    });
}

#[test]
fn nested_guard_keeps_existing_behavior_when_inactive() {
    for cmd in blocked_commands().into_iter().chain(allowed_commands()) {
        assert!(
            guard_nested_invocation_for(false, Some(&cmd)).is_ok(),
            "inactive path: {cmd:?}"
        );
    }
}

#[test]
fn nested_guard_keeps_no_explicit_command_behavior() {
    for active in [false, true] {
        assert!(guard_nested_invocation_for(active, None).is_ok());
    }
}

#[test]
#[serial(env_lock)]
fn nested_guard_environment_adapter_uses_the_real_flag_without_mutating_it() {
    let original = std::env::var_os(TOMCAT_AGENT_ACTIVE_ENV);
    let expected_active = original.as_deref() == Some(std::ffi::OsStr::new("1"));
    assert_eq!(nested_agent_invocation_active(), expected_active);
    for cmd in blocked_commands().into_iter().chain(allowed_commands()) {
        let expected = guard_nested_invocation_for(expected_active, Some(&cmd));
        let actual = guard_nested_invocation(Some(&cmd));
        assert_eq!(actual.is_ok(), expected.is_ok(), "adapter path: {cmd:?}");
    }
    assert!(guard_nested_invocation(None).is_ok());
    assert_eq!(std::env::var_os(TOMCAT_AGENT_ACTIVE_ENV), original);
}
