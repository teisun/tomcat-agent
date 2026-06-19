mod common;

use common::serve::{
    extract_json_body, response, spawn_scripted_openai_stream_server, sse_delta, sse_done,
    sse_finish,
};
use futures_util::StreamExt;
use serial_test::serial;
use std::sync::Arc;
use tomcat::{
    AppConfig, ChatMessage, ChatRequest, DefaultLlmResolver, LlmResolver, LlmScene, ModelCatalog,
};

fn resolve_main_call(cfg: &AppConfig) -> tomcat::ResolvedCall {
    let catalog = Arc::new(ModelCatalog::load(cfg).expect("load model catalog"));
    let resolver = DefaultLlmResolver::new(cfg.clone(), catalog);
    resolver
        .resolve(LlmScene::Main, None)
        .expect("resolve main model")
}

#[test]
#[serial(env_lock)]
fn default_openai_target_uses_gateway_model_and_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());

    unsafe {
        std::env::remove_var(common::OPENAI_TEST_MODEL_ENV);
        std::env::set_var(common::OPENAI_GATEWAY_TEST_API_KEY_ENV, "gateway-stub");
    }

    common::apply_openai_app_config(&mut cfg);
    let resolved = resolve_main_call(&cfg);

    assert_eq!(cfg.llm.default_model, "gpt-5.4_litellm-sunmi");
    assert_eq!(resolved.provider, "litellm-sunmi");
    assert_eq!(resolved.api, "openai-responses");
    assert_eq!(resolved.model, "gpt-5.4");
    assert_eq!(resolved.key_source, common::OPENAI_GATEWAY_TEST_API_KEY_ENV);

    unsafe {
        std::env::remove_var(common::OPENAI_GATEWAY_TEST_API_KEY_ENV);
    }
}

#[test]
#[serial(env_lock)]
fn openai_target_env_override_switches_back_to_builtin_openai() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());

    unsafe {
        std::env::set_var(common::OPENAI_TEST_MODEL_ENV, "gpt-5.4");
        std::env::set_var("OPENAI_API_KEY", "openai-stub");
    }

    common::apply_openai_app_config(&mut cfg);
    let resolved = resolve_main_call(&cfg);

    assert_eq!(cfg.llm.default_model, "gpt-5.4");
    assert_eq!(resolved.provider, "openai");
    assert_eq!(resolved.api, "openai-responses");
    assert_eq!(resolved.model, "gpt-5.4");
    assert_eq!(resolved.key_source, "OPENAI_API_KEY");

    unsafe {
        std::env::remove_var(common::OPENAI_TEST_MODEL_ENV);
        std::env::remove_var("OPENAI_API_KEY");
    }
}

#[tokio::test]
#[serial(env_lock)]
async fn gateway_model_routes_with_wire_name_in_request_body() {
    let server = spawn_scripted_openai_stream_server(vec![response(vec![
        sse_delta("ok"),
        sse_finish("stop"),
        sse_done(),
    ])]);
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    cfg.llm.default_model = "gpt-5.4_litellm-sunmi".to_string();
    cfg.context.compaction_model = "gpt-5.4".to_string();

    std::fs::write(
        dir.path().join("models.toml"),
        format!(
            r#"[[models]]
id = "gpt-5.4_litellm-sunmi"
model_name = "gpt-5.4"
api = "openai-responses"
provider = "litellm-sunmi"
api_key_env = "{env_name}"
base_url = "{base_url}"
thinking_format = "openai"
capabilities = {{ vision = true, files = true, tools = true, reasoning = true, web_search = false }}
"#,
            env_name = common::OPENAI_GATEWAY_TEST_API_KEY_ENV,
            base_url = server.base_url,
        ),
    )
    .expect("write gateway models.toml");

    unsafe {
        std::env::set_var(common::OPENAI_GATEWAY_TEST_API_KEY_ENV, "gateway-stub");
    }

    let resolved = resolve_main_call(&cfg);
    assert_eq!(resolved.model, "gpt-5.4");

    let mut stream = resolved
        .provider_impl
        .chat_stream(ChatRequest {
            messages: vec![ChatMessage::user("Say ok")],
            model: resolved.model.clone(),
            temperature: None,
            max_tokens: Some(16),
            stream: Some(true),
            model_override: None,
            tools: None,
        })
        .await
        .expect("start chat stream");

    while let Some(item) = stream.next().await {
        item.expect("stream item");
    }

    let request = server
        .captured_requests()
        .into_iter()
        .next()
        .expect("captured request");
    let body = extract_json_body(&request);
    assert_eq!(body["model"].as_str(), Some("gpt-5.4"));

    unsafe {
        std::env::remove_var(common::OPENAI_GATEWAY_TEST_API_KEY_ENV);
    }
}
