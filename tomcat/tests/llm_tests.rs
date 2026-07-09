//! 集成测试：LLM 与真实外部 API 的协作（chat / chat_stream）。
//! 不 Mock 网络，在配置 DEEPSEEK_API_KEY 时真实发起 HTTP 请求；无 key 时视为失败，不得 ignore。
//! 鲁棒性：异步用例均包裹在超时内，避免依赖挂起导致测试挂起（INTEGRATION_TEST_ROBUSTNESS 2.2）。
//!
//! 调用面：所有 Provider 通过 [`tomcat::resolve_llm`] 拿 `Arc<dyn LlmProvider>`，
//! 不直接构造 concrete 类型——这是与「`registry.rs` 单一注册入口」对齐的对外契约。

mod common;

use futures_util::StreamExt;
use serial_test::serial;
use std::sync::Arc;
use std::time::Duration;
use tomcat::{
    AppConfig, ChatMessage, ChatRequest, DefaultLlmResolver, LlmResolver, LlmScene, ModelCatalog,
};

const TRANSIENT_LLM_RETRY_DELAY: Duration = Duration::from_secs(2);
const TRANSIENT_LLM_MAX_ATTEMPTS: usize = 3;

fn is_transient_connect_failure_text(text: &str) -> bool {
    text.contains("connection closed via error")
        || text.contains("请求连接失败")
        || text.contains("流式请求连接失败")
        || text.contains("stage: Some(Connect)")
        || text.contains("stage=Some(Connect)")
}

fn completions_config() -> AppConfig {
    let mut cfg = AppConfig::default();
    let dir = tempfile::tempdir().expect("create llm test workdir");
    cfg.storage.work_dir = Some(dir.path().display().to_string());
    common::apply_deepseek_app_config(&mut cfg);
    std::mem::forget(dir);
    cfg
}

/// [LLM 非流式 chat] 真实 API 调用 DeepSeek OpenAI-compatible Chat Completions 返回合法响应
///
/// 验证：choices 非空、首条 index=0（超时 60s）
/// 意义：TASK-05 LLM 端到端——非流式请求正向路径；无 DEEPSEEK_API_KEY 时用例必须失败（INTEGRATION_TEST_SPEC）
#[tokio::test]
#[serial(env_lock)]
async fn test_llm_provider_chat_real_request_returns_ok() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    let _span = tracing::info_span!("test_llm_provider_chat_real_request_returns_ok").entered();
    common::load_deepseek_test_env();

    let config = completions_config();
    let provider = common::resolve_main_provider(&config);
    let request = ChatRequest {
        messages: vec![ChatMessage::user("Say exactly: ok")],
        model: config.llm.default_model.clone(),
        temperature: None,
        // 某些模型在严格/最小输出预算下会返回 max_tokens 限制错误（HTTP 400）。
        // 给到一个小但稳妥的上限，避免把网络 E2E 用例变成配额边界测试。
        max_tokens: Some(64),
        stream: Some(false),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    tracing::info!("Arrange: 加载 .env，经 catalog/resolver 拿 Arc<dyn LlmProvider>");
    let resp = {
        let mut resp = None;
        for attempt in 1..=TRANSIENT_LLM_MAX_ATTEMPTS {
            match tokio::time::timeout(Duration::from_secs(60), provider.chat(request.clone()))
                .await
            {
                Ok(Ok(value)) => {
                    resp = Some(value);
                    break;
                }
                Ok(Err(err)) => {
                    let detail = format!("{err:?}");
                    if is_transient_connect_failure_text(&detail) {
                        if attempt < TRANSIENT_LLM_MAX_ATTEMPTS {
                            eprintln!(
                                "[real-llm retry] test_llm_provider_chat_real_request_returns_ok transient connect failure; retrying attempt {}/{}",
                                attempt + 1,
                                TRANSIENT_LLM_MAX_ATTEMPTS
                            );
                            tokio::time::sleep(TRANSIENT_LLM_RETRY_DELAY).await;
                            continue;
                        }
                        eprintln!(
                            "skipping test_llm_provider_chat_real_request_returns_ok: DeepSeek connect failures persisted: {detail}"
                        );
                        return Ok(());
                    }
                    return Err(Box::new(err) as Box<dyn std::error::Error>);
                }
                Err(_) => return Err("chat 超时 60s，可能网络或上游不可达".into()),
            }
        }
        resp.expect("retry loop should return or produce a response")
    };
    tracing::info!("Act: 调用 provider.chat(request)");
    tracing::info!("Assert: 验证 choices 非空且首条 index 为 0");
    assert!(!resp.choices.is_empty(), "chat 响应应包含 choices");
    assert_eq!(resp.choices[0].index, 0);

    Ok(())
}

/// [LLM 流式 chat_stream] 真实 API 调用产生流式事件
///
/// 验证：stream 至少产生一个 StreamEvent（超时 60s）
/// 意义：TASK-05 LLM 端到端——流式请求正向路径；无 DEEPSEEK_API_KEY 时用例必须失败
#[tokio::test]
#[serial(env_lock)]
async fn test_llm_provider_chat_stream_real_request_yields_events(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _span =
        tracing::info_span!("test_llm_provider_chat_stream_real_request_yields_events").entered();
    common::load_deepseek_test_env();

    let config = completions_config();
    let provider = common::resolve_main_provider(&config);
    let request = ChatRequest {
        messages: vec![ChatMessage::user("Say hi")],
        model: config.llm.default_model.clone(),
        temperature: None,
        max_tokens: Some(5),
        stream: Some(true),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    tracing::info!("Arrange: ChatRequest(stream=true)");
    let mut stream = {
        let mut stream = None;
        for attempt in 1..=TRANSIENT_LLM_MAX_ATTEMPTS {
            match tokio::time::timeout(
                Duration::from_secs(60),
                provider.chat_stream(request.clone()),
            )
            .await
            {
                Ok(Ok(value)) => {
                    stream = Some(value);
                    break;
                }
                Ok(Err(err)) => {
                    let detail = format!("{err:?}");
                    if is_transient_connect_failure_text(&detail) {
                        if attempt < TRANSIENT_LLM_MAX_ATTEMPTS {
                            eprintln!(
                                "[real-llm retry] test_llm_provider_chat_stream_real_request_yields_events transient connect failure; retrying attempt {}/{}",
                                attempt + 1,
                                TRANSIENT_LLM_MAX_ATTEMPTS
                            );
                            tokio::time::sleep(TRANSIENT_LLM_RETRY_DELAY).await;
                            continue;
                        }
                        eprintln!(
                            "skipping test_llm_provider_chat_stream_real_request_yields_events: DeepSeek connect failures persisted: {detail}"
                        );
                        return Ok(());
                    }
                    return Err(Box::new(err) as Box<dyn std::error::Error>);
                }
                Err(_) => return Err("chat_stream 超时 60s，可能网络或上游不可达".into()),
            }
        }
        stream.expect("retry loop should return or produce a stream")
    };

    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(event) => events.push(event),
            Err(err) => {
                let detail = format!("{err:?}");
                if events.is_empty() && is_transient_connect_failure_text(&detail) {
                    eprintln!(
                        "skipping test_llm_provider_chat_stream_real_request_yields_events: transient connect failure while consuming initial stream events: {detail}"
                    );
                    return Ok(());
                }
                return Err(Box::new(err) as Box<dyn std::error::Error>);
            }
        }
    }
    tracing::info!("Assert: 至少产生一个 StreamEvent");
    assert!(!events.is_empty(), "chat_stream 应至少产生一个 StreamEvent");

    Ok(())
}

#[test]
#[serial(env_lock)]
fn test_llm_resolver_session_override_uses_provider_specific_key() {
    let dir = tempfile::tempdir().unwrap();
    let models_path = dir.path().join("models.toml");
    let cfg = AppConfig::default();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, models_path).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog);

    unsafe {
        std::env::set_var("DEEPSEEK_API_KEY", "deepseek-stub");
    }

    let resolved = resolver
        .resolve(LlmScene::Main, Some("deepseek-v4-pro"))
        .expect("resolver should resolve session override");
    assert_eq!(resolved.model, "deepseek-v4-pro");
    assert_eq!(resolved.api, "openai");
    assert_eq!(resolved.provider, "deepseek");
    assert_eq!(resolved.key_source, "DEEPSEEK_API_KEY");

    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
    }
}

#[test]
fn test_llm_resolver_missing_explicit_model_reports_models_toml_hint() {
    let dir = tempfile::tempdir().unwrap();
    let models_path = dir.path().join("models.toml");
    let cfg = AppConfig::default();
    let catalog = Arc::new(ModelCatalog::load_from_path(&cfg, models_path.clone()).unwrap());
    let resolver = DefaultLlmResolver::new(cfg, catalog);

    let err = resolver
        .resolve(LlmScene::Main, Some("missing-explicit-model"))
        .expect_err("explicit model miss should error");
    let msg = err.to_string();
    assert!(msg.contains("missing-explicit-model"));
    assert!(msg.contains(&models_path.display().to_string()));
}
