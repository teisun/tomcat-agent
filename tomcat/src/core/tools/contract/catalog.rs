//! # 内置工具 catalog
//!
//! 这里是内置工具描述的单一事实源。LLM function schema、system prompt 工具清单
//! 与 `docs/tool-catalog.md` 都从这里派生，避免多处手写后漂移。
//!
//! ## 描述 vs 跨工具规则（成功率红线）
//!
//! `description` 与参数 schema 只保留"影响 LLM 正确/成功调用工具"的用法约束
//! （精确格式、枚举、互斥、唯一性、坑点）；跨工具行为规则（read-before-edit、
//! 别粘显示前缀、优先 search_files、`path:line` 引用、别用 codeblock 假编辑等）
//! 下沉到各条目的 [`BuiltinToolCatalogEntry::prompt_guidelines`]，
//! 由 [`render_tool_guidelines_with_policy`] 聚合去重后**只说一遍**，注入
//! `system/tool_instructions.txt` 的 `{tool_guidelines}` 占位。
//! （UI 从 UX 出发的 #8 原则常驻 `system/core_identity.txt` 与 plan `planner.txt`，不在此。）

use serde_json::Value;

use crate::core::permission::PermissionScope;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolCategory {
    Filesystem,
    Exec,
    Config,
}

impl ToolCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            ToolCategory::Filesystem => "filesystem",
            ToolCategory::Exec => "exec",
            ToolCategory::Config => "config",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            ToolCategory::Filesystem => "Filesystem",
            ToolCategory::Exec => "Exec",
            ToolCategory::Config => "Config",
        }
    }
}

pub struct BuiltinToolCatalogEntry {
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub display_summary: Option<&'static str>,
    pub parameters: fn() -> Value,
    pub scope: PermissionScope,
    pub category: Option<ToolCategory>,
    pub read_only: bool,
    pub destructive: bool,
    pub search_hint: Option<&'static str>,
    /// 跨工具行为规则（不影响单次调用成功率的那部分）。由
    /// [`render_tool_guidelines_with_policy`] 聚合去重后注入 `tool_instructions.txt`，
    /// 避免在 `description` 里逐工具重复。空切片表示该工具无额外跨工具规则。
    pub prompt_guidelines: &'static [&'static str],
    /// PLAN 模式专属工具（`create_plan` / `update_plan` / `todos` / `ask_question`）。
    /// 默认 `false`：进入 chat_loop 默认 LLM 工具集；`true` 时：
    /// - 工具仍在 `BUILTIN_TOOL_CATALOG` 中（保持单一事实源、`tool-catalog.md` 文档完整）；
    /// - 不进 `build_function_definitions_for_chat_default()`（chat_loop 默认视图）；
    /// - 由 `PlanRuntime::visible_tools_for_mode(PlanState)` 在 PLAN/EXEC 模式时显式合入。
    ///   详见 [`plan-runtime.md`](../../../../docs/architecture/plan-runtime.md) §4.1 R6。
    pub plan_only: bool,
    /// 调用本工具是否需要等待用户交互（如 `ask_question` 在 CLI/IDE panel 阻塞 await）。
    /// 这是工具元属性，与 `read_only` / `destructive` 并列；写 catalog 时用作"工具是否会让 chat 主循环让出 stdin"的硬约束声明。
    /// 详见 [`ask-question.md`](../../../../docs/architecture/tools/ask-question.md) §4.2.1。
    pub requires_user_interaction: bool,
}

impl BuiltinToolCatalogEntry {
    pub fn effective_category(&self) -> ToolCategory {
        self.category
            .unwrap_or_else(|| derive_default_category(self.scope))
    }

    pub fn display_summary(&self) -> String {
        self.display_summary
            .map(str::to_string)
            .unwrap_or_else(|| summarize_tool_description(self.description))
    }
}

pub fn derive_default_category(scope: PermissionScope) -> ToolCategory {
    match scope {
        PermissionScope::Read | PermissionScope::Write | PermissionScope::Forbidden => {
            ToolCategory::Filesystem
        }
        PermissionScope::Bash | PermissionScope::BashApproval => ToolCategory::Exec,
    }
}

pub fn summarize_tool_description(description: &str) -> String {
    description
        .trim()
        .split("\n\n")
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

// ─── 跨工具规则常量（供多个工具共享同一字符串，聚合时按 byte 相等去重） ───
const G_EDIT_WORKFLOW: &str = "Default file-edit workflow: read -> edit; for repeated short snippets or line-anchored edits, use read(hashline=true) -> hashline_edit.";
const G_NO_DISPLAY_PREFIX: &str = "When copying from read output, never include display prefixes like `  N\\t` or `N#XX:` in edit.old_content.";
const G_NO_FAKE_EDIT: &str = "Make file changes with the edit/write tools directly; never print a code block pretending to edit a file.";
const G_PATH_LINE: &str =
    "When you point to code in a reply, cite it as a clickable `path:line` reference.";
const G_SEARCH_OVER_BASH: &str =
    "Use search_files to find file paths or content; prefer it over bash with grep/find/ls -R.";
const G_ANTI_HALLUCINATION: &str = "Only claim you can access directories you have successfully listed or read with tools; if unsure, verify with list_dir. Do not guess or fabricate accessible paths.";

pub const BUILTIN_TOOL_CATALOG: &[BuiltinToolCatalogEntry] = &[
    BuiltinToolCatalogEntry {
        name: "read",
        label: "Read",
        description: "Read a UTF-8 text file. Read a file before editing it. Use list_dir for directories; binary or non-UTF-8 files return a structured hint with the detected first bytes instead of a raw decode error.\n",
        display_summary: Some("Read a file from an authorized path."),
        parameters: read_parameters,
        scope: PermissionScope::Read,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("read file text utf-8 inspect"),
        prompt_guidelines: &["Use read to inspect a file before editing it.", G_PATH_LINE],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "load_skill",
        label: "Load Skill",
        description: "Load one skill body by its declared name instead of guessing a file path. Use this after reading `<available_skills>` when a skill's full instructions are needed. Required `name` selects the skill; optional `file` reads a relative attachment inside the same skill directory. The read still goes through the permission gate, and reviewer/verifier contexts may reject this tool.\n",
        display_summary: Some("Load one skill body or attachment by skill name."),
        parameters: load_skill_parameters,
        scope: PermissionScope::Read,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("load skill body attachment by name"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "write",
        label: "Write File",
        description: "Create or overwrite a file at an authorized path. Use this for new files or complete rewrites when the final content is known; prefer edit for small surgical changes. Writes may require user confirmation and are audited.\n",
        display_summary: Some("Create or overwrite a file after permission checks."),
        parameters: write_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("write create overwrite file"),
        prompt_guidelines: &[
            "Use write only for new files or complete rewrites; prefer edit for small changes to existing files.",
            G_NO_FAKE_EDIT,
        ],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "edit",
        label: "Edit File",
        description: "Edit an existing text file by replacing exact text. Two input shapes:\n  Shape A (single): { path, old_content, new_content, replace_all? }\n  Shape B (preferred, multiple): { path, edits: [ { old_content, new_content, replace_all? }, ... ] }\nWhen both appear, `edits` wins. Each segment matches the file's ORIGINAL snapshot (no chained matching). Without `replace_all: true` a segment must match exactly once, else the call returns an Ambiguous error. Read the file first (a fresh read stamp is required; mtime/size mismatch returns a Stale error). Do NOT include `cat -n`/hashline display prefixes (`  N\\t...` or `N#XX:...`) in `old_content`. Use write for new files; do not edit binary files.\n",
        display_summary: Some("Replace exact text in an existing file (multi-segment, original-snapshot)."),
        parameters: edit_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("edit replace old_content new_content file"),
        prompt_guidelines: &[G_EDIT_WORKFLOW, G_NO_DISPLAY_PREFIX, G_NO_FAKE_EDIT],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "hashline_edit",
        label: "Hashline Edit File",
        description: "Edit a file using line-number + 2-char content-hash anchors. Call `read(hashline=true)` first, then pass the returned `<line>#<2char>` anchors here. Each segment's anchor must match the file's CURRENT content; if the line changed, the anchor no longer matches and the call returns HashMismatch (no write). Operations: `replace` (anchor -> lines), `insert` (insert `lines` BEFORE the anchor line), `delete` (anchor[..end] -> empty). Use this when substring `edit` would be ambiguous (repeated short snippets) or when you need strong line-level consistency. A fresh read stamp is still required.\n",
        display_summary: Some("Line-number + content-hash anchored edits (companion to read hashline=true)."),
        parameters: hashline_edit_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("hashline edit line anchor hash"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "bash",
        label: "Bash",
        description: "Run a shell command through the permission gate (builds, tests, git inspection, other CLI workflows). Avoid destructive commands unless the user explicitly asked and the permission prompt allows it. Prefer tool-native file APIs over bash for reading or editing files; bash path access is still checked and audited.\n\nSet `run_in_background: true` for long-running commands (builds, watchers, dev servers): the call returns immediately with `task_id` + `log_path`, driven via `task_output` / `task_stop` / `task_list`. A trailing `&` still runs inside the same foreground call, so prefer `run_in_background: true` to outlive the current tool round.\n",
        display_summary: Some("Run an audited shell command (foreground or background)."),
        parameters: bash_parameters,
        scope: PermissionScope::Bash,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("bash shell command test build git background"),
        prompt_guidelines: &["Prefer tool-native file APIs over bash for reading or editing files."],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "task_output",
        label: "Bash Task Output",
        description: "Read incremental output from a background `bash` task (started with run_in_background=true). Returns a UTF-8 lossy chunk from `since` plus `finished` and `exit_code`; pass the previous response's `next_offset` as the next `since` to tail across turns (first call may omit `since`). `block=false` (default) returns immediately; `block=true` waits until new output, the task finishes, or `timeout_ms` elapses (default 5000, max 30000, `0` == block=false) and adds a `wakeReason` of `new_output` | `finished` | `timeout`. A `timeout` wakeReason is NOT a failure. Do not busy-poll. See the background bash tasks section in the system prompt for the full workflow.\n",
        display_summary: Some("Tail incremental output from a background bash task."),
        parameters: task_output_parameters,
        scope: PermissionScope::Bash,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("bash background task output tail log"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "task_stop",
        label: "Bash Task Stop",
        description: "Stop a background `bash` task by its `task_id` (SIGKILL to the whole process group on Unix). Subsequent `task_output` calls return `finished=true` with `exit_code=-1`.\n",
        display_summary: Some("Force-stop a background bash task by task_id."),
        parameters: task_stop_parameters,
        scope: PermissionScope::Bash,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("bash background task stop kill cancel"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "task_list",
        label: "Bash Task List",
        description: "List every background `bash` task in the current session with its status (`Running`, `Stopped`, or `Finished{exit_code}`), originating command, started_at timestamp, and log path. Use it to discover task ids to follow up on.\n",
        display_summary: Some("List background bash tasks and their status."),
        parameters: task_list_parameters,
        scope: PermissionScope::Bash,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("bash background task list status enumerate"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "list_dir",
        label: "List Directory",
        description: "List the immediate contents of an authorized directory (no recursion). Use it to discover nearby files before choosing read or edit; call it on subdirectories instead of guessing paths.\n",
        display_summary: Some("List immediate entries in a directory."),
        parameters: list_dir_parameters,
        scope: PermissionScope::Read,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("list directory files"),
        prompt_guidelines: &[G_ANTI_HALLUCINATION],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "search_files",
        label: "Search Files",
        description: "Search authorized files by content regex or file-path glob. Use target=content to search inside files and target=files to find file paths; target=files only uses pattern/path/head_limit/offset/include_hidden. Use list_dir for a single directory level and read when you already know the path.\n\nUses system `rg` (content) / `fd` (files), falling back to an in-process Rust regex engine when they are missing (no lookaround/back-references; large/binary files skipped). Both honour .gitignore/.ignore.\n",
        display_summary: Some("Search authorized files by content or file-path glob."),
        parameters: search_files_parameters,
        scope: PermissionScope::Read,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("search grep glob files content regex"),
        prompt_guidelines: &[G_SEARCH_OVER_BASH],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "web_search",
        label: "Web Search",
        description: "Search the web and return normalized search hits. Use this to discover candidate URLs/snippets; use `web_fetch` when you need one URL body afterward. Required `query`, plus optional `count`, `freshness`, `country`, `language`, and `domain_filter`. Results are normalized across hosted OpenAI search plus Tavily / Brave / Serper backends with automatic fallback in `auto` mode. Preserve source attribution when citing, and mind the current date for time-sensitive queries.\n",
        display_summary: Some("Search the web for normalized hits with backend fallback."),
        parameters: web_search_parameters,
        scope: PermissionScope::Read,
        category: Some(ToolCategory::Exec),
        read_only: true,
        destructive: false,
        search_hint: Some("web search internet tavily brave serper query"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "web_fetch",
        label: "Web Fetch",
        description: "Fetch one specific URL and return cleaned page content. Use this after `web_search` when you already have a candidate URL. Unsafe hosts (embedded credentials, single-label / IP-literal, private/loopback) are rejected; off-host redirects are not auto-followed and instead return structured redirect info so you can decide whether to refetch. Small text/html returns inline; large text and binary payloads (PDF/images) are persisted with a head preview plus `persisted_output_path`. Required `url`, plus optional `prompt` (warning-only) and `format` (`markdown` or `text`).\n",
        display_summary: Some("Fetch one URL body with safe redirects and persistence."),
        parameters: web_fetch_parameters,
        scope: PermissionScope::Read,
        category: Some(ToolCategory::Exec),
        read_only: true,
        destructive: false,
        search_hint: Some("web fetch url markdown html pdf redirect"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "config_get",
        label: "Config Get",
        description: "Read the current value of an allowed tomcat configuration key. Non-sensitive fields (workspace.*, agent.id, primitive.*, llm.default_model, and similar) are readable; sensitive fields (llm.api_key*, security.*, storage.*) are denied. Missing dot-path keys return not_set.\n",
        display_summary: Some("Read a non-sensitive tomcat configuration value."),
        parameters: config_get_parameters,
        scope: PermissionScope::Read,
        category: Some(ToolCategory::Config),
        read_only: true,
        destructive: false,
        search_hint: Some("config get workspace primitive model"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "config_set",
        label: "Config Set",
        description: "Append to or update an allowed tomcat configuration key. Every call shows a unified diff and requires confirmation. Array fields (workspace_roots, path_rules, bash_*, etc.) take `value` as one JSON element string and append only; scalar fields (llm.default_model, log.level, context.*) take `value` as the replacement. Deletion or arbitrary mutation is unsupported; sensitive fields (llm.api_key*, security.*, storage.*, agent.id, primitive.auto_confirm) are denied.\n",
        display_summary: Some("Modify an allowed tomcat configuration key after confirmation."),
        parameters: config_set_parameters,
        scope: PermissionScope::Write,
        category: Some(ToolCategory::Config),
        read_only: false,
        destructive: true,
        search_hint: Some("config set workspace roots path rules model"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    // ─── PLAN 模式专属工具（T2-P1-002/003）：plan_only=true，默认 chat catalog 不暴露 ───
    BuiltinToolCatalogEntry {
        name: "create_plan",
        label: "Create Plan",
        description: "Create a new plan file under `~/.tomcat/plans/<slug>_<hash>.plan.md` (PLAN mode only). Pass `goal` (short objective), `draft` (plan-body content), and an initial flat `todos` list; the runtime derives `plan_id` from goal (do NOT pass plan_id), normalizes `draft` into the `## Plan` section, writes frontmatter under an advisory lock, then runs an advisory reviewer whose summary rides back on this tool's result `review` field. Reviewer output is advisory only and does NOT gate `/plan build`. Calling outside Planning returns a tool error.\n",
        display_summary: Some("Create a plan file under ~/.tomcat/plans/ and run an advisory reviewer (PLAN mode only)."),
        parameters: create_plan_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: false,
        search_hint: Some("plan create planning goal draft todos reviewer"),
        prompt_guidelines: &[],
        plan_only: true,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "update_plan",
        label: "Update Plan",
        description: "Apply incremental todo-only ops (`upsert` / `set_status` / `remove`) to the active plan, persisted to its `.plan.md` frontmatter under an advisory lock. Visible in CHAT / PLAN / EXEC. `plan_id` and `path` target the plan; `replace=true` swaps the entire todo list with the provided upsert results. When all todos reach `completed` in EXEC, the runtime auto-derives state=completed and resets the reminder/catalog/visible labels. Only frontmatter.todos is mutated; plan body markdown is left untouched.\n",
        display_summary: Some("Apply todo-only incremental ops to the active plan (CHAT/PLAN/EXEC)."),
        parameters: update_plan_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: false,
        search_hint: Some("plan update todos upsert set_status remove replace"),
        prompt_guidelines: &[],
        plan_only: true,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "todos",
        label: "Todos",
        description: "Manage a session-local todo scratchpad and return a full snapshot of all items after each call. It NEVER writes the active PlanFile (advance plan todos via `update_plan`); when persistence is configured it is stored at `~/.tomcat/agents/<id>/todos/<session_id>.todo.md`. Use `new_todos=true` to clear the scratchpad and start fresh; use `replace=true` to replace the whole list with the provided upsert results. At most one todo may be `in_progress`.\n",
        display_summary: Some("Maintain a session todo scratchpad (single in_progress; returns full snapshot)."),
        parameters: todos_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: false,
        search_hint: Some("todos upsert set_status remove scratchpad new_todos replace"),
        prompt_guidelines: &[],
        plan_only: true,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "ask_question",
        label: "Ask Question",
        description: "Ask the user 1-4 structured single-choice questions. Each question has 2-4 `options` (stable `id` + `label`); exactly one option must carry `recommended: true` (UI renders it with an `— 推荐` suffix). The UI auto-appends a `__custom__` free-text slot (do NOT declare it) and a per-question `skip`. The tool blocks until the user answers, skips, or cancels (cancel -> `{ cancelled: true }`, not a ToolError). Visible in CHAT / PLAN / Pending / Completed; hidden in EXEC to avoid blocking the execution loop.\n",
        display_summary: Some("Block-await structured single-choice answers from the user."),
        parameters: ask_question_parameters,
        scope: PermissionScope::Read,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("plan ask question single choice recommended custom skip"),
        prompt_guidelines: &[],
        plan_only: true,
        requires_user_interaction: true,
    },
];

pub fn builtin_tool_by_name(name: &str) -> Option<&'static BuiltinToolCatalogEntry> {
    BUILTIN_TOOL_CATALOG.iter().find(|entry| entry.name == name)
}

pub fn build_function_definitions() -> Vec<Value> {
    BUILTIN_TOOL_CATALOG
        .iter()
        .map(|entry| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": entry.name,
                    "description": entry.description,
                    "parameters": (entry.parameters)(),
                }
            })
        })
        .collect()
}

/// chat_loop 默认 LLM 工具集（不含 `plan_only` 工具）。
///
/// PLAN 模式专属工具（`create_plan` / `update_plan` / `todos` / `ask_question`）依据 plan-runtime.md
/// §4.1 R6 在 PLAN/EXEC 模式由 `PlanRuntime::visible_tools_for_mode` 显式合入；
/// 未启用 `PlanRuntime` 时（or `mode == Chat`），chat_loop 用本 helper 装配 `tool_definitions`，
/// 避免 plan 工具暴露给 CHAT 期 LLM。
///
/// 全集（含 plan_only）仍由 [`build_function_definitions`] 输出，用于
/// `tool-catalog.md` 文档与 `catalog_and_function_definitions_have_same_names` 回归。
pub fn build_function_definitions_for_chat_default() -> Vec<Value> {
    BUILTIN_TOOL_CATALOG
        .iter()
        .filter(|entry| !entry.plan_only && entry.name != "load_skill")
        .map(|entry| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": entry.name,
                    "description": entry.description,
                    "parameters": (entry.parameters)(),
                }
            })
        })
        .collect()
}

pub fn render_core_identity_tool_lines() -> String {
    render_core_identity_tool_lines_with_policy(true)
}

pub fn render_core_identity_tool_lines_with_policy(allow_load_skill: bool) -> String {
    BUILTIN_TOOL_CATALOG
        .iter()
        .filter(|entry| allow_load_skill || entry.name != "load_skill")
        .map(|entry| format!("- {}: {}", entry.name, entry.display_summary()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 聚合各工具的 [`BuiltinToolCatalogEntry::prompt_guidelines`]，按 catalog 顺序展开、
/// 按 byte 相等去重（保留首次出现顺序），渲染成 `- <guideline>` 行，注入
/// `tool_instructions.txt` 的 `{tool_guidelines}` 占位。`allow_load_skill=false` 时
/// 跳过 `load_skill`（与 `render_core_identity_tool_lines_with_policy` 同口径）；
/// 由于 `load_skill` 无跨工具规则，实际输出对该开关不敏感。
pub fn render_tool_guidelines() -> String {
    render_tool_guidelines_with_policy(true)
}

pub fn render_tool_guidelines_with_policy(allow_load_skill: bool) -> String {
    let mut out: Vec<&'static str> = Vec::new();
    for entry in BUILTIN_TOOL_CATALOG {
        if !allow_load_skill && entry.name == "load_skill" {
            continue;
        }
        for guideline in entry.prompt_guidelines {
            if !out.iter().any(|g| g == guideline) {
                out.push(guideline);
            }
        }
    }
    out.iter()
        .map(|g| format!("- {g}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_tool_catalog_markdown() -> String {
    let mut out = String::new();
    out.push_str("# Tool Catalog\n\n");
    out.push_str("> This file is generated from `src/core/tools/contract/catalog.rs`.\n");
    out.push_str(
        "> Run `UPDATE_TOOL_CATALOG=1 cargo run --bin gen-tool-catalog` after catalog changes.\n",
    );
    out.push_str(
        "> `checkpoint` / `restore` 不在 tool catalog 中：它们是 `tomcat chat` 的本地斜杠命令（`/ckpt`、`/restore`），由 chat 层直接处理，不暴露给 LLM 作为工具。\n\n",
    );

    for category in [
        ToolCategory::Filesystem,
        ToolCategory::Exec,
        ToolCategory::Config,
    ] {
        out.push_str(&format!("## {}\n\n", category.title()));
        for entry in BUILTIN_TOOL_CATALOG
            .iter()
            .filter(|entry| entry.effective_category() == category)
        {
            out.push_str(&format!("### `{}`\n\n", entry.name));
            out.push_str(&format!("- Label: {}\n", entry.label));
            out.push_str(&format!("- Category: `{}`\n", category.as_str()));
            out.push_str(&format!("- Permission scope: `{:?}`\n", entry.scope));
            out.push_str(&format!("- Read only: `{}`\n", entry.read_only));
            out.push_str(&format!("- Destructive: `{}`\n", entry.destructive));
            if let Some(search_hint) = entry.search_hint {
                out.push_str(&format!("- Search hint: `{}`\n", search_hint));
            }
            out.push('\n');
            out.push_str(entry.description.trim());
            if !entry.prompt_guidelines.is_empty() {
                out.push_str("\n\nGuidelines:\n");
                for guideline in entry.prompt_guidelines {
                    out.push_str(&format!("- {}\n", guideline));
                }
            }
            out.push_str("\n\nParameters:\n\n");
            out.push_str("```json\n");
            out.push_str(
                &serde_json::to_string_pretty(&(entry.parameters)())
                    .unwrap_or_else(|_| "{}".to_string()),
            );
            out.push_str("\n```\n\n");
        }
    }

    out
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

fn todo_status_property(description: &str) -> Value {
    serde_json::json!({
        "type": "string",
        "enum": ["pending", "in_progress", "completed", "cancelled"],
        "description": description
    })
}

fn shared_todo_op_item_schema(status_description: &str) -> Value {
    serde_json::json!({
        "oneOf": [
            {
                "type": "object",
                "description": "`upsert` creates a todo if id is new, else updates the provided fields.",
                "properties": {
                    "kind": {
                        "type": "string",
                        "const": "upsert",
                        "description": "Operation kind."
                    },
                    "id": {
                        "type": "string",
                        "description": "Target todo id (kebab-case)."
                    },
                    "content": {
                        "type": "string",
                        "description": "Todo content. Required when creating a brand-new todo."
                    },
                    "status": todo_status_property(status_description)
                },
                "required": ["kind", "id"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "`set_status` only changes status for an existing todo.",
                "properties": {
                    "kind": {
                        "type": "string",
                        "const": "set_status",
                        "description": "Operation kind."
                    },
                    "id": {
                        "type": "string",
                        "description": "Target todo id (kebab-case)."
                    },
                    "status": todo_status_property(status_description)
                },
                "required": ["kind", "id", "status"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "description": "`remove` deletes a todo by id.",
                "properties": {
                    "kind": {
                        "type": "string",
                        "const": "remove",
                        "description": "Operation kind."
                    },
                    "id": {
                        "type": "string",
                        "description": "Target todo id (kebab-case)."
                    }
                },
                "required": ["kind", "id"],
                "additionalProperties": false
            }
        ]
    })
}

fn read_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "path": { "type": "string", "description": "Absolute or relative file path to read as UTF-8 text." },
            "offset": {
                "type": "integer",
                "minimum": 1,
                "description": "Optional 1-based line to start from. Defaults to 1."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 10000,
                "description": "Optional max lines to return (default 2000). On overflow the result appends a resume hint with the next offset."
            },
            "line_numbers": {
                "type": "boolean",
                "description": "Render `cat -n` style line numbers (default true). These prefixes are display-only — do not paste `  N\\t...` into edit.old_content."
            },
            "hashline": {
                "type": "boolean",
                "description": "Render each line as `{line}#{2-char hash}:{content}` for use with hashline_edit. Display-only prefix — do not paste into edit.old_content. Mutually exclusive with line_numbers (hashline wins). Default false."
            }
        }),
        &["path"],
    )
}

fn load_skill_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "name": {
                "type": "string",
                "description": "Skill 名称（来自 <available_skills> 的 name 字段）。"
            },
            "file": {
                "type": ["string", "null"],
                "description": "技能目录下的相对附件路径；省略或 null 表示读取主 SKILL.md 正文。"
            }
        }),
        &["name"],
    )
}

fn write_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "path": { "type": "string", "description": "Absolute or relative file path to create or overwrite." },
            "content": { "type": "string", "description": "Full file content to write." },
            "overwrite": { "type": "boolean", "description": "Whether an existing file may be overwritten. Defaults to false." }
        }),
        &["path", "content"],
    )
}

fn edit_parameters() -> Value {
    serde_json::json!({
        "type": "object",
        "description": "Edit a file (read -> edit). Provide Shape A (top-level old_content/new_content) or Shape B (edits[]); when both appear, `edits` wins. All segments match the file's ORIGINAL snapshot (no chained matching). Do not include read display prefixes (`  N\\t...` or `N#XX:...`) in old_content.",
        "properties": {
            "path": {
                "type": "string",
                "description": "Absolute or relative file path to edit."
            },
            "old_content": {
                "type": "string",
                "description": "Shape A: exact existing text to replace; include enough context to be unique unless `replace_all: true`. Copy real file text, not display prefixes."
            },
            "new_content": {
                "type": "string",
                "description": "Shape A: replacement text."
            },
            "replace_all": {
                "type": "boolean",
                "description": "Shape A: replace every occurrence of `old_content` instead of failing on multiple matches. Defaults to false."
            },
            "edits": {
                "type": "array",
                "minItems": 1,
                "description": "Shape B (preferred): edit segments applied to the file's ORIGINAL snapshot. Overlapping spans are rejected.",
                "items": {
                    "type": "object",
                    "properties": {
                        "old_content": {
                            "type": "string",
                            "description": "Exact existing text to replace in this segment (real file text, no display prefixes)."
                        },
                        "new_content": {
                            "type": "string",
                            "description": "Replacement text for this segment."
                        },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Replace every occurrence of `old_content` in this segment. Defaults to false."
                        }
                    },
                    "required": ["old_content", "new_content"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["path"]
    })
}

fn bash_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "command": {
                "type": "string",
                "description": "Shell command to execute. With `args` set, runs argv-style (no shell); otherwise via `sh -c` (Unix) / `cmd /C` (Windows)."
            },
            "cwd": {
                "type": "string",
                "description": "Optional working directory. Empty means unset. Use an absolute path or `~/...`; shell vars like `$HOME` are NOT expanded here. Defaults to the agent process cwd."
            },
            "args": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Optional argv elements appended to `command`; when present the command runs argv-style (no shell) — safer for paths with spaces or quotes."
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 1,
                "maximum": 600000,
                "description": "Optional wall-clock timeout in ms (default 120000, capped at 600000). On timeout the child is killed and `timed_out=true`. Ignored when run_in_background=true."
            },
            "run_in_background": {
                "type": "boolean",
                "description": "When true, spawn as a background task and return { task_id, log_path } immediately; pair with task_output/task_stop/task_list. Defaults to false."
            }
        }),
        &["command"],
    )
}

fn task_output_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "task_id": {
                "type": "string",
                "description": "The task_id returned by a previous `bash` call with run_in_background=true."
            },
            "since": {
                "type": "integer",
                "minimum": 0,
                "description": "Byte offset to start from; pass the previous response's `next_offset` to tail. Defaults to 0."
            },
            "block": {
                "type": "boolean",
                "description": "If true, wait until new output arrives, the task finishes, or `timeout_ms` elapses, and return a `wakeReason`. Default false."
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 0,
                "maximum": 30000,
                "description": "Wait slice in ms for block=true (default 5000, max 30000; `0` == block=false). A timeout is not a failure — inspect `content`/`finished` before waiting again."
            }
        }),
        &["task_id"],
    )
}

fn task_stop_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "task_id": {
                "type": "string",
                "description": "The task_id returned by a previous `bash` call with run_in_background=true."
            }
        }),
        &["task_id"],
    )
}

fn task_list_parameters() -> Value {
    object_schema(serde_json::json!({}), &[])
}

fn list_dir_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "path": { "type": "string", "description": "Directory path to list without recursion." }
        }),
        &["path"],
    )
}

fn hashline_edit_parameters() -> Value {
    serde_json::json!({
        "type": "object",
        "description": "Line-anchored edit. Call `read(hashline=true)` first, then pass the returned `<line>#<2char>` anchors here. Anchors are validated against the file's current content before any write; mismatches return HashMismatch.",
        "properties": {
            "path": {
                "type": "string",
                "description": "Absolute or relative file path to edit."
            },
            "edits": {
                "type": "array",
                "minItems": 1,
                "description": "Line-anchored operations applied against the CURRENT file content.",
                "items": {
                    "type": "object",
                    "properties": {
                        "op": {
                            "type": "string",
                            "enum": ["replace", "insert", "delete"],
                            "description": "Edit operation kind."
                        },
                        "pos": {
                            "type": "string",
                            "description": "Start-line anchor `<1-based-line>#<2char-hash>` (e.g. `42#Ab`). For `insert`, content goes BEFORE this line."
                        },
                        "end": {
                            "type": "string",
                            "description": "Optional inclusive end-line anchor (replace/delete only). Defaults to `pos`."
                        },
                        "lines": {
                            "type": "string",
                            "description": "Replacement / insertion text (end multi-line text with a newline). Ignored by `delete`."
                        }
                    },
                    "required": ["op", "pos"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["path", "edits"]
    })
}

fn search_files_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "pattern": {
                "type": "string",
                "description": "[both] Search expression. target=content: ripgrep regex over file contents. target=files: file-path glob such as `*.rs` or `src/**/*.rs`."
            },
            "target": {
                "type": "string",
                "enum": ["content", "files"],
                "description": "[both] `content` searches inside files; `files` searches file paths by glob. Defaults to `content`."
            },
            "path": {
                "type": "string",
                "description": "[both] Optional file or directory to search. Defaults to the workspace; must pass Read permission checks."
            },
            "glob": {
                "type": "string",
                "description": "[content only] Optional file glob filter such as `*.rs` or `**/*.md`. Omit when unused; do not pass an empty string."
            },
            "type": {
                "type": "string",
                "description": "[content only] Optional ripgrep file type filter such as `rust`, `js`, or `py`. Omit when unused."
            },
            "output_mode": {
                "type": "string",
                "enum": ["content", "files_with_matches", "count"],
                "description": "[content only] Return matched lines, files with matches, or per-file counts. Defaults to `files_with_matches`."
            },
            "context": {
                "type": "integer",
                "minimum": 0,
                "description": "[content only] Surrounding context lines when output_mode=content. Ignored otherwise."
            },
            "head_limit": {
                "anyOf": [
                    { "type": "integer", "minimum": 1, "maximum": 1024 },
                    { "type": "null" }
                ],
                "description": "[both] Max returned items after offset. Defaults to 64 for content and 128 for files. null = unlimited; 0 is rejected."
            },
            "offset": {
                "type": "integer",
                "minimum": 0,
                "description": "[both] Skip this many items before head_limit. Use next_offset when truncated=true."
            },
            "case_insensitive": {
                "type": "boolean",
                "description": "[content only] Ignore case, equivalent to ripgrep -i. Defaults to false."
            },
            "include_hidden": {
                "type": "boolean",
                "description": "[both] Include hidden files and directories. Defaults to false; .gitignore is still respected."
            }
        }),
        &["pattern"],
    )
}

fn web_search_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "query": {
                "type": "string",
                "description": "Search query text (required); prefer natural-language keywords."
            },
            "count": {
                "type": "integer",
                "minimum": 1,
                "maximum": 20,
                "description": "Number of hits to request. Defaults to 5, capped at 20."
            },
            "freshness": {
                "type": ["string", "null"],
                "enum": ["day", "week", "month", "year", null],
                "description": "Optional recency filter (`day`/`week`/`month`/`year`); omit or null for none."
            },
            "country": {
                "type": ["string", "null"],
                "description": "Optional ISO 3166-1 alpha-2 country hint such as `us` or `cn`."
            },
            "language": {
                "type": ["string", "null"],
                "description": "Optional ISO 639-1 language hint such as `en` or `zh`."
            },
            "domain_filter": {
                "type": "array",
                "description": "Optional allowlist of bare-host domains such as `github.com`.",
                "items": {
                    "type": "string",
                    "description": "One allowed domain suffix."
                }
            }
        }),
        &["query"],
    )
}

fn web_fetch_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "url": {
                "type": "string",
                "description": "Target URL (required). Must be an http(s) URL without embedded credentials or private/IP-literal hosts."
            },
            "prompt": {
                "type": ["string", "null"],
                "description": "Optional extraction intent (MVP: recorded as a warning only, does not change fetched content)."
            },
            "format": {
                "type": "string",
                "enum": ["markdown", "text"],
                "description": "Output format for textual pages. Defaults to `markdown`; use `text` for plain text."
            }
        }),
        &["url"],
    )
}

fn config_get_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "key": { "type": "string", "description": "Configuration dot-path, e.g. workspace.workspace_roots, primitive.path_rules, or agent.id." }
        }),
        &["key"],
    )
}

fn config_set_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "key": { "type": "string", "description": "Allowed configuration dot-path to update." },
            "value": { "type": "string", "description": "Scalar replacement value, or one JSON element string for append-only array fields such as workspace roots and path rules." }
        }),
        &["key", "value"],
    )
}

// ─── PLAN 模式工具 schema（T2-P1-002/003） ─────────────────────────────────

fn create_plan_parameters() -> Value {
    serde_json::json!({
        "type": "object",
        "description": "Create a plan file under ~/.tomcat/plans/. Only callable when PlanRuntime mode == Planning. plan_id is derived by runtime from goal; do NOT pass plan_id.",
        "properties": {
            "goal": {
                "type": "string",
                "description": "Concise plan objective (1-3 sentences). Becomes frontmatter `goal` and seeds the derived `plan_id`."
            },
            "draft": {
                "type": "string",
                "description": "Markdown for the plan body `## Plan` section (approach, key decisions, constraints; <= ~2000 chars). Do NOT include the `## Goal` / `## Plan` / `## Todos Board` headings yourself."
            },
            "todos": {
                "type": "array",
                "description": "Initial flat todo list (>= 1 item). `status` defaults to `pending`.",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Stable kebab-case todo id, unique within the plan."
                        },
                        "content": {
                            "type": "string",
                            "description": "Single-sentence imperative todo description."
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed", "cancelled"],
                            "description": "Initial status. Defaults to `pending`; at most one todo may be `in_progress`."
                        }
                    },
                    "required": ["id", "content"]
                }
            }
        },
        "required": ["goal", "draft", "todos"]
    })
}

fn update_plan_parameters() -> Value {
    serde_json::json!({
        "type": "object",
        "description": "Apply incremental todo-only ops to the active plan. Callable in CHAT / PLAN / EXEC; requires an active plan. `replace=true` swaps the whole todo list with the upsert results; each op is tagged by `kind` (`upsert` / `set_status` / `remove`).",
        "properties": {
            "plan_id": {
                "type": "string",
                "description": "Target plan_id. Optional in EXEC (defaults to the active plan); REQUIRED in CHAT / PLAN / Pending / Completed."
            },
            "path": {
                "type": "string",
                "description": "Alternative target path under ~/.tomcat/plans/. If both `plan_id` and `path` are given, `plan_id` wins."
            },
            "replace": {
                "type": "boolean",
                "description": "If true, replace the entire todos[] list with the upsert results in `ops`. Default false."
            },
            "ops": {
                "type": "array",
                "description": "Ordered mutations applied atomically (one frontmatter write under advisory lock).",
                "minItems": 1,
                "items": shared_todo_op_item_schema(
                    "For `upsert` (optional) and `set_status` (required). At most one todo may be `in_progress`; `in_progress` only allowed when plan.state == executing."
                )
            }
        },
        "required": ["ops"]
    })
}

fn todos_parameters() -> Value {
    serde_json::json!({
        "type": "object",
        "description": "Session-local todo scratchpad (any plan mode). Returns the full items snapshot after each call. It never writes the active PlanFile (advance plan todos via `update_plan`). `new_todos=true` clears the scratchpad and starts fresh; `replace=true` swaps the whole list with the upsert results.",
        "properties": {
            "new_todos": {
                "type": "boolean",
                "description": "If true, clear the current scratchpad before applying ops (same session file is overwritten). Default false."
            },
            "title": {
                "type": "string",
                "description": "Optional title stored in the new .todo.md frontmatter when `new_todos=true`."
            },
            "replace": {
                "type": "boolean",
                "description": "If true, replace the entire todo list with the upsert results in `ops`. Default false."
            },
            "ops": {
                "type": "array",
                "description": "Ordered list of mutations applied in order.",
                "minItems": 1,
                "items": shared_todo_op_item_schema(
                    "For `upsert` (optional) and `set_status` (required). At most one todo may be `in_progress`."
                )
            }
        },
        "required": ["ops"]
    })
}

fn ask_question_parameters() -> Value {
    serde_json::json!({
        "type": "object",
        "description": "Block-await structured single-choice answers from the user. Each question has 2-4 options with stable ids; exactly one option must carry `recommended: true`. The UI auto-appends a `__custom__` slot and a `skip` action — do not declare `__custom__` yourself.",
        "properties": {
            "questions": {
                "type": "array",
                "description": "1-4 questions presented in one panel turn.",
                "minItems": 1,
                "maxItems": 4,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Stable question id (kebab-case), unique within the panel turn."
                        },
                        "prompt": {
                            "type": "string",
                            "description": "Question text shown to the user (max 500 chars)."
                        },
                        "options": {
                            "type": "array",
                            "description": "2-4 options. Exactly one option must carry `recommended: true`.",
                            "minItems": 2,
                            "maxItems": 4,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": {
                                        "type": "string",
                                        "description": "Stable option id (kebab-case), unique within this question. Reserved id `__custom__` is forbidden — the UI appends it automatically."
                                    },
                                    "label": {
                                        "type": "string",
                                        "description": "Human-readable option label (max 200 chars)."
                                    },
                                    "recommended": {
                                        "type": "boolean",
                                        "description": "Mark exactly one option per question as recommended; the UI suffixes it with `— 推荐`."
                                    }
                                },
                                "required": ["id", "label"]
                            }
                        }
                    },
                    "required": ["id", "prompt", "options"]
                }
            }
        },
        "required": ["questions"]
    })
}

#[cfg(test)]
#[path = "tests/catalog_test.rs"]
mod tests;
