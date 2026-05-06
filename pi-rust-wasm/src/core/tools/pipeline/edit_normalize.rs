//! # `edit` 工具：T2 normalize 管道（PR-H）
//!
//! 实现 [openspec/specs/architecture/tools/edit.md §2.4.4](../../../openspec/specs/architecture/tools/edit.md)
//! 的 normalize 数据流：
//!
//! ```text
//!   disk bytes
//!        │
//!        ▼
//!   strip_bom ──▶ original（逻辑内容）
//!        │
//!        ▼
//!   normalize_to_lf ──▶ working（仅匹配用）
//!        │
//!        ▼
//!   find spans on working ──▶ map spans to original indices
//!        │
//!        ▼
//!   splice original ──▶ new_content
//!        │
//!        ▼
//!   restore_line_endings + prepend BOM if any ──▶ write_file_atomic
//! ```
//!
//! ## 设计要点
//!
//! - **纯函数**：本模块只暴露纯函数，不接触磁盘，便于 ≥99% 用例覆盖。
//! - **静默 LF 化仅在 working**：磁盘语义保留原 BOM 与原行尾（CRLF/CR）。
//! - **Span 映射零拷贝**：working 与 original 大多数情况是同一坐标（无 CR 时）；
//!   仅当文本含 CRLF 时才需要把 working 上的字节偏移映射回 original。
//! - **curly-quote / de-sanitize 表**：在本文件内冻结，PR-H 合入时不再变动。

use std::borrow::Cow;

/// 文件原始换行风格。`restore_line_endings` 用它把工作副本的 LF 还原到磁盘。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEndingKind {
    /// 全部 `\n`（Unix / 现代默认）。
    Lf,
    /// 全部 `\r\n`（Windows / 旧 IDE）。
    CrLf,
    /// 全部 `\r`（旧 Mac，几乎绝迹但保留兼容）。
    Cr,
    /// 混用或文件无换行。`restore_line_endings` 不做转换，原样写回。
    Mixed,
}

/// 剥掉文件头的 UTF-8 BOM（`\u{FEFF}`），返回剥离后内容与「是否曾有 BOM」。
///
/// 写回阶段 `had_bom == true` 时必须重新 prepend BOM，避免静默改变磁盘语义。
pub fn strip_bom(input: &str) -> (&str, bool) {
    if let Some(rest) = input.strip_prefix('\u{FEFF}') {
        (rest, true)
    } else {
        (input, false)
    }
}

/// 探测文件主体的换行风格（不考虑 BOM）。
///
/// 决策规则（与 cc-fork / pi-mono 同档）：
/// - 出现任意 `\r\n` 但**无**孤立 `\r` / `\n` → CrLf
/// - 出现任意 `\r` 但**无** `\n` → Cr
/// - 仅有 `\n` 或全无换行 → Lf
/// - 上述以外（混用） → Mixed（写回不做转换）
pub fn detect_line_ending(input: &str) -> LineEndingKind {
    let bytes = input.as_bytes();
    let mut has_crlf = false;
    let mut has_lone_cr = false;
    let mut has_lone_lf = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    has_crlf = true;
                    i += 2;
                    continue;
                }
                has_lone_cr = true;
            }
            b'\n' => has_lone_lf = true,
            _ => {}
        }
        i += 1;
    }
    match (has_crlf, has_lone_cr, has_lone_lf) {
        (true, false, false) => LineEndingKind::CrLf,
        (false, true, false) => LineEndingKind::Cr,
        (false, false, _) => LineEndingKind::Lf,
        _ => LineEndingKind::Mixed,
    }
}

/// 把任意 `\r\n` / `\r` 折叠为 `\n`，得到 `working` 副本。
///
/// 仅用于匹配；写回阶段必须用 [`restore_line_endings`] 反向还原。
pub fn normalize_to_lf(input: &str) -> Cow<'_, str> {
    if !input.contains('\r') {
        return Cow::Borrowed(input);
    }
    // 字节级实现：CR/LF/CRLF 都是 ASCII 边界字符，不会与多字节 UTF-8 序列冲突。
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                out.push(b'\n');
                i += 2;
                continue;
            }
            out.push(b'\n');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    Cow::Owned(String::from_utf8(out).expect("CRLF normalization preserves UTF-8 boundaries"))
}

/// 把 `working`（全 `\n`）按 `kind` 还原行尾。
pub fn restore_line_endings(kind: LineEndingKind, content: &str) -> Cow<'_, str> {
    match kind {
        LineEndingKind::Lf | LineEndingKind::Mixed => Cow::Borrowed(content),
        LineEndingKind::CrLf => Cow::Owned(content.replace('\n', "\r\n")),
        LineEndingKind::Cr => Cow::Owned(content.replace('\n', "\r")),
    }
}

/// 折叠常见智能引号（curly quotes）回到 ASCII 形态，便于「模型粘的弯引号」
/// 与「磁盘上的直引号」互相匹配。
///
/// 表内字符是双向：把 `“ ” ‘ ’` 全归一化到 `" " ' '`；同时把 `‟ „` 也归一到 `"`，
/// `‛ ‚` 归一到 `'`。本表与 cc-fork-01 `quoteFold` 集合一致。
pub fn fold_curly_quotes(input: &str) -> Cow<'_, str> {
    if !input
        .chars()
        .any(|c| matches!(c, '“' | '”' | '‟' | '„' | '‘' | '’' | '‛' | '‚'))
    {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '“' | '”' | '‟' | '„' => out.push('"'),
            '‘' | '’' | '‛' | '‚' => out.push('\''),
            other => out.push(other),
        }
    }
    Cow::Owned(out)
}

/// 把不可见 / 易混淆字符 desanitize 到常见 ASCII 等价物，避免「肉眼看不出区别但
/// 字节不一致」的假 NotFound。
///
/// 表内（与 cc-fork-01 `desanitize` 一致）：
/// - NBSP `\u{00A0}` → 普通空格
/// - Zero-width space / joiner / non-joiner / BOM `\u{200B} \u{200C} \u{200D} \u{FEFF}` → 删除
/// - Word joiner `\u{2060}` → 删除
/// - Em / en space `\u{2002} \u{2003}` → 普通空格
/// - Ideographic space `\u{3000}` → 普通空格（注意：会改变 CJK 视觉，但对源代码场景必要）
pub fn desanitize(input: &str) -> Cow<'_, str> {
    let needs = input.chars().any(|c| {
        matches!(
            c,
            '\u{00A0}'
                | '\u{200B}'
                | '\u{200C}'
                | '\u{200D}'
                | '\u{FEFF}'
                | '\u{2060}'
                | '\u{2002}'
                | '\u{2003}'
                | '\u{3000}'
        )
    });
    if !needs {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{3000}' => out.push(' '),
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{2060}' => {} // 删除
            other => out.push(other),
        }
    }
    Cow::Owned(out)
}

/// 一次性应用 normalize 全套（仅用于匹配 working / 段 old_content）。
///
/// 顺序：strip_bom → normalize_to_lf → fold_curly_quotes → desanitize。
pub fn normalize_for_match(input: &str) -> String {
    let (no_bom, _) = strip_bom(input);
    let lf = normalize_to_lf(no_bom);
    let folded = fold_curly_quotes(&lf);
    desanitize(&folded).into_owned()
}

/// 把单字符在「匹配语义」下归一化为零或多个 ASCII char。返回 `None` 表示
/// 该字符**保持原样**（自身的所有字节直接 1:1 拷贝到 normalized 缓冲）。
///
/// **注意**：本函数**不**处理 `\r` / `\r\n`——上层调用方应当先做
/// [`normalize_to_lf`]，否则 CR 会被当成普通字节按 1:1 拷贝（会污染匹配）。
fn normalize_char_for_match(ch: char) -> Option<&'static str> {
    Some(match ch {
        // 不可见字符 / 零宽 → 删除
        '\u{FEFF}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' => "",
        // 各类空格 → ASCII space
        '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{3000}' => " ",
        // curly quotes → ASCII
        '“' | '”' | '‟' | '„' => "\"",
        '‘' | '’' | '‛' | '‚' => "'",
        _ => return None,
    })
}

/// 构造 `(normalized, n_byte_to_orig_byte)` 双轨：
/// - `normalized`：fold_curly_quotes + desanitize 后的副本（**不**做 LF 化，
///   CR 必须由调用方先 [`normalize_to_lf`] 完）。
/// - `map[i]` 表示「normalized 第 i 个字节 起 对应原文的字节偏移」；
///   `map.len() == normalized.len() + 1`，最后一项是「原文末尾的 sentinel」。
///
/// 这样在 normalized 上跑 `match_indices(needle)` 后，命中位置 `n_idx` 可以直接
/// 通过 `map[n_idx]` 与 `map[n_idx + needle.len()]` 取得原文 splice 区间，
/// 而**不**改变原文未编辑字节的 BOM/CRLF/curly-quote 等语义。
pub fn build_normalized_byte_map(input: &str) -> (String, Vec<usize>) {
    let mut out_bytes: Vec<u8> = Vec::with_capacity(input.len());
    let mut map: Vec<usize> = Vec::with_capacity(input.len() + 1);
    let mut orig_byte = 0usize;
    for ch in input.chars() {
        let ch_byte_len = ch.len_utf8();
        match normalize_char_for_match(ch) {
            Some(replacement) => {
                // replacement 全部是 ASCII（" " / "\"" / "'" / ""）
                for b in replacement.bytes() {
                    map.push(orig_byte);
                    out_bytes.push(b);
                }
            }
            None => {
                let mut buf = [0u8; 4];
                let s = ch.encode_utf8(&mut buf);
                for (i, b) in s.as_bytes().iter().enumerate() {
                    map.push(orig_byte + i);
                    out_bytes.push(*b);
                }
            }
        }
        orig_byte += ch_byte_len;
    }
    map.push(orig_byte);
    // out_bytes 由 ASCII 替换 + 原 UTF-8 拷贝拼成，仍是合法 UTF-8。
    let out = String::from_utf8(out_bytes).expect("normalized buffer must be valid UTF-8");
    (out, map)
}

/// 探测扩展名是否需要走专用工具而非纯文本 `edit`。
///
/// 当前仅 `.ipynb`（[edit.md §2.4.4](../../../openspec/specs/architecture/tools/edit.md)
/// 第 4 条）；后续若新增扩展名（如 `.parquet`）在此扩展。
pub fn is_unsupported_structured_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".ipynb")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_bom_detects_and_removes() {
        let (s, had) = strip_bom("\u{FEFF}hello");
        assert!(had);
        assert_eq!(s, "hello");

        let (s, had) = strip_bom("hello");
        assert!(!had);
        assert_eq!(s, "hello");
    }

    #[test]
    fn detect_line_ending_recognizes_pure_styles() {
        assert_eq!(detect_line_ending("a\nb\n"), LineEndingKind::Lf);
        assert_eq!(detect_line_ending("a\r\nb\r\n"), LineEndingKind::CrLf);
        assert_eq!(detect_line_ending("a\rb\r"), LineEndingKind::Cr);
        assert_eq!(detect_line_ending("noeol"), LineEndingKind::Lf);
    }

    #[test]
    fn detect_line_ending_recognizes_mixed() {
        assert_eq!(detect_line_ending("a\r\nb\nc"), LineEndingKind::Mixed);
    }

    #[test]
    fn normalize_to_lf_borrows_when_no_cr() {
        let cow = normalize_to_lf("plain\nlf\n");
        assert!(matches!(cow, Cow::Borrowed(_)));
    }

    #[test]
    fn normalize_to_lf_collapses_crlf_and_cr() {
        let cow = normalize_to_lf("a\r\nb\rc\nd");
        assert_eq!(cow.as_ref(), "a\nb\nc\nd");
    }

    #[test]
    fn restore_line_endings_roundtrip() {
        let original = "a\r\nb\r\nc";
        let kind = detect_line_ending(original);
        let working = normalize_to_lf(original);
        // 假设我们在 working 上不做编辑
        let restored = restore_line_endings(kind, &working);
        assert_eq!(restored.as_ref(), original);
    }

    #[test]
    fn restore_line_endings_mixed_does_nothing() {
        let cow = restore_line_endings(LineEndingKind::Mixed, "a\nb");
        assert!(matches!(cow, Cow::Borrowed(_)));
    }

    #[test]
    fn fold_curly_quotes_handles_double_and_single() {
        let s = fold_curly_quotes("“double” and ‘single’");
        assert_eq!(s.as_ref(), "\"double\" and 'single'");
    }

    #[test]
    fn fold_curly_quotes_borrows_when_no_change_needed() {
        let s = fold_curly_quotes("\"plain\"");
        assert!(matches!(s, Cow::Borrowed(_)));
    }

    #[test]
    fn desanitize_replaces_invisible_and_nbsp() {
        let s = desanitize("a\u{00A0}b\u{200B}c\u{2060}d\u{3000}e");
        assert_eq!(s.as_ref(), "a bcd e");
    }

    #[test]
    fn desanitize_borrows_when_no_change_needed() {
        let s = desanitize("plain text");
        assert!(matches!(s, Cow::Borrowed(_)));
    }

    #[test]
    fn normalize_for_match_runs_full_pipeline() {
        let s = normalize_for_match("\u{FEFF}“hi”\r\n\u{00A0}world");
        assert_eq!(s, "\"hi\"\n world");
    }

    #[test]
    fn is_unsupported_structured_file_recognizes_ipynb() {
        assert!(is_unsupported_structured_file("notebook.ipynb"));
        assert!(is_unsupported_structured_file("/abs/path/x.IPYNB"));
        assert!(!is_unsupported_structured_file("file.py"));
        assert!(!is_unsupported_structured_file("ipynb.txt"));
    }
}
