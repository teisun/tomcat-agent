use super::invocation_error;
use crate::infra::i18n::tr;
use crate::infra::AppError;

#[test]
fn async_plugin_function_preserves_timeout_code_in_both_languages() {
    use crate::ext::{PluginEngineConfig, PluginVmInstance};
    for message in ["pi.fetch request timed out", "pi.fetch 请求超时"] {
        let mut vm =
            PluginVmInstance::new(PluginEngineConfig::default(), "timeout-i18n".into()).unwrap();
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
        assert_eq!(
            invocation_error("test", "fixture", &AppError::QuickJS(raw.into())).to_string(),
            AppError::Tool(tr("search.timeout", &[("backend", "test")])).to_string()
        );
    }
}

#[test]
fn only_the_timeout_code_maps_to_a_timeout() {
    let timeout = serde_json::json!({ "code": "timeout", "message": "diagnostic" }).to_string();
    assert!(invocation_error("tavily", "p", &AppError::Plugin(timeout))
        .to_string()
        .contains(&tr("search.timeout", &[("backend", "tavily")])));

    for error in [
        AppError::QuickJS(
            serde_json::json!({"code": "request_failed", "message": "opaque upstream detail"})
                .to_string(),
        ),
        AppError::QuickJS("Error: synthetic plugin failure".into()),
        AppError::Plugin("pi.fetch request timed out".into()),
    ] {
        let detail = error.to_string();
        let text = invocation_error("mimo", "tomcat.web-search-backends", &error).to_string();
        let expected = tr(
            "plugin.searchFailed",
            &[
                ("backend", "mimo"),
                ("plugin", "tomcat.web-search-backends"),
                ("detail", &detail),
            ],
        );
        assert!(text.contains(&expected), "{text}");
    }
}
