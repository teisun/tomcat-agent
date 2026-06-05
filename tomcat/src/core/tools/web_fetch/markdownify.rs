use std::sync::LazyLock;

use regex::Regex;

use super::types::WebFetchFormat;

static HTML_NOISE_REGEXES: LazyLock<[Regex; 4]> = LazyLock::new(|| {
    [
        Regex::new(r"(?is)<script\b[^>]*>.*?</script>").expect("valid script cleanup regex"),
        Regex::new(r"(?is)<style\b[^>]*>.*?</style>").expect("valid style cleanup regex"),
        Regex::new(r"(?is)<nav\b[^>]*>.*?</nav>").expect("valid nav cleanup regex"),
        Regex::new(r"(?is)<footer\b[^>]*>.*?</footer>").expect("valid footer cleanup regex"),
    ]
});

static MARKDOWN_CLEANUP_RULES: LazyLock<[(Regex, &'static str); 7]> = LazyLock::new(|| {
    [
        (
            Regex::new(r"(?m)^\s{0,3}#{1,6}\s*").expect("valid heading cleanup regex"),
            "",
        ),
        (
            Regex::new(r"(?m)^\s*[-*+]\s+").expect("valid bullet cleanup regex"),
            "",
        ),
        (
            Regex::new(r"(?m)^\s*\d+\.\s+").expect("valid list cleanup regex"),
            "",
        ),
        (
            Regex::new(r"!\[([^\]]*)\]\([^)]+\)").expect("valid image cleanup regex"),
            "$1",
        ),
        (
            Regex::new(r"\[([^\]]+)\]\([^)]+\)").expect("valid link cleanup regex"),
            "$1",
        ),
        (
            Regex::new(r"`([^`]+)`").expect("valid inline code cleanup regex"),
            "$1",
        ),
        (
            Regex::new(r"[*_~]{1,3}").expect("valid emphasis cleanup regex"),
            "",
        ),
    ]
});

static EXTRA_NEWLINES_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n{3,}").expect("valid whitespace regex"));

/// 把文本响应渲染成给模型的 `markdown` / `text`。
pub(crate) fn render_textual_body(
    body: &[u8],
    content_type: &str,
    format: WebFetchFormat,
) -> String {
    let raw = String::from_utf8_lossy(body);
    if looks_like_html(content_type, &raw) {
        let cleaned = strip_html_noise(&raw);
        let markdown = normalize_text(&html2md::parse_html(&cleaned));
        return match format {
            WebFetchFormat::Markdown => markdown,
            WebFetchFormat::Text => markdown_to_text(&markdown),
        };
    }

    if is_verbatim_text_content_type(content_type) {
        return raw.into_owned();
    }

    let normalized = normalize_text(&raw);
    match (format, normalized_content_type(content_type).as_str()) {
        (WebFetchFormat::Text, "text/markdown") => markdown_to_text(&normalized),
        _ => normalized,
    }
}

fn looks_like_html(content_type: &str, raw: &str) -> bool {
    let normalized = normalized_content_type(content_type);
    if normalized.contains("html") || normalized == "application/xhtml+xml" {
        return true;
    }
    let sample = raw.trim_start().chars().take(256).collect::<String>();
    let lower = sample.to_ascii_lowercase();
    lower.starts_with("<!doctype html")
        || lower.starts_with("<html")
        || lower.contains("<body")
        || lower.contains("<article")
}

fn strip_html_noise(raw: &str) -> String {
    let mut out = raw.to_string();
    for re in HTML_NOISE_REGEXES.iter() {
        out = re.replace_all(&out, "").into_owned();
    }
    out
}

fn markdown_to_text(markdown: &str) -> String {
    let mut out = markdown.to_string();
    for (re, replacement) in MARKDOWN_CLEANUP_RULES.iter() {
        out = re.replace_all(&out, *replacement).into_owned();
    }
    normalize_text(&out)
}

fn normalize_text(raw: &str) -> String {
    let unified = raw.replace("\r\n", "\n");
    EXTRA_NEWLINES_RE
        .replace_all(unified.trim(), "\n\n")
        .into_owned()
}

fn is_verbatim_text_content_type(content_type: &str) -> bool {
    matches!(
        normalized_content_type(content_type).as_str(),
        "application/json" | "application/xml" | "text/xml"
    ) || normalized_content_type(content_type).ends_with("+xml")
}

pub(crate) fn normalized_content_type(raw: &str) -> String {
    raw.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}
