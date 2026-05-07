//! # Thinking / Reasoning 请求字段策略
//!
//! 把「逻辑档位 `ThinkingLevel`」+「厂商格式 `ThinkingFormat`」映射到具体请求体字段。
//! 详见 `docs/architecture/llm-stream-events-cli-pipeline.md` §4.2.2。
//!
//! 设计要点：
//!
//! - **集中策略**：provider 层只调用 [`resolve_request_fields`]，不再各自写 if/else，
//!   避免后续厂商分化扩散到 N 个文件。
//! - **互斥**：返回 `(reasoning_effort, thinking)`，只会有一个为 `Some`，由 format 决定。
//! - **向后兼容**：`ThinkingLevel::Off` 总是返回 `(None, None)`，`enabled=false` 时
//!   provider 应直接跳过本函数，请求体保持与历史一致。

use serde::{Deserialize, Serialize};

use crate::infra::config::ThinkingConfig;

/// 逻辑档位。与 pi-mono 对齐；`xhigh` 仅模型白名单支持时使用，否则降级为 `high`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    Xhigh,
}

impl ThinkingLevel {
    /// 容错解析；未知字符串退化为 `Medium` 并返回 `false` 让 caller 决定是否报告。
    pub fn parse_or_medium(s: &str) -> (Self, bool) {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => (Self::Off, true),
            "minimal" => (Self::Minimal, true),
            "low" => (Self::Low, true),
            "medium" => (Self::Medium, true),
            "high" => (Self::High, true),
            "xhigh" | "x-high" => (Self::Xhigh, true),
            _ => (Self::Medium, false),
        }
    }
}

/// 厂商请求格式。`Auto` 表示按 provider 名推断；其它显式指定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThinkingFormat {
    /// 按 provider 名推断（OpenAI Completions/Responses → `Openai`，DeepSeek/网关 → `Openrouter` 等）。
    #[default]
    Auto,
    /// OpenAI Chat Completions / Responses：`reasoning_effort` 字符串档位。
    Openai,
    /// OpenRouter / 兼容 OpenAI 网关：与 `Openai` 同形态，单独枚举便于扩展。
    Openrouter,
    /// DeepSeek：`reasoning_content` 仅在响应侧出现，请求侧无显式开关。
    Deepseek,
    /// 智谱 / Z.AI：与 OpenAI Responses 同形态；占位。
    Zai,
    /// Qwen：占位（不在本期接通）。
    Qwen,
    /// 豆包 / Moonshot：`thinking: {"type":"enabled", "max_tokens": ?}` 对象。
    Doubao,
}

impl ThinkingFormat {
    pub fn parse_or_auto(s: Option<&str>) -> Self {
        match s.map(|v| v.trim().to_ascii_lowercase()) {
            None => Self::Auto,
            Some(v) => match v.as_str() {
                "openai" => Self::Openai,
                "openrouter" => Self::Openrouter,
                "deepseek" => Self::Deepseek,
                "zai" => Self::Zai,
                "qwen" => Self::Qwen,
                "doubao" | "moonshot" => Self::Doubao,
                _ => Self::Auto,
            },
        }
    }

    /// 当 format=Auto 时按 provider id（与 [`crate::core::llm::registry`] 注册名）推断。
    pub fn resolve(&self, provider_id: &str) -> Self {
        if !matches!(self, Self::Auto) {
            return *self;
        }
        match provider_id {
            "openai" | "openai-responses" => Self::Openai,
            "deepseek" => Self::Deepseek,
            "zai" => Self::Zai,
            "qwen" => Self::Qwen,
            "doubao" | "moonshot" => Self::Doubao,
            _ => Self::Openai,
        }
    }
}

/// `resolve_request_fields` 的输出：互斥的两个字段，最多一个 `Some`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThinkingRequestFields {
    /// OpenAI 系：`reasoning_effort: "low"|"medium"|"high"|...`
    pub reasoning_effort: Option<String>,
    /// 豆包/Moonshot：`thinking: {"type":"enabled", "max_tokens": ...}`
    pub thinking: Option<serde_json::Value>,
}

/// 把 `ThinkingConfig` + provider 推断出的 `ThinkingFormat` 翻译为具体请求字段。
///
/// 行为：
/// - `enabled=false` 或 `level=off` → 全 None；
/// - OpenAI 系：`reasoning_effort` 为 level 字符串；`xhigh` 不在白名单（外部决定）时 caller 应降级为 `high`；
/// - 豆包/Moonshot：`thinking={"type":"enabled"}`，带 `max_tokens` 时附带；
/// - DeepSeek/Qwen 等无显式请求字段：返回 None（响应解析仍走 reasoning_content 三路兜底）。
pub fn resolve_request_fields(
    cfg: &ThinkingConfig,
    fmt: ThinkingFormat,
) -> ThinkingRequestFields {
    if !cfg.enabled {
        return ThinkingRequestFields::default();
    }
    let (level, _ok) = ThinkingLevel::parse_or_medium(&cfg.level);
    if matches!(level, ThinkingLevel::Off) {
        return ThinkingRequestFields::default();
    }
    match fmt {
        ThinkingFormat::Openai | ThinkingFormat::Openrouter | ThinkingFormat::Zai => {
            let v = match level {
                ThinkingLevel::Off => return ThinkingRequestFields::default(),
                ThinkingLevel::Minimal => "low",
                ThinkingLevel::Low => "low",
                ThinkingLevel::Medium => "medium",
                ThinkingLevel::High => "high",
                ThinkingLevel::Xhigh => "high",
            };
            ThinkingRequestFields {
                reasoning_effort: Some(v.to_string()),
                thinking: None,
            }
        }
        ThinkingFormat::Doubao => {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "type".to_string(),
                serde_json::Value::String("enabled".to_string()),
            );
            if let Some(mx) = cfg.max_tokens {
                obj.insert(
                    "max_tokens".to_string(),
                    serde_json::Value::Number(mx.into()),
                );
            }
            ThinkingRequestFields {
                reasoning_effort: None,
                thinking: Some(serde_json::Value::Object(obj)),
            }
        }
        // DeepSeek / Qwen：请求侧无显式 reasoning 参数；仅靠响应侧 reasoning_content 解析。
        ThinkingFormat::Deepseek | ThinkingFormat::Qwen => ThinkingRequestFields::default(),
        // Auto 应该已经被 caller resolve 掉；保险起见兜底。
        ThinkingFormat::Auto => ThinkingRequestFields::default(),
    }
}

// ─── P6 / P7：多轮重发剥离 + 持久化策略（API 地基） ────────────────────────────

/// `strip_on_resend` + `format` 两个条件的总判定：决定多轮重发时是否剥离思考。
///
/// 规则：
/// - `strip_on_resend=false` → 不剥离；
/// - `strip_on_resend=true` 时按 `format` 分支：
///   - `Deepseek`：必须剥离（否则 400 cls error），返回 true；
///   - `Anthropic`（未来）：必须保留（带 signature），返回 false；
///   - 其它（OpenAI/OpenRouter/Doubao/Qwen 等）：默认剥离（true）。
pub fn should_strip_on_resend(cfg: &ThinkingConfig, fmt: ThinkingFormat) -> bool {
    if !cfg.strip_on_resend {
        return false;
    }
    !matches!(fmt, ThinkingFormat::Auto)
}

/// `persist=true` 时上层应把 Thinking 事件落 transcript；默认 false（仅展示不落盘）。
pub fn should_persist_thinking(cfg: &ThinkingConfig) -> bool {
    cfg.enabled && cfg.persist
}

/// Anthropic 风格的 assistant 消息 content 是数组 `[{type: "thinking", ...}, {type: "text", ...}]`；
/// 在多轮重发时对该结构剥离 `type=thinking` 的块。**不针对** OpenAI 协议（OpenAI 内
/// 部 ChatMessage 从未把 thinking 写进 content，自然无需剥）。
///
/// 行为：仅当 `value` 为 `Array` 时遍历过滤；其它形状返回 0；返回剥离条数。
pub fn strip_anthropic_thinking_blocks(value: &mut serde_json::Value) -> usize {
    let arr = match value.as_array_mut() {
        Some(a) => a,
        None => return 0,
    };
    let before = arr.len();
    arr.retain(|item| {
        item.get("type")
            .and_then(|t| t.as_str())
            .map(|s| s != "thinking")
            .unwrap_or(true)
    });
    before - arr.len()
}

#[cfg(test)]
mod tests;
