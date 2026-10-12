use crate::infra::error::AppError;
use crate::infra::i18n::tr;
use std::sync::Arc;

use async_trait::async_trait;

use crate::core::tools::web_search::backend::BackendFailure;
use crate::core::tools::web_search::plugin_backend::PluginSearchInvoker;
use crate::ext::{FunctionRegistry, PluginFunctionInvoker};

pub struct ExtPluginSearchInvoker {
    functions: Arc<FunctionRegistry>,
    function_invoker: Arc<PluginFunctionInvoker>,
}

impl ExtPluginSearchInvoker {
    pub fn new(
        functions: Arc<FunctionRegistry>,
        function_invoker: Arc<PluginFunctionInvoker>,
    ) -> Arc<Self> {
        Arc::new(Self {
            functions,
            function_invoker,
        })
    }
}

#[async_trait]
impl PluginSearchInvoker for ExtPluginSearchInvoker {
    async fn search(
        &self,
        backend: &str,
        params: serde_json::Value,
        session_id: &str,
    ) -> Result<serde_json::Value, BackendFailure> {
        let Some(provider) = self
            .functions
            .functions_for_point("web_search.backend")
            .into_iter()
            .next()
        else {
            return Err(unsupported_backend_error(backend));
        };

        match self
            .function_invoker
            .execute(&provider, params, Some(session_id))
            .await
        {
            Ok(value) => Ok(value),
            Err(err) => Err(classify_plugin_invocation_error(
                backend,
                provider.plugin_id.as_str(),
                &err,
            )),
        }
    }
}

fn unsupported_backend_error(backend: &str) -> BackendFailure {
    let detail = if backend == "auto" {
        tr("plugin.searchUnavailable", &[])
    } else {
        tr("plugin.searchMissing", &[("name", backend)])
    };
    BackendFailure::Incompatible { detail }
}

fn classify_plugin_invocation_error(
    backend: &str,
    plugin_id: &str,
    error: &AppError,
) -> BackendFailure {
    let err_text = error.to_string();
    let code = match error {
        AppError::Plugin(raw) | AppError::QuickJS(raw) => {
            serde_json::from_str::<serde_json::Value>(raw)
                .ok()
                .and_then(|value| {
                    value
                        .get("code")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
        }
        _ => None,
    };
    if code.as_deref() == Some("timeout") {
        return BackendFailure::Timeout;
    }
    let detail = tr(
        "plugin.searchFailed",
        &[
            ("backend", backend),
            ("plugin", plugin_id),
            ("detail", &err_text),
        ],
    );
    if code.is_none() && matches!(error, AppError::QuickJS(_)) {
        return BackendFailure::PluginRuntime { detail };
    }
    BackendFailure::Transport { detail }
}

#[cfg(test)]
#[path = "tests/plugin_search_invoker_test.rs"]
mod tests;
