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
        let providers = self.functions.functions_for_point("web_search.backend");
        if providers.is_empty() {
            return Err(unsupported_backend_error(backend));
        }

        for provider in providers {
            match self
                .function_invoker
                .execute(&provider, params.clone(), Some(session_id))
                .await
            {
                Ok(value) if reports_unsupported_backend(&value) => continue,
                Ok(value) => return Ok(value),
                Err(err) => {
                    return Err(BackendFailure::Transport {
                        detail: format!(
                            "web_search plugin backend `{backend}` via `{}` failed: {err}",
                            provider.plugin_id
                        ),
                    });
                }
            }
        }

        Err(unsupported_backend_error(backend))
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
