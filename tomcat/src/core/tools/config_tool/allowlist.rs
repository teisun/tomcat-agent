//! 键级白名单 / 硬黑名单（plan §6.2）。

/// 读白名单：精确匹配（点号路径）；只有精确命中才允许读。
const CONFIG_READ_ALLOWLIST: &[&str] = &[
    "workspace",
    "workspace.workspace_roots",
    "workspace.entries",
    "agent.id",
    "agent.workspace",
    "agent.agent_dir",
    "primitive.path_rules",
    "primitive.bash_approval_required",
    "primitive.bash_forbidden",
    "primitive.auto_confirm",
    "llm.default_model",
    "llm.provider",
    "context.context_window",
    "context.max_output_tokens",
    "context.keep_recent_turns",
    "context.compaction_model",
    "context.current_tail_compactable_min_chars",
    "context.current_tail_single_result_max_chars",
    "context.compaction_max_tokens",
    "log.level",
    "preflight.auto_install_search_tools",
    "preflight.show_search_tools_ui",
    "preflight.show_git_ui",
];

/// 读硬黑名单：通配前缀，优先级高于 [`CONFIG_READ_ALLOWLIST`]，即使误列也会被拦。
const CONFIG_HARDCODED_READ_DENY: &[&str] = &[
    "llm.api_key_env",
    "llm.api_key",
    "llm.api_base",
    "llm.api_base_fallback",
    "llm.proxy",
    "security.",
    "storage.",
];

/// 写白名单：精确匹配；其余字段一律拒绝。所有数组字段语义为「单元素追加」。
const CONFIG_WRITE_ALLOWLIST: &[&str] = &[
    "workspace.workspace_roots",
    "workspace.entries",
    "primitive.path_rules",
    "primitive.bash_approval_required",
    "primitive.bash_forbidden",
    "llm.default_model",
    "log.level",
    "preflight.auto_install_search_tools",
    "preflight.show_search_tools_ui",
    "preflight.show_git_ui",
    "context.keep_recent_turns",
    "context.current_tail_compactable_min_chars",
    "context.current_tail_single_result_max_chars",
    "context.compaction_max_tokens",
];

/// 写硬黑名单：优先级高于 [`CONFIG_WRITE_ALLOWLIST`]；任何敏感 / 自我提权字段一律拒绝。
const CONFIG_HARDCODED_WRITE_DENY: &[&str] = &[
    "llm.",
    "security.",
    "storage.",
    "agent.id",
    "agent.workspace",
    "agent.agent_dir",
    "primitive.auto_confirm",
    "primitive.path_whitelist",
    "primitive.bash_whitelist",
    "primitive.auto_confirm_whitelist",
];

/// 数组字段集合（单元素追加语义）；其余白名单内字段视为标量替换。
const ARRAY_FIELDS: &[&str] = &[
    "workspace.workspace_roots",
    "workspace.entries",
    "primitive.path_rules",
    "primitive.bash_approval_required",
    "primitive.bash_forbidden",
];

fn matches_prefix_list(key: &str, list: &[&str]) -> bool {
    list.iter().any(|p| {
        if let Some(stripped) = p.strip_suffix('.') {
            key.starts_with(stripped)
                && key
                    .get(stripped.len()..)
                    .is_some_and(|r| r.starts_with('.'))
                || key == stripped
        } else if p.ends_with('.') {
            key.starts_with(p)
        } else {
            // 也作为前缀（用于 `llm.` 等以 dot 结尾的 hardcoded deny）
            // 这里的入口就是 `llm.`、`security.` 等，已在上面分支处理；
            // 兜底退化为完全相等（保持与精确匹配一致）。
            key == *p
        }
    })
}

fn matches_exact_list(key: &str, list: &[&str]) -> bool {
    list.contains(&key)
}

/// 是否允许通过 `config_get` 读取 `key`。
pub fn is_readable(key: &str) -> bool {
    if matches_prefix_list(key, CONFIG_HARDCODED_READ_DENY) {
        return false;
    }
    matches_exact_list(key, CONFIG_READ_ALLOWLIST)
}

/// 是否允许通过 `config_set` 写入 `key`。
pub fn is_writable(key: &str) -> bool {
    if matches_prefix_list(key, CONFIG_HARDCODED_WRITE_DENY) {
        return false;
    }
    matches_exact_list(key, CONFIG_WRITE_ALLOWLIST)
}

/// 是否为「单元素追加」语义的数组字段。
pub fn is_array_field(key: &str) -> bool {
    ARRAY_FIELDS.contains(&key)
}
