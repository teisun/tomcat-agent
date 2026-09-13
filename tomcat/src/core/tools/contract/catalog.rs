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
    /// 计划相关工具（`create_plan` / `update_plan` / `todos` / `ask_question`）。
    /// 工具始终在稳定的 LLM catalog 中；`true` 表示其 handler 必须按
    /// `PlanRuntime` 状态实施额外的 mode policy。详情见
    /// [`plan-runtime.md`](../../../../docs/architecture/plan-runtime.md) §4.1。
    pub plan_only: bool,
    /// 调用本工具是否需要等待用户交互（如 `ask_question` 在 CLI/IDE panel 阻塞 await）。
    /// 这是工具元属性，与 `read_only` / `destructive` 并列；写 catalog 时用作"工具是否会让 chat 主循环让出 stdin"的硬约束声明。
    /// 详见 [`ask-question.md`](../../../../docs/architecture/tools/ask-question.md) §4.2.1。
    pub requires_user_interaction: bool,
}

/// 工具在进程掉线后能否从持久化的调用声明安全地继续。
///
/// 这与 `read_only` 无关：read 不改磁盘却无法证明此前是否已经执行；ask_question
/// 需要用户交互却没有副作用，重连时重新展示同一问题是安全的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolReplaySafety {
    /// 工具的已声明但无结果调用可在重连后继续。
    ReplaySafe,
    /// 无法区分「没有执行」与「已部分执行但结果丢失」，不能静默重跑。
    NotReplaySafe,
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

    /// 恢复契约的唯一权威入口。新增可续跑工具必须在这里显式登记并补回归测试。
    pub fn replay_safety(&self) -> ToolReplaySafety {
        match self.name {
            "ask_question" => ToolReplaySafety::ReplaySafe,
            _ => ToolReplaySafety::NotReplaySafe,
        }
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
const G_EDIT_WORKFLOW: &str = "Default file-edit workflow: read -> edit; prefer edit for prose and Markdown. old_content must be small, unique, exact, and one continuous excerpt—never join distant excerpts. A successful edit returns a bounded, numbered current view around its changes: use that current text for the next edit; if its target is not shown, read again first. For repeated short snippets or line-anchored code edits, use read(hashline=true) -> hashline_edit. hashline_edit may batch non-overlapping ranges from one original snapshot: every anchor is checked before a write, then spans apply bottom-up, so line-count changes do not shift other batch anchors. Prefer continuous ranges for reliability. For distant ranges, prefer edit; if hashline_edit is necessary, take a fresh hashline read covering every target immediately before the call and never mix anchors from separate reads. To modify text created by the batch, read again first. When changing multiple independent files, issue one edit call per file in the SAME tool round instead of serializing file by file.";
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
        description: "Read a UTF-8 text file. Read a file before editing it. Use list_dir for directories; binary or non-UTF-8 files return a structured hint with the detected first bytes instead of a raw decode error. A wide read costs the same round trip as a narrow one, so when you are new to a file read a window that actually covers it rather than paging through it 40 lines at a time. Use `paths` to read several files in one call.\n",
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
        description: "Edit one existing text file with exactly `{ path, edits }`. Each segment has mode `replace` (default), `insert_before`, or `insert_after`: insert modes keep `old_content` as an anchor and preserve it. Each segment matches the file's ORIGINAL snapshot (no chained matching). Without `replace_all: true` a segment must match exactly once, else the call returns an Ambiguous error; use `replace_all: true` only when you intentionally want every occurrence changed. A successful result includes a bounded, numbered current view around changed ranges; use that exact text for a following edit, or read again if the next target is not shown. For multiple independent files, issue multiple edit calls in the SAME tool round. Read the file first (a fresh read stamp is required; mtime/size mismatch returns a Stale error). old_content must be a small, unique, continuous exact excerpt; do not combine distant text. Do NOT include `cat -n`/hashline display prefixes (`  N\\t...` or `N#XX:...`) in `old_content`. Use write for new files; do not edit binary files.\n",
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
        description: "Edit a file using line-number + 2-char content-hash anchors. Call `read(hashline=true)` first, then pass the returned `<line>#<2char>` anchors here. Each segment's anchor must match the file's CURRENT content; if the line changed, the anchor no longer matches and the call returns HashMismatch for every invalid anchor (no write). Multiple non-overlapping segments use one original snapshot: all anchors validate before writing, then spans apply bottom-up, so line-count changes do not shift other batch anchors. Operations: `replace` (anchor -> lines), `insert` (insert `lines` BEFORE the anchor line), `delete` (anchor[..end] -> empty). Prefer normal `edit` for prose and Markdown; use this for repeated short snippets or strong line-level consistency. Prefer continuous ranges; for distant ranges, use a fresh hashline read covering every target. To modify text created by this batch, read again first. A fresh read stamp is still required.\n",
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
        description: "Read incremental output from a background `bash` task (started with run_in_background=true). Returns a UTF-8 lossy chunk from `since` plus `finished` and `exit_code`; pass the previous response's `next_offset` as the next `since` to tail across turns (first call may omit `since`). `block=false` (default) returns immediately; `block=true` waits until the task finishes or `wait_ms` elapses (default 5000; block=true clamps to 5000-600000ms; block=false ignores wait_ms). For builds and tests, prefer `block=true` with `wait_ms` at least 300000, up to 600000, unless another task truly needs the result sooner. Mid-stream output does not interrupt the wait. Blocking waits add a `wakeReason` of `finished` | `wait_window_elapsed`; a `wait_window_elapsed` wakeReason is NOT a failure, so inspect `content` first (`content=\"\"` means no new output arrived during that slice) and wait again only if you still need to. Do not busy-poll. See the background bash tasks section in the system prompt for the full workflow.\n",
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
        description: "Search authorized files by content regex or file-path glob. Use target=content to search inside files and target=files to find file paths; target=files only uses pattern/path/head_limit/offset/include_hidden. Use list_dir for a single directory level and read when you already know the path.\n",
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
        name: "tool_search",
        label: "Tool Search",
        description: "Discover deferred tools from connected connectors (MCP now; CLI/A2A/plugins later) that are intentionally kept OUT of your tool set to keep the prompt small and cache-stable. When load_skill is available, before first use in a session you MUST load the \"connectors\" skill via load_skill(\"connectors\") — it teaches the full search -> describe -> call workflow and when to batch calls via code. Modes: tool_search() lists sources (name, type, description, tool_count); tool_search(source=\"…\") lists that source's tool names + short descriptions (no schema); tool_search(query=\"…\") keyword-searches across sources (add source= to scope). Then use tool_describe([names]) to fetch schemas and tool_call(name, arguments) to invoke. Output returns in the conversation, never in the prompt prefix.\n",
        display_summary: Some("Discover deferred connector tools without expanding the prompt."),
        parameters: tool_search_parameters,
        scope: PermissionScope::Read,
        category: Some(ToolCategory::Exec),
        read_only: true,
        destructive: false,
        search_hint: Some("deferred connector MCP CLI A2A plugin tool search discover"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "tool_describe",
        label: "Tool Describe",
        description: "Fetch full input schemas for one or more deferred tools found via tool_search. Pass names (array; a single tool is [\"name\"]). Returns each tool's inputSchema + description in input order; unknown names are reported in errors without failing the rest. Batch related tools in one call to avoid repeated round trips, then tool_call to invoke.\n",
        display_summary: Some("Fetch schemas for one or more deferred tools."),
        parameters: tool_describe_parameters,
        scope: PermissionScope::Read,
        category: Some(ToolCategory::Exec),
        read_only: true,
        destructive: false,
        search_hint: Some("deferred connector MCP tool schema describe batch"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "tool_call",
        label: "Tool Call",
        description: "Invoke one deferred tool found via tool_search/tool_describe. Pass name (e.g. mcp__<source>__<tool>) and arguments matching its inputSchema. Runs through the same trust/permission gate as native connector tools; image results stream back to you. To call one tool many times (fan-out) or filter large results, prefer writing code (see the connectors skill) over many tool_call rounds.\n",
        display_summary: Some("Invoke a deferred connector tool through its stable name."),
        parameters: tool_call_parameters,
        scope: PermissionScope::Bash,
        category: Some(ToolCategory::Exec),
        read_only: false,
        destructive: false,
        search_hint: Some("deferred connector MCP CLI A2A plugin tool invoke call"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "tool_run_code",
        label: "Tool Run Code",
        description: "Run short JavaScript to call deferred connector tools repeatedly and return one compact final value. Before use, load the connectors skill for the allowed callTool(name, arguments) API and examples. Use this only for fan-out, filtering a large result, or aggregation; use tool_call for a short sequential workflow. The code runs in the existing plugin QuickJS VM with the configured heap, timeout, and interrupt limits. It has no host filesystem, shell, network, or arbitrary host API: callTool is its only host capability and uses the same connector trust path as tool_call. Return a JSON-compatible value. MCP image blocks are extracted for vision before final text is truncated.\n",
        display_summary: Some("Run JS for deferred-tool fan-out and result reduction."),
        parameters: tool_run_code_parameters,
        scope: PermissionScope::Bash,
        category: Some(ToolCategory::Exec),
        read_only: false,
        destructive: false,
        search_hint: Some("deferred connector MCP JavaScript code fan-out aggregate filter"),
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
    BuiltinToolCatalogEntry {
        name: "package_install",
        label: "Package Install",
        description: "Install a local Tomcat package, skill, or plugin after confirmation. Pass a local source path and scope (`scope`, `agent`, or `global`); `scope` requires the session's explicit project root. The operation validates the package before writing and returns a machine-readable `status` (`installed`, `cancelled`, `denied`, or `failed`) plus installed resources and `inventory_dirty` for the next session refresh.\n",
        display_summary: Some("Install a local package after confirmation."),
        parameters: package_install_parameters,
        scope: PermissionScope::Write,
        category: Some(ToolCategory::Config),
        read_only: false,
        destructive: true,
        search_hint: Some("package install skill plugin source scope global agent"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    BuiltinToolCatalogEntry {
        name: "dispatch_agent",
        label: "Dispatch Agent",
        description: "Delegate high-latency, read-only codebase investigation to one or more explorer subagents running in parallel. Direct `search_files` / `read` calls are the default. Do NOT dispatch for a simple task, project rules or AGENTS/README files, a known file or symbol, confirmation of one implementation, a question that can be answered in one or two batches of direct-tool calls, self-review or final audit of your own Plan, or work already covered by the automatic Plan/Code Reviewer. Use this tool only when all are true: (1) the task crosses multiple unknown subsystems, (2) the questions can be investigated independently in parallel, and (3) returning the necessary raw reads would materially bloat the parent context. Before dispatching, put every currently known independent question into one `tasks` array. After reports return, prefer direct tools to fill evidence gaps. Dispatch again only if the reports reveal a new blocker that direct tools cannot answer. Pass `tasks`: 1-6 entries of `{ id, prompt }`, where `prompt` is self-contained because the subagent sees none of this conversation, and `id` matches answers to questions. Each subagent may only read (`read` / `search_files` / `list_dir` / read-only `bash`) and returns concise findings with `path:line` references plus a conclusion, never raw file contents. Only its final report enters the parent context.\n",
        display_summary: Some("Run parallel read-only explorer subagents that return findings, not file contents."),
        parameters: dispatch_agent_parameters,
        scope: PermissionScope::Read,
        category: Some(ToolCategory::Exec),
        read_only: true,
        destructive: false,
        search_hint: Some("dispatch agent explorer subagent investigate parallel findings"),
        prompt_guidelines: &[],
        plan_only: false,
        requires_user_interaction: false,
    },
    // ─── PLAN 模式专属工具（T2-P1-002/003）：plan_only=true，默认 chat catalog 不暴露 ───
    BuiltinToolCatalogEntry {
        name: "create_plan",
        label: "Create Plan",
        description: "Create a new plan file under `~/.tomcat/plans/<slug>_<hash>.plan.md` (PLAN mode only). Pass `goal` (short objective), `draft` (plan-body content), and an initial flat `todos` list; the runtime derives `plan_id` from goal (do NOT pass plan_id), normalizes `draft` into the `## Plan` section, writes frontmatter under an advisory lock, then appends runtime-owned `[gate] review` and `[gate] Acceptance` todos to the end of the returned list. The gates are the visible close-out flow and must not be supplied by the caller. The runtime then runs an advisory reviewer whose summary rides back on this tool's result `review` field. Reviewer output is advisory only and does NOT gate `/plan build`. Calling outside Planning returns a tool error.\n",
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
        description: "Apply incremental todo-only ops (`upsert` / `set_status` / `remove`) to the active plan, persisted to its `.plan.md` frontmatter under an advisory lock. Visible in CHAT / PLAN / EXEC. `plan_id` and `path` target the plan; `replace=true` swaps the entire todo list with the provided upsert results. In EXEC, an existing work todo's `content` is frozen; record progress and verification in `set_status.evidence` instead. Up to three independent todos may be `in_progress`. When all todos reach `completed` in EXEC, the runtime runs applicable completion gates before allowing state=completed. Only frontmatter.todos is mutated; plan body markdown is left untouched.\n",
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
        description: "Manage a session-local todo scratchpad and return a full snapshot of all items after each call. It NEVER writes the active PlanFile (advance plan todos via `update_plan`); when persistence is configured it is stored at `~/.tomcat/agents/<id>/todos/<session_id>.todo.md`. Use `new_todos=true` to clear the scratchpad and start fresh; use `replace=true` to replace the whole list with the provided upsert results. At most three independent todos may be `in_progress`.\n",
        display_summary: Some("Maintain a session todo scratchpad (up to three in_progress; returns full snapshot)."),
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

/// 供 session hydrate / resume 使用的工具恢复契约查询。
pub fn is_replay_safe_tool(name: &str) -> bool {
    builtin_tool_by_name(name)
        .is_some_and(|entry| entry.replay_safety() == ToolReplaySafety::ReplaySafe)
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

/// The two prompt representations of the stable built-in catalog.
///
/// Build this once per request surface so the function array and the system
/// prompt's human-readable tool list cannot drift apart.
#[derive(Debug, Clone)]
pub struct BuiltinToolSurface {
    pub function_definitions: Vec<Value>,
    pub identity_tool_lines: String,
}

pub fn builtin_tool_surface_with_policy(allow_load_skill: bool) -> BuiltinToolSurface {
    builtin_tool_surface_with_policies(allow_load_skill, false)
}

/// Builds the stable builtin surface from configuration-only policies. In
/// particular, deferred connector tools must not appear/disappear with MCP's
/// asynchronous Ready state, because this surface is part of the prompt prefix.
pub fn builtin_tool_surface_with_policies(
    allow_load_skill: bool,
    allow_connector_tools: bool,
) -> BuiltinToolSurface {
    builtin_tool_surface_with_policies_and_install(allow_load_skill, allow_connector_tools, true)
}

/// Builds a session-aware tool surface. Package installation is omitted while a
/// session is planning, so the model cannot propose a write-only operation that
/// the runtime will reject. The execution guard remains authoritative.
pub fn builtin_tool_surface_with_policies_and_install(
    allow_load_skill: bool,
    allow_connector_tools: bool,
    allow_package_install: bool,
) -> BuiltinToolSurface {
    let entries = BUILTIN_TOOL_CATALOG
        .iter()
        .filter(|entry| allow_load_skill || entry.name != "load_skill")
        .filter(|entry| allow_package_install || entry.name != "package_install")
        .filter(|entry| {
            allow_connector_tools
                || !matches!(
                    entry.name,
                    "tool_search" | "tool_describe" | "tool_call" | "tool_run_code"
                )
        })
        .collect::<Vec<_>>();
    BuiltinToolSurface {
        function_definitions: entries
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
            .collect(),
        identity_tool_lines: entries
            .iter()
            .map(|entry| format!("- {}: {}", entry.name, entry.display_summary()))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

pub fn render_core_identity_tool_lines() -> String {
    render_core_identity_tool_lines_with_policy(true)
}

pub fn render_core_identity_tool_lines_with_policy(allow_load_skill: bool) -> String {
    builtin_tool_surface_with_policy(allow_load_skill).identity_tool_lines
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

fn shared_todo_op_item_schema(status_description: &str, include_evidence: bool) -> Value {
    let mut set_status_properties = serde_json::json!({
        "kind": {
            "type": "string",
            "const": "set_status",
            "description": "Operation kind."
        },
        "id": {
            "type": "string",
            "description": "Target todo id (kebab-case)."
        },
        "status": todo_status_property(status_description),
    });
    if include_evidence {
        set_status_properties["evidence"] = serde_json::json!({
            "type": "array",
            "items": {"type": "string"},
            "description": "Completion evidence. Only valid with status=`completed`; include concrete verification such as task:<id>, file:<path>, or a command outcome."
        });
    }
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
                "properties": set_status_properties,
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
            "path": { "type": "string", "description": "Absolute or relative file path to read as UTF-8 text. Provide exactly one of `path` or `paths`." },
            "paths": {
                "type": "array",
                "minItems": 1,
                "maxItems": 10,
                "description": "Read several files in one call. Mutually exclusive with `path`. Entries are read in order and share one output budget; anything that does not fit is reported as SKIPPED with a resume call rather than dropped silently. Keep a batch to 3-5 files so the combined result stays small enough to remain inline.",
                "items": object_schema(
                    serde_json::json!({
                        "path": { "type": "string", "description": "File to read." },
                        "offset": { "type": "integer", "minimum": 1, "description": "Optional 1-based start line for this entry." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 10000, "description": "Optional max lines for this entry." }
                    }),
                    &["path"],
                )
            },
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
                "description": "Render `cat -n` style line numbers (default true). Applies to every entry when using `paths`. These prefixes are display-only — do not paste `  N\\t...` into edit.old_content."
            },
            "hashline": {
                "type": "boolean",
                "description": "Render each line as `{line}#{2-char hash}:{content}` for use with hashline_edit. Display-only prefix — do not paste into edit.old_content. Mutually exclusive with line_numbers (hashline wins). Default false."
            }
        }),
        &[],
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

fn tool_search_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "query": {
                "type": ["string", "null"],
                "description": "Optional keywords for cross-source search. Omit together with source to list sources."
            },
            "source": {
                "type": ["string", "null"],
                "description": "Optional connector source name. Without query, lists this source's tool cards; with query, scopes search."
            },
            "limit": {
                "type": ["integer", "null"],
                "minimum": 1,
                "maximum": 100,
                "description": "Maximum returned entries. Defaults to 20."
            },
            "offset": {
                "type": ["integer", "null"],
                "minimum": 0,
                "description": "Number of entries to skip. Defaults to 0."
            }
        }),
        &[],
    )
}

fn tool_describe_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "names": {
                "type": "array",
                "minItems": 1,
                "description": "One or more canonical names returned by tool_search. Batch related tools to fetch schemas together.",
                "items": {
                    "type": "string",
                    "minLength": 1
                }
            }
        }),
        &["names"],
    )
}

fn tool_call_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "name": {
                "type": "string",
                "minLength": 1,
                "description": "Canonical deferred tool name returned by tool_search."
            },
            "arguments": {
                "type": "object",
                "description": "Arguments matching the inputSchema returned by tool_describe."
            }
        }),
        &["name", "arguments"],
    )
}

fn tool_run_code_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "code": {
                "type": "string",
                "minLength": 1,
                "description": "JavaScript function body. Use await callTool(name, arguments) and return one JSON-compatible final value."
            }
        }),
        &["code"],
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
    object_schema(
        serde_json::json!({
            "path": {
                "type": "string",
                "description": "Absolute or relative path to the one existing file to edit."
            },
            "edits": {
                "type": "array",
                "minItems": 1,
                "description": "One or more independent segments applied to this file's ORIGINAL snapshot. Overlapping spans are rejected.",
                "items": {
                    "type": "object",
                    "properties": {
                        "old_content": {
                            "type": "string",
                            "description": "Exact existing text or insertion anchor (real file text; no read display prefixes)."
                        },
                        "new_content": {
                            "type": "string",
                            "description": "Replacement text, or text to insert for insert modes."
                        },
                        "mode": {
                            "type": "string",
                            "enum": ["replace", "insert_before", "insert_after"],
                            "description": "Defaults to replace. Insert modes preserve old_content as the anchor."
                        },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Change every occurrence intentionally; defaults to false. Valid only with mode=replace."
                        }
                    },
                    "required": ["old_content", "new_content"],
                    "additionalProperties": false
                }
            }
        }),
        &["path", "edits"],
    )
}

fn bash_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "command": {
                "type": "string",
                "description": "Complete shell command line, including the executable and every argument. Supports pipes, &&, ||, ;, redirects, globbing, and quotes. Always runs via `sh -c` (Unix) / `cmd /C` (Windows)."
            },
            "cwd": {
                "type": "string",
                "description": "Optional working directory. Empty means unset. Use an absolute path or `~/...`; shell vars like `$HOME` are NOT expanded here. Defaults to the agent process cwd."
            },
            "foreground_wait_ms": {
                "type": "integer",
                "description": "How long this call waits in the foreground. Values clamp to 8000-16000ms; expiry keeps the same tracked process running. Ignored when run_in_background=true."
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
                "description": "If true, wait until the task finishes or `wait_ms` elapses, and return wakeReason `finished` or `wait_window_elapsed`. Mid-stream output does not interrupt the wait. Default false."
            },
            "wait_ms": {
                "type": "integer",
                "minimum": 0,
                "maximum": 600000,
                "description": "Observation window for block=true (default 5000, clamped to 5000-600000ms). Ignored when block=false. It never stops the task."
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
        "description": "Line-anchored edit. Call `read(hashline=true)` first, then pass the returned `<line>#<2char>` anchors here. Anchors are validated against the file's current content before any write; invalid anchors are all reported and no write occurs.",
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

fn dispatch_agent_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "tasks": {
                "type": "array",
                "minItems": 1,
                "maxItems": 6,
                "description": "Investigation tasks to run in parallel. Keep each one scoped to a single area; split unrelated questions into separate tasks instead of writing one broad prompt.",
                "items": object_schema(
                    serde_json::json!({
                        "id": {
                            "type": "string",
                            "description": "Short unique label such as `webview-paste`. Used to match the returned report back to this task. Defaults to `task-<n>` when omitted."
                        },
                        "prompt": {
                            "type": "string",
                            "description": "Self-contained question. The subagent cannot see this conversation, so state the goal, the area to look at, and what a useful answer contains."
                        }
                    }),
                    &["prompt"],
                )
            }
        }),
        &["tasks"],
    )
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
                "description": "[content only] Surrounding context lines when output_mode=content. Defaults to 3 so you can judge a hit without a follow-up read; pass 0 for matched lines only. Ignored otherwise."
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

fn package_install_parameters() -> Value {
    let mut schema = object_schema(
        serde_json::json!({
            "source": { "type": "string", "description": "Absolute local directory or package.json/plugin.json/SKILL.md to install." },
            "scope": { "type": "string", "enum": ["scope", "agent", "global"], "description": "Installation scope. Defaults to scope. Scope requires this session to have an explicit project root." }
        }),
        &["source"],
    );
    schema["additionalProperties"] = Value::Bool(false);
    schema
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
                            "description": "Initial status. Defaults to `pending`."
                        }
                    },
                    "required": ["id", "content"]
                }
            },
            "acceptance_commands": {
                "type": "array",
                "description": "Declared acceptance commands: the mandatory floor that `[gate] Acceptance` must run. One complete, runnable shell command per entry, exactly as it will be launched (put any `cd` inside the command). Size it to the change's impact radius; the verify skill decides how far beyond this floor to widen. Blank entries are dropped and duplicates removed.",
                "items": { "type": "string" }
            }
        },
        "required": ["goal", "draft", "todos"]
    })
}

fn update_plan_parameters() -> Value {
    serde_json::json!({
        "type": "object",
        "description": "Apply todo ops, submit a P1 code-review dispute, or submit verified green-build evidence to the active plan. Callable in CHAT / PLAN / EXEC; requires an active plan. `replace=true` swaps the whole todo list with the upsert results; each op is tagged by `kind` (`upsert` / `set_status` / `remove`).",
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
                "description": "Ordered todo mutations applied atomically. Omit or pass [] only when submitting dispute_findings or green-build evidence.",
                "items": shared_todo_op_item_schema(
                    "For `upsert` (optional) and `set_status` (required). At most one todo may be `in_progress`; `in_progress` only allowed when plan.state == executing.",
                    true,
                )
            },
            "dispute_findings": {
                "type": "array",
                "description": "P1 findings, or P0 findings after an explicit user acknowledgement of a P0 handoff, that the main Agent accepts as a trade-off. Use only for wontfix; fixing code is communicated by a later review, not here.",
                "items": {
                    "type": "object",
                    "properties": {
                        "ref": { "type": "string", "description": "Round-local finding reference such as F01." },
                        "area": { "type": "string", "description": "Finding area copied for audit readability; matching uses ref." },
                        "resolution": { "type": "string", "enum": ["wontfix"] },
                        "reason": { "type": "string", "description": "Concrete accepted trade-off reason." }
                    },
                    "required": ["ref", "area", "resolution", "reason"],
                    "additionalProperties": false
                }
            },
            "green_build_pass": {
                "type": "boolean",
                "description": "Set true only after loading the verify skill and completing its background acceptance commands."
            },
            "green_build_evidence": {
                "type": "array",
                "description": "Finished background bash commands used as green-build evidence. Required with green_build_pass=true; command must exactly match the recorded task, and every declared `acceptance_commands` entry must appear here with its own task_id (extra commands are fine, narrower substitutes are not).",
                "items": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "task_id": { "type": "string" }
                    },
                    "required": ["command", "task_id"],
                    "additionalProperties": false
                }
            },
            "acceptance_commands": {
                "type": "array",
                "description": "Complete replacement for the plan's declared acceptance command list (one runnable command per entry). Omit to leave it unchanged. While planning/pending the list is replaced as given; once executing it is a ratchet: the new list must still contain every previously declared command, so it can only grow. A completed plan is immutable.",
                "items": { "type": "string" }
            }
        }
    })
}

fn todos_parameters() -> Value {
    serde_json::json!({
        "type": "object",
        "description": "Session-local todo scratchpad (visible in CHAT, PLAN, and EXEC). Returns the full items snapshot after each call. It never writes the active PlanFile (advance plan todos via `update_plan`). `new_todos=true` clears the scratchpad and starts fresh; `replace=true` swaps the whole list with the upsert results.",
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
                    "For `upsert` (optional) and `set_status` (required).",
                    false,
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
