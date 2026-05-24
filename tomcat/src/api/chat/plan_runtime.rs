//! `core::plan_runtime` 的 API 层转发入口。
//!
//! 在全仓 import 完成 rewiring 之前，保留旧路径 `api::chat::plan_runtime::*`
//! 指向新的核心实现，避免迁移过程中的大规模编译断点。

pub use crate::core::plan_runtime::*;

pub mod ask_question_panel {
    pub use crate::api::chat::panels::{CliAskQuestionPanel, IdeAskQuestionPanel};
    pub use crate::core::plan_runtime::panels::{
        Answer, AskQuestionPanel, AskQuestionResult, MockAskQuestionPanel, Question,
        QuestionOption, CUSTOM_OPTION_ID,
    };
}

pub mod todos_panel {
    pub use crate::api::chat::panels::CliTodosPanel;
    pub use crate::core::plan_runtime::panels::{
        next_panel_snapshot_id, NoopTodosPanel, RefreshNotifier, TodosPanel, TodosPanelSnapshot,
    };
}
