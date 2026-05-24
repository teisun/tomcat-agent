use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;

use crate::core::plan_runtime::panels::{AskQuestionPanel, AskQuestionResult, Question};

/// IDE 端 stub。当前直接 cancelled；后续 TUI 接入后由实际 panel 替换。
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
