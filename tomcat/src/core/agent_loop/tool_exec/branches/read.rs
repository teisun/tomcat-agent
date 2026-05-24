use super::super::args::parse_optional_u64;
use super::super::guard::validate_read_bounds;
use super::super::{ToolExecCtx, AGENT_PLUGIN_ID};

pub(in super::super) async fn handle_read(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<(String, Vec<crate::core::llm::ChatMessageContentPart>), String> {
    let path = args["path"].as_str().unwrap_or("");
    let offset = parse_optional_u64(args, "offset");
    let limit = parse_optional_u64(args, "limit");
    validate_read_bounds(offset, limit)?;

    let line_numbers = args
        .get("line_numbers")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let hashline = args
        .get("hashline")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let resolved = crate::infra::platform::normalize_path(path).unwrap_or_else(|_| path.into());
    let stub_short_circuit = ctx.read_file_state.and_then(|state| {
        let stamp = state.get(&resolved)?;
        let meta = std::fs::metadata(&resolved).ok()?;
        if meta.is_dir() {
            return None;
        }
        let mtime = crate::core::tools::pipeline::read_state::metadata_mtime_ms(&meta);
        if stamp.matches_request(mtime, meta.len(), offset, limit) {
            Some(crate::core::tools::pipeline::read_state::FILE_UNCHANGED_STUB.to_string())
        } else {
            None
        }
    });
    if let Some(stub) = stub_short_circuit {
        return Ok((stub, Vec::new()));
    }

    let exec_result = ctx
        .primitive
        .read(path, offset, limit, line_numbers, hashline, AGENT_PLUGIN_ID)
        .await;

    if let (Ok(result), Some(state)) = (exec_result.as_ref(), ctx.read_file_state) {
        if let Ok(meta) = std::fs::metadata(&resolved) {
            if !meta.is_dir() {
                let path_bytes: Vec<u8>;
                let hash_input: &[u8] = match result {
                    crate::core::tools::primitive::ReadResult::Text(t) => t.content.as_bytes(),
                    crate::core::tools::primitive::ReadResult::Image(b)
                    | crate::core::tools::primitive::ReadResult::Pdf(b) => {
                        path_bytes = b.path.as_os_str().as_encoded_bytes().to_vec();
                        &path_bytes[..]
                    }
                    crate::core::tools::primitive::ReadResult::FileUnchanged { .. } => &[],
                };
                let stamp = crate::core::tools::pipeline::read_state::ReadStamp {
                    mtime_ms: crate::core::tools::pipeline::read_state::metadata_mtime_ms(&meta),
                    size: meta.len(),
                    content_hash: crate::core::tools::pipeline::read_state::hash_content(hash_input),
                    offset,
                    limit,
                    is_partial_view: offset.is_some() || limit.is_some(),
                };
                state.put(resolved.clone(), stamp);
            }
        }
    }

    match exec_result {
        Ok(result) => {
            let mut follow_up_parts = Vec::new();
            match &result {
                crate::core::tools::primitive::ReadResult::Image(b) => {
                    let decision =
                        crate::core::llm::openai_files::upload_decision_by_size(b.original_size);
                    let mut uploaded = false;
                    if let Some(runtime) = ctx.openai_files_runtime {
                        if !matches!(
                            decision,
                            crate::core::llm::openai_files::UploadDecision::InlinePreferred
                        ) {
                            match runtime
                                .resolve_or_upload_path(
                                    &b.path,
                                    &b.mime,
                                    &b.filename,
                                    crate::core::llm::openai_files::FilePurpose::Vision,
                                )
                                .await
                            {
                                Ok(meta) => {
                                    match crate::core::llm::ChatMessageContentPart::image_file_id(
                                        meta.id,
                                    ) {
                                        Ok(part) => {
                                            follow_up_parts.push(part);
                                            uploaded = true;
                                        }
                                        Err(e) => tracing::warn!(
                                            error = %e,
                                            path = %b.path.display(),
                                            "read T3-c: upload succeeded but failed to build image_file_id part"
                                        ),
                                    }
                                }
                                Err(e) => {
                                    if matches!(
                                        decision,
                                        crate::core::llm::openai_files::UploadDecision::UploadRequired
                                    ) {
                                        return Err(format!(
                                            "Read attachment upload failed (required by policy): {}",
                                            e
                                        ));
                                    }
                                    tracing::warn!(
                                        error = %e,
                                        path = %b.path.display(),
                                        "read T3-c: upload failed on preferred path; fallback to inline"
                                    );
                                }
                            }
                        }
                    } else if matches!(
                        decision,
                        crate::core::llm::openai_files::UploadDecision::UploadRequired
                    ) {
                        return Err(
                            "Read attachment requires OpenAI Files upload, but current provider/runtime does not support it; 请改用支持 Files API 的 provider 或缩小附件后走 inline".to_string(),
                        );
                    }

                    if !uploaded {
                        match crate::core::llm::ChatMessageContentPart::image_b64(
                            b.mime.clone(),
                            &b.path,
                        ) {
                            Ok(part) => follow_up_parts.push(part),
                            Err(e) => tracing::warn!(
                                error = %e,
                                path = %b.path.display(),
                                "read T3-c: failed to build InputImage part; falling back to text-only tool message"
                            ),
                        }
                    }
                }
                crate::core::tools::primitive::ReadResult::Pdf(b) => {
                    let decision =
                        crate::core::llm::openai_files::upload_decision_by_size(b.original_size);
                    let mut uploaded = false;
                    if let Some(runtime) = ctx.openai_files_runtime {
                        if !matches!(
                            decision,
                            crate::core::llm::openai_files::UploadDecision::InlinePreferred
                        ) {
                            match runtime
                                .resolve_or_upload_path(
                                    &b.path,
                                    &b.mime,
                                    &b.filename,
                                    crate::core::llm::openai_files::FilePurpose::UserData,
                                )
                                .await
                            {
                                Ok(meta) => {
                                    match crate::core::llm::ChatMessageContentPart::file_file_id(
                                        meta.id,
                                        Some(b.filename.clone()),
                                    ) {
                                        Ok(part) => {
                                            follow_up_parts.push(part);
                                            uploaded = true;
                                        }
                                        Err(e) => tracing::warn!(
                                            error = %e,
                                            path = %b.path.display(),
                                            "read T3-c: upload succeeded but failed to build file_file_id part"
                                        ),
                                    }
                                }
                                Err(e) => {
                                    if matches!(
                                        decision,
                                        crate::core::llm::openai_files::UploadDecision::UploadRequired
                                    ) {
                                        return Err(format!(
                                            "Read attachment upload failed (required by policy): {}",
                                            e
                                        ));
                                    }
                                    tracing::warn!(
                                        error = %e,
                                        path = %b.path.display(),
                                        "read T3-c: upload failed on preferred path; fallback to inline"
                                    );
                                }
                            }
                        }
                    } else if matches!(
                        decision,
                        crate::core::llm::openai_files::UploadDecision::UploadRequired
                    ) {
                        return Err(
                            "Read attachment requires OpenAI Files upload, but current provider/runtime does not support it; 请改用支持 Files API 的 provider 或缩小附件后走 inline".to_string(),
                        );
                    }

                    if !uploaded {
                        match crate::core::llm::ChatMessageContentPart::file_b64(
                            b.filename.clone(),
                            b.mime.clone(),
                            &b.path,
                        ) {
                            Ok(part) => follow_up_parts.push(part),
                            Err(e) => tracing::warn!(
                                error = %e,
                                path = %b.path.display(),
                                "read T3-c: failed to build InputFile part; falling back to text-only tool message"
                            ),
                        }
                    }
                }
                crate::core::tools::primitive::ReadResult::Text(_)
                | crate::core::tools::primitive::ReadResult::FileUnchanged { .. } => {}
            }
            Ok((result.to_tool_text(), follow_up_parts))
        }
        Err(e) => Err(e.to_string()),
    }
}
