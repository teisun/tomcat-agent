//! Utility 模型摘要：thinking 折叠标题与会话标题。

mod title_generator;
mod tool_summary;

pub use title_generator::{
    fallback_command_summary, fallback_turn_summary, generate_command_summary,
    generate_command_summary_with_output_limit, generate_session_title,
    generate_session_title_with_cache_key, generate_session_title_with_cache_key_and_output_limit,
    generate_turn_summary, generate_turn_summary_with_output_limit, ToolSnapshot,
};
pub use tool_summary::one_line_summary;

#[cfg(test)]
mod tests;
