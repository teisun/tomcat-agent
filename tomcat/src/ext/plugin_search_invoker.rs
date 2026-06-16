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
            Ok(value) if reports_unsupported_backend(&value) => {
                Err(unsupported_backend_error(backend))
            }
            Ok(value) => Ok(value),
            Err(err) => Err(classify_plugin_invocation_error(
                backend,
                provider.plugin_id.as_str(),
                &err.to_string(),
            )),
        }
    }
}

fn reports_unsupported_backend(value: &serde_json::Value) -> bool {
    value
        .get("unsupported_backend")
        .or_else(|| value.get("unsupportedBackend"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn unsupported_backend_error(backend: &str) -> BackendFailure {
    let detail = if backend == "auto" {
        "未找到可用的 web_search 插件后端".to_string()
    } else {
        format!("未找到名为 `{backend}` 的 web_search 插件后端")
    };
    BackendFailure::Incompatible { detail }
}

fn classify_plugin_invocation_error(
    backend: &str,
    plugin_id: &str,
    err_text: &str,
) -> BackendFailure {
    if err_text.contains("pi.fetch request timed out") || err_text.contains("execution exceeded ") {
        return BackendFailure::Timeout;
    }
    BackendFailure::Transport {
        detail: format!(
            "web_search plugin backend `{backend}` via `{plugin_id}` failed: {err_text}"
        ),
    }
}
