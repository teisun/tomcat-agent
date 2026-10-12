//! OpenAI prompt-cache affinity keys.
//!
//! A key is scoped to one session and one request family. Families must remain
//! distinct because their stable prompt prefixes are intentionally different.
//! Anthropic does not accept this field: its cache is controlled solely by
//! `cache_control` breakpoints in the Messages request body.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptCacheKeyFamily<'a> {
    Main,
    Subagent(&'a str),
    Compaction,
    Title,
    Extension,
}

impl PromptCacheKeyFamily<'_> {
    pub fn key_for(self, session_id: &str) -> Option<String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return None;
        }
        let family = match self {
            Self::Main => "main".to_string(),
            Self::Subagent(kind) => format!("subagent:{kind}"),
            Self::Compaction => "compaction".to_string(),
            Self::Title => "title".to_string(),
            Self::Extension => "ext".to_string(),
        };
        Some(format!("{session_id}:{family}"))
    }
}
