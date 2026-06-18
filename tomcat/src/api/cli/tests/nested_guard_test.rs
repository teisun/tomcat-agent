use std::ffi::OsString;
use std::path::PathBuf;

use serial_test::serial;

use super::super::*;
use crate::infra::error::AppError;

struct EnvGuard {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: test-scoped env mutation is serialized via `serial(env_lock)`.
        unsafe { std::env::set_var(key, value.into()) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(prev) => {
                // SAFETY: restore original env during test teardown.
                unsafe { std::env::set_var(self.key, prev) };
            }
            None => {
                // SAFETY: clear test-only env during teardown.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }
}

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
    match guard_nested_invocation(Some(&cmd)) {
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
    let result = guard_nested_invocation(Some(&cmd));
    assert!(
        result.is_ok(),
        "command should stay allowed under nested guard, got: {:?}",
        result
    );
}

#[test]
#[serial(env_lock)]
fn nested_guard_blocks_mutating_commands_when_agent_env_is_set() {
    let _guard = EnvGuard::set("TOMCAT_AGENT_ACTIVE", "1");
    for cmd in blocked_commands() {
        assert_blocked(cmd);
    }
}

#[test]
#[serial(env_lock)]
fn nested_guard_allows_readonly_commands_when_agent_env_is_set() {
    let _guard = EnvGuard::set("TOMCAT_AGENT_ACTIVE", "1");
    for cmd in allowed_commands() {
        assert_allowed(cmd);
    }
}

#[test]
#[serial(env_lock)]
fn nested_guard_keeps_existing_behavior_when_env_is_absent() {
    let blocked_representatives = vec![
        Commands::Session {
            sub: SessionSub::New { scope: None },
        },
        Commands::Config {
            sub: ConfigSub::Set {
                key: "log.level".to_string(),
                value: "debug".to_string(),
            },
        },
        Commands::Code { resume: false },
    ];
    for cmd in blocked_representatives {
        assert!(
            guard_nested_invocation(Some(&cmd)).is_ok(),
            "env-absent path must remain a no-op for {:?}",
            cmd
        );
    }
}
