//! # 拖拽路径解析（plan §7）
//!
//! 用户从终端拖入文件/目录时，shell 会把绝对路径插入命令行。本模块负责区分两种语义：
//!
//! - **行内含意图（拖拽 + 文字）**：例如「帮我看下 /Users/yan/proj/foo」。
//!   把所有路径加进 `SessionGrants`（AllowOnce），原行原样发给 LLM。
//! - **整行只有路径 token（纯拖拽）**：例如「/Users/yan/proj/foo」。
//!   弹一个 5 选项 TUI 菜单，让用户选择如何持久化授权范围。
//!
//! 详细设计与流程图见 plan §7。
//!
//! ## 模块边界
//!
//! - 本模块只做**解析**与**菜单数据结构**，不直接操作 `SessionGrants` /
//!   `pi.config.toml`。具体落盘由 [`crate::infra::config::append`] 系列函数承担。
//! - 实际接入 chat_loop 时由调用方决定：基于 `DragOutcome` 选择走 AutoAllow（直接发
//!   LLM）还是 PromptMenu（与用户做一轮交互）。

use std::path::PathBuf;

use crate::core::permission::{PathRuleMode, PermissionDecision, PermissionGate};

/// 拖拽解析结果（三态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DragOutcome {
    /// 行内未发现任何路径 token —— 调用方按普通输入处理。
    None,
    /// 行内含意图文字 + 一个或多个路径 → 路径自动 AllowOnce；原行发给 LLM。
    AutoAllow {
        /// 命中的路径（去重后保留输入顺序）。
        paths: Vec<PathBuf>,
        /// 原输入行（可直接发 LLM）。
        original_line: String,
    },
    /// 整行只是一个或多个纯路径 → 弹 TUI 5 选项菜单。
    PromptMenu {
        paths: Vec<PathBuf>,
        original_line: String,
    },
}

/// 解析一行用户输入，区分「拖拽 + 文字」/「纯拖拽」/「无路径」。
///
/// 判定规则（plan §7 + hotfix §B 修正）：
///
/// 1. `shell-words::split(line)` 取出所有 token；
/// 2. 形如 `/...`、`~/...` 的 token 走 [`split_path_and_suffix`] 在 token 内做
///    「存在性 + 字符边界切分」 —— 因为 POSIX shell 单引号闭合后紧贴的字符仍属
///    同一 token（例：`'foo''bar'` = `foobar`），所以 `'/abs/path'紧贴中文意图`
///    会被合成单 token；纯前缀判断会把整个 token 当路径，导致用户「拖拽 + 中文意图」
///    误入纯拖拽 5 选项菜单。
/// 3. 切分结果：
///    - `(path, "")` —— token 整体是有效路径，path 计入候选；
///    - `(path, suffix)` —— token 由真实存在路径 + 非 ASCII suffix 组成，
///      path 计入候选，同时 `has_intent_text = true`（消除 §7 原 bug）；
///    - `None` —— token 不像路径（`/` 单字符 / 纯 ASCII 不存在路径仍按
///      [`DragOutcome::PromptMenu`] 旧规则处理）。
/// 4. 路径 token 数 = 0 → [`DragOutcome::None`]；
///    `has_intent_text = false` → [`DragOutcome::PromptMenu`]；
///    否则 → [`DragOutcome::AutoAllow`]。
///
/// 注意：本函数**不**对路径做归一化或权限决策，仅做字符串/存在性级别的分类。
/// 实际授权 / 持久化由 menu 阶段或 [`crate::core::permission`] 处理。
pub fn interpret_dragged_paths(line: &str) -> DragOutcome {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return DragOutcome::None;
    }

    let tokens = match shell_words::split(trimmed) {
        Ok(t) => t,
        Err(_) => return DragOutcome::None,
    };
    if tokens.is_empty() {
        return DragOutcome::None;
    }

    let mut paths = Vec::new();
    let mut has_intent_text = false;
    for tok in &tokens {
        if !is_path_prefix_token(tok) {
            // 普通文字 token（如 "帮我看下"）。
            has_intent_text = true;
            continue;
        }

        match split_path_and_suffix(tok) {
            Some((path, suffix)) => {
                let pb = PathBuf::from(path);
                if !paths.contains(&pb) {
                    paths.push(pb);
                }
                if !suffix.is_empty() {
                    // 路径后紧贴 suffix（多为非 ASCII 中文意图）—— hotfix §B
                    // 关键修正点：此时 `has_intent_text = true`，避免误入纯拖拽菜单。
                    has_intent_text = true;
                }
            }
            None => {
                // 看起来像路径但既不存在又是 ASCII 边角 case（例如 `/` 单独出现）；
                // 这里不计入 paths，但视作普通文字。
                has_intent_text = true;
            }
        }
    }

    if paths.is_empty() {
        return DragOutcome::None;
    }
    if has_intent_text {
        DragOutcome::AutoAllow {
            paths,
            original_line: trimmed.to_string(),
        }
    } else {
        DragOutcome::PromptMenu {
            paths,
            original_line: trimmed.to_string(),
        }
    }
}

/// 路径前缀判定：以 `/` 或 `~/` 开头，且长度 > 1。
///
/// 仅用作快速过滤；真正的路径合法性由 [`split_path_and_suffix`] 决定。
fn is_path_prefix_token(tok: &str) -> bool {
    if tok == "/" || tok == "~" {
        return false;
    }
    tok.starts_with('/') || tok.starts_with("~/")
}

/// Token 内「存在性 + 字符边界」切分 —— hotfix §B.3：
///
/// 1. 整个 token 在文件系统上存在 → 返回 `(token, "")`，case A；
/// 2. token 不存在 + 含非 ASCII 字符 → 从右往左按 char 边界逐次缩短 path，
///    直到找到一个真实存在的前缀；命中则返回 `(prefix, suffix)`，case B；
/// 3. token 不存在 + 纯 ASCII → 兼容 plan §7 旧行为，整段视作路径 token，
///    返回 `(token, "")`，case C；
/// 4. 极端情况（无任何 char 边界匹配）→ `None`，case D，调用方按文字处理。
///
/// 区分 ASCII / 非 ASCII 的原因（hotfix §B.4）：shell-words 的 token 切分依赖
/// 空白；非 ASCII 字符不是空白，单引号闭合后会被并进同一 token，这是「拖拽 +
/// 中文意图」的典型外观；ASCII 之间必有空格分隔，所以 ASCII 不存在路径保留旧
/// 行为可避免回归。
pub fn split_path_and_suffix(tok: &str) -> Option<(String, String)> {
    let path_obj = std::path::Path::new(tok);
    if path_obj.exists() {
        return Some((tok.to_string(), String::new()));
    }

    if tok.is_ascii() {
        // case C：兼容旧行为（用户敲不存在但合法的纯 ASCII 路径）。
        return Some((tok.to_string(), String::new()));
    }

    // case B：从右往左按 char 边界剥离 suffix，找最长存在前缀。
    let chars: Vec<char> = tok.chars().collect();
    for end in (1..chars.len()).rev() {
        let prefix: String = chars[..end].iter().collect();
        // 至少要 > 1 字符且仍以路径前缀开头，避免把 `/` 当合法 path。
        if prefix.len() <= 1 {
            break;
        }
        if !is_path_prefix_token(&prefix) {
            break;
        }
        let prefix_path = std::path::Path::new(&prefix);
        if prefix_path.exists() {
            let suffix: String = chars[end..].iter().collect();
            if prefix_path.is_dir() {
                let suffix_starts_like_path_component = suffix
                    .chars()
                    .next()
                    .is_some_and(|c| c == std::path::MAIN_SEPARATOR || c.is_ascii());
                if suffix_starts_like_path_component {
                    return None;
                }
            }
            return Some((prefix, suffix));
        }
    }

    None
}

/// TUI 菜单可用选项集合。`render_drag_menu` 根据 path_rule 预检查结果裁剪。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuOptions {
    /// `[a]` 本会话允许（SessionGrant）。
    pub allow_once: bool,
    /// `[w]` 加入工作区持久化（extra_roots）。
    pub persist_extra_root: bool,
    /// `[r]` 加入只读规则（path_rules readonly）。
    pub persist_readonly: bool,
    /// `[d]` 加入禁止规则（path_rules deny）。
    pub persist_deny: bool,
    /// `[c]` 取消，按聊天处理。
    pub cancel: bool,
    /// 菜单顶部的提示信息（builtin deny / readonly 命中时给出说明）。
    pub note: Option<String>,
}

impl MenuOptions {
    /// 5 选项全开（默认场景）。
    pub fn full() -> Self {
        Self {
            allow_once: true,
            persist_extra_root: true,
            persist_readonly: true,
            persist_deny: true,
            cancel: true,
            note: None,
        }
    }

    /// 命中 deny —— 不再显示任何授权选项，只允许取消。
    pub fn deny_only(note: impl Into<String>) -> Self {
        Self {
            allow_once: false,
            persist_extra_root: false,
            persist_readonly: false,
            persist_deny: false,
            cancel: true,
            note: Some(note.into()),
        }
    }

    /// 命中 readonly path_rule —— 允许确认本次读取，但不允许持久写入工作区。
    pub fn readonly_only(note: impl Into<String>) -> Self {
        Self {
            allow_once: true,
            persist_extra_root: false,
            persist_readonly: true,
            persist_deny: true,
            cancel: true,
            note: Some(note.into()),
        }
    }
}

/// 基于 path_rules 预检查决定可用菜单选项（plan §7）。
///
/// 用 [`PermissionGate::check`] 模拟一次 read 操作：
///
/// - 命中 `Deny` —— 仅 `[c]`，警告"此路径已被禁止访问"；
/// - 命中 `PathRuleReadOnly` —— `[a]/[r]/[d]/[c]`，不允许 `[w]`；
/// - 其它 —— 全 5 选项。
pub fn render_drag_menu(path: &std::path::Path, gate: &dyn PermissionGate) -> MenuOptions {
    use crate::core::primitives::PrimitiveOperation;

    let probe = gate.check(PrimitiveOperation::Read, &path.to_string_lossy());
    match probe {
        Ok(PermissionDecision::Deny { .. }) => {
            MenuOptions::deny_only(format!("该路径已被禁止读写访问：{}", path.display()))
        }
        Ok(PermissionDecision::Allow {
            source: crate::core::permission::GrantSource::PathRuleReadOnly,
            ..
        }) => MenuOptions::readonly_only(format!(
            "这是只读路径，本次会话可以读取其中内容，但不能写入、修改或删除：{}",
            path.display()
        )),
        _ => MenuOptions::full(),
    }
}

/// 用户在 TUI 菜单上选择的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuChoice {
    /// `[a]` SessionGrant（仅本会话）。
    AllowOnce,
    /// `[w]` 加入 `[workspace] extra_roots`（持久化）。
    PersistExtraRoot,
    /// `[r]` 追加 `path_rules` `readonly` 规则。
    PersistReadonly,
    /// `[d]` 追加 `path_rules` `deny` 规则。
    PersistDeny,
    /// `[c]` 取消，按聊天处理。
    Cancel,
}

impl MenuChoice {
    pub fn from_input(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "a" | "allow" | "allow_once" => Some(Self::AllowOnce),
            "w" | "workspace" | "persist" => Some(Self::PersistExtraRoot),
            "r" | "readonly" => Some(Self::PersistReadonly),
            "d" | "deny" => Some(Self::PersistDeny),
            "c" | "cancel" => Some(Self::Cancel),
            _ => None,
        }
    }

    /// 映射到 `path_rules` 模式（仅 readonly / deny 有意义）。
    pub fn as_rule_mode(self) -> Option<PathRuleMode> {
        match self {
            Self::PersistReadonly => Some(PathRuleMode::Readonly),
            Self::PersistDeny => Some(PathRuleMode::Deny),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_line_returns_none() {
        assert_eq!(interpret_dragged_paths(""), DragOutcome::None);
        assert_eq!(interpret_dragged_paths("   "), DragOutcome::None);
    }

    #[test]
    fn line_without_path_tokens_returns_none() {
        assert_eq!(interpret_dragged_paths("hello world"), DragOutcome::None);
        assert_eq!(interpret_dragged_paths("帮我写一段代码"), DragOutcome::None);
    }

    #[test]
    fn pure_drag_single_path_returns_prompt_menu() {
        let out = interpret_dragged_paths("/Users/yan/proj/foo");
        match out {
            DragOutcome::PromptMenu { paths, .. } => {
                assert_eq!(paths, vec![PathBuf::from("/Users/yan/proj/foo")]);
            }
            other => panic!("expected PromptMenu, got {:?}", other),
        }
    }

    #[test]
    fn pure_drag_multiple_paths_returns_prompt_menu() {
        let out = interpret_dragged_paths("/Users/yan/proj/foo  /etc/hosts");
        match out {
            DragOutcome::PromptMenu { paths, .. } => {
                assert_eq!(
                    paths,
                    vec![
                        PathBuf::from("/Users/yan/proj/foo"),
                        PathBuf::from("/etc/hosts"),
                    ]
                );
            }
            other => panic!("expected PromptMenu, got {:?}", other),
        }
    }

    #[test]
    fn drag_plus_intent_returns_auto_allow() {
        let out = interpret_dragged_paths("帮我看下 /Users/yan/proj/foo");
        match out {
            DragOutcome::AutoAllow {
                paths,
                original_line,
            } => {
                assert_eq!(paths, vec![PathBuf::from("/Users/yan/proj/foo")]);
                assert_eq!(original_line, "帮我看下 /Users/yan/proj/foo");
            }
            other => panic!("expected AutoAllow, got {:?}", other),
        }
    }

    #[test]
    fn tilde_path_recognized() {
        let out = interpret_dragged_paths("~/proj/foo");
        match out {
            DragOutcome::PromptMenu { paths, .. } => {
                assert_eq!(paths, vec![PathBuf::from("~/proj/foo")]);
            }
            other => panic!("expected PromptMenu, got {:?}", other),
        }
    }

    #[test]
    fn solo_slash_or_tilde_not_path_token() {
        assert_eq!(interpret_dragged_paths("/"), DragOutcome::None);
        assert_eq!(interpret_dragged_paths("~"), DragOutcome::None);
    }

    #[test]
    fn duplicate_paths_deduped() {
        let out = interpret_dragged_paths("/a /a /a");
        match out {
            DragOutcome::PromptMenu { paths, .. } => {
                assert_eq!(paths, vec![PathBuf::from("/a")]);
            }
            other => panic!("expected PromptMenu, got {:?}", other),
        }
    }

    #[test]
    fn quoted_path_with_space_recognized() {
        let out = interpret_dragged_paths("\"/Users/yan/My Documents/foo\"");
        match out {
            DragOutcome::PromptMenu { paths, .. } => {
                assert_eq!(paths, vec![PathBuf::from("/Users/yan/My Documents/foo")]);
            }
            other => panic!("expected PromptMenu, got {:?}", other),
        }
    }

    #[test]
    fn menu_choice_parses_inputs() {
        assert_eq!(MenuChoice::from_input("a"), Some(MenuChoice::AllowOnce));
        assert_eq!(
            MenuChoice::from_input("W"),
            Some(MenuChoice::PersistExtraRoot)
        );
        assert_eq!(
            MenuChoice::from_input("r"),
            Some(MenuChoice::PersistReadonly)
        );
        assert_eq!(MenuChoice::from_input("d"), Some(MenuChoice::PersistDeny));
        assert_eq!(MenuChoice::from_input("c"), Some(MenuChoice::Cancel));
        assert_eq!(MenuChoice::from_input(""), None);
        assert_eq!(MenuChoice::from_input("xyz"), None);
    }

    #[test]
    fn menu_options_full_has_all_options() {
        let m = MenuOptions::full();
        assert!(m.allow_once && m.persist_extra_root && m.persist_readonly && m.persist_deny);
        assert!(m.note.is_none());
    }

    #[test]
    fn menu_options_deny_only_only_cancel() {
        let m = MenuOptions::deny_only("note");
        assert!(!m.allow_once && !m.persist_extra_root && !m.persist_readonly);
        assert!(!m.persist_deny && m.cancel);
        assert!(m.note.is_some());
    }

    #[test]
    fn menu_options_readonly_allows_session_read_but_not_extra_root() {
        let m = MenuOptions::readonly_only("note");
        assert!(m.allow_once);
        assert!(!m.persist_extra_root);
        assert!(m.persist_readonly && m.persist_deny && m.cancel);
    }

    // ── hotfix §B：split_path_and_suffix + 拖拽紧贴中文意图回归 ──

    #[test]
    fn split_path_and_suffix_existing_returns_full_token() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().to_string();
        let res = split_path_and_suffix(&dir).expect("existing dir should split");
        assert_eq!(res.0, dir);
        assert!(res.1.is_empty());
    }

    #[test]
    fn split_path_and_suffix_nonexistent_ascii_keeps_legacy() {
        // 全 ASCII 不存在路径：保留旧行为（视作整体路径，suffix 空）。
        let res = split_path_and_suffix("/etc/foo/nonexistent")
            .expect("ASCII nonexistent should still classify as path");
        assert_eq!(res.0, "/etc/foo/nonexistent");
        assert!(res.1.is_empty());
    }

    #[test]
    fn split_path_and_suffix_nonascii_suffix_splits_correctly() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().to_string_lossy().to_string();
        let mixed = format!("{}这个项目下面", real);
        let (path, suffix) =
            split_path_and_suffix(&mixed).expect("nonascii suffix should split off");
        assert_eq!(path, real);
        assert_eq!(suffix, "这个项目下面");
    }

    #[test]
    fn split_path_and_suffix_missing_file_with_intent_does_not_authorize_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.png");
        let mixed = format!("{}看下", missing.to_string_lossy());
        assert_eq!(
            split_path_and_suffix(&mixed),
            None,
            "不存在文件 + 中文意图不得回退到父目录"
        );
    }

    #[test]
    fn split_path_and_suffix_nonascii_existing_path_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("项目").join("foo");
        std::fs::create_dir_all(&nested).unwrap();
        let s = nested.to_string_lossy().to_string();
        let (path, suffix) =
            split_path_and_suffix(&s).expect("existing nonascii path should split");
        assert_eq!(path, s);
        assert!(suffix.is_empty(), "整体存在的非 ASCII 路径应保留完整");
    }

    #[test]
    fn quoted_path_with_intent_text_returns_auto_allow() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().to_string_lossy().to_string();
        let line = format!("'{}'这个文件夹下面有几个文件?", real);
        match interpret_dragged_paths(&line) {
            DragOutcome::AutoAllow { paths, .. } => {
                assert_eq!(paths, vec![PathBuf::from(real)]);
            }
            other => panic!(
                "带引号路径 + 紧贴中文意图应识别为 AutoAllow，得到 {:?}",
                other
            ),
        }
    }

    #[test]
    fn quoted_path_with_space_and_intent_returns_auto_allow() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("My Documents").join("foo");
        std::fs::create_dir_all(&nested).unwrap();
        let real = nested.to_string_lossy().to_string();
        let line = format!("\"{}\" 帮我看下", real);
        match interpret_dragged_paths(&line) {
            DragOutcome::AutoAllow { paths, .. } => {
                assert_eq!(paths, vec![PathBuf::from(real)]);
            }
            other => panic!("expected AutoAllow, got {:?}", other),
        }
    }

    #[test]
    fn nonexistent_ascii_path_keeps_prompt_menu() {
        // 全 ASCII 不存在路径：仍走 PromptMenu（plan §7 兼容性）。
        let out = interpret_dragged_paths("/etc/foo/nonexistent");
        match out {
            DragOutcome::PromptMenu { paths, .. } => {
                assert_eq!(paths, vec![PathBuf::from("/etc/foo/nonexistent")]);
            }
            other => panic!("expected PromptMenu, got {:?}", other),
        }
    }
}
