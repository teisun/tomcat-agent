//! 集成测试：OpenAI Files 上传管理（T2-P0-015）。
//!
//! 说明：
//! - 真实网络调用，依赖各 provider 的文件能力 key（OpenAI / fcodex / Moonshot / Anthropic）。
//! - 当前用例默认 `#[ignore]`；手动开启后会走真网验证 upload -> file_id -> chat roundtrip。
//! - 手动执行被忽略的用例：
//!   `PI_LIVE_OPENAI_FILES=1 cargo test --test openai_files_integration_tests -- --ignored --nocapture`

mod common;

use serial_test::serial;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tomcat::core::llm::files_api::FilesApiAdapter;
use tomcat::core::llm::openai_files::FilePurpose;
use tomcat::{AppConfig, ChatMessage, ChatMessageContentPart, ChatRequest, LlmProvider};

const SAMPLE_IMAGE_PATH: &str = "tests/fixtures/llm_multimodal/sample_image.png";
const SAMPLE_IMAGE_B64: &str = include_str!("fixtures/llm_multimodal/sample_image_b64.txt");
const SAMPLE_PDF_B64: &str = include_str!("fixtures/llm_multimodal/sample_pdf_b64.txt");

struct ProviderFixture {
    _home: common::TempHomeGuard,
    config: AppConfig,
}

fn responses_fixture() -> ProviderFixture {
    let home = common::TempHomeGuard::new();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(
        common::dot_tomcat_e2e_workdir("openai_files")
            .display()
            .to_string(),
    );
    common::apply_openai_app_config(&mut cfg);
    ProviderFixture {
        _home: home,
        config: cfg,
    }
}

fn fcodex_fixture() -> ProviderFixture {
    let home = common::TempHomeGuard::new();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(
        common::dot_tomcat_e2e_workdir("openai_files_fcodex")
            .display()
            .to_string(),
    );
    common::apply_fcodex_app_config(&mut cfg);
    ProviderFixture {
        _home: home,
        config: cfg,
    }
}

fn kimi_fixture() -> ProviderFixture {
    let home = common::TempHomeGuard::new();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(
        common::dot_tomcat_e2e_workdir("openai_files_kimi")
            .display()
            .to_string(),
    );
    common::apply_kimi_app_config(&mut cfg);
    ProviderFixture {
        _home: home,
        config: cfg,
    }
}

fn anthropic_fixture() -> ProviderFixture {
    let home = common::TempHomeGuard::new();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(
        common::dot_tomcat_e2e_workdir("openai_files_anthropic")
            .display()
            .to_string(),
    );
    common::apply_anthropic_app_config(&mut cfg);
    ProviderFixture {
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

fn require_live_anthropic_files_opt_in(test_name: &str) -> bool {
    if std::env::var("PI_LIVE_ANTHROPIC_FILES").ok().as_deref() == Some("1") {
        return true;
    }
    eprintln!(
        "skip {test_name}: set PI_LIVE_ANTHROPIC_FILES=1 to enable live Anthropic attachment tests"
    );
    false
}

fn sample_image_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(SAMPLE_IMAGE_PATH)
}

async fn chat_text_with_parts(
    provider: &dyn LlmProvider,
    model: &str,
    parts: Vec<ChatMessageContentPart>,
    max_tokens: u32,
    timeout_secs: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    let req = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(parts)],
        model: model.to_string(),
        temperature: None,
        max_tokens: Some(max_tokens),
        stream: Some(false),
        model_override: None,
        thinking_level: None,
        tools: None,
    };
    let resp = tokio::time::timeout(Duration::from_secs(timeout_secs), provider.chat(req))
        .await
        .map_err(|_| format!("provider.chat timeout {timeout_secs}s"))??;
    assert!(
        !resp.choices.is_empty(),
        "provider.chat 应返回至少一个 choice"
    );
    Ok(resp.choices[0]
        .message
        .text_content()
        .unwrap_or("")
        .to_ascii_lowercase())
}

fn files_adapter_from_provider(
    provider: &dyn LlmProvider,
    cfg: &AppConfig,
) -> Result<Arc<dyn FilesApiAdapter>, Box<dyn std::error::Error>> {
    provider
        .files_adapter(&cfg.llm.files)
        .ok_or_else(|| "provider 未返回 Files API adapter".into())
}

struct CleanupGuard {
    adapter: Arc<dyn FilesApiAdapter>,
    ids: Vec<String>,
}

impl CleanupGuard {
    fn new(adapter: Arc<dyn FilesApiAdapter>) -> Self {
        Self {
            adapter,
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
            let adapter = self.adapter.clone();
            handle.spawn(async move {
                for id in ids {
                    let _ = adapter.delete(&id).await;
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
    let adapter = files_adapter_from_provider(provider.as_ref(), &fixture.config)?;
    let prefix = unique_prefix();

    let mut guard = CleanupGuard::new(adapter.clone());
    for size in [1024usize, 100 * 1024, 5 * 1024 * 1024, 20 * 1024 * 1024] {
        let filename = format!("{prefix}-{size}.bin");
        let payload = vec![b'a'; size];
        let uploaded = tokio::time::timeout(
            Duration::from_secs(180),
            adapter.upload(
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
        adapter.delete(&uploaded.id).await?;
        guard.untrack(&uploaded.id);
    }
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
    let adapter = files_adapter_from_provider(provider.as_ref(), &fixture.config)?;
    let prefix = unique_prefix();
    let filename = format!("{prefix}-sample.txt");
    let uploaded = adapter
        .upload(
            FilePurpose::UserData,
            &filename,
            "text/plain",
            b"Hello from T2-P0-015 integration test.",
        )
        .await?;

    let mut guard = CleanupGuard::new(adapter.clone());
    guard.track(uploaded.id.clone());
    let text = chat_text_with_parts(
        provider.as_ref(),
        &fixture.config.llm.default_model,
        vec![
            ChatMessageContentPart::text(
                "Read the attached file and answer with one short sentence.",
            ),
            ChatMessageContentPart::file_file_id(uploaded.id.clone(), Some(filename.clone()))?,
        ],
        96,
        120,
    )
    .await?;
    assert!(!text.trim().is_empty(), "responses 文本不应为空");

    adapter.delete(&uploaded.id).await?;
    guard.untrack(&uploaded.id);
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
    let adapter = files_adapter_from_provider(provider.as_ref(), &fixture.config)?;
    let prefix = unique_prefix();
    let image_path = sample_image_path();
    let image_bytes = std::fs::read(&image_path)?;
    let filename = format!("{prefix}-cli-image.png");

    let mut guard = CleanupGuard::new(adapter.clone());
    // CLI 单轮：先 upload（或命中 cache）再组同轮 ChatRequest。
    let uploaded = adapter
        .upload(FilePurpose::Vision, &filename, "image/png", &image_bytes)
        .await?;
    guard.track(uploaded.id.clone());

    let text = chat_text_with_parts(
        provider.as_ref(),
        &fixture.config.llm.default_model,
        vec![
            ChatMessageContentPart::text(
                "Describe what you see in this image in one short sentence.",
            ),
            ChatMessageContentPart::image_file_id(uploaded.id.clone())?,
        ],
        96,
        120,
    )
    .await?;
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

    adapter.delete(&uploaded.id).await?;
    guard.untrack(&uploaded.id);
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
    let adapter = files_adapter_from_provider(provider.as_ref(), &fixture.config)?;
    let prefix = unique_prefix();
    let filename = format!("{prefix}-tui-two-phase.pdf");
    let pdf_tmp = decode_b64_to_tempfile(SAMPLE_PDF_B64.trim())?;
    let pdf_bytes = std::fs::read(pdf_tmp.path())?;

    let mut guard = CleanupGuard::new(adapter.clone());
    // 阶段 A：先上传（不发 Responses）。
    let uploaded = adapter
        .upload(
            FilePurpose::UserData,
            &filename,
            "application/pdf",
            &pdf_bytes,
        )
        .await?;
    guard.track(uploaded.id.clone());

    // 阶段 B：文本 + file_id 发起对话请求。
    let text = chat_text_with_parts(
        provider.as_ref(),
        &fixture.config.llm.default_model,
        vec![
            ChatMessageContentPart::text("Summarize the attached PDF in one short sentence."),
            ChatMessageContentPart::file_file_id(uploaded.id.clone(), Some(filename.clone()))?,
        ],
        96,
        120,
    )
    .await?;
    assert!(!text.trim().is_empty(), "PDF 描述文本不应为空");
    let keywords = ["hello", "pdf", "summary", "summarize", "test", "content"];
    assert!(
        contains_any(&text, &keywords),
        "PDF 描述应命中关键词 {:?} 之一，实际: {:?}",
        keywords,
        text
    );

    adapter.delete(&uploaded.id).await?;
    guard.untrack(&uploaded.id);
    Ok(())
}

#[tokio::test]
#[ignore = "live fcodex Files API smoke; requires PI_LIVE_OPENAI_FILES=1"]
#[serial]
async fn fcodex_files_upload_smoke_real_api() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    if !require_live_openai_files_opt_in("fcodex_files_upload_smoke_real_api") {
        return Ok(());
    }
    common::load_openai_test_env();
    let fixture = fcodex_fixture();
    let provider = common::resolve_main_provider(&fixture.config);
    let adapter = files_adapter_from_provider(provider.as_ref(), &fixture.config)?;
    let filename = format!("{}-fcodex-smoke.txt", unique_prefix());
    let uploaded = tokio::time::timeout(
        Duration::from_secs(120),
        adapter.upload(
            FilePurpose::UserData,
            &filename,
            "text/plain",
            b"fcodex files smoke probe",
        ),
    )
    .await
    .map_err(|_| "fcodex upload smoke timeout 120s")??;
    assert!(
        !uploaded.id.is_empty(),
        "fcodex smoke upload 应返回 file_id"
    );
    adapter.delete(&uploaded.id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "live fcodex file_id roundtrip; requires PI_LIVE_OPENAI_FILES=1"]
#[serial]
async fn fcodex_file_id_roundtrip_text_image_pdf_real_api() -> Result<(), Box<dyn std::error::Error>>
{
    common::setup_logging();
    if !require_live_openai_files_opt_in("fcodex_file_id_roundtrip_text_image_pdf_real_api") {
        return Ok(());
    }
    common::load_openai_test_env();
    let fixture = fcodex_fixture();
    let provider = common::resolve_main_provider(&fixture.config);
    let adapter = files_adapter_from_provider(provider.as_ref(), &fixture.config)?;
    let mut guard = CleanupGuard::new(adapter.clone());
    let prefix = unique_prefix();

    let text_token = "hello-from-fcodex-files";
    let text_filename = format!("{prefix}-fcodex.txt");
    let text_upload = adapter
        .upload(
            FilePurpose::UserData,
            &text_filename,
            "text/plain",
            text_token.as_bytes(),
        )
        .await?;
    guard.track(text_upload.id.clone());
    let text_reply = chat_text_with_parts(
        provider.as_ref(),
        &fixture.config.llm.default_model,
        vec![
            ChatMessageContentPart::text(
                "Read the attached text file and repeat the token exactly.",
            ),
            ChatMessageContentPart::file_file_id(
                text_upload.id.clone(),
                Some(text_filename.clone()),
            )?,
        ],
        64,
        120,
    )
    .await?;
    assert!(
        text_reply.contains(text_token),
        "fcodex 文本 file_id roundtrip 应包含原 token，实际: {:?}",
        text_reply
    );

    let image_filename = format!("{prefix}-fcodex-image.png");
    let image_bytes = std::fs::read(sample_image_path())?;
    let image_upload = adapter
        .upload(
            FilePurpose::Vision,
            &image_filename,
            "image/png",
            &image_bytes,
        )
        .await?;
    guard.track(image_upload.id.clone());
    let image_reply = chat_text_with_parts(
        provider.as_ref(),
        &fixture.config.llm.default_model,
        vec![
            ChatMessageContentPart::text(
                "Describe what you see in this image in one short sentence.",
            ),
            ChatMessageContentPart::image_file_id(image_upload.id.clone())?,
        ],
        96,
        120,
    )
    .await?;
    let image_keywords = [
        "dog",
        "puppy",
        "animal",
        "pet",
        "canine",
        "beagle",
        "labrador",
        "retriever",
    ];
    assert!(
        contains_any(&image_reply, &image_keywords),
        "fcodex 图片 file_id roundtrip 应命中关键词 {:?}，实际: {:?}",
        image_keywords,
        image_reply
    );

    let pdf_filename = format!("{prefix}-fcodex.pdf");
    let pdf_tmp = decode_b64_to_tempfile(SAMPLE_PDF_B64.trim())?;
    let pdf_bytes = std::fs::read(pdf_tmp.path())?;
    let pdf_upload = adapter
        .upload(
            FilePurpose::UserData,
            &pdf_filename,
            "application/pdf",
            &pdf_bytes,
        )
        .await?;
    guard.track(pdf_upload.id.clone());
    let pdf_reply = chat_text_with_parts(
        provider.as_ref(),
        &fixture.config.llm.default_model,
        vec![
            ChatMessageContentPart::text("Summarize the attached PDF in one short sentence."),
            ChatMessageContentPart::file_file_id(
                pdf_upload.id.clone(),
                Some(pdf_filename.clone()),
            )?,
        ],
        96,
        120,
    )
    .await?;
    let pdf_keywords = ["hello", "pdf", "summary", "summarize", "test", "content"];
    assert!(
        contains_any(&pdf_reply, &pdf_keywords),
        "fcodex PDF file_id roundtrip 应命中关键词 {:?}，实际: {:?}",
        pdf_keywords,
        pdf_reply
    );

    for file_id in [
        text_upload.id.as_str(),
        image_upload.id.as_str(),
        pdf_upload.id.as_str(),
    ] {
        adapter.delete(file_id).await?;
        guard.untrack(file_id);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "live kimi-k3 multimodal/files; requires PI_LIVE_OPENAI_FILES=1"]
#[serial]
async fn kimi_k3_inline_and_uploaded_image_real_api() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    if !require_live_openai_files_opt_in("kimi_k3_inline_and_uploaded_image_real_api") {
        return Ok(());
    }
    common::load_openai_test_env();
    let fixture = kimi_fixture();
    let provider = common::resolve_main_provider(&fixture.config);
    let adapter = files_adapter_from_provider(provider.as_ref(), &fixture.config)?;
    let image_path = sample_image_path();
    let inline_reply = chat_text_with_parts(
        provider.as_ref(),
        &fixture.config.llm.default_model,
        vec![
            ChatMessageContentPart::text(
                "Describe what you see in this image in one short sentence.",
            ),
            ChatMessageContentPart::image_b64("image/png", &image_path)?,
        ],
        96,
        120,
    )
    .await?;
    let keywords = [
        "dog",
        "puppy",
        "animal",
        "pet",
        "canine",
        "beagle",
        "labrador",
        "retriever",
    ];
    assert!(
        contains_any(&inline_reply, &keywords),
        "kimi-k3 inline image(base64) 应命中关键词 {:?}，实际: {:?}",
        keywords,
        inline_reply
    );

    let image_bytes = std::fs::read(&image_path)?;
    let uploaded = adapter
        .upload(
            FilePurpose::Vision,
            &format!("{}-kimi-image.png", unique_prefix()),
            "image/png",
            &image_bytes,
        )
        .await?;
    let mut guard = CleanupGuard::new(adapter.clone());
    guard.track(uploaded.id.clone());
    let uploaded_reply = chat_text_with_parts(
        provider.as_ref(),
        &fixture.config.llm.default_model,
        vec![
            ChatMessageContentPart::text(
                "Describe what you see in this uploaded image in one short sentence.",
            ),
            ChatMessageContentPart::image_file_id(uploaded.id.clone())?,
        ],
        96,
        120,
    )
    .await?;
    assert!(
        contains_any(&uploaded_reply, &keywords),
        "kimi-k3 uploaded image(ms://) 应命中关键词 {:?}，实际: {:?}",
        keywords,
        uploaded_reply
    );
    adapter.delete(&uploaded.id).await?;
    guard.untrack(&uploaded.id);
    Ok(())
}

#[tokio::test]
#[ignore = "live anthropic inline image; requires PI_LIVE_ANTHROPIC_FILES=1"]
#[serial]
async fn anthropic_inline_image_real_api() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    if !require_live_anthropic_files_opt_in("anthropic_inline_image_real_api") {
        return Ok(());
    }
    common::load_openai_test_env();
    let fixture = anthropic_fixture();
    let provider = common::resolve_main_provider(&fixture.config);
    let reply = chat_text_with_parts(
        provider.as_ref(),
        &fixture.config.llm.default_model,
        vec![
            ChatMessageContentPart::text(
                "Describe what you see in this image in one short sentence.",
            ),
            ChatMessageContentPart::image_base64_data("image/png", SAMPLE_IMAGE_B64.trim())?,
        ],
        96,
        120,
    )
    .await?;
    let keywords = ["dog", "pet", "canine", "beagle", "labrador", "retriever"];
    assert!(
        contains_any(&reply, &keywords),
        "anthropic inline image 应命中关键词 {:?}，实际: {:?}",
        keywords,
        reply
    );
    Ok(())
}

#[tokio::test]
#[ignore = "live anthropic inline pdf; requires PI_LIVE_ANTHROPIC_FILES=1"]
#[serial]
async fn anthropic_inline_pdf_real_api() -> Result<(), Box<dyn std::error::Error>> {
    common::setup_logging();
    if !require_live_anthropic_files_opt_in("anthropic_inline_pdf_real_api") {
        return Ok(());
    }
    common::load_openai_test_env();
    let fixture = anthropic_fixture();
    let provider = common::resolve_main_provider(&fixture.config);
    let reply = chat_text_with_parts(
        provider.as_ref(),
        &fixture.config.llm.default_model,
        vec![
            ChatMessageContentPart::text("Summarize the attached PDF in one short sentence."),
            ChatMessageContentPart::file_base64_data(
                "sample.pdf",
                "application/pdf",
                SAMPLE_PDF_B64.trim(),
            )?,
        ],
        96,
        120,
    )
    .await?;
    let keywords = ["hello", "pdf", "summary", "summarize", "test", "content"];
    assert!(
        contains_any(&reply, &keywords),
        "anthropic inline PDF 应命中关键词 {:?}，实际: {:?}",
        keywords,
        reply
    );
    Ok(())
}
