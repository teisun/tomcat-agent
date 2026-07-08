mod common;

use futures_util::StreamExt;
use std::time::Duration;
use tomcat::{AppConfig, ChatMessage, ChatRequest, StreamEvent};

const STREAM_TIMEOUT: Duration = Duration::from_secs(120);

fn live_model_presets_opt_in(test_name: &str) -> bool {
    match std::env::var("PI_LIVE_MODEL_PRESETS") {
        Ok(value)
            if matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ) =>
        {
            true
        }
        _ => {
            eprintln!(
                "skip {test_name}: set PI_LIVE_MODEL_PRESETS=1 to enable live preset smoke tests"
            );
            false
        }
    }
}

fn require_api_key(env_key: &str, test_name: &str) {
    common::setup_logging();
    common::load_openai_test_env();
    assert!(
        std::env::var(env_key).is_ok(),
        "{test_name} 必须设置 {env_key}（环境变量或 tomcat/.env）"
    );
}

fn kimi_config() -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(common::dot_tomcat_e2e_workdir("live_kimi_builtin").display().to_string());
    common::apply_kimi_app_config(&mut cfg);
    cfg
}

fn anthropic_config() -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir =
        Some(common::dot_tomcat_e2e_workdir("live_anthropic_builtin").display().to_string());
    common::apply_anthropic_app_config(&mut cfg);
    cfg
}

async fn run_stream_smoke(
    label: &str,
    config: AppConfig,
    expected_provider: &str,
    expected_api: &str,
    expected_key_env: &str,
    expected_model: &str,
    max_tokens: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let call = common::resolve_main_call(&config);
    assert_eq!(call.provider, expected_provider, "{label} provider mismatch");
    assert_eq!(call.api, expected_api, "{label} api mismatch");
    assert_eq!(call.model, expected_model, "{label} model mismatch");
    assert_eq!(call.key_source, expected_key_env, "{label} key env mismatch");

    let mut stream = tokio::time::timeout(
        STREAM_TIMEOUT,
        call.provider_impl.chat_stream(ChatRequest {
            messages: vec![ChatMessage::user("Reply with exactly one word: ok")],
            model: call.model.clone(),
            temperature: None,
            max_tokens: Some(max_tokens),
            stream: Some(true),
            model_override: None,
            thinking_level: None,
            tools: None,
        }),
    )
    .await
    .map_err(|_| format!("{label} 启动 chat_stream 超时"))??;

    let mut answer = String::new();
    let mut saw_finish = false;

    loop {
        let next = tokio::time::timeout(STREAM_TIMEOUT, stream.next())
            .await
            .map_err(|_| format!("{label} 读取流事件超时"))?;
        let Some(event) = next else {
            break;
        };
        match event? {
            StreamEvent::ContentDelta { delta } => answer.push_str(&delta),
            StreamEvent::FinishReason { .. } => saw_finish = true,
            _ => {}
        }
    }

    assert!(
        !answer.trim().is_empty(),
        "{label} 应返回至少一段正文，实际为空"
    );
    assert!(
        answer.to_ascii_lowercase().contains("ok"),
        "{label} 应回到简短 ok，实际: {answer:?}"
    );
    assert!(saw_finish, "{label} 应看到 FinishReason");
    Ok(())
}

#[tokio::test]
async fn kimi_builtin_stream_smoke_real_request() -> Result<(), Box<dyn std::error::Error>> {
    if !live_model_presets_opt_in("kimi_builtin_stream_smoke_real_request") {
        return Ok(());
    }
    require_api_key(
        common::KIMI_TEST_API_KEY_ENV,
        "kimi_builtin_stream_smoke_real_request",
    );
    let expected_model = common::kimi_test_model();
    run_stream_smoke(
        "kimi builtin",
        kimi_config(),
        "moonshot",
        "openai",
        common::KIMI_TEST_API_KEY_ENV,
        expected_model.as_str(),
        128,
    )
    .await
}

#[tokio::test]
async fn anthropic_builtin_stream_smoke_via_relay() -> Result<(), Box<dyn std::error::Error>> {
    if !live_model_presets_opt_in("anthropic_builtin_stream_smoke_via_relay") {
        return Ok(());
    }
    require_api_key(
        common::ANTHROPIC_TEST_API_KEY_ENV,
        "anthropic_builtin_stream_smoke_via_relay",
    );
    let expected_model = common::anthropic_test_model();
    run_stream_smoke(
        "anthropic builtin",
        anthropic_config(),
        "anthropic",
        "anthropic-messages",
        common::ANTHROPIC_TEST_API_KEY_ENV,
        expected_model.as_str(),
        32,
    )
    .await
}
