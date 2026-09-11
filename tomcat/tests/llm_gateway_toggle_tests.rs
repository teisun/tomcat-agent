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
    let resolver = DefaultLlmResolver::new(cfg.clone(), catalog, common::model_prefs_for(cfg));
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

/// 派发子 Agent 只能用 catalog id，不能用 `ResolvedCall::model` 那个线上模型名。
///
/// 两者在网关型条目上必然不同：`gpt-5.4_litellm-sunmi` 走到线上是 `gpt-5.4`。子 Agent
/// 拿到线上名后会再回 catalog 查一次 provider，于是有两种结局 —— 查不到就 404，查到了
/// 同名的另一个条目就更糟：复审在你完全不知情的情况下换了个账号组跑。
///
/// 实测（2026-07-27 冒烟）：会话切到 `fcodex/claude-opus-4-8` 后，复审子 Agent 带着
/// `claude-opus-4-8` 去派发，报 `Model "claude-opus-4-8" is not supported by any
/// configured account in this group`，复审连一轮都没跑起来。
#[test]
#[serial(env_lock)]
fn wire_model_name_is_not_a_catalog_id_so_subagents_must_dispatch_by_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());

    unsafe {
        std::env::remove_var(common::OPENAI_TEST_MODEL_ENV);
        std::env::set_var(common::OPENAI_GATEWAY_TEST_API_KEY_ENV, "gateway-stub");
    }
    common::apply_openai_app_config(&mut cfg);

    let catalog = ModelCatalog::load(&cfg).expect("load model catalog");
    let id = cfg.llm.default_model.clone();
    let wire = resolve_main_call(&cfg).model;

    assert_ne!(id, wire, "网关条目的 catalog id 与线上名本就不同");
    let by_id = catalog.lookup(&id).expect("catalog id 必须能查回条目");
    assert_eq!(by_id.provider, "litellm-sunmi");

    // 这份 catalog 里 `gpt-5.4` 恰好也是一个条目 —— 于是拿线上名去派发不会报错，
    // 只会安静地换一个 provider 跑。这比 404 更难发现，也正是必须传 id 的理由。
    assert_eq!(
        catalog.lookup(&wire).map(|entry| entry.provider.clone()),
        Some("openai".to_string()),
        "线上名 `{wire}` 指向的是另一个条目，拿它派发子 Agent 会串到别的 provider"
    );

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

#[serial(env_lock)]
fn openai_target_env_override_treats_55_and_56_as_builtin_openai() {
    for model_id in ["gpt-5.5", "gpt-5.6"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());

        unsafe {
            std::env::set_var(common::OPENAI_TEST_MODEL_ENV, model_id);
            std::env::set_var("OPENAI_API_KEY", "openai-stub");
        }

        common::apply_openai_app_config(&mut cfg);
        let resolved = resolve_main_call(&cfg);

        assert_eq!(cfg.llm.default_model, model_id);
        assert_eq!(resolved.provider, "openai");
        assert_eq!(resolved.api, "openai-responses");
        assert_eq!(resolved.model, model_id);
        assert_eq!(resolved.key_source, "OPENAI_API_KEY");

        unsafe {
            std::env::remove_var(common::OPENAI_TEST_MODEL_ENV);
            std::env::remove_var("OPENAI_API_KEY");
        }
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

    let mut request = ChatRequest {
        messages: vec![ChatMessage::user("Say ok")],
        model: resolved.model.clone(),
        temperature: None,
        max_tokens: Some(16),
        resolved_output_limit: None,
        diagnostic_request_id: None,
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        cache_key: None,
        tools: None,
    };
    resolved.apply_resolved_output_limit(&mut request);
    let mut stream = resolved
        .provider_impl
        .chat_stream(request)
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
