use crate::infra::error::AppError;
use crate::infra::i18n::tr;
use std::sync::Arc;

use async_trait::async_trait;

use crate::core::tools::web_search::declarations::{parse_declarations, SEARCH_POINT};
use crate::core::tools::web_search::plugin_backend::{PluginSearchCatalog, PluginSearchInvoker};
use crate::ext::plugin::RegisteredFunction;
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

    fn winner(&self) -> Option<RegisteredFunction> {
        self.functions
            .functions_for_point(SEARCH_POINT)
            .into_iter()
            .next()
    }
}

#[async_trait]
impl PluginSearchInvoker for ExtPluginSearchInvoker {
    fn search_catalog(&self) -> PluginSearchCatalog {
        let winner = self.winner();
        PluginSearchCatalog {
            declarations: parse_declarations(
                winner.as_ref().and_then(|entry| entry.metadata.as_ref()),
            ),
            saved_keys: winner.is_some_and(|entry| entry.source.can_use_saved_keys()),
        }
    }

    async fn search(
        &self,
        backend: &str,
        params: serde_json::Value,
        session_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        let provider = self
            .winner()
            .ok_or_else(|| AppError::Tool(tr("plugin.searchMissing", &[("name", backend)])))?;
        self.function_invoker
            .execute(&provider, params, Some(session_id))
            .await
            .map_err(|err| invocation_error(backend, &provider.plugin_id, &err))
    }
}

fn invocation_error(backend: &str, plugin_id: &str, error: &AppError) -> AppError {
    let timed_out = match error {
        AppError::Plugin(raw) | AppError::QuickJS(raw) => {
            serde_json::from_str::<serde_json::Value>(raw).is_ok_and(|value| {
                value.get("code").and_then(|code| code.as_str()) == Some("timeout")
            })
        }
        _ => false,
    };
    AppError::Tool(if timed_out {
        tr("search.timeout", &[("backend", backend)])
    } else {
        tr(
            "plugin.searchFailed",
            &[
                ("backend", backend),
                ("plugin", plugin_id),
                ("detail", &error.to_string()),
            ],
        )
    })
}

#[cfg(test)]
#[path = "tests/plugin_search_invoker_test.rs"]
mod tests;
