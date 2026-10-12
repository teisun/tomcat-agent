use crate::core::llm::{
    list_model_views_with_prefs, list_provider_keys, remove_user_model_with_config_path,
    resolve_provider_key_env_name, set_default_model, set_provider_key, upsert_user_model,
    Capabilities, ModelEntryInput, ModelView, ProviderKeyInput,
};
use crate::infra::config::resolve_model_thinking_path;
use crate::infra::i18n::tr;
use crate::{AppConfig, AppError, ModelCatalog, ModelPrefsStore, ThinkingLevel};

use super::config_cmd::config_file_path;
use super::{ModelKeySub, ModelSub};

pub(crate) fn format_model_cli_line(view: &ModelView) -> String {
    let source = match view.source {
        crate::core::llm::ModelSource::Builtin => tr("term.model.builtin", &[]),
        crate::core::llm::ModelSource::User => tr("term.model.user", &[]),
    };
    let readiness = if view.key_present {
        tr("term.model.ready", &[])
    } else {
        tr("term.model.needsKey", &[])
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
            println!("{}", tr("cli.model.list", &[]));
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
                "{}",
                tr(
                    "cli.model.saved",
                    &[
                        ("id", &model.model.id),
                        ("api", &model.model.api),
                        ("provider", &model.model.provider),
                        ("key", &model.model.api_key_env)
                    ]
                )
            );
        }
        ModelSub::Remove { id } => {
            let path = config_file_path()?;
            remove_user_model_with_config_path(cfg, Some(&path), &id)?;
            println!("{}", tr("cli.model.removed", &[("id", id.trim())]));
        }
        ModelSub::Key { sub } => match sub {
            ModelKeySub::Set { provider, value } => {
                let catalog = ModelCatalog::load(cfg)?;
                let env_name = resolve_provider_key_env_name(&catalog, &provider);
                let value = match value {
                    Some(raw) => raw,
                    None => dialoguer::Password::new()
                        .with_prompt(tr("cli.model.keyPrompt", &[("name", &env_name)]))
                        .allow_empty_password(false)
                        .interact()
                        .map_err(|error| {
                            AppError::Config(tr(
                                "cli.model.keyReadFailed",
                                &[("detail", &error.to_string())],
                            ))
                        })?,
                };
                let status = set_provider_key(cfg, ProviderKeyInput { env_name, value })?;
                println!(
                    "{}",
                    tr(
                        "cli.model.keySaved",
                        &[
                            ("name", &status.env_name),
                            ("present", &status.key_present.to_string())
                        ]
                    )
                );
            }
            ModelKeySub::List => {
                println!("{}", tr("cli.model.keys", &[]));
                for item in list_provider_keys(cfg)? {
                    println!("- {} [{}]", item.env_name, tr("term.model.ready", &[]));
                }
            }
        },
        ModelSub::Default { model } => {
            let path = config_file_path()?;
            set_default_model(cfg, &path, &model)?;
            println!("{}", tr("cli.model.default", &[("id", model.trim())]));
        }
    }
    Ok(())
}
