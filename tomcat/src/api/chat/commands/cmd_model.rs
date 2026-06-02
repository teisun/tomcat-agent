use crate::api::chat::ChatContext;
use crate::core::llm::{Capabilities, LlmScene};

use super::parse::{ChatCommand, ChatCommandOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCommand {
    Current,
    List,
    Use { model_id: String },
}

pub(crate) fn parse_args(tokens: Vec<String>) -> ChatCommand {
    match tokens.as_slice() {
        [_cmd] => ChatCommand::Model(ModelCommand::Current),
        [_cmd, sub] if sub == "current" => ChatCommand::Model(ModelCommand::Current),
        [_cmd, sub] if sub == "list" => ChatCommand::Model(ModelCommand::List),
        [_cmd, sub, model_id] if sub == "use" => ChatCommand::Model(ModelCommand::Use {
            model_id: model_id.to_string(),
        }),
        _ => ChatCommand::UsageError {
            message: "用法错误：/model [current|list|use <model_id>]".to_string(),
        },
    }
}

pub(crate) fn run(ctx: &ChatContext, command: ModelCommand) -> ChatCommandOutcome {
    match command {
        ModelCommand::Current => run_current(ctx),
        ModelCommand::List => run_list(ctx),
        ModelCommand::Use { model_id } => run_use(ctx, &model_id),
    }
}

fn run_current(ctx: &ChatContext) -> ChatCommandOutcome {
    let entry = match ctx.session.get_session(ctx.session.current_session_key()) {
        Ok(entry) => entry,
        Err(err) => {
            println!("[model] 读取当前会话失败: {}", err);
            return ChatCommandOutcome::Handled;
        }
    };
    let current_model = ctx.effective_model(entry.as_ref());
    println!("当前会话模型: {}", current_model);
    println!("全局默认模型: {}", ctx.config.llm.default_model);
    match ctx.resolve_call(LlmScene::Main, entry.as_ref()) {
        Ok(resolved) => {
            println!(
                "解析结果: api={} provider={} base_url={} key_source={}",
                resolved.api,
                resolved.provider,
                resolved.base_url.as_deref().unwrap_or("(provider default)"),
                resolved.key_source
            );
        }
        Err(err) => {
            println!("解析结果: {}", err);
        }
    }
    ChatCommandOutcome::Handled
}

fn run_list(ctx: &ChatContext) -> ChatCommandOutcome {
    let entry = match ctx.session.get_session(ctx.session.current_session_key()) {
        Ok(entry) => entry,
        Err(err) => {
            println!("[model] 读取当前会话失败: {}", err);
            return ChatCommandOutcome::Handled;
        }
    };
    let current_model = ctx.effective_model(entry.as_ref());
    let default_model = ctx.config.llm.default_model.as_str();

    println!("可用模型:");
    for item in ctx.model_catalog.entries() {
        let mut tags = Vec::new();
        if item.id == current_model {
            tags.push("current");
        }
        if item.id == default_model {
            tags.push("default");
        }
        let tag_text = if tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", tags.join(", "))
        };
        println!(
            "  - {}{}  api={} provider={} caps={}",
            item.id,
            tag_text,
            item.api,
            item.provider,
            format_capabilities(&item.capabilities)
        );
    }
    ChatCommandOutcome::Handled
}

fn run_use(ctx: &ChatContext, model_id: &str) -> ChatCommandOutcome {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        println!("[model] 用法错误：/model use <model_id>");
        return ChatCommandOutcome::Handled;
    }

    let entry = match ctx.model_catalog.lookup_explicit(model_id) {
        Ok(entry) => entry,
        Err(err) => {
            println!("[model] {}", err);
            return ChatCommandOutcome::Handled;
        }
    };

    match ctx
        .session
        .switch_current_model(Some(&entry.provider), Some(&entry.id))
    {
        Ok(()) => {
            println!(
                "[model] 当前会话已切换到 {}（api={} provider={}）",
                entry.id, entry.api, entry.provider
            );
        }
        Err(err) => {
            println!("[model] 切换失败: {}", err);
        }
    }
    ChatCommandOutcome::Handled
}

fn format_capabilities(capabilities: &Capabilities) -> String {
    let mut labels = Vec::new();
    if capabilities.vision {
        labels.push("vision");
    }
    if capabilities.files {
        labels.push("files");
    }
    if capabilities.tools {
        labels.push("tools");
    }
    if capabilities.reasoning {
        labels.push("reasoning");
    }
    if labels.is_empty() {
        "none".to_string()
    } else {
        labels.join("+")
    }
}
