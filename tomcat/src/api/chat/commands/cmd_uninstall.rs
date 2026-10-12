use super::cmd_install::InstallTarget;
use super::shared::{render_sync_summary, SlashReply};
use crate::api::chat::ChatContext;
use crate::core::package::PackageManager;
use crate::infra::i18n::tr;

pub(crate) async fn run(ctx: &ChatContext, package: &str, target: InstallTarget) -> SlashReply {
    let config = ctx.config.clone();
    let root = ctx.scope_services.agent_workspace_dir.clone();
    let name = package.to_string();
    let visibility = target.clone().into_visibility();
    let result = tokio::task::spawn_blocking(move || {
        PackageManager::new(&config).uninstall(&name, visibility, Some(&root))
    })
    .await;
    let outcome = match result {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => {
            return SlashReply::error(tr(
                "slash.uninstall.failed",
                &[("detail", &error.to_string())],
            ))
        }
        Err(error) => {
            return SlashReply::error(tr(
                "slash.uninstall.taskFailed",
                &[("detail", &error.to_string())],
            ))
        }
    };
    let mut lines = vec![tr(
        "slash.uninstall.done",
        &[("name", &outcome.record.name), ("target", target.label())],
    )];
    for (kind, id) in outcome.record.resource_descriptors() {
        lines.push(format!("  - {}: {id}", kind.as_str()));
    }
    match ctx.sync_resource_inventory().await {
        Ok(report) => {
            lines.push(render_sync_summary("uninstall", &report));
            SlashReply::success(lines.join("\n"))
        }
        Err(error) => {
            lines.push(tr(
                "slash.uninstall.partial",
                &[("detail", &error.to_string())],
            ));
            SlashReply::error(lines.join("\n"))
        }
    }
}
