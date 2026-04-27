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
/// 判定规则（与 plan §7 对齐）：
///
/// 1. `shell-words::split(line)` 取出所有 token；
/// 2. 取出形如 `/...`、`~/...` 的 token 视为路径 token；
/// 3. 路径 token 数量 = 0 → [`DragOutcome::None`]；
/// 4. 全部 token 都是路径 token → [`DragOutcome::PromptMenu`]；
/// 5. 否则 → [`DragOutcome::AutoAllow`]。
///
/// 注意：**仅基于字符串前缀判定**，不做存在性校验（让 LLM 拿到不存在的路径也得告警，
/// 而不是被这里悄悄丢掉）；存在性 + 实际授权由 menu 或 grant 阶段处理。
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
    let mut all_paths = true;
    for tok in &tokens {
        if is_path_token(tok) {
            let pb = PathBuf::from(tok);
            if !paths.contains(&pb) {
                paths.push(pb);
            }
        } else {
            all_paths = false;
        }
    }

    if paths.is_empty() {
        return DragOutcome::None;
    }
    if all_paths {
        DragOutcome::PromptMenu {
            paths,
            original_line: trimmed.to_string(),
        }
    } else {
        DragOutcome::AutoAllow {
            paths,
            original_line: trimmed.to_string(),
        }
    }
}

/// 路径 token 判定：以 `/` 或 `~/` 开头，且长度 > 1。
fn is_path_token(tok: &str) -> bool {
    if tok == "/" || tok == "~" {
        return false;
    }
    tok.starts_with('/') || tok.starts_with("~/")
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

    /// 命中 builtin deny —— 仅放 `[d]/[c]`，提醒用户该路径被默认安全规则保护。
    pub fn deny_only(note: impl Into<String>) -> Self {
        Self {
            allow_once: false,
            persist_extra_root: false,
            persist_readonly: false,
            persist_deny: true,
            cancel: true,
            note: Some(note.into()),
        }
    }

    /// 命中 readonly path_rule —— 仅放 `[r]/[d]/[c]`。
    pub fn readonly_only(note: impl Into<String>) -> Self {
        Self {
            allow_once: false,
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
/// - 命中 `Deny` —— 仅 `[d]/[c]`，警告"此路径受默认安全规则保护"；
/// - 命中 `PathRuleReadOnly` —— 仅 `[r]/[d]/[c]`，提示"此路径已是只读规则"；
/// - 其它 —— 全 5 选项。
pub fn render_drag_menu(path: &std::path::Path, gate: &dyn PermissionGate) -> MenuOptions {
    use crate::core::primitives::PrimitiveOperation;

    let probe = gate.check(PrimitiveOperation::Read, &path.to_string_lossy());
    match probe {
        Ok(PermissionDecision::Deny { .. }) => MenuOptions::deny_only(format!(
            "此路径受默认安全规则保护，仅允许 [d] 加入禁止 / [c] 取消（路径: {}）",
            path.display()
        )),
        Ok(PermissionDecision::Allow {
            source: crate::core::permission::GrantSource::PathRuleReadOnly,
            ..
        }) => MenuOptions::readonly_only(format!(
            "此路径已是只读规则，[a]/[w] 不会改变其只读状态（路径: {}）",
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
    fn menu_options_deny_only_only_d_and_c() {
        let m = MenuOptions::deny_only("note");
        assert!(!m.allow_once && !m.persist_extra_root && !m.persist_readonly);
        assert!(m.persist_deny && m.cancel);
        assert!(m.note.is_some());
    }

    #[test]
    fn menu_options_readonly_only_r_d_c() {
        let m = MenuOptions::readonly_only("note");
        assert!(!m.allow_once && !m.persist_extra_root);
        assert!(m.persist_readonly && m.persist_deny && m.cancel);
    }
}
