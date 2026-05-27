//! T2-P0-002 Phase B 验收用例 —— 摘要 prompt 9 节模板 snapshot + Compaction 请求显式禁工具。
//!
//! 单一事实来源：[`docs/reports/compaction-prompt-cc-vs-pi.md`](../../../../../docs/reports/compaction-prompt-cc-vs-pi.md)
//! §5.3（BASE）/§5.4（UPDATE）/§5.7.1（Two-pass 决议固化）。
//!
//! 设计原则：
//! - 仅断言 `preheat.rs` 内 `const`，**不** `include_str!` 任何 spec / report 文件，避免 doc-test 与文档演进互锁；
//! - mock LLM provider 用 `OnceLock` 捕获 `ChatRequest`，验证 compaction 请求 `tools.is_none()` 与首行 prompt 双保险一致。

use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use tokio_stream::Stream;

use crate::core::compaction::preheat::{
    generate_summary, SUMMARIZATION_PROMPT, UPDATE_SUMMARIZATION_PROMPT,
};
use crate::core::llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatResponseChoice, LlmProvider, StreamEvent,
};
use crate::infra::error::AppError;

// ---------------------------------------------------------------------------
// BASE prompt snapshot — 子项 1（9 节模板）
// ---------------------------------------------------------------------------

#[test]
fn summarization_prompt_starts_with_no_tools_directive() {
    assert!(
        SUMMARIZATION_PROMPT.starts_with("Respond with text only. Do not call any tools."),
        "BASE prompt 首行必须与 ChatRequest.tools=None 形成双保险，禁止 provider 误调工具",
    );
}

#[test]
fn summarization_prompt_contains_first_reason_directive() {
    assert!(
        SUMMARIZATION_PROMPT.contains("First reason internally, then output the final summary."),
        "BASE prompt 指令区必须含 Two-pass 替代措辞（关闭 #T-044），让模型走内部隐式推理",
    );
}

#[test]
fn summarization_prompt_contains_9_section_headings() {
    // 9 节锚点（§5.3 报告唯一来源）：Goal / Constraints & Preferences / Progress（含 Done / In Progress / Blocked）
    // / Errors Encountered / Key Decisions / Recent User Messages / Next Steps / Critical Context
    let required_headings = [
        "## Goal",
        "## Constraints & Preferences",
        "## Progress",
        "### Done",
        "### In Progress",
        "### Blocked",
        "## Errors Encountered",
        "## Key Decisions",
        "## Recent User Messages",
        "## Next Steps",
        "## Critical Context",
    ];
    for h in required_headings {
        assert!(
            SUMMARIZATION_PROMPT.contains(h),
            "BASE prompt 缺失 9 节标题：{h}（来源 docs/reports/compaction-prompt-cc-vs-pi.md §5.3）",
        );
    }
}

#[test]
fn summarization_prompt_recent_user_messages_keeps_last_10() {
    assert!(
        SUMMARIZATION_PROMPT.contains("10 most recent non-tool user messages"),
        "Recent User Messages 节必须明确「最近 10 条用户原话」（统一口径，详见报告 §5.5）",
    );
}

#[test]
fn summarization_prompt_next_steps_requires_verbatim_quote() {
    assert!(
        SUMMARIZATION_PROMPT.contains("Include a short quote from the latest conversation"),
        "Next Steps 第一条必须带 verbatim 短引用，防摘要漂移（报告 §5.5 与 CC Next Step 借鉴）",
    );
}

#[test]
fn summarization_prompt_progress_done_carries_file_anchor_hint() {
    assert!(
        SUMMARIZATION_PROMPT.contains("(file: path/to/file, if applicable)"),
        "Progress::Done 子项必须保留文件路径锚点提示（合并 CC Files & Code 精简版）",
    );
}

// ---------------------------------------------------------------------------
// UPDATE prompt snapshot — 子项 1（增量更新 9 节）
// ---------------------------------------------------------------------------

#[test]
fn update_summarization_prompt_starts_with_no_tools_directive() {
    assert!(
        UPDATE_SUMMARIZATION_PROMPT.starts_with("Respond with text only. Do not call any tools."),
        "UPDATE prompt 首行同样需要 text-only 声明，与 BASE 保持一致",
    );
}

#[test]
fn update_summarization_prompt_contains_first_reason_directive() {
    assert!(
        UPDATE_SUMMARIZATION_PROMPT
            .contains("First reason internally, then output the final summary."),
        "UPDATE prompt 指令区也必须含 Two-pass 替代措辞，模板间口径一致",
    );
}

#[test]
fn update_summarization_prompt_keeps_existing_summary_placeholder() {
    assert!(
        UPDATE_SUMMARIZATION_PROMPT.contains("{existing_summary}"),
        "UPDATE prompt 必须保留 {{existing_summary}} 占位符——generate_summary 会用 .replace 注入旧摘要",
    );
}

#[test]
fn update_summarization_prompt_has_rules_block_and_format_reference() {
    let rules_anchors = [
        "RULES:",
        "PRESERVE information from the previous summary",
        "ADD new progress, decisions, errors, and context",
        "UPDATE Progress: move items from \"In Progress\" to \"Done\"",
        "UPDATE \"Next Steps\" and \"Recent User Messages\"",
        "REMOVE information that is no longer relevant",
        "PRESERVE exact file paths, function names, and error messages",
    ];
    for a in rules_anchors {
        assert!(
            UPDATE_SUMMARIZATION_PROMPT.contains(a),
            "UPDATE prompt RULES 块缺失锚点：{a}（来源报告 §5.4）",
        );
    }
    assert!(
        UPDATE_SUMMARIZATION_PROMPT.contains(
            "Use the EXACT same format as the original summary (Goal / Constraints & Preferences / Progress / Errors Encountered / Key Decisions / Recent User Messages / Next Steps / Critical Context)."
        ),
        "UPDATE prompt 必须显式列出 9 节格式回链，避免增量更新走形",
    );
}

// ---------------------------------------------------------------------------
// generate_summary 行为校验 —— 子项 2（ChatRequest.tools = None）
// ---------------------------------------------------------------------------

/// Mock LLM Provider：捕获 `ChatRequest`，返回固定 9 节摘要。
struct CapturingMockProvider {
    captured: Arc<Mutex<Option<ChatRequest>>>,
}

impl CapturingMockProvider {
    fn new() -> (Self, Arc<Mutex<Option<ChatRequest>>>) {
        let captured = Arc::new(Mutex::new(None));
        (
            Self {
                captured: captured.clone(),
            },
            captured,
        )
    }
}

#[async_trait]
impl LlmProvider for CapturingMockProvider {
    fn provider_name(&self) -> &str {
        "capturing_mock"
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AppError> {
        *self.captured.lock().unwrap() = Some(request);
        Ok(ChatResponse {
            id: Some("test_resp".into()),
            choices: vec![ChatResponseChoice {
                index: 0,
                message: ChatMessage::assistant("## Goal\nstub summary"),
                finish_reason: Some("stop".into()),
            }],
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        _request: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent, AppError>> + Send + Unpin>, AppError>
    {
        Err(AppError::Llm(
            "not used in compaction snapshot tests".into(),
        ))
    }

    fn count_tokens(&self, _messages: &[ChatMessage]) -> Result<u32, AppError> {
        Ok(0)
    }
}

#[tokio::test]
async fn compaction_request_carries_no_tools() {
    let (provider, captured) = CapturingMockProvider::new();
    let snapshot = vec![ChatMessage::user("hello"), ChatMessage::assistant("world")];

    let summary = generate_summary(&snapshot, None, &provider, "gpt-5.4")
        .await
        .expect("mock provider 应返回非空摘要");
    assert!(
        !summary.is_empty(),
        "mock 摘要不应为空，否则 generate_summary 会判定 EmptyResponse"
    );

    let req = captured
        .lock()
        .unwrap()
        .clone()
        .expect("mock provider 必须捕获到 ChatRequest");
    assert!(
        req.tools.is_none(),
        "Compaction MUST NOT carry tools（双保险：prompt 首行 + req.tools = None）；当前 tools = {:?}",
        req.tools,
    );
    assert_eq!(
        req.stream,
        Some(false),
        "compaction 走非流式响应，stream 必须显式 false（而非 None）",
    );

    let system_msg_text = req
        .messages
        .iter()
        .find(|m| matches!(m.role, crate::core::llm::ChatMessageRole::System))
        .and_then(|m| m.text_content())
        .unwrap_or("");
    assert!(
        system_msg_text.starts_with("Respond with text only. Do not call any tools."),
        "system 消息首行应携带 text-only 声明，确保 prompt + tools=None 双保险一致",
    );
}

#[tokio::test]
async fn compaction_request_uses_update_prompt_when_existing_summary_present() {
    let (provider, captured) = CapturingMockProvider::new();
    let snapshot = vec![ChatMessage::user("next user msg")];
    let previous = "## Goal\nold goal\n## Next Steps\n1. old step";

    let _ = generate_summary(&snapshot, Some(previous), &provider, "gpt-4o-mini")
        .await
        .expect("mock provider 应返回非空摘要");

    let req = captured
        .lock()
        .unwrap()
        .clone()
        .expect("应捕获 ChatRequest");
    let system_msg_text = req
        .messages
        .iter()
        .find(|m| matches!(m.role, crate::core::llm::ChatMessageRole::System))
        .and_then(|m| m.text_content())
        .unwrap_or("")
        .to_string();
    assert!(
        system_msg_text.contains("Update the existing structured summary"),
        "存在 previous_summary 时应用 UPDATE 模板",
    );
    assert!(
        system_msg_text.contains("old goal") && system_msg_text.contains("old step"),
        "UPDATE 模板里的 {{existing_summary}} 占位符必须被替换为旧摘要文本",
    );
    assert!(
        req.tools.is_none(),
        "UPDATE 路径同样禁工具：req.tools 必须为 None",
    );
}

// ---------------------------------------------------------------------------
// 静态可见性自检 —— 防止后续重构误把 const 退回私有，破坏其他子模块的 snapshot 引用
// ---------------------------------------------------------------------------

static _BASE_PTR: OnceLock<&'static str> = OnceLock::new();
static _UPDATE_PTR: OnceLock<&'static str> = OnceLock::new();

#[test]
fn const_visibility_remains_pub_super_for_compaction_tests() {
    let base = *_BASE_PTR.get_or_init(|| SUMMARIZATION_PROMPT);
    let upd = *_UPDATE_PTR.get_or_init(|| UPDATE_SUMMARIZATION_PROMPT);
    // 静态访问能编译说明可见性正确；额外断言两段不为空，作为冒烟。
    assert!(!base.is_empty());
    assert!(!upd.is_empty());
}
