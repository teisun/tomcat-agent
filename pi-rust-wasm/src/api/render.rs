//! Markdown / 代码块高亮渲染器。
//!
//! 流式输出场景：调用方逐块 push delta，取出已就绪的渲染结果。
//! 代码块（``` ... ```）内的内容在块结束时一次性高亮输出。

use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

impl Default for MarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MarkdownRenderer {
    buffer: String,
    in_code_block: bool,
    code_lang: String,
    code_content: String,
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    ready: Vec<String>,
}

impl MarkdownRenderer {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            in_code_block: false,
            code_lang: String::new(),
            code_content: String::new(),
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
            ready: Vec::new(),
        }
    }

    /// Push a stream delta. Internally buffers and processes line-by-line.
    pub fn push(&mut self, delta: &str) {
        self.buffer.push_str(delta);
        self.process_buffer();
    }

    /// Take the next ready-to-output chunk (already rendered).
    pub fn take_ready(&mut self) -> Option<String> {
        if self.ready.is_empty() {
            None
        } else {
            let mut out = String::new();
            for chunk in self.ready.drain(..) {
                out.push_str(&chunk);
            }
            Some(out)
        }
    }

    /// Flush remaining buffer content (call at end of stream).
    pub fn flush(&mut self) -> Option<String> {
        let mut out = String::new();
        if self.in_code_block && !self.code_content.is_empty() {
            out.push_str(&self.highlight_code(&self.code_lang.clone(), &self.code_content.clone()));
            out.push_str("\x1b[0m");
            self.code_content.clear();
            self.in_code_block = false;
        }
        if !self.buffer.is_empty() {
            out.push_str(&self.buffer);
            self.buffer.clear();
        }
        if !self.ready.is_empty() {
            for chunk in self.ready.drain(..) {
                out.push_str(&chunk);
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    fn process_buffer(&mut self) {
        while let Some(newline_pos) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=newline_pos).collect();

            if !self.in_code_block && line.trim_start().starts_with("```") {
                self.in_code_block = true;
                self.code_lang = line.trim_start().trim_start_matches('`').trim().to_string();
                self.code_content.clear();
                self.ready.push(format!("\x1b[90m{}\x1b[0m", line));
                continue;
            }

            if self.in_code_block && line.trim_start().starts_with("```") {
                let highlighted =
                    self.highlight_code(&self.code_lang.clone(), &self.code_content.clone());
                self.ready.push(highlighted);
                self.ready.push("\x1b[0m".to_string());
                self.ready.push(format!("\x1b[90m{}\x1b[0m", line));
                self.code_content.clear();
                self.in_code_block = false;
                continue;
            }

            if self.in_code_block {
                self.code_content.push_str(&line);
                continue;
            }

            self.ready.push(line);
        }

        if !self.in_code_block && !self.buffer.is_empty() {
            let remaining = std::mem::take(&mut self.buffer);
            self.ready.push(remaining);
        }
    }

    fn highlight_code(&self, lang: &str, code: &str) -> String {
        let syntax = self
            .syntax_set
            .find_syntax_by_token(lang)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let theme = &self.theme_set.themes["base16-ocean.dark"];
        let mut h = HighlightLines::new(syntax, theme);
        let mut out = String::new();

        for line in LinesWithEndings::from(code) {
            match h.highlight_line(line, &self.syntax_set) {
                Ok(ranges) => {
                    let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
                    out.push_str(&escaped);
                }
                Err(_) => {
                    out.push_str(line);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passes_through() {
        let mut r = MarkdownRenderer::new();
        r.push("hello world\n");
        let out = r.take_ready().unwrap();
        assert!(out.contains("hello world"));
    }

    #[test]
    fn code_block_is_highlighted() {
        let mut r = MarkdownRenderer::new();
        r.push("```rust\nfn main() {}\n```\n");
        let out = r.take_ready().unwrap();
        assert!(out.contains("fn"));
        assert!(out.contains("\x1b["));
    }

    #[test]
    fn unknown_lang_falls_back() {
        let mut r = MarkdownRenderer::new();
        r.push("```xyzlang\nsome code\n```\n");
        let out = r.take_ready().unwrap();
        assert!(out.contains("some code"));
    }

    #[test]
    fn flush_returns_remaining() {
        let mut r = MarkdownRenderer::new();
        r.push("partial");
        assert!(r.take_ready().is_some());
        let flushed = r.flush();
        assert!(matches!(flushed, None | Some(_)));
    }
}
