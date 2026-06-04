//! # 内置工具 catalog
//!
//! 这里是内置工具描述的单一事实源。LLM function schema、system prompt 工具清单
//! 与 `docs/tool-catalog.md` 都从这里派生，避免多处手写后漂移。

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

pub const BUILTIN_TOOL_CATALOG: &[BuiltinToolCatalogEntry] = &[
    BuiltinToolCatalogEntry {
        name: "read",
        label: "Read",
        description: "Read a file from the local filesystem. Use this before editing or when the user asks to inspect file contents. Default workflow: `read` -> `edit`. For repeated short snippets or line-anchored edits, use `read(hashline=true)` -> `hashline_edit`. Use list_dir for directories; binary or non-UTF-8 files return a structured hint with the detected first bytes instead of raw decode errors.\n",
        display_summary: Some("Read a file from an authorized path."),
        parameters: read_parameters,
        scope: PermissionScope::Read,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("read file text utf-8 inspect"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "write",
        label: "Write File",
        description: "Create or overwrite a file at an authorized path. Use this for new files or complete rewrites when the intended final content is known. Prefer edit for small surgical changes to existing files. Writes may require user confirmation and are audited.\n",
        display_summary: Some("Create or overwrite a file after permission checks."),
        parameters: write_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("write create overwrite file"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "edit",
        label: "Edit File",
        description: "Edit an existing text file by replacing exact text. Two input shapes are accepted:\n  Shape A (single edit, legacy): { path, old_content, new_content, replace_all? }\n  Shape B (multiple edits, preferred): { path, edits: [ { old_content, new_content, replace_all? }, ... ] }\nWhen both shapes appear, `edits` wins. Each segment matches against the file's ORIGINAL snapshot (no chained / incremental matching), so multi-segment edits are safe to compose. Set `replace_all: true` to replace every occurrence; otherwise the segment must match exactly once or the call returns an Ambiguous error. Read the file first (default workflow: `read` -> `edit`; the tool requires a fresh read stamp, and mtime/size mismatch returns a Stale error). Do NOT include `cat -n` line-number prefixes (`  N\\t...`) or hashline prefixes (`N#XX:...`) in `old_content` — those are display prefixes, not file content. If repeated short snippets make substring edit ambiguous, prefer `read(hashline=true)` + `hashline_edit`. Use write for new files or complete rewrites; do not use edit on binary files.\n",
        display_summary: Some("Replace exact text in an existing file (multi-segment, original-snapshot)."),
        parameters: edit_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("edit replace old_content new_content file"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "hashline_edit",
        label: "Hashline Edit File",
        description: "Edit a file with line-number + 2-char content hash anchors. Call `read` with `hashline: true` first, then pass those anchors to `hashline_edit`. Each edit segment carries an anchor `<line>#<2char>` that must match the file's CURRENT content; if the line content changed, the anchor stops matching and the call returns HashMismatch (no write). Operations: `replace` (anchor → lines), `insert` (insert `lines` BEFORE anchor line), `delete` (anchor[..end] → empty). Use this when sub-string `edit` would be ambiguous (repeated short snippets) or when you need strong line-level consistency. Reads are still required first; the file's read stamp is checked.\n",
        display_summary: Some("Line-number + content-hash anchored edits (companion to read hashline=true)."),
        parameters: hashline_edit_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("hashline edit line anchor hash"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "bash",
        label: "Bash",
        description: "Run a shell command through the permission gate. Use it for builds, tests, git inspection, and other command-line workflows. Avoid destructive commands unless the user explicitly asked and the permission prompt allows it. Prefer tool-native file APIs for reading or editing files; bash path access is still checked and audited as command execution.\n\nSet `run_in_background: true` for long-running commands (builds, watchers, dev servers). The call returns immediately with a `task_id` + `log_path`; use `task_output` / `task_stop` / `task_list` to drive the task across follow-up turns instead of blocking a single tool round.\n",
        display_summary: Some("Run an audited shell command (foreground or background)."),
        parameters: bash_parameters,
        scope: PermissionScope::Bash,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("bash shell command test build git background"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "task_output",
        label: "Bash Task Output",
        description: "Read incremental output from a background `bash` task started with `run_in_background: true`. Returns a UTF-8 lossy chunk of `[since, next_offset)` bytes from the task's log file plus a `finished` flag. Use the previous response's `next_offset` as the next `since` to tail the task across turns; first call may omit `since` to read from byte 0. When the task has finished or been stopped, `finished=true` and `exit_code` is populated.\n\nWaiting modes:\n- `block=false` (default): non-blocking; returns immediately with whatever bytes are already on disk. Good for an occasional progress glance — but **do not busy-poll**.\n- `block=true`: blocks until any of {new output appears | the task finishes | `timeout_ms` elapses}. Returns an extra `wakeReason` field with one of `\"new_output\" | \"finished\" | \"timeout\"`. **`timeout` is NOT a failure** — when `wakeReason=\"timeout\" && finished=false` the response is just an empty wait slice (`content=\"\"`, `next_offset == since`); call `task_output(block=true)` again to keep waiting.\n\nWhen to use which:\n1. The current todo cannot proceed without the shell result → `task_output(block=true, timeout_ms=...)`. Loop on `wakeReason=\"timeout\"`.\n2. The current todo can do other independent work first → spawn `bash(run_in_background=true)` and immediately do other tools/edits/reads. The runtime will inject a synthetic `<background-task-finished task_id=\"...\" exit_code=\"...\" log_path=\"...\">tail</background-task-finished>` user message **automatically** when the shell finishes; you do not need to poll.\n3. Just want a peek at progress → one-shot `task_output(block=false)`.\n\nWhen you see the `<background-task-finished ...>` tag, treat it as a system signal that a previously blocked todo can now proceed (NOT as new user input); pull the full log with `task_output(task_id, since=...)` if the tail body is insufficient.\n\n`timeout_ms` defaults to 5000, is capped at 30000, and `0` is equivalent to `block=false`.\n",
        display_summary: Some("Tail incremental output from a background bash task."),
        parameters: task_output_parameters,
        scope: PermissionScope::Bash,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("bash background task output tail log"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "task_stop",
        label: "Bash Task Stop",
        description: "Stop a background `bash` task by its `task_id`. Sends SIGKILL to the entire process group on Unix (mirroring the foreground `bash` timeout path) and marks the task `Stopped`. Subsequent `task_output` calls return `finished=true` with `exit_code=-1`.\n",
        display_summary: Some("Force-stop a background bash task by task_id."),
        parameters: task_stop_parameters,
        scope: PermissionScope::Bash,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("bash background task stop kill cancel"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "task_list",
        label: "Bash Task List",
        description: "Enumerate every background `bash` task started in the current session with its current status (`Running`, `Stopped`, or `Finished{exit_code}`), the originating command, the started_at timestamp, and the log path. Use this to discover task ids when you need to follow up on a long-running task.\n",
        display_summary: Some("List background bash tasks and their status."),
        parameters: task_list_parameters,
        scope: PermissionScope::Bash,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("bash background task list status enumerate"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "list_dir",
        label: "List Directory",
        description: "List the immediate contents of an authorized directory. Use this to discover nearby files before choosing read or edit. It does not recurse; call it on subdirectories as needed instead of guessing paths.\n",
        display_summary: Some("List immediate entries in a directory."),
        parameters: list_dir_parameters,
        scope: PermissionScope::Read,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("list directory files"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "search_files",
        label: "Search Files",
        description: "Search authorized files by content regex or file path glob. Use target=content to search inside files and target=files to find file paths; target=files only uses pattern/path/head_limit/offset/include_hidden and silently ignores content-only fields.\n\nUse this instead of bash with grep/find/ls -R. Use list_dir when you only need one directory level, and read when you already know the exact path.\n\nDual implementation with one schema: Tier1 spawns the system rg (content) and fd/fdfind (files); when either binary is missing search_files transparently falls back to Tier2 (in-process ignore::WalkBuilder + globset + Rust regex). Both tiers honour .gitignore/.ignore by default. Tier2 caveats are reported in `warnings`: regex dialect is the Rust regex crate (no lookaround/back-references; unsupported regex returns an empty match set with a warning); files larger than 5 MiB and binary files are skipped; before/after context lines are not emitted; the wall-clock budget defaults to 10s and can be overridden with PI_SEARCH_TIER2_DEADLINE_MS, after which the result is `truncated=true`.\n",
        display_summary: Some("Search authorized files by content or file-path glob."),
        parameters: search_files_parameters,
        scope: PermissionScope::Read,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("search grep glob files content regex"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "web_search",
        label: "Web Search",
        description: "Search the web and return normalized search hits. Use this to discover candidate URLs and snippets for a query; use `web_fetch` when you need to fetch one URL body afterward. Input fields align with the architecture doc: required `query`, plus optional `count`, `freshness`, `country`, `language`, and `domain_filter`.\n\nResults are normalized across hosted OpenAI search plus Tavily / Brave / Serper backends, with automatic fallback in `auto` mode. Preserve source attribution when citing results, and pay attention to the current date for time-sensitive queries.\n",
        display_summary: Some("Search the web for normalized hits with backend fallback."),
        parameters: web_search_parameters,
        scope: PermissionScope::Read,
        category: Some(ToolCategory::Exec),
        read_only: true,
        destructive: false,
        search_hint: Some("web search internet tavily brave serper query"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "web_fetch",
        label: "Web Fetch",
        description: "Fetch one specific URL and return cleaned page content. Use this after `web_search` when you already have a candidate URL and need the actual page body. Private/authenticated URLs, URLs with embedded credentials, single-label hosts, IP literal hosts, and private/loopback targets are rejected before any request; off-host redirects are not auto-followed and instead return structured redirect info so the model can decide whether to refetch with the new URL.\n\nSmall text/html pages are returned inline as markdown or plain text. Large text responses are persisted to `tool-results` and return a head preview plus `persisted_output_path`; PDF/images and other binary payloads are persisted instead of being inlined. Input fields align with the architecture doc: required `url`, plus optional `prompt` (MVP warning-only hint) and `format` (`markdown` or `text`).\n",
        display_summary: Some("Fetch one URL body with safe redirects and persistence."),
        parameters: web_fetch_parameters,
        scope: PermissionScope::Read,
        category: Some(ToolCategory::Exec),
        read_only: true,
        destructive: false,
        search_hint: Some("web fetch url markdown html pdf redirect"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "config_get",
        label: "Config Get",
        description: "Read the current value of an allowed tomcat configuration key. The tool is constrained by CONFIG_READ_ALLOWLIST and CONFIG_HARDCODED_READ_DENY: workspace.*, agent.id, primitive.path_rules, primitive.bash_*, llm.default_model and similar non-sensitive fields are readable; llm.api_key*, llm.api_base, security.*, storage.* and other sensitive fields are denied. Missing dot-path keys return not_set.\n",
        display_summary: Some("Read a non-sensitive tomcat configuration value."),
        parameters: config_get_parameters,
        scope: PermissionScope::Read,
        category: Some(ToolCategory::Config),
        read_only: true,
        destructive: false,
        search_hint: Some("config get workspace primitive model"),
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "config_set",
        label: "Config Set",
        description: "Append to or update an allowed tomcat configuration key. Every call shows the user a unified diff and requires confirmation. CONFIG_WRITE_ALLOWLIST and CONFIG_HARDCODED_WRITE_DENY protect sensitive or self-escalating fields.\n\nSemantics: array fields such as workspace.workspace_roots, workspace.entries, primitive.path_rules, primitive.bash_forbidden, and primitive.bash_approval_required accept value as one JSON element string and append it only. Scalar fields such as llm.default_model, log.level, context.keep_recent_turns, context.current_tail_compactable_min_chars, context.current_tail_single_result_max_chars, and context.compaction_max_tokens accept value as the replacement string. Deleting or arbitrary mutation is not supported; return an error that guides the user to tomcat config edit.\n\nForbidden fields include llm.api_key*, security.*, storage.*, agent.id, agent.workspace, and primitive.auto_confirm.\n",
        display_summary: Some("Modify an allowed tomcat configuration key after confirmation."),
        parameters: config_set_parameters,
        scope: PermissionScope::Write,
        category: Some(ToolCategory::Config),
        read_only: false,
        destructive: true,
        search_hint: Some("config set workspace roots path rules model"),
        plan_only: false,
        requires_user_interaction: false,
    },
    // ─── PLAN 模式专属工具（T2-P1-002/003）：plan_only=true，默认 chat catalog 不暴露 ───
    BuiltinToolCatalogEntry {
        name: "create_plan",
        label: "Create Plan",
        description: "Create a new plan file under `~/.tomcat/plans/<slug>_<hash>.plan.md` (PLAN mode only). Caller passes `goal` (short objective), `draft` (plan-body content), and an initial flat `todos` list; the runtime derives `plan_id` from goal (caller does NOT supply plan_id), normalizes `draft` into the plan body's `## Plan` section, and writes frontmatter (`plan_id`, `goal`, `state=planning`, `todos`, `schema_version=1`) under an exclusive advisory lock, then synchronously dispatches an internal reviewer sub-agent whose `ReviewSummary` rides back on this tool's result `review` field. Reviewer output is advisory only and does NOT gate `/plan build` — the user must call `/plan build <plan_id/path>` to enter EXEC. Visible only when `mode == Planning`; calling outside Planning returns a tool error.\n",
        display_summary: Some("Create a plan file under ~/.tomcat/plans/ and run an advisory reviewer (PLAN mode only)."),
        parameters: create_plan_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: false,
        search_hint: Some("plan create planning goal draft todos reviewer"),
        plan_only: true,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "update_plan",
        label: "Update Plan",
        description: "Apply incremental todo-only ops (`upsert` / `set_status` / `remove`) to the active plan, persisted to its `.plan.md` frontmatter under the same advisory lock. Visible in CHAT / PLAN / EXEC modes — model uses it to refine the todo list during PLAN or to advance todos during EXEC. `plan_id` and `path` are plan-specific targeting fields; `replace=true` swaps the entire todo list with the provided upsert results. When all todos transition to `completed` in EXEC, runtime auto-derives `state=completed` and resets system reminder / catalog / visible prompt labels. The tool only mutates frontmatter.todos; plan body markdown is left untouched.\n",
        display_summary: Some("Apply todo-only incremental ops to the active plan (CHAT/PLAN/EXEC)."),
        parameters: update_plan_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: false,
        search_hint: Some("plan update todos upsert set_status remove replace"),
        plan_only: true,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "todos",
        label: "Todos",
        description: "Manage a session-local todo scratchpad and return a full snapshot of all items after each call. The list is persisted under `~/.tomcat/agents/<id>/sessions/<session_key>/todos/<todos_id>.todo.md` when persistence is configured, and it NEVER writes the active PlanFile. Use `new_todos=true` to rotate to a new scratchpad file; use `replace=true` to replace the whole list with the provided upsert results. Only one todo may be `in_progress` at a time — attempting to mark a second `in_progress` returns a structured error. The full items snapshot in the response lets the model self-orient between rounds without re-listing.\n",
        display_summary: Some("Maintain a session todo scratchpad (single in_progress; returns full snapshot)."),
        parameters: todos_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: false,
        search_hint: Some("todos upsert set_status remove scratchpad new_todos replace"),
        plan_only: true,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "ask_question",
        label: "Ask Question",
        description: "Ask the user 1–4 structured single-choice questions. Each question has 2–4 `options` (each with a stable `id` and `label`); exactly one option must carry `recommended: true` (UI renders it with an `— 推荐` suffix). The UI panel automatically appends a synthetic `__custom__` slot (do NOT declare it manually) where the user can type free-form text up to 500 chars, and also supports `skip` to skip only the current question. The tool blocks until the user answers, skips, or cancels (cancel → `{ cancelled: true }`, not a ToolError). Visible in CHAT / PLAN / Pending / Completed; hidden in EXEC to avoid blocking the execution loop.\n",
        display_summary: Some("Block-await structured single-choice answers from the user."),
        parameters: ask_question_parameters,
        scope: PermissionScope::Read,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("plan ask question single choice recommended custom skip"),
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
        .filter(|entry| !entry.plan_only)
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
    BUILTIN_TOOL_CATALOG
        .iter()
        .map(|entry| format!("- {}: {}", entry.name, entry.display_summary()))
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
                "description": "Optional 1-based line number to start reading from. Defaults to 1 (first line)."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 10000,
                "description": "Optional max number of lines to return; defaults to 2000. When the file has more lines, the result includes a `... [N more lines truncated; resume with offset=<next>, limit=<same>]` hint so you can paginate."
            },
            "line_numbers": {
                "type": "boolean",
                "description": "Render output with `cat -n` style line numbers (`{:>6}\\t{content}`); defaults to true. These prefixes are display-only, so do not paste `  N\\t...` into `edit.old_content`. Set false only when piping the content into a tool that itself parses line numbers (e.g. diff)."
            },
            "hashline": {
                "type": "boolean",
                "description": "When true, render each line as `{:>6}#{2-char hash}:{content}` (xxh32 over whitespace-stripped content). Use with `hashline_edit` when you need line-number + content-hash anchors. The `N#XX:` prefix is display-only, so do not paste it into `edit.old_content`. Mutually exclusive with line_numbers — hashline takes priority. Defaults to false."
            }
        }),
        &["path"],
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
        "description": "Edit a file. Default workflow: `read` -> `edit`. Provide either Shape A (top-level old_content/new_content) or Shape B (edits[]); when both appear, `edits` wins. All segments match the file's ORIGINAL snapshot (no chained matching). Do not include read display prefixes (`  N\\t...` or `N#XX:...`) in `old_content`; for repeated short snippets or line-anchored edits, prefer `read(hashline=true)` + `hashline_edit`.",
        "properties": {
            "path": {
                "type": "string",
                "description": "Absolute or relative file path to edit."
            },
            "old_content": {
                "type": "string",
                "description": "Shape A only: exact existing text to replace; include enough context to make it unique unless `replace_all: true`. Copy the real file text, not read-only display prefixes like `  N\\t...` or `N#XX:...`."
            },
            "new_content": {
                "type": "string",
                "description": "Shape A only: replacement text."
            },
            "replace_all": {
                "type": "boolean",
                "description": "Shape A only: replace every occurrence of `old_content` instead of failing on multiple matches. Defaults to false."
            },
            "edits": {
                "type": "array",
                "minItems": 1,
                "description": "Shape B (preferred): list of edit segments applied to the file's ORIGINAL snapshot. Overlapping spans are rejected with Overlap.",
                "items": {
                    "type": "object",
                    "properties": {
                        "old_content": {
                            "type": "string",
                            "description": "Exact existing text to replace within this segment. Copy the real file text, not read-only display prefixes like `  N\\t...` or `N#XX:...`."
                        },
                        "new_content": {
                            "type": "string",
                            "description": "Replacement text for this segment."
                        },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Replace every occurrence of `old_content` for this segment. Defaults to false."
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
                "description": "Shell command to execute. With `args` set, runs argv-style without sh -c; otherwise runs through `sh -c` (Unix) / `cmd /C` (Windows)."
            },
            "cwd": {
                "type": "string",
                "description": "Optional working directory. Omit when not needed. Empty strings are treated as missing. Pass a real absolute path or `~/...`; shell env vars like `$HOME/...` are NOT expanded here. When omitted, falls back to the agent process working directory."
            },
            "args": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Optional argv elements appended to `command`. When present, the command runs argv-style (no shell) — safer for paths with spaces or quotes. When absent, the command is interpreted by the system shell."
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 1,
                "maximum": 600000,
                "description": "Optional wall-clock timeout in milliseconds. Defaults to 120000 (2 min); the runtime caps any value above 600000 (10 min). On timeout the child process is killed; the response carries `timed_out=true`. Ignored when `run_in_background=true` — background tasks have no implicit deadline; use `task_stop` to terminate them."
            },
            "run_in_background": {
                "type": "boolean",
                "description": "When true, spawn the command as a background task and return immediately with { task_id, log_path } instead of blocking the tool call until the process exits. Use this for builds, watchers or dev servers; pair with `task_output` (tail), `task_stop` (kill) and `task_list` (enumerate). Defaults to false."
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
                "description": "Byte offset to start reading from; pass the previous response's `next_offset` to tail. Defaults to 0 (read from start)."
            },
            "block": {
                "type": "boolean",
                "description": "If true, wait until new output arrives, the task finishes, or `timeout_ms` elapses. Returns an extra `wakeReason` field. Default false (non-blocking)."
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 0,
                "maximum": 30000,
                "description": "Wait slice in milliseconds for `block=true`. Default 5000, max 30000 (values above are capped). `0` is equivalent to `block=false`. Timeout is NOT a failure: when `wakeReason=\"timeout\" && finished=false`, you may call `task_output(block=true)` again to continue waiting."
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
        "description": "Line-anchored edit. Call `read(hashline=true)` first, then pass the returned `<line>#<2char>` anchors here. Anchors are validated against the file's current hashline before any write; mismatches return HashMismatch.",
        "properties": {
            "path": {
                "type": "string",
                "description": "Absolute or relative file path to edit."
            },
            "edits": {
                "type": "array",
                "minItems": 1,
                "description": "List of line-anchored edit operations applied against the CURRENT file content.",
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
                            "description": "Anchor for the start line, formatted `<1-based-line>#<2char-hash>` (e.g. `42#Ab`). For `insert`, content is inserted BEFORE this line."
                        },
                        "end": {
                            "type": "string",
                            "description": "Optional anchor for the inclusive end line (only valid for `replace` / `delete`). Defaults to `pos`."
                        },
                        "lines": {
                            "type": "string",
                            "description": "Replacement / insertion text (must end with a newline if multi-line). Ignored by `delete`."
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
                "description": "[both] Search expression. With target=content this is a ripgrep regex matched against file contents. With target=files this is a file-path glob such as `*.rs` or `src/**/*.rs`."
            },
            "target": {
                "type": "string",
                "enum": ["content", "files"],
                "description": "[both] What to search. `content` searches inside files; `files` searches file paths by glob. Defaults to `content`."
            },
            "path": {
                "type": "string",
                "description": "[both] Optional file or directory to search. Defaults to the current workspace path and must pass Read permission checks."
            },
            "glob": {
                "type": "string",
                "description": "[content only] Optional file glob filter such as `*.rs` or `**/*.md`. Omit when not used; do not pass an empty string."
            },
            "type": {
                "type": "string",
                "description": "[content only] Optional ripgrep file type filter such as `rust`, `js`, or `py`. Omit when not used; do not pass an empty string."
            },
            "output_mode": {
                "type": "string",
                "enum": ["content", "files_with_matches", "count"],
                "description": "[content only] Return matched lines, files with matches, or per-file counts. Defaults to `files_with_matches`."
            },
            "context": {
                "type": "integer",
                "minimum": 0,
                "description": "[content only] Number of surrounding context lines when output_mode=content. Ignored for other output modes."
            },
            "head_limit": {
                "anyOf": [
                    { "type": "integer", "minimum": 1, "maximum": 1024 },
                    { "type": "null" }
                ],
                "description": "[both] Maximum returned items after offset. Defaults to 64 for target=content and 128 for target=files. Use null for unlimited; 0 is rejected."
            },
            "offset": {
                "type": "integer",
                "minimum": 0,
                "description": "[both] Skip this many result items before applying head_limit. Use next_offset when truncated=true."
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
                "description": "Search query text. Required; prefer natural-language keywords that describe what the user wants to find."
            },
            "count": {
                "type": "integer",
                "minimum": 1,
                "maximum": 20,
                "description": "Optional number of hits to request. Defaults to 5 and is capped at 20."
            },
            "freshness": {
                "type": ["string", "null"],
                "enum": ["day", "week", "month", "year", null],
                "description": "Optional recency filter. Use `day`, `week`, `month`, or `year`; omit / pass null when no freshness constraint is needed."
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
                "description": "Optional allowlist of domains to constrain results to. Each item should be a bare host like `github.com`.",
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
                "description": "Target URL to fetch. Required; must be an http(s) URL without embedded credentials, localhost-style hosts, or private/IP-literal targets."
            },
            "prompt": {
                "type": ["string", "null"],
                "description": "Optional extraction intent. In the current MVP this is recorded as a warning only and does not change the fetched content."
            },
            "format": {
                "type": "string",
                "enum": ["markdown", "text"],
                "description": "Optional output format for textual pages. Defaults to `markdown`; use `text` when you want plain text without markdown syntax."
            }
        }),
        &["url"],
    )
}

fn config_get_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "key": { "type": "string", "description": "Configuration dot path, for example workspace.workspace_roots, workspace.entries, primitive.path_rules, primitive.bash_forbidden, or agent.id." }
        }),
        &["key"],
    )
}

fn config_set_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "key": { "type": "string", "description": "Allowed configuration dot path to update." },
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
                "description": "Concise plan objective (1–3 sentences) — what success looks like. Becomes the frontmatter `goal` field and the seed for the derived `plan_id`."
            },
            "draft": {
                "type": "string",
                "description": "Markdown content for the plan body's `## Plan` section: ordered bullet points or short paragraphs covering the approach, key decisions, and constraints (≤ ~2000 chars). The runtime wraps it with `## Goal` / `## Plan` / `## Todos Board`; do NOT include those headings yourself. If you accidentally include legacy headings such as `## Draft` or `## Notes`, runtime will normalize them."
            },
            "todos": {
                "type": "array",
                "description": "Initial flat todo list (≥ 1 item). `status` defaults to `pending`.",
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
        "description": "Apply incremental todo-only ops to the active plan. Callable in CHAT / PLAN / EXEC; requires an active plan. `plan_id` and `path` target the plan, `replace=true` replaces the whole todo list with the provided upsert results, and each op is tagged by `kind` (`upsert` / `set_status` / `remove`).",
        "properties": {
            "plan_id": {
                "type": "string",
                "description": "Target plan_id. Optional in EXEC mode (defaults to the active plan); REQUIRED in CHAT / PLAN / Pending / Completed."
            },
            "path": {
                "type": "string",
                "description": "Alternative target path under ~/.tomcat/plans/. If both `plan_id` and `path` are provided, `plan_id` wins."
            },
            "replace": {
                "type": "boolean",
                "description": "If true, replace the entire todos[] list with the upsert results in `ops`. Default false."
            },
            "ops": {
                "type": "array",
                "description": "Ordered list of mutations applied atomically (one frontmatter write under advisory lock).",
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
        "description": "Session-local todo scratchpad (any plan mode). Returns the full items snapshot after each call. It never writes the active PlanFile; advance plan todos via `update_plan`. Use `new_todos=true` to rotate to a new scratchpad file; use `replace=true` to replace the whole list with the provided upsert results.",
        "properties": {
            "new_todos": {
                "type": "boolean",
                "description": "If true, create a new active todos file for this session before applying ops. Default false."
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
        "description": "Block-await structured single-choice answers from the user (PLAN mode only). Each question must have 2–4 options with stable ids; exactly one option must carry `recommended: true`. The UI auto-appends a `__custom__` slot and a `skip` action for the current question — do not declare `__custom__` yourself.",
        "properties": {
            "questions": {
                "type": "array",
                "description": "1–4 questions presented to the user in one panel turn.",
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
                            "description": "2–4 options. Exactly one option must carry `recommended: true`.",
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
