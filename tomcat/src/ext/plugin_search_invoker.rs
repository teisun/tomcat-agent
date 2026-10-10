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
mod tests {
    use super::classify_plugin_invocation_error;
    use crate::core::tools::web_search::backend::BackendFailure;

    #[test]
    fn async_plugin_function_preserves_timeout_code_in_both_languages() {
        use crate::ext::{PluginEngineConfig, PluginVmInstance};
        use crate::infra::AppError;
        for message in ["pi.fetch request timed out", "pi.fetch 请求超时"] {
            let mut vm =
                PluginVmInstance::new(PluginEngineConfig::default(), "timeout-i18n".into())
                    .unwrap();
            let message = message.to_string();
            vm.register_host_binding(move |_| {
                Ok(serde_json::json!({
                    "ok": false,
                    "error": serde_json::json!({"code":"timeout","message":message}).to_string(),
                })
                .to_string())
            })
            .unwrap();
            let value = vm.run_agent_code(r#"
                tomcat.registerFunction('lookup', async function () {
                    return await tomcat.fetch({url:'https://example.com'});
                });
                return await __pi_execute_function_async(JSON.stringify({functionName:'lookup',params:{}}));
            "#).unwrap();
            assert_eq!(value["ok"], false);
            let raw = value["error"].as_str().unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(raw).unwrap()["code"],
                "timeout"
            );
            assert!(matches!(
                classify_plugin_invocation_error("test", "fixture", &AppError::QuickJS(raw.into())),
                BackendFailure::Timeout
            ));
        }
    }

    #[test]
    fn classify_plugin_invocation_error_uses_timeout_code_without_reading_message() {
        for message in ["diagnostic A", "diagnostic B"] {
            let raw = serde_json::json!({ "code": "timeout", "message": message }).to_string();
            let failure = classify_plugin_invocation_error(
                "tavily",
                "tomcat.web-search-backends",
                &crate::infra::AppError::Plugin(raw),
            );
            assert!(matches!(failure, BackendFailure::Timeout));
        }
    }

    #[test]
    fn coded_host_failure_keeps_transport_identity_inside_a_js_exception() {
        let error = crate::infra::AppError::QuickJS(
            serde_json::json!({
                "code": "request_failed", "message": "opaque upstream detail",
            })
            .to_string(),
        );
        assert!(matches!(
            classify_plugin_invocation_error("test", "fixture", &error),
            BackendFailure::Transport { .. }
        ));
    }

    #[test]
    fn classify_plugin_invocation_error_marks_vm_failures_non_retryable() {
        let failure = classify_plugin_invocation_error(
            "mimo",
            "tomcat.web-search-backends",
            &crate::infra::AppError::QuickJS("Error converting from js 'RustHost' into type 'QuickJsHost': async hostcall requires a Tokio runtime handle".into()),
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
        for message in [
            "unexpected backend failure",
            "pi.fetch request timed out",
            "JS执行错误: 请求超时",
        ] {
            let failure = classify_plugin_invocation_error(
                "mimo",
                "tomcat.web-search-backends",
                &crate::infra::AppError::Plugin(message.into()),
            );
            match failure {
                BackendFailure::Transport { detail } => assert!(detail.contains(message)),
                other => panic!("expected Transport, got {other:?}"),
            }
        }
    }
}
