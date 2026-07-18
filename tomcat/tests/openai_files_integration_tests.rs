//! 集成测试：OpenAI Files 上传管理（T2-P0-015）。
//!
//! 说明：
//! - 真实网络调用，依赖 `OPENAI_API_KEY`（环境变量或 `.env`，与 `tests/openai_responses_integration_tests.rs` 同模式）。
//! - **TODO(T2-P0-015)**：当前用例全部 `#[ignore]`——部分账号/项目 key 对 `/v1/files` 返回 401/500（与 Responses 可用性不一致），待拿到**明确支持 Files API** 的 key 后去掉 `#[ignore]` 并跑通验收。
//! - 手动执行被忽略的用例：
//!   `PI_LIVE_OPENAI_FILES=1 cargo test --test openai_files_integration_tests -- --ignored --nocapture`

mod common;

use serial_test::serial;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tomcat::core::llm::openai_files::{FilePurpose, OpenAiFilesClient};
use tomcat::{AppConfig, ChatMessage, ChatMessageContentPart, ChatRequest, LlmProvider};

const SAMPLE_IMAGE_PATH: &str = "tests/fixtures/llm_multimodal/sample_image.png";
const SAMPLE_PDF_B64: &str = include_str!("fixtures/llm_multimodal/sample_pdf_b64.txt");

struct ResponsesFixture {
    _home: common::TempHomeGuard,
    config: AppConfig,
}

fn responses_fixture() -> ResponsesFixture {
    let home = common::TempHomeGuard::new();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(
        common::dot_tomcat_e2e_workdir("openai_files")
            .display()
            .to_string(),
    );
    common::apply_openai_app_config(&mut cfg);
    ResponsesFixture {
        _home: home,
        config: cfg,
    }
}

fn decode_b64_to_tempfile(
    b64: &str,
) -> Result<tempfile::NamedTempFile, Box<dyn std::error::Error>> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes())?;
    let mut f = tempfile::NamedTempFile::new()?;
    std::io::Write::write_all(&mut f, &bytes)?;
    Ok(f)
}

fn contains_any(text: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|kw| text.contains(kw))
}

fn unique_prefix() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("t2-p0-015-it-{ts}")
}

fn require_live_openai_files_opt_in(test_name: &str) -> bool {
    if std::env::var("PI_LIVE_OPENAI_FILES").ok().as_deref() == Some("1") {
        return true;
    }
    eprintln!("skip {test_name}: set PI_LIVE_OPENAI_FILES=1 to enable live OpenAI Files API tests");
    false
}

fn files_client_from_provider(
    provider: &dyn LlmProvider,
    cfg: &AppConfig,
) -> Result<OpenAiFilesClient, Box<dyn std::error::Error>> {
    if !provider.supports_openai_files_api() {
        return Err("当前 provider 不支持 OpenAI Files API".into());
    }
    provider
        .openai_files_client(&cfg.llm.files)
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

// TODO(T2-P0-015): 具备可用 `/v1/files` 的 OPENAI_API_KEY 后去掉下一行 `#[ignore]`。
#[tokio::test]
#[ignore = "T2-P0-015: 待可用 Files API 的 key；手动 cargo test --test openai_files_integration_tests -- --ignored"]
#[serial]
async fn openai_files_roundtrip_four_sizes_real_api() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    if !require_live_openai_files_opt_in("openai_files_roundtrip_four_sizes_real_api") {
        return Ok(());
    }
    common::load_openai_test_env();
    let fixture = responses_fixture();
    let provider = common::resolve_main_provider(&fixture.config);
    let client = files_client_from_provider(provider.as_ref(), &fixture.config)?;
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

// TODO(T2-P0-015): 具备可用 `/v1/files` 的 OPENAI_API_KEY 后去掉下一行 `#[ignore]`。
#[tokio::test]
#[ignore = "T2-P0-015: 待可用 Files API 的 key；手动 cargo test --test openai_files_integration_tests -- --ignored"]
#[serial]
async fn openai_file_id_reference_roundtrip_real_api() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    if !require_live_openai_files_opt_in("openai_file_id_reference_roundtrip_real_api") {
        return Ok(());
    }
    common::load_openai_test_env();
    let fixture = responses_fixture();
    let provider = common::resolve_main_provider(&fixture.config);
    let client = files_client_from_provider(provider.as_ref(), &fixture.config)?;
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
        model: fixture.config.llm.default_model.clone(),
        temperature: None,
        max_tokens: Some(96),
        stream: Some(false),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let resp = tokio::time::timeout(Duration::from_secs(120), provider.chat(req))
        .await
        .map_err(|_| "responses chat timeout 120s")??;
    assert!(
        !resp.choices.is_empty(),
        "responses 引用 file_id 后应返回 choices"
    );
    let text = resp.choices[0]
        .message
        .text_content()
        .unwrap_or("")
        .to_ascii_lowercase();
    assert!(!text.trim().is_empty(), "responses 文本不应为空");

    client.delete(&uploaded.id).await?;
    guard.untrack(&uploaded.id);
    let leftovers = client.list(Some(&prefix), None).await?;
    assert!(leftovers.is_empty(), "roundtrip 后不应残留测试文件");
    Ok(())
}

/// CLI 单轮编排：同一轮内执行「本地图片 -> Files upload -> file_id part -> Responses」。
///
/// 验证：真实图片附件（`sample_image.png`）经 file_id 通道可被模型正确描述。
// TODO(T2-P0-015): 具备可用 `/v1/files` 的 OPENAI_API_KEY 后去掉下一行 `#[ignore]`。
#[tokio::test]
#[ignore = "T2-P0-015: 待可用 Files API 的 key；手动 cargo test --test openai_files_integration_tests -- --ignored"]
#[serial]
async fn openai_files_cli_single_turn_image_describe_real_api(
) -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    if !require_live_openai_files_opt_in("openai_files_cli_single_turn_image_describe_real_api") {
        return Ok(());
    }
    common::load_openai_test_env();
    let fixture = responses_fixture();
    let provider = common::resolve_main_provider(&fixture.config);
    let client = files_client_from_provider(provider.as_ref(), &fixture.config)?;
    let prefix = unique_prefix();
    let image_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(SAMPLE_IMAGE_PATH);
    let image_bytes = std::fs::read(&image_path)?;
    let filename = format!("{prefix}-cli-image.png");

    let mut guard = CleanupGuard::new(client.clone());
    // CLI 单轮：先 upload（或命中 cache）再组同轮 ChatRequest。
    let uploaded = client
        .upload(FilePurpose::Vision, &filename, "image/png", &image_bytes)
        .await?;
    guard.track(uploaded.id.clone());

    let parts = vec![
        ChatMessageContentPart::text("Describe what you see in this image in one short sentence."),
        ChatMessageContentPart::image_file_id(uploaded.id.clone())?,
    ];
    let req = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(parts)],
        model: fixture.config.llm.default_model.clone(),
        temperature: None,
        max_tokens: Some(96),
        stream: Some(false),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let resp = tokio::time::timeout(Duration::from_secs(120), provider.chat(req))
        .await
        .map_err(|_| "CLI-like image describe timeout 120s")??;
    assert!(!resp.choices.is_empty(), "图片描述响应应包含 choices");
    let text = resp.choices[0]
        .message
        .text_content()
        .unwrap_or("")
        .to_ascii_lowercase();
    assert!(!text.trim().is_empty(), "图片描述文本不应为空");
    let keywords = [
        "dog",
        "puppy",
        "animal",
        "pet",
        "canine",
        "beagle",
        "labrador",
        "retriever",
        "terrier",
        "shepherd",
        "poodle",
        "bulldog",
        "corgi",
        "husky",
    ];
    assert!(
        contains_any(&text, &keywords),
        "图片描述应命中关键词 {:?} 之一，实际: {:?}",
        keywords,
        text
    );

    client.delete(&uploaded.id).await?;
    guard.untrack(&uploaded.id);
    let leftovers = client.list(Some(&prefix), None).await?;
    assert!(leftovers.is_empty(), "CLI-like 图片测试后不应残留文件");
    Ok(())
}

/// TUI 两阶段编排：
/// - 阶段 A：仅上传 PDF 到 Files，拿到 file_id；
/// - 阶段 B：文本 + 阶段 A 的 file_id 一起发给 Responses。
// TODO(T2-P0-015): 具备可用 `/v1/files` 的 OPENAI_API_KEY 后去掉下一行 `#[ignore]`。
#[tokio::test]
#[ignore = "T2-P0-015: 待可用 Files API 的 key；手动 cargo test --test openai_files_integration_tests -- --ignored"]
#[serial]
async fn openai_files_tui_two_phase_pdf_describe_real_api() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    if !require_live_openai_files_opt_in("openai_files_tui_two_phase_pdf_describe_real_api") {
        return Ok(());
    }
    common::load_openai_test_env();
    let fixture = responses_fixture();
    let provider = common::resolve_main_provider(&fixture.config);
    let client = files_client_from_provider(provider.as_ref(), &fixture.config)?;
    let prefix = unique_prefix();
    let filename = format!("{prefix}-tui-two-phase.pdf");
    let pdf_tmp = decode_b64_to_tempfile(SAMPLE_PDF_B64.trim())?;
    let pdf_bytes = std::fs::read(pdf_tmp.path())?;

    let mut guard = CleanupGuard::new(client.clone());
    // 阶段 A：先上传（不发 Responses）。
    let uploaded = client
        .upload(
            FilePurpose::UserData,
            &filename,
            "application/pdf",
            &pdf_bytes,
        )
        .await?;
    guard.track(uploaded.id.clone());

    // 阶段 B：文本 + file_id 发起对话请求。
    let parts = vec![
        ChatMessageContentPart::text("Summarize the attached PDF in one short sentence."),
        ChatMessageContentPart::file_file_id(uploaded.id.clone(), Some(filename.clone()))?,
    ];
    let req = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(parts)],
        model: fixture.config.llm.default_model.clone(),
        temperature: None,
        max_tokens: Some(96),
        stream: Some(false),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let resp = tokio::time::timeout(Duration::from_secs(120), provider.chat(req))
        .await
        .map_err(|_| "TUI two-phase PDF describe timeout 120s")??;
    assert!(!resp.choices.is_empty(), "PDF 描述响应应包含 choices");
    let text = resp.choices[0]
        .message
        .text_content()
        .unwrap_or("")
        .to_ascii_lowercase();
    assert!(!text.trim().is_empty(), "PDF 描述文本不应为空");
    let keywords = ["hello", "pdf", "summary", "summarize", "test", "content"];
    assert!(
        contains_any(&text, &keywords),
        "PDF 描述应命中关键词 {:?} 之一，实际: {:?}",
        keywords,
        text
    );

    client.delete(&uploaded.id).await?;
    guard.untrack(&uploaded.id);
    let leftovers = client.list(Some(&prefix), None).await?;
    assert!(leftovers.is_empty(), "TUI two-phase PDF 测试后不应残留文件");
    Ok(())
}
