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
        description: "Read a file from the local filesystem. Use this before editing or when the user asks to inspect file contents. Use list_dir for directories; binary or non-UTF-8 files return a structured hint with the detected first bytes instead of raw decode errors.\n",
        display_summary: Some("Read a file from an authorized path."),
        parameters: read_parameters,
        scope: PermissionScope::Read,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("read file text utf-8 inspect"),
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
    },
    BuiltinToolCatalogEntry {
        name: "edit",
        label: "Edit File",
        description: "Edit an existing text file by replacing exact text. Two input shapes are accepted:\n  Shape A (single edit, legacy): { path, old_content, new_content, replace_all? }\n  Shape B (multiple edits, preferred): { path, edits: [ { old_content, new_content, replace_all? }, ... ] }\nWhen both shapes appear, `edits` wins. Each segment matches against the file's ORIGINAL snapshot (no chained / incremental matching), so multi-segment edits are safe to compose. Set `replace_all: true` to replace every occurrence; otherwise the segment must match exactly once or the call returns an Ambiguous error. Read the file first (the tool requires a fresh read stamp; mtime/size mismatch returns a Stale error). Use write for new files or complete rewrites; do not use edit on binary files.\n",
        display_summary: Some("Replace exact text in an existing file (multi-segment, original-snapshot)."),
        parameters: edit_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("edit replace old_content new_content file"),
    },
    BuiltinToolCatalogEntry {
        name: "hashline_edit",
        label: "Hashline Edit File",
        description: "Edit a file with line-number + 2-char content hash anchors (use AFTER `read` with `hashline: true`). Each edit segment carries an anchor `<line>#<2char>` that must match the file's CURRENT content; if the line content changed, the anchor stops matching and the call returns HashMismatch (no write). Operations: `replace` (anchor → lines), `insert` (insert `lines` BEFORE anchor line), `delete` (anchor[..end] → empty). Use this when sub-string `edit` would be ambiguous (repeated short snippets) or when you need strong line-level consistency. Reads are still required first; the file's read stamp is checked.\n",
        display_summary: Some("Line-number + content-hash anchored edits (companion to read hashline=true)."),
        parameters: hashline_edit_parameters,
        scope: PermissionScope::Write,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("hashline edit line anchor hash"),
    },
    BuiltinToolCatalogEntry {
        name: "execute_bash",
        label: "Execute Bash",
        description: "Run a shell command through the permission gate. Use it for builds, tests, git inspection, and other command-line workflows. Avoid destructive commands unless the user explicitly asked and the permission prompt allows it. Prefer tool-native file APIs for reading or editing files; bash path access is still checked and audited as command execution.\n",
        display_summary: Some("Run an audited shell command."),
        parameters: execute_bash_parameters,
        scope: PermissionScope::Bash,
        category: None,
        read_only: false,
        destructive: true,
        search_hint: Some("bash shell command test build git"),
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
    },
    BuiltinToolCatalogEntry {
        name: "search_files",
        label: "Search Files",
        description: "Search authorized files by content regex or file path glob. Use target=content to search inside files and target=files to find file paths; target=files only uses pattern/path/head_limit/offset/include_hidden and silently ignores content-only fields.\n\nUse this instead of execute_bash with grep/find/ls -R. Use list_dir when you only need one directory level, and read when you already know the exact path.\n\nDual implementation with one schema: Tier1 spawns the system rg (content) and fd/fdfind (files); when either binary is missing search_files transparently falls back to Tier2 (in-process ignore::WalkBuilder + globset + Rust regex). Both tiers honour .gitignore/.ignore by default. Tier2 caveats are reported in `warnings`: regex dialect is the Rust regex crate (no lookaround/back-references; unsupported regex returns an empty match set with a warning); files larger than 5 MiB and binary files are skipped; before/after context lines are not emitted; the wall-clock budget defaults to 10s and can be overridden with PI_SEARCH_TIER2_DEADLINE_MS, after which the result is `truncated=true`.\n",
        display_summary: Some("Search authorized files by content or file-path glob."),
        parameters: search_files_parameters,
        scope: PermissionScope::Read,
        category: None,
        read_only: true,
        destructive: false,
        search_hint: Some("search grep glob files content regex"),
    },
    BuiltinToolCatalogEntry {
        name: "config_get",
        label: "Config Get",
        description: "Read the current value of an allowed pi configuration key. The tool is constrained by CONFIG_READ_ALLOWLIST and CONFIG_HARDCODED_READ_DENY: workspace.*, agent.id, primitive.path_rules, primitive.bash_*, llm.default_model and similar non-sensitive fields are readable; llm.api_key*, llm.api_base, security.*, storage.* and other sensitive fields are denied. Missing dot-path keys return not_set.\n",
        display_summary: Some("Read a non-sensitive pi configuration value."),
        parameters: config_get_parameters,
        scope: PermissionScope::Read,
        category: Some(ToolCategory::Config),
        read_only: true,
        destructive: false,
        search_hint: Some("config get workspace primitive model"),
    },
    BuiltinToolCatalogEntry {
        name: "config_set",
        label: "Config Set",
        description: "Append to or update an allowed pi configuration key. Every call shows the user a unified diff and requires confirmation. CONFIG_WRITE_ALLOWLIST and CONFIG_HARDCODED_WRITE_DENY protect sensitive or self-escalating fields.\n\nSemantics: array fields such as workspace.workspace_roots, workspace.entries, primitive.path_rules, primitive.bash_forbidden, and primitive.bash_approval_required accept value as one JSON element string and append it only. Scalar fields such as llm.default_model, log.level, and context.compaction_turns accept value as the replacement string. Deleting or arbitrary mutation is not supported; return an error that guides the user to pi config edit.\n\nForbidden fields include llm.api_key*, security.*, storage.*, agent.id, agent.workspace, and primitive.auto_confirm.\n",
        display_summary: Some("Modify an allowed pi configuration key after confirmation."),
        parameters: config_set_parameters,
        scope: PermissionScope::Write,
        category: Some(ToolCategory::Config),
        read_only: false,
        destructive: true,
        search_hint: Some("config set workspace roots path rules model"),
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
        "> Run `UPDATE_TOOL_CATALOG=1 cargo run --bin gen-tool-catalog` after catalog changes.\n\n",
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
                "description": "Render output with `cat -n` style line numbers (`{:>6}\\t{content}`); defaults to true. Set false only when piping the content into a tool that itself parses line numbers (e.g. diff)."
            },
            "hashline": {
                "type": "boolean",
                "description": "When true, render each line as `{:>6}#{2-char hash}:{content}` (xxh32 over whitespace-stripped content). Use for hashline-aware edits where you want both line addressing and external-edit detection. Mutually exclusive with line_numbers — hashline takes priority. Defaults to false."
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
        "description": "Edit a file. Provide either Shape A (top-level old_content/new_content) or Shape B (edits[]); when both appear, `edits` wins. All segments match the file's ORIGINAL snapshot (no chained matching).",
        "properties": {
            "path": {
                "type": "string",
                "description": "Absolute or relative file path to edit."
            },
            "old_content": {
                "type": "string",
                "description": "Shape A only: exact existing text to replace; include enough context to make it unique unless `replace_all: true`."
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
                            "description": "Exact existing text to replace within this segment."
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

fn execute_bash_parameters() -> Value {
    object_schema(
        serde_json::json!({
            "command": { "type": "string", "description": "Shell command to execute." },
            "cwd": { "type": "string", "description": "Optional working directory. Use the project cwd when the user asks to run in the current project." }
        }),
        &["command"],
    )
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
        "description": "Line-anchored edit. Each segment carries a `<line>#<2char>` anchor (output of `read hashline=true`). Anchors are validated against the file's current hashline before any write; mismatches return HashMismatch.",
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
                "description": "[content only] Optional file glob filter such as `*.rs` or `**/*.md`."
            },
            "type": {
                "type": "string",
                "description": "[content only] Optional ripgrep file type filter such as `rust`, `js`, or `py`."
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

#[cfg(test)]
#[path = "../tests/catalog_test.rs"]
mod tests;
