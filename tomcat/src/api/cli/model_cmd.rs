use crate::core::llm::{
    list_model_views, list_provider_keys, remove_user_model, resolve_provider_key_env_name,
    set_default_model, set_provider_key, upsert_user_model, Capabilities, ModelEntryInput,
    ProviderKeyInput,
};
use crate::{AppConfig, AppError, ModelCatalog};

use super::config_cmd::config_file_path;
use super::{ModelKeySub, ModelSub};

pub(crate) fn run_model(sub: ModelSub, cfg: &AppConfig) -> Result<(), AppError> {
    match sub {
        ModelSub::List => {
            let catalog = ModelCatalog::load(cfg)?;
            println!("可用模型:");
            for view in list_model_views(&catalog) {
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
                    .context_window
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string());
                println!(
                    "- {} [{}] api={} provider={} context_window={} key={} source={}",
                    view.id,
                    readiness,
                    view.api,
                    view.provider,
                    context_window,
                    view.api_key_env,
                    source
                );
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
                    thinking_format,
                },
            )?;
            println!(
                "已保存模型 {}（api={} provider={} key={}）。",
                model.id, model.api, model.provider, model.api_key_env
            );
        }
        ModelSub::Remove { id } => {
            remove_user_model(cfg, &id)?;
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
                let catalog = ModelCatalog::load(cfg)?;
                println!("Provider Keys:");
                for item in list_provider_keys(&catalog) {
                    let status = if item.key_present { "ready" } else { "missing" };
                    println!(
                        "- {} [{}] provider={} models={}",
                        item.env_name,
                        status,
                        item.provider,
                        item.model_ids.join(", ")
                    );
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
