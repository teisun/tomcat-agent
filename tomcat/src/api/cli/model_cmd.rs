use crate::core::llm::{
    list_model_views_with_prefs, list_provider_keys, remove_user_model_with_config_path,
    resolve_provider_key_env_name, set_default_model, set_provider_key, upsert_user_model,
    Capabilities, ModelEntryInput, ModelView, ProviderKeyInput,
};
use crate::infra::config::resolve_model_thinking_path;
use crate::{AppConfig, AppError, ModelCatalog, ModelPrefsStore, ThinkingLevel};

use super::config_cmd::config_file_path;
use super::{ModelKeySub, ModelSub};

pub(crate) fn format_model_cli_line(view: &ModelView) -> String {
    let source = match view.source {
        crate::core::llm::ModelSource::Builtin => "builtin",
        crate::core::llm::ModelSource::User => "user",
    };
    let readiness = if view.key_present {
        "ready"
    } else {
        "needs-key"
    };
    let context_window = view
        .selected_context_window
        .or(view.context_window)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let reasoning = view.selected_reasoning_level.as_deref().unwrap_or("-");
    let context_options = if view.context_window_options.is_empty() {
        "-".to_string()
    } else {
        format!("{:?}", view.context_window_options)
    };
    let description = view.description.as_deref().unwrap_or("-");
    format!(
        "- {} [{}] api={} provider={} context_window={} context_options={} reasoning={} description={} key={} source={}",
        view.id,
        readiness,
        view.api,
        view.provider,
        context_window,
        context_options,
        reasoning,
        description,
        view.api_key_env,
        source
    )
}

pub(crate) fn run_model(sub: ModelSub, cfg: &AppConfig) -> Result<(), AppError> {
    match sub {
        ModelSub::List => {
            let catalog = ModelCatalog::load(cfg)?;
            let default_reasoning = ThinkingLevel::parse_or_medium(&cfg.llm.thinking.level).0;
            let prefs =
                ModelPrefsStore::load(resolve_model_thinking_path(cfg)?, default_reasoning)?;
            println!("可用模型:");
            for view in list_model_views_with_prefs(&catalog, &prefs) {
                println!("{}", format_model_cli_line(&view));
            }
        }
        ModelSub::Add {
            id,
            api,
            provider,
            model_name,
            api_key_env,
            base_url,
            vision,
            files,
            tools,
            reasoning,
            web_search,
            context_window,
            context_window_options,
            max_output_tokens,
            description,
            thinking_format,
        } => {
            let model = upsert_user_model(
                cfg,
                ModelEntryInput {
                    id,
                    model_name,
                    api,
                    provider,
                    api_key_env,
                    base_url,
                    capabilities: Capabilities {
                        vision,
                        files,
                        tools,
                        reasoning,
                        web_search,
                    },
                    context_window,
                    context_window_options: (!context_window_options.is_empty())
                        .then_some(context_window_options),
                    max_output_tokens,
                    description,
                    supported_speeds: None,
                    supported_reasoning_levels: None,
                    thinking_format,
                },
            )?;
            println!(
                "已保存模型 {}（api={} provider={} key={}）。",
                model.model.id, model.model.api, model.model.provider, model.model.api_key_env
            );
        }
        ModelSub::Remove { id } => {
            let path = config_file_path()?;
            remove_user_model_with_config_path(cfg, Some(&path), &id)?;
            println!("已删除用户模型 {}。", id.trim());
        }
        ModelSub::Key { sub } => match sub {
            ModelKeySub::Set { provider, value } => {
                let catalog = ModelCatalog::load(cfg)?;
                let env_name = resolve_provider_key_env_name(&catalog, &provider);
                let value = match value {
                    Some(raw) => raw,
                    None => dialoguer::Password::new()
                        .with_prompt(format!("请输入 {} 的值", env_name))
                        .allow_empty_password(false)
                        .interact()
                        .map_err(|error| AppError::Config(format!("读取 API Key 失败: {error}")))?,
                };
                let status = set_provider_key(cfg, ProviderKeyInput { env_name, value })?;
                println!(
                    "已写入 {}（key_present={}）。",
                    status.env_name, status.key_present
                );
            }
            ModelKeySub::List => {
                println!("Provider Keys:");
                for item in list_provider_keys(cfg)? {
                    println!("- {} [ready]", item.env_name);
                }
            }
        },
        ModelSub::Default { model } => {
            let path = config_file_path()?;
            set_default_model(cfg, &path, &model)?;
            println!("已设置 llm.default_model = {}", model.trim());
        }
    }
    Ok(())
}
