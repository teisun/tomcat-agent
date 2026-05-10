//! 集成测试：OpenAI Files 上传管理（T2-P0-015）。
//!
//! 说明：
//! - 真实网络调用，依赖 `OPENAI_API_KEY`（环境变量或 `.env`）。
//! - 为避免日常开发被大文件上传拖慢，默认 `#[ignore]`，按需手动执行：
//!   `cargo test --test openai_files_integration_tests -- --ignored --nocapture`

mod common;

use serial_test::serial;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tomcat::{
    resolve_llm, ChatMessage, ChatMessageContentPart, ChatRequest, LlmConfig, LlmProvider,
};
use tomcat::core::llm::openai_files::{FilePurpose, OpenAiFilesClient};

fn responses_config() -> LlmConfig {
    LlmConfig {
        provider: "openai-responses".to_string(),
        ..LlmConfig::default()
    }
}

fn unique_prefix() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("t2-p0-015-it-{ts}")
}

fn files_client_from_provider(
    provider: &dyn LlmProvider,
    cfg: &LlmConfig,
) -> Result<OpenAiFilesClient, Box<dyn std::error::Error>> {
    if !provider.supports_openai_files_api() {
        return Err("当前 provider 不支持 OpenAI Files API".into());
    }
    provider
        .openai_files_client(&cfg.files)
        .ok_or_else(|| "provider 未返回 OpenAI Files client".into())
}

struct CleanupGuard {
    client: OpenAiFilesClient,
    ids: Vec<String>,
}

impl CleanupGuard {
    fn new(client: OpenAiFilesClient) -> Self {
        Self {
            client,
            ids: Vec::new(),
        }
    }
    fn track(&mut self, file_id: String) {
        self.ids.push(file_id);
    }
    fn untrack(&mut self, file_id: &str) {
        self.ids.retain(|id| id != file_id);
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if self.ids.is_empty() {
            return;
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let ids = std::mem::take(&mut self.ids);
            let client = self.client.clone();
            handle.spawn(async move {
                for id in ids {
                    let _ = client.delete(&id).await;
                }
            });
        }
    }
}

#[tokio::test]
#[serial]
#[ignore = "真实 API + 大文件上传，手动触发"]
async fn openai_files_roundtrip_four_sizes_real_api() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _ = dotenvy::dotenv().ok();
    let cfg = responses_config();
    let provider = resolve_llm(&cfg)
        .expect("集成测试要求设置 OPENAI_API_KEY（环境变量或 .env），无 key 视为失败");
    let client = files_client_from_provider(provider.as_ref(), &cfg)?;
    let prefix = unique_prefix();

    let mut guard = CleanupGuard::new(client.clone());
    for size in [1024usize, 100 * 1024, 5 * 1024 * 1024, 20 * 1024 * 1024] {
        let filename = format!("{prefix}-{size}.bin");
        let payload = vec![b'a'; size];
        let uploaded = tokio::time::timeout(
            Duration::from_secs(180),
            client.upload(
                FilePurpose::UserData,
                &filename,
                "application/octet-stream",
                &payload,
            ),
        )
        .await
        .map_err(|_| format!("upload timeout for size={size}"))??;
        assert!(!uploaded.id.is_empty(), "size={size} 上传应返回 file_id");
        guard.track(uploaded.id.clone());
        let fetched = client.get(&uploaded.id).await?;
        assert!(fetched.is_some(), "size={size} get 应返回记录");
        client.delete(&uploaded.id).await?;
        guard.untrack(&uploaded.id);
    }

    let leftovers = client.list(Some(&prefix), None).await?;
    assert!(
        leftovers.is_empty(),
        "测试结束后不应有残留 files，leftovers={:?}",
        leftovers.iter().map(|f| f.id.clone()).collect::<Vec<_>>()
    );
    Ok(())
}

#[tokio::test]
#[serial]
#[ignore = "真实 API，手动触发"]
async fn openai_file_id_reference_roundtrip_real_api() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    let _ = dotenvy::dotenv().ok();
    let cfg = responses_config();
    let provider = resolve_llm(&cfg)
        .expect("集成测试要求设置 OPENAI_API_KEY（环境变量或 .env），无 key 视为失败");
    let client = files_client_from_provider(provider.as_ref(), &cfg)?;
    let prefix = unique_prefix();
    let filename = format!("{prefix}-sample.txt");
    let uploaded = client
        .upload(
            FilePurpose::UserData,
            &filename,
            "text/plain",
            b"Hello from T2-P0-015 integration test.",
        )
        .await?;

    let mut guard = CleanupGuard::new(client.clone());
    guard.track(uploaded.id.clone());
    let parts = vec![
        ChatMessageContentPart::text("Read the attached file and answer with one short sentence."),
        ChatMessageContentPart::file_file_id(uploaded.id.clone(), Some(filename.clone()))?,
    ];
    let req = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(parts)],
        model: cfg.default_model.clone(),
        temperature: None,
        max_tokens: Some(96),
        stream: Some(false),
        model_override: None,
        tools: None,
    };
    let resp = tokio::time::timeout(Duration::from_secs(120), provider.chat(req))
        .await
        .map_err(|_| "responses chat timeout 120s")??;
    assert!(!resp.choices.is_empty(), "responses 引用 file_id 后应返回 choices");

    client.delete(&uploaded.id).await?;
    guard.untrack(&uploaded.id);
    let leftovers = client.list(Some(&prefix), None).await?;
    assert!(leftovers.is_empty(), "roundtrip 后不应残留测试文件");
    Ok(())
}
