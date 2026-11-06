//! Read the service's catalog without constructing a runtime, preferences, or sessions.
use std::path::Path;

use crate::api::serve::{commands::list_models_payload, service};
use crate::core::llm::ModelCatalog;
use crate::infra::{config, i18n::tr};
use crate::AppError;

pub(super) fn run(config_path: Option<&Path>) -> Result<(), AppError> {
    // Parser errors may include configuration lines or dotenv secrets. Never forward them.
    let config_error = |_: AppError| AppError::Config(tr("cli.serveModels.configFailed", &[]));
    let (client, _) = config::load_config_with_ignored_keys(config_path).map_err(config_error)?;
    let (work_dir, agent_id) = service::client_identity(&client).map_err(config_error)?;
    let (cfg, _) = service::load_saved_configuration(config_path, &work_dir, &agent_id)
        .map_err(config_error)?;
    crate::validate_config(&cfg).map_err(config_error)?;
    super::preload_runtime_env(&cfg)
        .map_err(|_| AppError::Config(tr("cli.serveModels.credentialsFailed", &[])))?;
    let catalog = ModelCatalog::load(&cfg)
        .map_err(|_| AppError::Config(tr("cli.serveModels.catalogFailed", &[])))?;
    let payload = list_models_payload(&cfg, &catalog, None);
    println!(
        "{}",
        serde_json::to_string(&payload).map_err(AppError::Serialize)?
    );
    Ok(())
}
