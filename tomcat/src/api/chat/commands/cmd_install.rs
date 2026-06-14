use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::api::chat::panels::{Question, QuestionOption};
use crate::api::chat::ChatContext;
use crate::core::package::{PackageManager, PackageResourceKind, PackageVisibility};

use super::parse::{ChatCommand, ChatCommandOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallTarget {
    CurrentProject,
    Agent,
    Global,
}

impl InstallTarget {
    fn into_visibility(self) -> PackageVisibility {
        match self {
            Self::CurrentProject => PackageVisibility::Scope,
            Self::Agent => PackageVisibility::Agent,
            Self::Global => PackageVisibility::Global,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::CurrentProject => "current-project",
            Self::Agent => "agent",
            Self::Global => "global",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallCommand {
    pub source: String,
    pub target: Option<InstallTarget>,
}

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd, source] => ChatCommand::Install(InstallCommand {
            source: source.to_string(),
            target: None,
        }),
        [_cmd, source, target] => match parse_target(target) {
            Some(target) => ChatCommand::Install(InstallCommand {
                source: source.to_string(),
                target: Some(target),
            }),
            None => ChatCommand::UsageError {
                message: usage_text(),
            },
        },
        _ => ChatCommand::UsageError {
            message: usage_text(),
        },
    }
}

fn parse_target(raw: &str) -> Option<InstallTarget> {
    match raw {
        "current-project" | "scope" => Some(InstallTarget::CurrentProject),
        "agent" => Some(InstallTarget::Agent),
        "global" => Some(InstallTarget::Global),
        _ => None,
    }
}

fn usage_text() -> String {
    "用法错误：/install <source> [current-project|agent|global]".to_string()
}

pub(crate) async fn run(ctx: &ChatContext, command: InstallCommand) -> ChatCommandOutcome {
    let target = match command.target {
        Some(target) => target,
        None => match choose_target(ctx).await {
            Some(target) => target,
            None => {
                println!("[install] 已取消，未写入任何文件。");
                return ChatCommandOutcome::Handled;
            }
        },
    };

    let visibility = target.clone().into_visibility();
    let manager = PackageManager::new(&ctx.config);
    let prepared = match manager.prepare_install(
        &command.source,
        visibility,
        Some(&ctx.scope_services.agent_workspace_dir),
        false,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            println!("[install] 安装准备失败: {error}");
            return ChatCommandOutcome::Handled;
        }
    };

    let plugin_ids = prepared
        .resources
        .iter()
        .filter(|resource| resource.kind == PackageResourceKind::Plugin)
        .map(|resource| resource.id.clone())
        .collect::<Vec<_>>();
    let has_skill = prepared
        .resources
        .iter()
        .any(|resource| resource.kind == PackageResourceKind::Skill);

    let outcome = match manager.install(prepared) {
        Ok(outcome) => outcome,
        Err(error) => {
            println!("[install] 安装失败: {error}");
            return ChatCommandOutcome::Handled;
        }
    };

    let mut warnings = outcome.warnings.clone();
    if has_skill {
        let _ = ctx.reload_skill_set().await;
        println!("[install] 当前会话 SkillSet 已刷新。");
    }
    if !plugin_ids.is_empty() {
        match ctx.refresh_plugin_catalog_inventory().await {
            Ok(mut refresh_warnings) => {
                warnings.append(&mut refresh_warnings);
                println!("[install] 当前会话 plugin catalog/static tools 已刷新。");
            }
            Err(error) => warnings.push(format!("当前会话 plugin 清单刷新失败: {error}")),
        }
    }

    let current_session_id = ctx
        .session_runtime
        .session
        .current_session_id()
        .ok()
        .flatten();
    let retained_loaded_plugins = plugin_ids
        .iter()
        .filter(|plugin_id| {
            let loaded = ctx
                .global_services
                .plugin_manager
                .as_ref()
                .and_then(|pm| pm.get_plugin(plugin_id))
                .map(|info| info.loaded_at > 0)
                .unwrap_or(false);
            let has_session_vm = current_session_id
                .as_deref()
                .and_then(|session_id| {
                    ctx.global_services
                        .plugin_manager
                        .as_ref()
                        .map(|pm| pm.has_session_vm(session_id, plugin_id))
                })
                .unwrap_or(false);
            loaded || has_session_vm
        })
        .cloned()
        .collect::<Vec<_>>();
    if !retained_loaded_plugins.is_empty() {
        warnings.push(format!(
            "已加载 plugin 不会热更新: {}",
            retained_loaded_plugins.join(", ")
        ));
    }

    println!(
        "[install] 已安装 package {}@{} -> {}",
        outcome.record.name,
        outcome.record.version,
        target.label()
    );
    for resource in &outcome.record.resources {
        println!("  - {}: {}", resource.kind.as_str(), resource.id);
    }
    if !warnings.is_empty() {
        println!("[install] warnings:");
        for warning in warnings {
            println!("  - {warning}");
        }
    }

    ChatCommandOutcome::Handled
}

async fn choose_target(ctx: &ChatContext) -> Option<InstallTarget> {
    let panel = ctx.session_runtime.plan_runtime.ask_question_panel()?;
    let result = panel
        .ask(
            vec![Question {
                id: "install-target".to_string(),
                prompt: "请选择 `/install` 的目标层。".to_string(),
                options: vec![
                    QuestionOption {
                        id: "scope".to_string(),
                        label: "current-project".to_string(),
                        recommended: true,
                    },
                    QuestionOption {
                        id: "agent".to_string(),
                        label: "agent".to_string(),
                        recommended: false,
                    },
                    QuestionOption {
                        id: "global".to_string(),
                        label: "global".to_string(),
                        recommended: false,
                    },
                ],
            }],
            Arc::new(AtomicBool::new(false)),
        )
        .await;
    if result.cancelled {
        return None;
    }
    let answer = result.answers.into_iter().find(|answer| !answer.skipped)?;
    answer
        .option_ids
        .first()
        .and_then(|option_id| match option_id.as_str() {
            "scope" => Some(InstallTarget::CurrentProject),
            "agent" => Some(InstallTarget::Agent),
            "global" => Some(InstallTarget::Global),
            _ => None,
        })
}
