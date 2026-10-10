use crate::api::chat::ChatContext;
use crate::core::connector::mcp::config::{
    add_global_server, remove_global_server, set_global_tool_filter, McpServerConfig, ToolFilter,
};
use crate::core::connector::ConnectorRegistry;
use crate::core::security::project_trust::ProjectTrustStore;
use crate::infra::i18n::tr;

use super::parse::{ChatCommand, ChatCommandOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorCommand {
    List,
    Add {
        name: String,
        command: String,
        args: Vec<String>,
    },
    Remove {
        name: String,
    },
    TrustProject,
    Test {
        name: String,
    },
    Login {
        name: String,
    },
    Logout {
        name: String,
    },
    Reload,
    Tools {
        name: String,
        include: Vec<String>,
        exclude: Vec<String>,
    },
}

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [command, sub] if command == "/connector" && sub == "list" => {
            ChatCommand::Connector(ConnectorCommand::List)
        }
        [command, sub, name, rest @ ..] if command == "/connector" && sub == "add" => {
            let Some((program, args)) = rest.split_first() else {
                return usage();
            };
            ChatCommand::Connector(ConnectorCommand::Add {
                name: name.clone(),
                command: program.clone(),
                args: args.to_vec(),
            })
        }
        [command, sub, name] if command == "/connector" && sub == "remove" => {
            ChatCommand::Connector(ConnectorCommand::Remove { name: name.clone() })
        }
        [command, sub] if command == "/connector" && sub == "trust-project" => {
            ChatCommand::Connector(ConnectorCommand::TrustProject)
        }
        [command, sub, name] if command == "/connector" && sub == "test" => {
            ChatCommand::Connector(ConnectorCommand::Test { name: name.clone() })
        }
        [command, sub, name] if command == "/connector" && sub == "login" => {
            ChatCommand::Connector(ConnectorCommand::Login { name: name.clone() })
        }
        [command, sub, name] if command == "/connector" && sub == "logout" => {
            ChatCommand::Connector(ConnectorCommand::Logout { name: name.clone() })
        }
        [command, sub] if command == "/connector" && sub == "reload" => {
            ChatCommand::Connector(ConnectorCommand::Reload)
        }
        [command, sub, name, filter_args @ ..] if command == "/connector" && sub == "tools" => {
            let Some((include, exclude)) = parse_tool_filter(filter_args) else {
                return usage();
            };
            ChatCommand::Connector(ConnectorCommand::Tools {
                name: name.clone(),
                include,
                exclude,
            })
        }
        _ => usage(),
    }
}

pub(crate) async fn run(ctx: &ChatContext, command: ConnectorCommand) -> ChatCommandOutcome {
    match command {
        ConnectorCommand::List => list(ctx),
        ConnectorCommand::Tools {
            name,
            include,
            exclude,
        } => tools(ctx, &name, include, exclude).await,
        ConnectorCommand::Add {
            name,
            command,
            args,
        } => add(ctx, name, command, args).await,
        ConnectorCommand::Remove { name } => remove(ctx, &name).await,
        ConnectorCommand::TrustProject => trust_project(ctx),
        ConnectorCommand::Test { name } => test(ctx, &name).await,
        ConnectorCommand::Login { name } => login(ctx, &name).await,
        ConnectorCommand::Logout { name } => logout(ctx, &name),
        ConnectorCommand::Reload => reload(ctx).await,
    }
}

fn usage() -> ChatCommand {
    ChatCommand::UsageError {
        message: tr("slash.connector.usage", &[]),
    }
}

fn registry(
    ctx: &ChatContext,
) -> Option<&std::sync::Arc<crate::core::connector::ConnectorRegistry>> {
    ctx.global_services.connector_registry.as_ref()
}

fn list(ctx: &ChatContext) -> ChatCommandOutcome {
    let Some(registry) = registry(ctx) else {
        println!("{}", tr("slash.connector.disabledHint", &[]));
        return ChatCommandOutcome::Handled;
    };
    let statuses = registry.mcp_manager().statuses();
    if statuses.is_empty() {
        println!("{}", tr("slash.connector.empty", &[]));
        return ChatCommandOutcome::Handled;
    }
    println!("{}", tr("slash.connector.servers", &[]));
    for status in statuses {
        println!(
            "{}",
            tr(
                "slash.connector.status",
                &[
                    ("name", &status.name),
                    ("source", status.source.as_str()),
                    ("state", &status.state.display_label()),
                    ("tools", &status.tool_count.to_string()),
                    ("resources", &status.resource_count.to_string())
                ]
            )
        );
    }
    ChatCommandOutcome::Handled
}

async fn tools(
    ctx: &ChatContext,
    name: &str,
    include: Vec<String>,
    exclude: Vec<String>,
) -> ChatCommandOutcome {
    if !include.is_empty() || !exclude.is_empty() {
        match set_global_tool_filter(&ctx.config, name, ToolFilter { include, exclude }) {
            Ok(()) => {
                if let Some(registry) = registry(ctx) {
                    if let Err(error) = registry.reload().await {
                        println!(
                            "{}",
                            tr(
                                "slash.connector.filterPartial",
                                &[("detail", &error.to_string())]
                            )
                        );
                    }
                }
            }
            Err(error) => {
                println!(
                    "{}",
                    tr(
                        "slash.connector.filterFailed",
                        &[("detail", &error.to_string())]
                    )
                );
                return ChatCommandOutcome::Handled;
            }
        }
    }
    let Some(registry) = registry(ctx) else {
        println!("{}", tr("slash.connector.disabled", &[]));
        return ChatCommandOutcome::Handled;
    };
    let tools = registry.mcp_manager().tool_defs(name);
    if tools.is_empty() {
        println!("{}", tr("slash.connector.noTools", &[("name", name)]));
        return ChatCommandOutcome::Handled;
    }
    println!("{}", tr("slash.connector.tools", &[("name", name)]));
    for tool in tools {
        println!("  - {} ({})", tool.model_name, tool.raw_name);
    }
    ChatCommandOutcome::Handled
}

fn parse_tool_filter(tokens: &[String]) -> Option<(Vec<String>, Vec<String>)> {
    let mut include = Vec::new();
    let mut exclude = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let flag = tokens.get(index)?;
        let value = tokens.get(index + 1)?;
        match flag.as_str() {
            "--include" => include.push(value.clone()),
            "--exclude" => exclude.push(value.clone()),
            _ => return None,
        }
        index += 2;
    }
    Some((include, exclude))
}

async fn add(
    ctx: &ChatContext,
    name: String,
    command: String,
    args: Vec<String>,
) -> ChatCommandOutcome {
    let config = McpServerConfig {
        command,
        args,
        env: Default::default(),
        url: None,
        auth: None,
        headers: Default::default(),
        oauth: None,
        cwd: None,
        tool_filter: ToolFilter::default(),
    };
    match add_global_server(&ctx.config, name.clone(), config) {
        Ok(()) => {
            println!("{}", tr("slash.connector.saved", &[("name", &name)]));
            if let Some(registry) = registry(ctx) {
                if let Err(error) = registry.reload().await {
                    println!(
                        "{}",
                        tr(
                            "slash.connector.savePartial",
                            &[("detail", &error.to_string())]
                        )
                    );
                }
            } else {
                println!("{}", tr("slash.connector.enableHint", &[]));
            }
        }
        Err(error) => println!(
            "{}",
            tr(
                "slash.connector.addFailed",
                &[("detail", &error.to_string())]
            )
        ),
    }
    ChatCommandOutcome::Handled
}

async fn remove(ctx: &ChatContext, name: &str) -> ChatCommandOutcome {
    match remove_global_server(&ctx.config, name) {
        Ok(true) => {
            println!("{}", tr("slash.connector.removed", &[("name", name)]));
            if let Some(registry) = registry(ctx) {
                if let Err(error) = registry.reload().await {
                    println!(
                        "{}",
                        tr(
                            "slash.connector.removePartial",
                            &[("detail", &error.to_string())]
                        )
                    );
                }
            }
        }
        Ok(false) => println!("{}", tr("slash.connector.missing", &[("name", name)])),
        Err(error) => println!(
            "{}",
            tr(
                "slash.connector.removeFailed",
                &[("detail", &error.to_string())]
            )
        ),
    }
    ChatCommandOutcome::Handled
}

fn trust_project(ctx: &ChatContext) -> ChatCommandOutcome {
    let Some(root) = ctx.scope_services.session_project_root.as_deref() else {
        println!("{}", tr("slash.connector.noProject", &[]));
        return ChatCommandOutcome::Handled;
    };
    match ProjectTrustStore::root_for(root).and_then(|root| {
        ProjectTrustStore::open(&ctx.config)?.trust(&root)?;
        ConnectorRegistry::project_trusted(&ctx.config, &root)?;
        Ok(root)
    }) {
        Ok(root) => println!(
            "{}",
            tr(
                "slash.connector.trusted",
                &[("path", &root.display().to_string())]
            )
        ),
        Err(error) => println!(
            "{}",
            tr(
                "slash.connector.trustFailed",
                &[("detail", &error.to_string())]
            )
        ),
    }
    ChatCommandOutcome::Handled
}

async fn test(ctx: &ChatContext, name: &str) -> ChatCommandOutcome {
    let Some(registry) = registry(ctx) else {
        println!("{}", tr("slash.connector.disabled", &[]));
        return ChatCommandOutcome::Handled;
    };
    match registry.mcp_manager().test_server(name).await {
        Ok(()) => println!("{}", tr("slash.connector.testPassed", &[("name", name)])),
        Err(error) => println!(
            "{}",
            tr(
                "slash.connector.testFailed",
                &[("detail", &error.to_string())]
            )
        ),
    }
    ChatCommandOutcome::Handled
}

async fn login(ctx: &ChatContext, name: &str) -> ChatCommandOutcome {
    let Some(registry) = registry(ctx) else {
        println!("{}", tr("slash.connector.disabled", &[]));
        return ChatCommandOutcome::Handled;
    };
    match registry.mcp_manager().login_server(name).await {
        Ok(()) => println!("{}", tr("slash.connector.loggedIn", &[("name", name)])),
        Err(error) => println!(
            "{}",
            tr(
                "slash.connector.loginFailed",
                &[("detail", &error.to_string())]
            )
        ),
    }
    ChatCommandOutcome::Handled
}

fn logout(ctx: &ChatContext, name: &str) -> ChatCommandOutcome {
    let Some(registry) = registry(ctx) else {
        println!("{}", tr("slash.connector.disabled", &[]));
        return ChatCommandOutcome::Handled;
    };
    match registry.mcp_manager().logout_server(name) {
        Ok(true) => println!("{}", tr("slash.connector.loggedOut", &[("name", name)])),
        Ok(false) => println!("{}", tr("slash.connector.noCredentials", &[("name", name)])),
        Err(error) => println!(
            "{}",
            tr(
                "slash.connector.logoutFailed",
                &[("detail", &error.to_string())]
            )
        ),
    }
    ChatCommandOutcome::Handled
}

async fn reload(ctx: &ChatContext) -> ChatCommandOutcome {
    let Some(registry) = registry(ctx) else {
        println!("{}", tr("slash.connector.disabled", &[]));
        return ChatCommandOutcome::Handled;
    };
    match registry.reload().await {
        Ok(()) => println!("{}", tr("slash.connector.reloaded", &[])),
        Err(error) => println!(
            "{}",
            tr(
                "slash.connector.reloadFailed",
                &[("detail", &error.to_string())]
            )
        ),
    }
    list(ctx)
}
