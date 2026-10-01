use super::shared::{render_sync_summary, SlashReply};
use crate::api::chat::ChatContext;

pub(crate) async fn run(ctx: &ChatContext) -> SlashReply {
    match ctx.sync_resource_inventory().await {
        Ok(report) => SlashReply::success(render_sync_summary("reload", &report)),
        Err(error) => SlashReply::error(format!("[reload] 核对失败: {error}")),
    }
}
