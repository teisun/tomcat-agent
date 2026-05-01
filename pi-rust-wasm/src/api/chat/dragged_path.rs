//! # 拖拽路径解析
//!
//! 用户从终端拖入文件/目录时，shell 会把路径插入命令行。本模块只识别一种拖拽授权语义：
//!
//! - **整行只有路径 token（纯拖拽）**：例如「/Users/yan/proj/foo」会弹出授权菜单。
//! - **路径 + 意图文字**：例如「帮我看下 /Users/yan/proj/foo」或
//!   `'/abs/path'看下里面`，完全等同普通聊天输入，不在拖拽层自动授权。
//!
//! 后续 read/write/bash 是否允许，由 LLM 触发工具调用时的 `PermissionGate` 统一决定。
//!
//! ## 模块边界
//!
//! - 本模块只做**解析**与**菜单数据结构**，不直接操作 `SessionGrants` /
//!   `pi.config.toml`。具体落盘由 [`crate::infra::config::append`] 系列函数承担。
//! - 实际接入 chat_loop 时由调用方决定：`DragOutcome::None` 直接走普通对话，
//!   `DragOutcome::PromptMenu` 与用户做一轮授权交互。

use std::path::PathBuf;

use crate::core::permission::{PathRuleMode, PermissionDecision, PermissionGate};

/// 拖拽解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DragOutcome {
    /// 非纯路径行 —— 调用方按普通输入处理。
    None,
    /// 整行只是一个或多个纯路径 → 弹 TUI 5 选项菜单。
    PromptMenu {
        paths: Vec<PathBuf>,
        original_line: String,
    },
}

/// 解析一行用户输入，只在整行都是路径 token 时返回授权菜单。
///
/// 判定规则：
///
/// 1. `shell-words::split(line)` 取出所有 token；
/// 2. 所有 token 都必须是合法路径 token；
/// 3. 只要任一 token 是普通文字或「路径 + 意图」混合形态，就返回 [`DragOutcome::None`]。
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
    for tok in &tokens {
        if !is_valid_pure_path_token(tok) {
            return DragOutcome::None;
        }
        let pb = PathBuf::from(tok);
        if !paths.contains(&pb) {
            paths.push(pb);
        }
    }

    if paths.is_empty() {
        return DragOutcome::None;
    }
    DragOutcome::PromptMenu {
        paths,
        original_line: trimmed.to_string(),
    }
}

/// 路径前缀判定：以 `/` 或 `~/` 开头，且长度 > 1。
///
/// 仅用作快速过滤；真正的纯路径合法性由 [`is_valid_pure_path_token`] 决定。
fn is_path_prefix_token(tok: &str) -> bool {
    if tok == "/" || tok == "~" {
        return false;
    }
    tok.starts_with('/') || tok.starts_with("~/")
}

fn is_valid_pure_path_token(tok: &str) -> bool {
    if !is_path_prefix_token(tok) {
        return false;
    }
    if std::path::Path::new(tok).exists() {
        return true;
    }
    tok.is_ascii()
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
    fn mixed_line_returns_none() {
        let out = interpret_dragged_paths("帮我看下 /Users/yan/proj/foo");
        assert_eq!(out, DragOutcome::None);
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
    fn render_drag_menu_with_deny_rule_hides_authorization_choices() {
        use crate::core::permission::{
            DefaultPermissionGate, DraggedPaths, GateConfig, PathRule, SessionGrants,
        };

        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        let denied = tmp.path().join("secret");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&denied).unwrap();
        let gate = DefaultPermissionGate::new(
            GateConfig {
                agent_definition_dir: workspace,
                extra_roots: vec![],
                agent_data_readonly_dirs: vec![],
                user_path_rules: vec![PathRule::new(
                    denied.to_string_lossy().to_string(),
                    PathRuleMode::Deny,
                )],
                user_bash_forbidden: vec![],
                user_bash_approval: vec![],
                auto_confirm: false,
            },
            SessionGrants::new(),
            DraggedPaths::new(),
        );

        let menu = render_drag_menu(&denied, &gate);

        assert!(menu.cancel);
        assert!(!menu.allow_once, "deny 命中后不得允许本次授权");
        assert!(!menu.persist_extra_root, "deny 命中后不得允许持久扩权");
        assert!(
            !menu.persist_readonly,
            "deny 命中后不得降级为 readonly 扩权"
        );
        assert!(!menu.persist_deny, "deny 命中后无需再展示重复 deny 选项");
        assert!(menu.note.as_deref().unwrap_or("").contains("禁止读写访问"));
    }

    #[test]
    fn menu_options_readonly_allows_session_read_but_not_extra_root() {
        let m = MenuOptions::readonly_only("note");
        assert!(m.allow_once);
        assert!(!m.persist_extra_root);
        assert!(m.persist_readonly && m.persist_deny && m.cancel);
    }

    #[test]
    fn quoted_path_with_intent_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().to_string_lossy().to_string();
        let line = format!("'{}'这个文件夹下面有几个文件?", real);
        assert_eq!(interpret_dragged_paths(&line), DragOutcome::None);
    }

    #[test]
    fn quoted_path_with_space_and_intent_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("My Documents").join("foo");
        std::fs::create_dir_all(&nested).unwrap();
        let real = nested.to_string_lossy().to_string();
        let line = format!("\"{}\" 帮我看下", real);
        assert_eq!(interpret_dragged_paths(&line), DragOutcome::None);
    }

    #[test]
    fn nonexistent_ascii_path_keeps_prompt_menu() {
        // 全 ASCII 不存在路径仍可作为用户想授权的纯路径。
        let out = interpret_dragged_paths("/etc/foo/nonexistent");
        match out {
            DragOutcome::PromptMenu { paths, .. } => {
                assert_eq!(paths, vec![PathBuf::from("/etc/foo/nonexistent")]);
            }
            other => panic!("expected PromptMenu, got {:?}", other),
        }
    }

    #[test]
    fn nonascii_token_without_existence_returns_none() {
        assert_eq!(interpret_dragged_paths("/abs/path中文"), DragOutcome::None);
    }
}
