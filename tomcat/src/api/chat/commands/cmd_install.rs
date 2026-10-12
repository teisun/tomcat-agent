use super::shared::{render_sync_summary, SlashReply};
use crate::api::chat::panels::{Question, QuestionOption};
use crate::api::chat::ChatContext;
use crate::core::package::{PackageManager, PackageVisibility};
use crate::infra::i18n::tr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallTarget {
    CurrentProject,
    Agent,
    Global,
}
impl InstallTarget {
    pub(super) fn into_visibility(self) -> PackageVisibility {
        match self {
            Self::CurrentProject => PackageVisibility::Scope,
            Self::Agent => PackageVisibility::Agent,
            Self::Global => PackageVisibility::Global,
        }
    }
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::CurrentProject => "current-project",
            Self::Agent => "agent",
            Self::Global => "global",
        }
    }
}
pub(super) fn parse_target(raw: &str) -> Option<InstallTarget> {
    match raw {
        "current-project" | "scope" => Some(InstallTarget::CurrentProject),
        "agent" => Some(InstallTarget::Agent),
        "global" => Some(InstallTarget::Global),
        _ => None,
    }
}

pub(crate) async fn run(ctx: &ChatContext, source: &str, target: InstallTarget) -> SlashReply {
    let config = ctx.config.clone();
    let root = ctx.scope_services.agent_workspace_dir.clone();
    // Resolve local sources against the session, never Serve's process cwd.
    let source = root.join(source);
    let visibility = target.clone().into_visibility();
    let result = tokio::task::spawn_blocking(move || {
        let manager = PackageManager::new(&config);
        let prepared = manager.prepare_install(source, visibility, Some(&root), false)?;
        manager.install(prepared)
    })
    .await;
    let outcome = match result {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => {
            return SlashReply::error(tr(
                "slash.install.failed",
                &[("detail", &error.to_string())],
            ))
        }
        Err(error) => {
            return SlashReply::error(tr(
                "slash.install.taskFailed",
                &[("detail", &error.to_string())],
            ))
        }
    };
    let mut lines = vec![tr(
        "slash.install.done",
        &[
            ("name", &outcome.record.name),
            ("version", &outcome.record.version),
            ("target", target.label()),
        ],
    )];
    for (kind, id) in outcome.record.resource_descriptors() {
        lines.push(format!("  - {}: {id}", kind.as_str()));
    }
    for warning in outcome.warnings {
        lines.push(tr("slash.warning", &[("detail", &warning)]));
    }
    match ctx.sync_resource_inventory().await {
        Ok(report) => {
            lines.push(render_sync_summary("install", &report));
            SlashReply::success(lines.join("\n"))
        }
        Err(error) => {
            lines.push(tr(
                "slash.install.partial",
                &[("detail", &error.to_string())],
            ));
            SlashReply::error(lines.join("\n"))
        }
    }
}

// Terminal adapter only: Serve calls the noninteractive shared executor.
pub(super) async fn choose_target(ctx: &ChatContext, command: &str) -> Option<InstallTarget> {
    let panel = ctx.session_runtime.plan_runtime.ask_question_panel()?;
    let result = panel
        .ask(
            vec![Question {
                allow_custom: true,
                id: format!("{command}-target"),
                prompt: tr("slash.chooseTarget", &[("command", command)]),
                options: vec![
                    QuestionOption {
                        id: "scope".into(),
                        label: "current-project".into(),
                        recommended: true,
                    },
                    QuestionOption {
                        id: "agent".into(),
                        label: "agent".into(),
                        recommended: false,
                    },
                    QuestionOption {
                        id: "global".into(),
                        label: "global".into(),
                        recommended: false,
                    },
                ],
            }],
            crate::core::plan_runtime::AskQuestionTermination::default(),
        )
        .await;
    if result.legacy_cancelled() {
        return None;
    }
    let answer = result.answers.into_iter().find(|answer| !answer.skipped)?;
    answer.option_ids.first().and_then(|id| parse_target(id))
}
