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
                &err.to_string(),
            )),
        }
    }
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
    if err_text.contains("pi.fetch request timed out") {
        return BackendFailure::Timeout;
    }
    let detail =
        format!("web_search plugin backend `{backend}` via `{plugin_id}` failed: {err_text}");
    if looks_like_plugin_runtime_error(err_text) {
        return BackendFailure::PluginRuntime { detail };
    }
    BackendFailure::Transport { detail }
}

fn looks_like_plugin_runtime_error(err_text: &str) -> bool {
    [
        "QuickJsHost",
        "RustHost",
        "JS执行错误",
        "VM execution error",
        "VM actor channel closed",
        "plugin runtime",
        "plugin execution interrupted",
        "execution exceeded ",
        "async hostcall",
        "LlmProvider not configured",
        "LlmResolver not configured",
        "create runtime for scope activation failed",
    ]
    .iter()
    .any(|needle| err_text.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::classify_plugin_invocation_error;
    use crate::core::tools::web_search::backend::BackendFailure;

    #[test]
    fn classify_plugin_invocation_error_keeps_fetch_timeouts_retryable() {
        let failure = classify_plugin_invocation_error(
            "tavily",
            "tomcat.web-search-backends",
            "JS执行错误: Error: pi.fetch request timed out",
        );
        assert!(matches!(failure, BackendFailure::Timeout));
    }

    #[test]
    fn classify_plugin_invocation_error_marks_vm_failures_non_retryable() {
        let failure = classify_plugin_invocation_error(
            "mimo",
            "tomcat.web-search-backends",
            "JS执行错误: Error converting from js 'RustHost' into type 'QuickJsHost': 插件错误: async hostcall requires a Tokio runtime handle",
        );
        match failure {
            BackendFailure::PluginRuntime { detail } => {
                assert!(detail.contains("async hostcall requires a Tokio runtime handle"));
                assert!(detail.contains("tomcat.web-search-backends"));
            }
            other => panic!("expected PluginRuntime, got {other:?}"),
        }
    }

    #[test]
    fn classify_plugin_invocation_error_falls_back_to_transport_for_other_errors() {
        let failure = classify_plugin_invocation_error(
            "mimo",
            "tomcat.web-search-backends",
            "unexpected backend failure",
        );
        match failure {
            BackendFailure::Transport { detail } => {
                assert!(detail.contains("unexpected backend failure"));
            }
            other => panic!("expected Transport, got {other:?}"),
        }
    }
}
