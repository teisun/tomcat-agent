//! Utility 模型摘要：thinking 折叠标题与会话标题。

mod title_generator;
mod tool_summary;

pub use title_generator::{
    fallback_turn_summary, generate_session_title, generate_turn_summary, ToolSnapshot,
};
pub use tool_summary::one_line_summary;

#[cfg(test)]
mod tests;
