use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;

use crate::core::plan_runtime::panels::{AskQuestionPanel, AskQuestionResult, Question};

/// IDE 端占位实现。
///
/// 当前阶段只保留 `cancelled` 兜底；未来若 IDE host 接入，需复用
/// `ask_question_wire` 中的通用 bridge，而不是再定义一套 IDE 专用事件。
pub struct IdeAskQuestionPanel;

#[async_trait]
impl AskQuestionPanel for IdeAskQuestionPanel {
    async fn ask(
        &self,
        _questions: Vec<Question>,
        _cancel_signal: Arc<AtomicBool>,
    ) -> AskQuestionResult {
        AskQuestionResult {
            answers: vec![],
            cancelled: true,
        }
    }
}
