//! # Token 消耗统计
//!
//! 单次调用 usage 由 ChatResponse/StreamEvent 提供；会话级汇总由调用方累加并写入 SessionEntry（当 003 可用时）。

use serde::{Deserialize, Serialize};

/// 会话级 Token 消耗汇总，由调用方在每次 LLM 调用后累加；
/// 持久化到 SessionEntry.input_tokens / output_tokens 由上层完成。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionTokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl SessionTokenUsage {
    pub fn add(&mut self, prompt_tokens: u32, completion_tokens: u32) {
        self.input_tokens += u64::from(prompt_tokens);
        self.output_tokens += u64::from(completion_tokens);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_token_usage_add() {
        let mut u = SessionTokenUsage::default();
        u.add(10, 20);
        assert_eq!(u.input_tokens, 10);
        assert_eq!(u.output_tokens, 20);
        u.add(5, 15);
        assert_eq!(u.input_tokens, 15);
        assert_eq!(u.output_tokens, 35);
    }
}
