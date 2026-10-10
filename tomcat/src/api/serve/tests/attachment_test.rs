//! serve 层附件端到端测试。
//!
//! 这一层此前**完全缺失**（旧计划把它勾成了已完成）。它覆盖的正是本次风险最高的那几条：
//!
//! - `ingest_attachment` 往返：字节真的落盘了、返回的哈希真的能取回同一份字节
//! - 完整发送链：`ingest → prompt(blobSha) → input_image → transcript → get_messages`
//! - **零拷贝提升**：发送前后 blob 文件的 inode/mtime 不变，证明字节没有被重写一遍
//! - 伪造哈希被拒：客户端无法绕过后端校验
//! - `attachment_mode` 两条路径：默认 `inline` 行为不变、`reference` 模式不回 base64
//! - `delete_session` 联动清理
//! - **写放大契约**：对全部 serve 命令的 schema 做静态扫描，
//!   断言除 `ingest_attachment` / `cache_attachment_thumbnail` 外没有任何命令携带图片字节

use super::*;
use base64::Engine as _;
use std::sync::Arc;
use std::time::Duration;

use serial_test::serial;

use crate::api::serve::registry::SessionSlot;
use crate::api::serve::test_support::SharedWriterBuffer;
use crate::core::llm::{
    ChatMessageContent, ChatMessageContentPart, ChatMessageRole, ImageSource, StreamEvent,
};

async fn wait_for_line(
    buffer: &SharedWriterBuffer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value> {
    for _ in 0..50 {
        let lines = read_ndjson_lines(buffer);
        if lines.iter().any(&predicate) {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    read_ndjson_lines(buffer)
}

fn latest_persisted_user_message(
    request: &crate::core::llm::ChatRequest,
) -> &crate::core::llm::ChatMessage {
    request
        .messages
        .iter()
        .rev()
        .find(|message| {
            matches!(message.role, ChatMessageRole::User)
                && message.kind != crate::core::llm::MessageKind::EphemeralTail
        })
        .expect("persisted user message")
}

// ── 夹具 ──────────────────────────────────────────────────────────────

fn png_bytes() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0x60,
        0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27, 0xDE, 0xFC, 0x00, 0x00, 0x00, 0x00,
        0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]
}

fn pdf_bytes() -> Vec<u8> {
    b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<<>>\n%%EOF\n".to_vec()
}

/// 一张真实设计工具会导出的 SVG —— 带 `style=`、`<style>` 与 `url(#grad)`。
///
/// 旧实现的文本黑名单会把这一类全部误杀（`" style="` / `"<style"` / `"url("` 都在名单上），
/// 也就是说用户从 Figma / Illustrator 拿到的图标基本都传不上来。这条用例守住那个回归。
fn design_tool_svg() -> Vec<u8> {
    br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24">
  <defs><linearGradient id="grad"><stop offset="0" stop-color="#f00"/></linearGradient></defs>
  <style>.icon { stroke-width: 2 }</style>
  <rect class="icon" style="opacity:0.8" fill="url(#grad)" width="24" height="24"/>
</svg>"##
        .to_vec()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn ok_stream() -> Vec<Result<StreamEvent, crate::AppError>> {
    vec![Ok(StreamEvent::FinishReason {
        reason: "stop".to_string(),
    })]
}

/// 调一次 `ingest_attachment` 并取回响应 payload。
async fn ingest(
    state: &Arc<ServeState>,
    buffer: &SharedWriterBuffer,
    slot: &Arc<SessionSlot>,
    command_id: &str,
    attachment: IngestAttachmentInput,
) -> serde_json::Value {
    handle_command(
        Arc::clone(state),
        ServeCommand::IngestAttachment {
            id: Some(command_id.to_string()),
            session_id: Some(slot.session_id.clone()),
            attachment,
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some(command_id)
    })
    .await;
    lines
        .into_iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some(command_id))
        .expect("ingest response")
}

fn image_input(bytes: &[u8], mime: &str) -> IngestAttachmentInput {
    IngestAttachmentInput {
        kind: ServeAttachmentKind::Image,
        filename: Some("pic.png".to_string()),
        mime_type: mime.to_string(),
        data_base64: b64(bytes),
        thumb_base64: None,
        provider_base64: None,
        provider_mime_type: None,
    }
}

fn file_input(bytes: &[u8], mime: &str) -> IngestAttachmentInput {
    IngestAttachmentInput {
        kind: ServeAttachmentKind::File,
        filename: Some("brief.pdf".to_string()),
        mime_type: mime.to_string(),
        data_base64: b64(bytes),
        thumb_base64: None,
        provider_base64: None,
        provider_mime_type: None,
    }
}

// ── ingest 往返 ───────────────────────────────────────────────────────

#[tokio::test]
#[serial(env_lock)]
async fn ingest_attachment_round_trips_bytes_and_returns_hashes() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;

    let bytes = png_bytes();
    let thumb = png_bytes();
    let response = ingest(
        &state,
        &buffer,
        &slot,
        "ingest-1",
        IngestAttachmentInput {
            thumb_base64: Some(b64(&thumb)),
            ..image_input(&bytes, "image/png")
        },
    )
    .await;

    assert_eq!(response["success"].as_bool(), Some(true));
    let payload = &response["payload"];
    let blob_sha = payload["blobSha"].as_str().expect("blobSha");
    assert_eq!(blob_sha.len(), 64, "必须是 64 位十六进制 sha256");
    assert_eq!(payload["bytes"].as_u64(), Some(bytes.len() as u64));
    assert_eq!(payload["mimeType"].as_str(), Some("image/png"));
    assert_eq!(payload["filename"].as_str(), Some("pic.png"));

    let store = slot.ctx.session_runtime.session.attachment_store();
    assert_eq!(
        store.get(blob_sha).unwrap().as_deref(),
        Some(bytes.as_slice()),
        "按返回的哈希必须能取回同一份字节"
    );
    // 缩略图按源字节的哈希存放，所以协议只需要回一个布尔值。
    assert_eq!(payload["hasThumb"].as_bool(), Some(true));
    assert!(store.has_thumbnail(blob_sha));
    assert_eq!(
        store.list_pending(&slot.session_id).unwrap(),
        vec![blob_sha.to_string()],
        "未发送的字节必须持有租约；缩略图是派生数据，不占租约"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn ingest_attachment_deduplicates_identical_bytes() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;
    let bytes = png_bytes();

    let first = ingest(
        &state,
        &buffer,
        &slot,
        "dedup-1",
        image_input(&bytes, "image/png"),
    )
    .await;
    let second = ingest(
        &state,
        &buffer,
        &slot,
        "dedup-2",
        image_input(&bytes, "image/png"),
    )
    .await;

    assert_eq!(first["payload"]["blobSha"], second["payload"]["blobSha"]);
    let blobs = std::fs::read_dir(
        slot.ctx
            .session_runtime
            .session
            .attachment_store()
            .blobs_dir(),
    )
    .unwrap()
    .count();
    assert_eq!(blobs, 1, "同一张图粘两次只该占一份磁盘");
}

#[tokio::test]
#[serial(env_lock)]
async fn ingest_file_deduplicates_identical_bytes() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;
    let bytes = pdf_bytes();

    let first = ingest(
        &state,
        &buffer,
        &slot,
        "dedup-file-1",
        file_input(&bytes, "application/pdf"),
    )
    .await;
    let second = ingest(
        &state,
        &buffer,
        &slot,
        "dedup-file-2",
        file_input(&bytes, "application/pdf"),
    )
    .await;

    assert_eq!(first["payload"]["blobSha"], second["payload"]["blobSha"]);
    let blobs = std::fs::read_dir(
        slot.ctx
            .session_runtime
            .session
            .attachment_store()
            .blobs_dir(),
    )
    .unwrap()
    .count();
    assert_eq!(blobs, 1, "同一份 PDF 粘两次只该占一份磁盘");
}

#[tokio::test]
#[serial(env_lock)]
async fn ingest_attachment_accepts_real_design_tool_svg() {
    // §3.8 假阳性回归：旧实现的文本黑名单会拒掉这一类正常 SVG。
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;

    let svg = design_tool_svg();
    let png = png_bytes();
    let response = ingest(
        &state,
        &buffer,
        &slot,
        "svg-1",
        IngestAttachmentInput {
            filename: Some("icon.svg".to_string()),
            provider_base64: Some(b64(&png)),
            provider_mime_type: Some("image/png".to_string()),
            ..image_input(&svg, "image/svg+xml")
        },
    )
    .await;

    assert_eq!(
        response["success"].as_bool(),
        Some(true),
        "带 style= / <style> / url(#grad) 的 SVG 必须能收下，实际：{}",
        response["error"]
    );
    let payload = &response["payload"];
    assert_ne!(
        payload["providerSha"].as_str(),
        payload["blobSha"].as_str(),
        "原始 SVG 与转出的 PNG 是两份不同的字节"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn ingest_attachment_rejects_mislabelled_bytes() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;

    let response = ingest(
        &state,
        &buffer,
        &slot,
        "bad-magic",
        image_input(b"this is definitely not a png", "image/png"),
    )
    .await;

    assert_eq!(response["success"].as_bool(), Some(false));
    assert!(
        response["error"]
            .as_str()
            .unwrap_or_default()
            .contains("magic byte"),
        "实际错误：{}",
        response["error"]
    );
}

// ── 完整发送链 + 零拷贝 ────────────────────────────────────────────────

#[tokio::test]
#[serial(env_lock)]
async fn ingest_then_prompt_reaches_transcript_and_provider_without_rewriting_bytes() {
    let _api_key = install_test_api_key();
    let (state, buffer, temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;

    let bytes = png_bytes();
    let ingested = ingest(
        &state,
        &buffer,
        &slot,
        "chain-ingest",
        image_input(&bytes, "image/png"),
    )
    .await;
    let blob_sha = ingested["payload"]["blobSha"].as_str().unwrap().to_string();

    // 零拷贝断言的取样点：发送前记下 blob 文件的身份。
    let store = slot.ctx.session_runtime.session.attachment_store();
    let blob_path = store.blobs_dir().join(&blob_sha);
    let before = std::fs::metadata(&blob_path).unwrap();
    let before_modified = before.modified().unwrap();
    #[cfg(unix)]
    let before_inode = std::os::unix::fs::MetadataExt::ino(&before);
    std::thread::sleep(std::time::Duration::from_millis(20));

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("chain-prompt".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "describe this".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    filename: Some("pic.png".to_string()),
                    mime_type: Some("image/png".to_string()),
                    blob_sha: Some(blob_sha.clone()),
                    provider_sha: None,
                    file_id: None,
                }],
                user_message_id: Some("chain-msg".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();
    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    // (1) provider 真的收到了这张图的字节
    let captured = requests.0.lock();
    let user_message = latest_persisted_user_message(&captured[0]);
    let Some(ChatMessageContent::Parts(parts)) = &user_message.content else {
        panic!("expected multimodal parts, got {:?}", user_message.content);
    };
    let image = parts
        .iter()
        .find_map(|part| match part {
            ChatMessageContentPart::InputImage {
                source: ImageSource::Inline(inline),
                ..
            } => Some(inline),
            _ => None,
        })
        .expect("input_image part");
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(
        image.data,
        b64(&bytes),
        "provider 收到的字节必须与 ingest 时一致"
    );
    drop(captured);

    // (2) transcript 只保留 CAS 引用；字节的唯一副本在 blob store。
    let transcript = std::fs::read_to_string(
        slot.ctx
            .session_runtime
            .session
            .transcript_path(&slot.session_id),
    )
    .unwrap();
    assert!(
        transcript.contains("\"type\":\"input_image_ref\"")
            && transcript.contains(&blob_sha)
            && !transcript.contains(&b64(&bytes)),
        "transcript 必须是无字节的 CAS 引用"
    );
    let _ = temp;

    // (3) 零拷贝：blob 文件原地不动
    let after = std::fs::metadata(&blob_path).unwrap();
    assert!(
        after.modified().unwrap() >= before_modified,
        "发送提升必须刷新 blob mtime，避免立即被孤儿 sweep"
    );
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&after),
        before_inode,
        "发送不得替换 blob 文件"
    );

    // (4) 租约已释放，字节留给 transcript 引用
    assert!(
        store.list_pending(&slot.session_id).unwrap().is_empty(),
        "发送成功后租约必须释放"
    );
    assert!(store.exists(&blob_sha), "字节必须保留");
}

#[tokio::test]
#[serial(env_lock)]
async fn prompt_with_provider_sha_sends_png_but_archives_svg() {
    // SVG 的显示与「发给模型」是两条路：历史里留原始 SVG，模型收到 webview 转出的 PNG。
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream(), ok_stream()]).await;

    let svg = design_tool_svg();
    let png = png_bytes();
    let ingested = ingest(
        &state,
        &buffer,
        &slot,
        "svg-chain",
        IngestAttachmentInput {
            filename: Some("icon.svg".to_string()),
            provider_base64: Some(b64(&png)),
            provider_mime_type: Some("image/png".to_string()),
            ..image_input(&svg, "image/svg+xml")
        },
    )
    .await;
    let blob_sha = ingested["payload"]["blobSha"].as_str().unwrap().to_string();
    let provider_sha = ingested["payload"]["providerSha"]
        .as_str()
        .unwrap()
        .to_string();

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("svg-prompt".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "what icon is this".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    filename: Some("icon.svg".to_string()),
                    mime_type: Some("image/svg+xml".to_string()),
                    blob_sha: Some(blob_sha.clone()),
                    provider_sha: Some(provider_sha.clone()),
                    file_id: None,
                }],
                user_message_id: Some("svg-msg".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();
    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    {
        let captured = requests.0.lock();
        assert!(
            !captured.is_empty(),
            "provider should receive the image request; frames={:?}",
            read_ndjson_lines(&buffer)
        );
        let user_message = latest_persisted_user_message(&captured[0]);
        let Some(ChatMessageContent::Parts(parts)) = &user_message.content else {
            panic!("expected parts");
        };
        let image = parts
            .iter()
            .find_map(|part| match part {
                ChatMessageContentPart::InputImage {
                    source: ImageSource::Inline(inline),
                    ..
                } => Some(inline),
                _ => None,
            })
            .expect("input_image part");
        assert_eq!(image.mime_type, "image/png", "模型必须收到 PNG，不是 SVG");
        assert_eq!(image.data, b64(&png));
    }

    let transcript = std::fs::read_to_string(
        slot.ctx
            .session_runtime
            .session
            .transcript_path(&slot.session_id),
    )
    .unwrap();
    assert!(
        transcript.contains("\"type\":\"input_image_ref\"")
            && transcript.contains(&blob_sha)
            && !transcript.contains(&b64(&svg)),
        "transcript 必须以 CAS 引用保留用户实际附上的原始 SVG"
    );
    assert!(
        transcript.contains("image/svg+xml"),
        "transcript 的 MIME 也应保持 SVG"
    );
    assert!(!transcript.contains("submitted_attachments"));
    assert!(transcript.contains(&format!("\"provider_sha\":\"{provider_sha}\"")));
    let live = slot
        .ctx
        .session_runtime
        .session
        .collect_live_blob_shas()
        .unwrap();
    assert!(
        live.shas.contains(&provider_sha),
        "provider PNG must remain GC-live"
    );
    // This is a rendition test, not the checkpoint timeout test. The shared mock
    // fixture points at the real repository, so let its asynchronous snapshot settle.
    tokio::time::timeout(Duration::from_secs(15), async {
        while slot.is_busy() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        crate::api::chat::drain_checkpoint_record_tasks(&slot.ctx, Duration::from_secs(30)).await
    );
    let history = get_messages_payload(
        &state,
        &buffer,
        &slot,
        "svg-edit-history",
        AttachmentMode::Reference,
    )
    .await;
    let serialized = history.to_string();
    assert!(
        serialized.contains(&provider_sha),
        "editable history retains the PNG rendition reference"
    );
    assert!(serialized.contains("icon.svg"));
    assert!(!serialized.contains("provider_sha"));
    assert!(serialized.contains("providerSha"));
    let inline_history =
        get_messages_payload(&state, &buffer, &slot, "svg-inline", AttachmentMode::Inline).await;
    let inline_user = inline_history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "svg-msg")
        .unwrap();
    let inline_image = inline_user["message"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|part| part["type"] == "input_image")
        .unwrap();
    for field in ["provider_sha", "filename", "blob_sha"] {
        assert!(
            inline_image.get(field).is_none(),
            "{field} leaked into Inline"
        );
    }
    assert!(
        !serialized.contains("submitted_attachments"),
        "internal archival metadata is not a wire field"
    );
    handle_command(
        state.clone(),
        ServeCommand::RewindAndResend {
            id: Some("svg-edit".into()),
            session_id: slot.session_id.clone(),
            message_id: "svg-msg".into(),
            files: RewindFiles::Keep,
            message: RewindMessage {
                text: "edited icon question".into(),
                params: ServeMessageParams {
                    attachments: vec![ServeAttachment {
                        kind: ServeAttachmentKind::Image,
                        filename: Some("icon.svg".into()),
                        mime_type: Some("image/svg+xml".into()),
                        blob_sha: Some(blob_sha),
                        provider_sha: Some(provider_sha),
                        file_id: None,
                    }],
                    ..ServeMessageParams::default()
                },
            },
        },
    )
    .await
    .unwrap();
    let edit_frames = wait_for_line(&buffer, |frame| frame["id"] == "svg-edit").await;
    let edit_response = edit_frames
        .iter()
        .find(|frame| frame["id"] == "svg-edit")
        .unwrap();
    assert_eq!(edit_response["success"], true, "{edit_response:?}");
    tokio::time::timeout(Duration::from_secs(15), async {
        while slot.is_busy() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let captured = requests.0.lock();
    let edited = latest_persisted_user_message(captured.last().unwrap());
    assert!(captured.len() >= 2, "editing must actually issue a second provider request; frames={:?}",
        read_ndjson_lines(&buffer).into_iter().filter(|f| f["id"] == "svg-edit" || (f["type"] == "agent_end" && !f["error"].is_null())).collect::<Vec<_>>());
    assert!(edited.content.as_ref().is_some_and(|content| matches!(content, ChatMessageContent::Parts(parts)
        if parts.iter().any(|part| matches!(part, ChatMessageContentPart::InputImage { source: ImageSource::Inline(image), .. }
            if image.mime_type == "image/png" && image.data == b64(&png))))));
}

// ── 伪造哈希 ──────────────────────────────────────────────────────────

#[tokio::test]
#[serial(env_lock)]
async fn prompt_rejects_a_blob_sha_that_was_never_ingested() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("forged".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "sneaky".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    filename: Some("pic.png".to_string()),
                    mime_type: Some("image/png".to_string()),
                    blob_sha: Some("a".repeat(64)),
                    provider_sha: None,
                    file_id: None,
                }],
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("forged")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("forged"))
        .expect("forged sha response");
    assert_eq!(response["success"].as_bool(), Some(false));
    let error = response["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("unknown attachment blob") && error.contains("ingest_attachment"),
        "实际错误：{error}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn prompt_rejects_a_file_blob_sha_that_was_never_ingested() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("forged-file".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "sneaky pdf".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::File,
                    filename: Some("brief.pdf".to_string()),
                    mime_type: Some("application/pdf".to_string()),
                    blob_sha: Some("b".repeat(64)),
                    provider_sha: None,
                    file_id: None,
                }],
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("forged-file")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("forged-file"))
        .expect("forged file sha response");
    assert_eq!(response["success"].as_bool(), Some(false));
    let error = response["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("unknown attachment blob") && error.contains("ingest_attachment"),
        "实际错误：{error}"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn prompt_rejects_a_malformed_blob_sha() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("malformed".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "sneaky".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    filename: Some("pic.png".to_string()),
                    mime_type: Some("image/png".to_string()),
                    // 试图用路径成分逃出 blobs 目录。
                    blob_sha: Some("../../../etc/passwd".to_string()),
                    provider_sha: None,
                    file_id: None,
                }],
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();

    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("malformed")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("malformed"))
        .expect("malformed sha response");
    assert_eq!(response["success"].as_bool(), Some(false));
    assert!(
        response["error"]
            .as_str()
            .unwrap_or_default()
            .contains("invalid blob sha256"),
        "实际错误：{}",
        response["error"]
    );
}

// ── attachment_mode 两条路径 ──────────────────────────────────────────

async fn seed_session_with_one_image(
    state: &Arc<ServeState>,
    buffer: &SharedWriterBuffer,
    slot: &Arc<SessionSlot>,
    bytes: &[u8],
) {
    let ingested = ingest(
        state,
        buffer,
        slot,
        "seed-ingest",
        image_input(bytes, "image/png"),
    )
    .await;
    let blob_sha = ingested["payload"]["blobSha"].as_str().unwrap().to_string();
    handle_command(
        Arc::clone(state),
        ServeCommand::Prompt {
            id: Some("seed-prompt".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "look".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::Image,
                    filename: Some("pic.png".to_string()),
                    mime_type: Some("image/png".to_string()),
                    blob_sha: Some(blob_sha),
                    provider_sha: None,
                    file_id: None,
                }],
                user_message_id: Some("seed-msg".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();
    let _ = wait_for_line(buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;
}

async fn seed_session_with_one_file(
    state: &Arc<ServeState>,
    buffer: &SharedWriterBuffer,
    slot: &Arc<SessionSlot>,
    bytes: &[u8],
) {
    let ingested = ingest(
        state,
        buffer,
        slot,
        "seed-file-ingest",
        file_input(bytes, "application/pdf"),
    )
    .await;
    let blob_sha = ingested["payload"]["blobSha"].as_str().unwrap().to_string();
    handle_command(
        Arc::clone(state),
        ServeCommand::Prompt {
            id: Some("seed-file-prompt".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "read this pdf".to_string(),
            params: ServeMessageParams {
                attachments: vec![ServeAttachment {
                    kind: ServeAttachmentKind::File,
                    filename: Some("brief.pdf".to_string()),
                    mime_type: Some("application/pdf".to_string()),
                    blob_sha: Some(blob_sha),
                    provider_sha: None,
                    file_id: None,
                }],
                user_message_id: Some("seed-file-msg".to_string()),
                ..ServeMessageParams::default()
            },
        },
    )
    .await
    .unwrap();
    let _ = wait_for_line(buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;
}

async fn get_messages_payload(
    state: &Arc<ServeState>,
    buffer: &SharedWriterBuffer,
    slot: &Arc<SessionSlot>,
    command_id: &str,
    mode: AttachmentMode,
) -> serde_json::Value {
    handle_command(
        Arc::clone(state),
        ServeCommand::GetMessages {
            id: Some(command_id.to_string()),
            session_id: Some(slot.session_id.clone()),
            params: GetMessagesParams {
                attachment_mode: mode,
                ..GetMessagesParams::default()
            },
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some(command_id)
    })
    .await;
    lines
        .into_iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some(command_id))
        .expect("get_messages response")["payload"]
        .clone()
}

#[tokio::test]
#[serial(env_lock)]
async fn tool_owned_media_history_reference_matches_live_shape_without_bytes() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;
    let session = &slot.ctx.session_runtime.session;
    let bytes = png_bytes();
    let sha = session.attachment_store().put(&bytes).unwrap();
    session.append_message(serde_json::json!({"role":"assistant","tool_calls":[{"id":"tool-image","type":"function","function":{"name":"read","arguments":"{}"}}]})).unwrap();
    session.append_message(serde_json::json!({"role":"tool","tool_call_id":"tool-image","content":[{"type":"input_text","text":"[Image attached]"},{"type":"input_image_ref","blob_sha":sha,"mime_type":"image/png"}],"tool_display":{"kind":"text","text":"image result"}})).unwrap();
    let payload = get_messages_payload(
        &state,
        &buffer,
        &slot,
        "gm-tool-media",
        AttachmentMode::Reference,
    )
    .await;
    assert!(!payload.to_string().contains("image_b64"));
    assert!(!payload.to_string().contains(&b64(&bytes)));
    let row = payload["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["message"]["tool_call_id"] == "tool-image")
        .unwrap();
    let part = &row["message"]["content"][1];
    assert_eq!(part["type"], "input_image_ref");
    assert_eq!(part["blobSha"], sha);
    assert_eq!(part["hasThumb"], false);
    assert_eq!(row["message"]["tool_display"]["text"], "image result");
}

#[tokio::test]
#[serial(env_lock)]
async fn get_messages_defaults_to_inline_for_cli_compatibility() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;
    let bytes = png_bytes();
    seed_session_with_one_image(&state, &buffer, &slot, &bytes).await;

    let payload =
        get_messages_payload(&state, &buffer, &slot, "gm-inline", AttachmentMode::Inline).await;
    let serialized = serde_json::to_string(&payload).unwrap();

    assert!(
        serialized.contains(&b64(&bytes)),
        "默认 inline 必须原样回 base64，CLI 与既有调用方行为不变"
    );
    assert!(
        !serialized.contains("blobSha"),
        "inline 模式不该出现引用字段"
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn get_messages_reference_mode_returns_hashes_instead_of_bytes() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;
    let bytes = png_bytes();
    seed_session_with_one_image(&state, &buffer, &slot, &bytes).await;

    let payload =
        get_messages_payload(&state, &buffer, &slot, "gm-ref", AttachmentMode::Reference).await;
    let serialized = serde_json::to_string(&payload).unwrap();

    assert!(
        !serialized.contains(&b64(&bytes)),
        "reference 模式下宿主绝不该收到 base64"
    );
    assert!(!serialized.contains("image_b64"), "内联字段必须被移除");
    assert!(serialized.contains("blobSha"), "必须回引用");
    assert!(serialized.contains("hasThumb"), "必须告知缩略图是否已就绪");

    // 引用真的指得到字节。
    let sha = serialized
        .split("\"blobSha\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("blobSha in payload")
        .to_string();
    assert_eq!(
        slot.ctx
            .session_runtime
            .session
            .attachment_store()
            .get(&sha)
            .unwrap()
            .as_deref(),
        Some(bytes.as_slice())
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn get_messages_reference_mode_returns_pdf_hashes_instead_of_bytes() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;
    let bytes = pdf_bytes();
    seed_session_with_one_file(&state, &buffer, &slot, &bytes).await;
    let transcript = std::fs::read_to_string(
        slot.ctx
            .session_runtime
            .session
            .transcript_path(&slot.session_id),
    )
    .unwrap();
    assert!(
        transcript.contains(&b64(&bytes)),
        "transcript 必须保留用户发出的 PDF 原始字节"
    );
    assert!(
        transcript.contains("application/pdf"),
        "transcript 必须保留 PDF MIME，后续拉历史才能正确渲染"
    );

    let payload = get_messages_payload(
        &state,
        &buffer,
        &slot,
        "gm-pdf-ref",
        AttachmentMode::Reference,
    )
    .await;
    let serialized = serde_json::to_string(&payload).unwrap();

    assert!(
        !serialized.contains(&b64(&bytes)),
        "reference 模式下宿主绝不该收到 PDF base64"
    );
    assert!(!serialized.contains("file_b64"), "PDF 内联字段必须被移除");
    assert!(serialized.contains("blobSha"), "必须回引用");
    assert!(
        serialized.contains("\"mimeType\":\"application/pdf\""),
        "必须回传 MIME 供宿主渲染 PDF"
    );
    assert!(
        serialized.contains("\"filename\":\"brief.pdf\""),
        "必须回传 filename 供宿主展示 PDF 标签"
    );

    let sha = serialized
        .split("\"blobSha\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("blobSha in payload")
        .to_string();
    assert_eq!(
        slot.ctx
            .session_runtime
            .session
            .attachment_store()
            .get(&sha)
            .unwrap()
            .as_deref(),
        Some(bytes.as_slice())
    );
}

#[tokio::test]
#[serial(env_lock)]
async fn reference_mode_keeps_missing_blob_visible_without_recreating_bytes() {
    // Blob 是原图唯一副本，不是缓存。异常丢失时历史仍保留引用，但不能凭空重建字节。
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;
    let bytes = png_bytes();
    seed_session_with_one_image(&state, &buffer, &slot, &bytes).await;

    let store = slot.ctx.session_runtime.session.attachment_store();
    // 模拟磁盘损坏：删掉唯一 blob，只留下 transcript 引用。
    let blobs = store.blobs_dir();
    if blobs.exists() {
        for entry in std::fs::read_dir(&blobs).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
    }

    let payload = get_messages_payload(
        &state,
        &buffer,
        &slot,
        "gm-rebuild",
        AttachmentMode::Reference,
    )
    .await;
    let serialized = serde_json::to_string(&payload).unwrap();
    assert!(
        serialized.contains("\"type\":\"input_image_ref\"") && !serialized.contains("blobSha"),
        "缺失 blob 时必须保留历史引用、但不伪造可访问的 blobSha"
    );
    assert_eq!(
        std::fs::read_dir(store.blobs_dir()).unwrap().count(),
        0,
        "get_messages 不得从 transcript 内联字节重建 blob"
    );
}

// ── 缩略图补交 ────────────────────────────────────────────────────────

#[tokio::test]
#[serial(env_lock)]
async fn cache_attachment_thumbnail_stores_and_is_reported_by_reference_mode() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;
    let bytes = png_bytes();
    seed_session_with_one_image(&state, &buffer, &slot, &bytes).await;

    let payload = get_messages_payload(
        &state,
        &buffer,
        &slot,
        "thumb-before",
        AttachmentMode::Reference,
    )
    .await;
    let serialized = serde_json::to_string(&payload).unwrap();
    assert!(serialized.contains("\"hasThumb\":false"), "起初没有缩略图");
    let sha = serialized
        .split("\"blobSha\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("blobSha")
        .to_string();

    handle_command(
        Arc::clone(&state),
        ServeCommand::CacheAttachmentThumbnail {
            id: Some("thumb-put".to_string()),
            session_id: Some(slot.session_id.clone()),
            thumbnail: CacheThumbnailInput {
                source_sha: sha.clone(),
                thumb_base64: b64(&png_bytes()),
            },
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("thumb-put")
    })
    .await;
    assert_eq!(
        lines
            .iter()
            .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("thumb-put"))
            .expect("thumb response")["success"]
            .as_bool(),
        Some(true)
    );

    let after = get_messages_payload(
        &state,
        &buffer,
        &slot,
        "thumb-after",
        AttachmentMode::Reference,
    )
    .await;
    assert!(
        serde_json::to_string(&after)
            .unwrap()
            .contains("\"hasThumb\":true"),
        "补交后应报告缩略图已就绪"
    );
    assert!(slot
        .ctx
        .session_runtime
        .session
        .attachment_store()
        .has_thumbnail(&sha));
}

#[tokio::test]
#[serial(env_lock)]
async fn cache_attachment_thumbnail_rejects_an_unknown_source() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::CacheAttachmentThumbnail {
            id: Some("thumb-orphan".to_string()),
            session_id: Some(slot.session_id.clone()),
            thumbnail: CacheThumbnailInput {
                source_sha: "b".repeat(64),
                thumb_base64: b64(&png_bytes()),
            },
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("id").and_then(serde_json::Value::as_str) == Some("thumb-orphan")
    })
    .await;
    let response = lines
        .iter()
        .find(|line| line.get("id").and_then(serde_json::Value::as_str) == Some("thumb-orphan"))
        .expect("orphan thumb response");
    assert_eq!(response["success"].as_bool(), Some(false));
}

// ── get_state 回传附件根 ──────────────────────────────────────────────

#[tokio::test]
#[serial(env_lock)]
async fn initialize_reports_the_attachment_root_for_local_resource_roots() {
    // 宿主要用这个路径配置 webview 的 localResourceRoots，它自己算不出来。而且必须在
    // 握手时就拿到：webview 渲染之后再改资源根会触发重载，用户会看到一次白屏。
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;

    handle_control_or_interrupt(
        Arc::clone(&state),
        ServeCommand::ControlRequest {
            request_id: "init-root".to_string(),
            subtype: "initialize".to_string(),
            session_id: None,
            payload: serde_json::Value::Null,
        },
    )
    .await
    .unwrap();
    let lines = wait_for_line(&buffer, |line| {
        line.get("requestId").and_then(serde_json::Value::as_str) == Some("init-root")
    })
    .await;
    let payload = &lines
        .iter()
        .find(|line| line.get("requestId").and_then(serde_json::Value::as_str) == Some("init-root"))
        .expect("initialize response")["payload"];

    let root = payload["attachmentRoot"].as_str().expect("attachmentRoot");
    assert_eq!(
        std::path::Path::new(root),
        slot.ctx.session_runtime.session.attachment_store().root()
    );
}

// ── delete_session 联动 ───────────────────────────────────────────────

#[tokio::test]
#[serial(env_lock)]
async fn deleting_a_session_releases_attachment_lease_for_orphan_sweep() {
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, _requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;

    let ingested = ingest(
        &state,
        &buffer,
        &slot,
        "del-ingest",
        image_input(&png_bytes(), "image/png"),
    )
    .await;
    let blob_sha = ingested["payload"]["blobSha"].as_str().unwrap().to_string();
    let store = slot.ctx.session_runtime.session.attachment_store();
    assert!(store.exists(&blob_sha));

    slot.ctx.session_runtime.session.release_session_usage();
    slot.ctx
        .session_runtime
        .session
        .delete_session(&slot.session_id)
        .expect("delete session");

    assert!(
        store.exists(&blob_sha),
        "删除会话只释放租约；blob 要等统一 orphan sweep 的宽限窗"
    );
    assert!(store.list_pending(&slot.session_id).unwrap().is_empty());
}

// ── 无附件的 CLI 兼容路径 ─────────────────────────────────────────────

#[tokio::test]
#[serial(env_lock)]
async fn prompt_without_attachments_still_works() {
    // 回归保护：CLI 从来没有图片附件入口，这条路必须完全不受影响。
    let _api_key = install_test_api_key();
    let (state, buffer, _temp, slot, requests) =
        build_initialized_state_with_recorded_streams(vec![ok_stream()]).await;

    handle_command(
        Arc::clone(&state),
        ServeCommand::Prompt {
            id: Some("plain".to_string()),
            session_id: Some(slot.session_id.clone()),
            text: "hello".to_string(),
            params: ServeMessageParams::default(),
        },
    )
    .await
    .unwrap();
    let _ = wait_for_line(&buffer, |line| {
        line.get("type").and_then(serde_json::Value::as_str) == Some("agent_end")
    })
    .await;

    let captured = requests.0.lock();
    let user_message = latest_persisted_user_message(&captured[0]);
    assert!(matches!(
        &user_message.content,
        Some(ChatMessageContent::Text(text)) if text == "hello"
    ));
}

// ── 写放大契约：schema 静态扫描 ────────────────────────────────────────

#[test]
fn only_ingest_commands_may_carry_image_bytes() {
    // 这一条比逐个用例更能防回归：只要有人往任何别的命令上加回一个字节字段，
    // 这个测试立刻红，不需要有人记得去补一个专门的用例。
    let schema = serde_json::to_value(schema_for_serve_command()).expect("serialize schema");
    let commands = schema["oneOf"]
        .as_array()
        .expect("ServeCommand should be a tagged union");

    // 允许携带字节的命令白名单 —— 增删这个列表就是一次显式的架构决定。
    const BYTE_CARRYING_COMMANDS: [&str; 2] = ["ingest_attachment", "cache_attachment_thumbnail"];

    let mut offenders: Vec<(String, Vec<String>)> = Vec::new();
    for command in commands {
        let name = command["properties"]["type"]["enum"][0]
            .as_str()
            .unwrap_or("<unnamed>")
            .to_string();
        if BYTE_CARRYING_COMMANDS.contains(&name.as_str()) {
            continue;
        }
        // 顺着 `$ref` 递归收集**字段名**（不是描述文字），
        // 这样嵌套在 ServeMessageParams / ServeAttachment 里的字段也躲不掉，
        // 同时又不会被文档注释里出现的 "base64" 字样误判。
        let fields = collect_field_names(command, &schema["definitions"]);
        let byte_fields: Vec<String> = fields
            .into_iter()
            .filter(|field| looks_like_a_byte_payload(field))
            .collect();
        if !byte_fields.is_empty() {
            offenders.push((name, byte_fields));
        }
    }

    assert!(
        offenders.is_empty(),
        "这些命令携带了图片字节字段，违反「只有 ingest 传字节」的契约：{offenders:?}"
    );
}

#[test]
fn ingest_attachment_response_is_part_of_the_generated_schema() {
    // 响应方向必须纳入生成，否则 TS 侧只能 `as any` 加手写 parser。
    let dts = serve_dts();
    assert!(dts.contains("export interface IngestAttachmentResponse {"));
    assert!(dts.contains("blobSha: string;"));
    assert!(dts.contains("hasThumb: boolean;"));
    assert!(dts.contains("providerSha?: null | string;"));
    assert!(dts.contains("export interface RetainAttachmentLeaseRef {"));
    assert!(dts.contains("attachments: RetainAttachmentLeaseRef[];"));
    assert!(dts.contains("export interface RetainAttachmentLeasesResponse {"));
    assert!(dts.contains("retainedShas: string[];"));
    assert!(dts.contains("type: \"retain_attachment_leases\";"));
    assert!(dts.contains("detached?: boolean;"));
    assert!(dts.contains("export interface IngestAttachmentInput {"));
    // 引用式附件：ServeAttachment 上只有哈希，没有字节。
    assert!(dts.contains("export interface ServeAttachment {"));
    assert!(dts.contains("blobSha?: null | string;"));
    assert!(!dts.contains("dataBase64?: null | string;"));
}

fn schema_for_serve_command() -> schemars::schema::RootSchema {
    schemars::schema_for!(ServeCommand)
}

/// 顺着 `$ref` 递归收集 schema 里所有对象的字段名。
///
/// 只收字段名、不收描述文字 —— 否则文档注释里提到 "base64" 就会造成误判。
fn collect_field_names(
    value: &serde_json::Value,
    definitions: &serde_json::Value,
) -> std::collections::BTreeSet<String> {
    fn walk(
        value: &serde_json::Value,
        definitions: &serde_json::Value,
        visited: &mut std::collections::BTreeSet<String>,
        fields: &mut std::collections::BTreeSet<String>,
    ) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(serde_json::Value::as_str) {
                    if let Some(name) = reference.rsplit('/').next() {
                        if visited.insert(name.to_string()) {
                            if let Some(definition) = definitions.get(name) {
                                walk(definition, definitions, visited, fields);
                            }
                        }
                    }
                }
                if let Some(properties) = object.get("properties").and_then(|p| p.as_object()) {
                    for (field, child) in properties {
                        fields.insert(field.clone());
                        walk(child, definitions, visited, fields);
                    }
                }
                // `oneOf` / `anyOf` / `allOf` / `items` / `additionalProperties` 等结构分支
                // 也要跟进去，否则包在枚举或数组里的字段会漏掉。
                for key in [
                    "oneOf",
                    "anyOf",
                    "allOf",
                    "items",
                    "additionalProperties",
                    "not",
                ] {
                    if let Some(child) = object.get(key) {
                        walk(child, definitions, visited, fields);
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, definitions, visited, fields);
                }
            }
            _ => {}
        }
    }
    let mut fields = std::collections::BTreeSet::new();
    let mut visited = std::collections::BTreeSet::new();
    walk(value, definitions, &mut visited, &mut fields);
    fields
}

fn looks_like_a_byte_payload(field: &str) -> bool {
    const MARKERS: [&str; 5] = ["base64", "b64", "bytesinline", "inlinebytes", "databytes"];
    let normalized = field.replace(['_', '-'], "").to_ascii_lowercase();
    MARKERS.iter().any(|marker| normalized.contains(marker))
}
