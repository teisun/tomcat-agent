# Tool Catalog

> This file is generated from `src/core/tools/contract/catalog.rs`.
> Run `UPDATE_TOOL_CATALOG=1 cargo run --bin gen-tool-catalog` after catalog changes.
> `checkpoint` / `restore` 不在 tool catalog 中：它们是 `tomcat chat` 的本地斜杠命令（`/ckpt`、`/restore`），由 chat 层直接处理，不暴露给 LLM 作为工具。

## Filesystem

### `read`

- Label: Read
- Category: `filesystem`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `read file text utf-8 inspect`

Read a file from the local filesystem. Use this before editing or when the user asks to inspect file contents. Use list_dir for directories; binary or non-UTF-8 files return a structured hint with the detected first bytes instead of raw decode errors.

Parameters:

```json
{
  "properties": {
    "hashline": {
      "description": "When true, render each line as `{:>6}#{2-char hash}:{content}` (xxh32 over whitespace-stripped content). Use for hashline-aware edits where you want both line addressing and external-edit detection. Mutually exclusive with line_numbers — hashline takes priority. Defaults to false.",
      "type": "boolean"
    },
    "limit": {
      "description": "Optional max number of lines to return; defaults to 2000. When the file has more lines, the result includes a `... [N more lines truncated; resume with offset=<next>, limit=<same>]` hint so you can paginate.",
      "maximum": 10000,
      "minimum": 1,
      "type": "integer"
    },
    "line_numbers": {
      "description": "Render output with `cat -n` style line numbers (`{:>6}\\t{content}`); defaults to true. Set false only when piping the content into a tool that itself parses line numbers (e.g. diff).",
      "type": "boolean"
    },
    "offset": {
      "description": "Optional 1-based line number to start reading from. Defaults to 1 (first line).",
      "minimum": 1,
      "type": "integer"
    },
    "path": {
      "description": "Absolute or relative file path to read as UTF-8 text.",
      "type": "string"
    }
  },
  "required": [
    "path"
  ],
  "type": "object"
}
```

### `write`

- Label: Write File
- Category: `filesystem`
- Permission scope: `Write`
- Read only: `false`
- Destructive: `true`
- Search hint: `write create overwrite file`

Create or overwrite a file at an authorized path. Use this for new files or complete rewrites when the intended final content is known. Prefer edit for small surgical changes to existing files. Writes may require user confirmation and are audited.

Parameters:

```json
{
  "properties": {
    "content": {
      "description": "Full file content to write.",
      "type": "string"
    },
    "overwrite": {
      "description": "Whether an existing file may be overwritten. Defaults to false.",
      "type": "boolean"
    },
    "path": {
      "description": "Absolute or relative file path to create or overwrite.",
      "type": "string"
    }
  },
  "required": [
    "path",
    "content"
  ],
  "type": "object"
}
```

### `edit`

- Label: Edit File
- Category: `filesystem`
- Permission scope: `Write`
- Read only: `false`
- Destructive: `true`
- Search hint: `edit replace old_content new_content file`

Edit an existing text file by replacing exact text. Two input shapes are accepted:
  Shape A (single edit, legacy): { path, old_content, new_content, replace_all? }
  Shape B (multiple edits, preferred): { path, edits: [ { old_content, new_content, replace_all? }, ... ] }
When both shapes appear, `edits` wins. Each segment matches against the file's ORIGINAL snapshot (no chained / incremental matching), so multi-segment edits are safe to compose. Set `replace_all: true` to replace every occurrence; otherwise the segment must match exactly once or the call returns an Ambiguous error. Read the file first (the tool requires a fresh read stamp; mtime/size mismatch returns a Stale error). Use write for new files or complete rewrites; do not use edit on binary files.

Parameters:

```json
{
  "description": "Edit a file. Provide either Shape A (top-level old_content/new_content) or Shape B (edits[]); when both appear, `edits` wins. All segments match the file's ORIGINAL snapshot (no chained matching).",
  "properties": {
    "edits": {
      "description": "Shape B (preferred): list of edit segments applied to the file's ORIGINAL snapshot. Overlapping spans are rejected with Overlap.",
      "items": {
        "additionalProperties": false,
        "properties": {
          "new_content": {
            "description": "Replacement text for this segment.",
            "type": "string"
          },
          "old_content": {
            "description": "Exact existing text to replace within this segment.",
            "type": "string"
          },
          "replace_all": {
            "description": "Replace every occurrence of `old_content` for this segment. Defaults to false.",
            "type": "boolean"
          }
        },
        "required": [
          "old_content",
          "new_content"
        ],
        "type": "object"
      },
      "minItems": 1,
      "type": "array"
    },
    "new_content": {
      "description": "Shape A only: replacement text.",
      "type": "string"
    },
    "old_content": {
      "description": "Shape A only: exact existing text to replace; include enough context to make it unique unless `replace_all: true`.",
      "type": "string"
    },
    "path": {
      "description": "Absolute or relative file path to edit.",
      "type": "string"
    },
    "replace_all": {
      "description": "Shape A only: replace every occurrence of `old_content` instead of failing on multiple matches. Defaults to false.",
      "type": "boolean"
    }
  },
  "required": [
    "path"
  ],
  "type": "object"
}
```

### `hashline_edit`

- Label: Hashline Edit File
- Category: `filesystem`
- Permission scope: `Write`
- Read only: `false`
- Destructive: `true`
- Search hint: `hashline edit line anchor hash`

Edit a file with line-number + 2-char content hash anchors (use AFTER `read` with `hashline: true`). Each edit segment carries an anchor `<line>#<2char>` that must match the file's CURRENT content; if the line content changed, the anchor stops matching and the call returns HashMismatch (no write). Operations: `replace` (anchor → lines), `insert` (insert `lines` BEFORE anchor line), `delete` (anchor[..end] → empty). Use this when sub-string `edit` would be ambiguous (repeated short snippets) or when you need strong line-level consistency. Reads are still required first; the file's read stamp is checked.

Parameters:

```json
{
  "description": "Line-anchored edit. Each segment carries a `<line>#<2char>` anchor (output of `read hashline=true`). Anchors are validated against the file's current hashline before any write; mismatches return HashMismatch.",
  "properties": {
    "edits": {
      "description": "List of line-anchored edit operations applied against the CURRENT file content.",
      "items": {
        "additionalProperties": false,
        "properties": {
          "end": {
            "description": "Optional anchor for the inclusive end line (only valid for `replace` / `delete`). Defaults to `pos`.",
            "type": "string"
          },
          "lines": {
            "description": "Replacement / insertion text (must end with a newline if multi-line). Ignored by `delete`.",
            "type": "string"
          },
          "op": {
            "description": "Edit operation kind.",
            "enum": [
              "replace",
              "insert",
              "delete"
            ],
            "type": "string"
          },
          "pos": {
            "description": "Anchor for the start line, formatted `<1-based-line>#<2char-hash>` (e.g. `42#Ab`). For `insert`, content is inserted BEFORE this line.",
            "type": "string"
          }
        },
        "required": [
          "op",
          "pos"
        ],
        "type": "object"
      },
      "minItems": 1,
      "type": "array"
    },
    "path": {
      "description": "Absolute or relative file path to edit.",
      "type": "string"
    }
  },
  "required": [
    "path",
    "edits"
  ],
  "type": "object"
}
```

### `list_dir`

- Label: List Directory
- Category: `filesystem`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `list directory files`

List the immediate contents of an authorized directory. Use this to discover nearby files before choosing read or edit. It does not recurse; call it on subdirectories as needed instead of guessing paths.

Parameters:

```json
{
  "properties": {
    "path": {
      "description": "Directory path to list without recursion.",
      "type": "string"
    }
  },
  "required": [
    "path"
  ],
  "type": "object"
}
```

### `search_files`

- Label: Search Files
- Category: `filesystem`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `search grep glob files content regex`

Search authorized files by content regex or file path glob. Use target=content to search inside files and target=files to find file paths; target=files only uses pattern/path/head_limit/offset/include_hidden and silently ignores content-only fields.

Use this instead of bash with grep/find/ls -R. Use list_dir when you only need one directory level, and read when you already know the exact path.

Dual implementation with one schema: Tier1 spawns the system rg (content) and fd/fdfind (files); when either binary is missing search_files transparently falls back to Tier2 (in-process ignore::WalkBuilder + globset + Rust regex). Both tiers honour .gitignore/.ignore by default. Tier2 caveats are reported in `warnings`: regex dialect is the Rust regex crate (no lookaround/back-references; unsupported regex returns an empty match set with a warning); files larger than 5 MiB and binary files are skipped; before/after context lines are not emitted; the wall-clock budget defaults to 10s and can be overridden with PI_SEARCH_TIER2_DEADLINE_MS, after which the result is `truncated=true`.

Parameters:

```json
{
  "properties": {
    "case_insensitive": {
      "description": "[content only] Ignore case, equivalent to ripgrep -i. Defaults to false.",
      "type": "boolean"
    },
    "context": {
      "description": "[content only] Number of surrounding context lines when output_mode=content. Ignored for other output modes.",
      "minimum": 0,
      "type": "integer"
    },
    "glob": {
      "description": "[content only] Optional file glob filter such as `*.rs` or `**/*.md`.",
      "type": "string"
    },
    "head_limit": {
      "anyOf": [
        {
          "maximum": 1024,
          "minimum": 1,
          "type": "integer"
        },
        {
          "type": "null"
        }
      ],
      "description": "[both] Maximum returned items after offset. Defaults to 64 for target=content and 128 for target=files. Use null for unlimited; 0 is rejected."
    },
    "include_hidden": {
      "description": "[both] Include hidden files and directories. Defaults to false; .gitignore is still respected.",
      "type": "boolean"
    },
    "offset": {
      "description": "[both] Skip this many result items before applying head_limit. Use next_offset when truncated=true.",
      "minimum": 0,
      "type": "integer"
    },
    "output_mode": {
      "description": "[content only] Return matched lines, files with matches, or per-file counts. Defaults to `files_with_matches`.",
      "enum": [
        "content",
        "files_with_matches",
        "count"
      ],
      "type": "string"
    },
    "path": {
      "description": "[both] Optional file or directory to search. Defaults to the current workspace path and must pass Read permission checks.",
      "type": "string"
    },
    "pattern": {
      "description": "[both] Search expression. With target=content this is a ripgrep regex matched against file contents. With target=files this is a file-path glob such as `*.rs` or `src/**/*.rs`.",
      "type": "string"
    },
    "target": {
      "description": "[both] What to search. `content` searches inside files; `files` searches file paths by glob. Defaults to `content`.",
      "enum": [
        "content",
        "files"
      ],
      "type": "string"
    },
    "type": {
      "description": "[content only] Optional ripgrep file type filter such as `rust`, `js`, or `py`.",
      "type": "string"
    }
  },
  "required": [
    "pattern"
  ],
  "type": "object"
}
```

## Exec

### `bash`

- Label: Bash
- Category: `exec`
- Permission scope: `Bash`
- Read only: `false`
- Destructive: `true`
- Search hint: `bash shell command test build git background`

Run a shell command through the permission gate. Use it for builds, tests, git inspection, and other command-line workflows. Avoid destructive commands unless the user explicitly asked and the permission prompt allows it. Prefer tool-native file APIs for reading or editing files; bash path access is still checked and audited as command execution.

Set `run_in_background: true` for long-running commands (builds, watchers, dev servers). The call returns immediately with a `task_id` + `log_path`; use `task_output` / `task_stop` / `task_list` to drive the task across follow-up turns instead of blocking a single tool round.

Parameters:

```json
{
  "properties": {
    "args": {
      "description": "Optional argv elements appended to `command`. When present, the command runs argv-style (no shell) — safer for paths with spaces or quotes. When absent, the command is interpreted by the system shell.",
      "items": {
        "type": "string"
      },
      "type": "array"
    },
    "command": {
      "description": "Shell command to execute. With `args` set, runs argv-style without sh -c; otherwise runs through `sh -c` (Unix) / `cmd /C` (Windows).",
      "type": "string"
    },
    "cwd": {
      "description": "Optional working directory. Use the project cwd when the user asks to run in the current project; missing falls back to the agent process working directory.",
      "type": "string"
    },
    "run_in_background": {
      "description": "When true, spawn the command as a background task and return immediately with { task_id, log_path } instead of blocking the tool call until the process exits. Use this for builds, watchers or dev servers; pair with `task_output` (tail), `task_stop` (kill) and `task_list` (enumerate). Defaults to false.",
      "type": "boolean"
    },
    "timeout_ms": {
      "description": "Optional wall-clock timeout in milliseconds. Defaults to 120000 (2 min); the runtime caps any value above 600000 (10 min). On timeout the child process is killed; the response carries `timed_out=true`. Ignored when `run_in_background=true` — background tasks have no implicit deadline; use `task_stop` to terminate them.",
      "maximum": 600000,
      "minimum": 1,
      "type": "integer"
    }
  },
  "required": [
    "command"
  ],
  "type": "object"
}
```

### `task_output`

- Label: Bash Task Output
- Category: `exec`
- Permission scope: `Bash`
- Read only: `true`
- Destructive: `false`
- Search hint: `bash background task output tail log`

Read incremental output from a background `bash` task started with `run_in_background: true`. Returns a UTF-8 lossy chunk of `[since, next_offset)` bytes from the task's log file plus a `finished` flag. Use the previous response's `next_offset` as the next `since` to tail the task across turns; first call may omit `since` to read from byte 0. When the task has finished or been stopped, `finished=true` and `exit_code` is populated.

Parameters:

```json
{
  "properties": {
    "since": {
      "description": "Byte offset to start reading from; pass the previous response's `next_offset` to tail. Defaults to 0 (read from start).",
      "minimum": 0,
      "type": "integer"
    },
    "task_id": {
      "description": "The task_id returned by a previous `bash` call with run_in_background=true.",
      "type": "string"
    }
  },
  "required": [
    "task_id"
  ],
  "type": "object"
}
```

### `task_stop`

- Label: Bash Task Stop
- Category: `exec`
- Permission scope: `Bash`
- Read only: `false`
- Destructive: `true`
- Search hint: `bash background task stop kill cancel`

Stop a background `bash` task by its `task_id`. Sends SIGKILL to the entire process group on Unix (mirroring the foreground `bash` timeout path) and marks the task `Stopped`. Subsequent `task_output` calls return `finished=true` with `exit_code=-1`.

Parameters:

```json
{
  "properties": {
    "task_id": {
      "description": "The task_id returned by a previous `bash` call with run_in_background=true.",
      "type": "string"
    }
  },
  "required": [
    "task_id"
  ],
  "type": "object"
}
```

### `task_list`

- Label: Bash Task List
- Category: `exec`
- Permission scope: `Bash`
- Read only: `true`
- Destructive: `false`
- Search hint: `bash background task list status enumerate`

Enumerate every background `bash` task started in the current session with its current status (`Running`, `Stopped`, or `Finished{exit_code}`), the originating command, the started_at timestamp, and the log path. Use this to discover task ids when you need to follow up on a long-running task.

Parameters:

```json
{
  "properties": {},
  "required": [],
  "type": "object"
}
```

## Config

### `config_get`

- Label: Config Get
- Category: `config`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `config get workspace primitive model`

Read the current value of an allowed tomcat configuration key. The tool is constrained by CONFIG_READ_ALLOWLIST and CONFIG_HARDCODED_READ_DENY: workspace.*, agent.id, primitive.path_rules, primitive.bash_*, llm.default_model and similar non-sensitive fields are readable; llm.api_key*, llm.api_base, security.*, storage.* and other sensitive fields are denied. Missing dot-path keys return not_set.

Parameters:

```json
{
  "properties": {
    "key": {
      "description": "Configuration dot path, for example workspace.workspace_roots, workspace.entries, primitive.path_rules, primitive.bash_forbidden, or agent.id.",
      "type": "string"
    }
  },
  "required": [
    "key"
  ],
  "type": "object"
}
```

### `config_set`

- Label: Config Set
- Category: `config`
- Permission scope: `Write`
- Read only: `false`
- Destructive: `true`
- Search hint: `config set workspace roots path rules model`

Append to or update an allowed tomcat configuration key. Every call shows the user a unified diff and requires confirmation. CONFIG_WRITE_ALLOWLIST and CONFIG_HARDCODED_WRITE_DENY protect sensitive or self-escalating fields.

Semantics: array fields such as workspace.workspace_roots, workspace.entries, primitive.path_rules, primitive.bash_forbidden, and primitive.bash_approval_required accept value as one JSON element string and append it only. Scalar fields such as llm.default_model, log.level, and context.compaction_turns accept value as the replacement string. Deleting or arbitrary mutation is not supported; return an error that guides the user to tomcat config edit.

Forbidden fields include llm.api_key*, security.*, storage.*, agent.id, agent.workspace, and primitive.auto_confirm.

Parameters:

```json
{
  "properties": {
    "key": {
      "description": "Allowed configuration dot path to update.",
      "type": "string"
    },
    "value": {
      "description": "Scalar replacement value, or one JSON element string for append-only array fields such as workspace roots and path rules.",
      "type": "string"
    }
  },
  "required": [
    "key",
    "value"
  ],
  "type": "object"
}
```

