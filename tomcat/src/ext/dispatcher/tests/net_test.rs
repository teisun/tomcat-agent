use super::{
    fetch_error_with_optional_details, is_allowed_host, parse_fetch_spec, resolve_secret_text,
};
use crate::core::llm::auth::{
    record_preloaded_env_names, refresh_managed_credentials_from_entries,
};
use crate::ext::plugin::PluginSource;
use crate::infra::AppError;
use serde_json::{json, Value};

fn error_json(err: AppError) -> Value {
    match err {
        AppError::Plugin(raw) => serde_json::from_str(&raw).expect("plugin errors should be JSON"),
        other => panic!("expected AppError::Plugin JSON, got {other:?}"),
    }
}

#[test]
fn rejects_undeclared_and_missing_header_secrets_with_details() {
    for (required, code) in [
        (vec![], "forbidden_secret"),
        (vec!["EXAMPLE_KEY".into()], "missing_secret"),
    ] {
        let err =
            resolve_secret_text("Bearer {{secret:EXAMPLE_KEY}}", &required, &|_| None).unwrap_err();
        let error = error_json(err);
        assert_eq!(error["code"], code);
        assert_eq!(error["details"]["secretName"], "EXAMPLE_KEY");
    }
}

#[test]
fn rejects_placeholders_outside_header_values() {
    for params in [
        json!({"url":"https://example.com/{{secret:EXAMPLE_KEY}}"}),
        json!({"url":"https://example.com/search", "headers":{"{{secret:EXAMPLE_KEY}}":"x"}}),
        json!({"url":"https://example.com/search", "query":{"q":["{{secret:EXAMPLE_KEY}}"]}}),
        json!({"url":"https://example.com/search", "query":{"q":"{{secret:EXAMPLE_KEY}}"}}),
        json!({"url":"https://example.com/search", "query":{"{{secret:EXAMPLE_KEY}}":"q"}}),
    ] {
        assert_eq!(
            error_json(parse_fetch_spec(&params, &[], false).unwrap_err())["code"],
            "forbidden_secret"
        );
    }
}

#[test]
#[serial_test::serial(env_lock)]
fn layer_policy_applies_only_to_headers_and_keeps_saved_keys_live() {
    const SLOT: &str = "TOMCAT_FETCH_LAYER_TEST_KEY";
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            std::env::remove_var(SLOT);
            refresh_managed_credentials_from_entries(&Default::default());
        }
    }
    let _reset = Reset;
    std::env::set_var(SLOT, "explicit");
    let params = json!({"url":"https://unit.example/search", "method":"POST",
        "headers":{"Authorization":"Bearer {{secret:TOMCAT_FETCH_LAYER_TEST_KEY}}"},
        "body":{"query":["{{secret:TOMCAT_FETCH_LAYER_TEST_KEY}}", "{{secret:UNDECLARED}}"]}});
    let required = [SLOT.into()];
    for key in ["saved-first", "saved-second"] {
        refresh_managed_credentials_from_entries(&[(SLOT.into(), key.into())].into());
        for source in [
            PluginSource::Managed,
            PluginSource::Agent,
            PluginSource::Project,
        ] {
            let spec = parse_fetch_spec(&params, &required, source.can_use_saved_keys()).unwrap();
            let expected = if source == PluginSource::Project {
                "explicit"
            } else {
                key
            };
            assert_eq!(spec.headers["authorization"], format!("Bearer {expected}"));
            assert!(spec.headers["authorization"].is_sensitive());
            assert_eq!(
                serde_json::from_str::<Value>(&spec.body.unwrap()).unwrap(),
                params["body"]
            );
        }
    }
    refresh_managed_credentials_from_entries(&Default::default());
    assert_eq!(
        parse_fetch_spec(&params, &required, true).unwrap().headers["authorization"],
        "Bearer explicit"
    );
    // Preload tracking is additive; use a separate slot so this case doesn't
    // change the provenance of the explicit variable above.
    const PRELOADED: &str = "TOMCAT_FETCH_LAYER_PRELOADED_TEST_KEY";
    record_preloaded_env_names([PRELOADED.to_string()]);
    std::env::set_var(PRELOADED, "preloaded");
    let preloaded_params = json!({"url": "https://unit.example/search", "headers": {
        "Authorization": "Bearer {{secret:TOMCAT_FETCH_LAYER_PRELOADED_TEST_KEY}}"
    }});
    let result = parse_fetch_spec(&preloaded_params, &[PRELOADED.into()], false);
    std::env::remove_var(PRELOADED);
    assert_eq!(error_json(result.unwrap_err())["code"], "missing_secret");
}

#[test]
fn shared_serve_only_resolves_saved_secrets_for_user_installed_plugins() {
    // Shared Serve's saved-only flag cannot be reset. Exercise it in a fresh process.
    const CHILD: &str = "TOMCAT_TEST_SHARED_FETCH_LAYER_CHILD";
    let module = module_path!().split_once("::").unwrap().1;
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!(
                    "{module}::shared_serve_only_resolves_saved_secrets_for_user_installed_plugins"
                ),
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("TOMCAT_FETCH_LAYER_TEST_KEY", "inherited")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("SHARED_FETCH_ASSERTIONS_PASSED"));
        return;
    }
    crate::infra::config::use_saved_configuration_only();
    let required = ["TOMCAT_FETCH_LAYER_TEST_KEY".into()];
    let params = json!({"url":"https://example.com/search", "headers":{"X-Api-Key":"{{secret:TOMCAT_FETCH_LAYER_TEST_KEY}}"}});
    assert_eq!(
        error_json(parse_fetch_spec(&params, &required, true).unwrap_err())["code"],
        "missing_secret"
    );
    refresh_managed_credentials_from_entries(
        &[("TOMCAT_FETCH_LAYER_TEST_KEY".into(), "saved".into())].into(),
    );
    assert_eq!(
        parse_fetch_spec(&params, &required, PluginSource::Agent.can_use_saved_keys())
            .unwrap()
            .headers["x-api-key"],
        "saved"
    );
    assert_eq!(
        error_json(
            parse_fetch_spec(
                &params,
                &required,
                PluginSource::Project.can_use_saved_keys()
            )
            .unwrap_err()
        )["code"],
        "missing_secret"
    );
    println!("SHARED_FETCH_ASSERTIONS_PASSED");
}

#[test]
fn is_allowed_host_is_case_insensitive_and_trims_trailing_dots() {
    assert!(is_allowed_host(
        "API.Search.Brave.Com.",
        &["api.search.brave.com".into()]
    ));
    assert!(!is_allowed_host(
        "api.search.brave.com.evil.example",
        &["api.search.brave.com".into()]
    ));
}

#[test]
fn fetch_error_json_keeps_optional_details_shape() {
    let parsed = error_json(fetch_error_with_optional_details(
        "missing_secret",
        "secret missing",
        Some(json!({ "secretName": "TAVILY_API_KEY" })),
    ));
    assert_eq!(parsed["code"], "missing_secret");
    assert_eq!(parsed["message"], "secret missing");
    assert_eq!(parsed["details"]["secretName"], "TAVILY_API_KEY");
}
