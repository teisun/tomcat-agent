use serde::{Deserialize, Serialize};

use crate::infra::AppError;

/// `web_fetch.url` 的最大字符数。
pub const MAX_URL_LENGTH: usize = 2_000;

/// `web_fetch` 工具入参。
#[derive(Debug, Clone, Deserialize)]
pub struct WebFetchArgs {
    pub url: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
}

/// `web_fetch` 的输出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WebFetchFormat {
    Markdown,
    Text,
}

impl WebFetchFormat {
    pub(crate) fn parse(raw: Option<&str>) -> Result<Self, AppError> {
        match raw
            .unwrap_or("markdown")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "markdown" => Ok(Self::Markdown),
            "text" => Ok(Self::Text),
            other => Err(AppError::Tool(format!(
                "web_fetch: `format` 非法 `{other}`，允许 markdown/text"
            ))),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Text => "text",
        }
    }

    pub(crate) fn persisted_extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Text => "txt",
        }
    }
}

/// 归一化后的 `web_fetch` 请求。
#[derive(Debug, Clone)]
pub(crate) struct WebFetchRequest {
    pub raw_url: String,
    pub prompt: Option<String>,
    pub format: WebFetchFormat,
}

impl WebFetchRequest {
    pub(crate) fn from_tool_args(args: WebFetchArgs) -> Result<Self, AppError> {
        let raw_url = args.url.trim();
        if raw_url.is_empty() {
            return Err(AppError::Tool("web_fetch: 缺少必填字段 `url`".to_string()));
        }
        Ok(Self {
            raw_url: raw_url.to_string(),
            prompt: args
                .prompt
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            format: WebFetchFormat::parse(args.format.as_deref())?,
        })
    }
}

/// off-host 重定向时返回给模型的结构化信息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedirectInfo {
    pub original_url: String,
    pub redirect_url: String,
    pub status_code: u16,
}

/// `web_fetch` 工具输出。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebFetchOutput {
    pub url: String,
    pub code: u16,
    pub code_text: String,
    pub content_type: String,
    pub bytes: u64,
    pub result: String,
    pub total_chars: u64,
    pub duration_ms: u64,
    pub cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persisted_output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect: Option<RedirectInfo>,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

impl WebFetchOutput {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        url: String,
        code: u16,
        code_text: String,
        content_type: String,
        bytes: u64,
        result: String,
        total_chars: u64,
        duration_ms: u64,
        persisted_output_path: Option<String>,
        redirect: Option<RedirectInfo>,
        truncated: bool,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            url,
            code,
            code_text,
            content_type,
            bytes,
            result,
            total_chars,
            duration_ms,
            cached: false,
            persisted_output_path,
            redirect,
            truncated,
            warnings,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn degraded(
        url: String,
        code: u16,
        code_text: String,
        content_type: String,
        bytes: u64,
        duration_ms: u64,
        truncated: bool,
        warnings: Vec<String>,
    ) -> Self {
        Self::new(
            url,
            code,
            code_text,
            content_type,
            bytes,
            String::new(),
            0,
            duration_ms,
            None,
            None,
            truncated,
            warnings,
        )
    }

    pub(crate) fn redirect(
        url: String,
        original_url: String,
        redirect_url: String,
        status_code: u16,
        code_text: String,
        duration_ms: u64,
        warnings: Vec<String>,
    ) -> Self {
        Self::new(
            url,
            status_code,
            code_text,
            String::new(),
            0,
            String::new(),
            0,
            duration_ms,
            None,
            Some(RedirectInfo {
                original_url,
                redirect_url,
                status_code,
            }),
            false,
            warnings,
        )
    }
}
