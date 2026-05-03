# Tool Catalog

> This file is generated from `src/core/tools/catalog.rs`.
> Run `UPDATE_TOOL_CATALOG=1 cargo run --bin gen-tool-catalog` after catalog changes.

## Filesystem

### `read_file`

- Label: Read File
- Category: `filesystem`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `read file text utf-8 inspect`

Read a UTF-8 text file from an authorized path. Use this before editing or when the user asks to inspect file contents. Do not use it for directories, binary files, images, or very large files; use list_dir for directories and explain binary attachment limits when UTF-8 decoding fails.

Parameters:

```json
{
  "properties": {
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

### `write_file`

- Label: Write File
- Category: `filesystem`
- Permission scope: `Write`
- Read only: `false`
- Destructive: `true`
- Search hint: `write create overwrite file`

Create or overwrite a file at an authorized path. Use this for new files or complete rewrites when the intended final content is known. Prefer edit_file for small surgical changes to existing files. Writes may require user confirmation and are audited.

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

### `edit_file`

- Label: Edit File
- Category: `filesystem`
- Permission scope: `Write`
- Read only: `false`
- Destructive: `true`
- Search hint: `edit replace old_content new_content file`

Edit an existing text file by replacing exact old_content with new_content. Use this for focused changes after reading the file. old_content must match exactly, including whitespace; if the same snippet appears more than once, include more surrounding context before calling the tool. Do not use it for binary files or broad rewrites.

Parameters:

```json
{
  "properties": {
    "new_content": {
      "description": "Replacement text.",
      "type": "string"
    },
    "old_content": {
      "description": "Exact existing text to replace; include enough context to make it unique.",
      "type": "string"
    },
    "path": {
      "description": "Absolute or relative file path to edit.",
      "type": "string"
    }
  },
  "required": [
    "path",
    "old_content",
    "new_content"
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

List the immediate contents of an authorized directory. Use this to discover nearby files before choosing read_file or edit_file. It does not recurse; call it on subdirectories as needed instead of guessing paths.

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

Use this instead of execute_bash with grep/find/ls -R. Use list_dir when you only need one directory level, and read_file when you already know the exact path.

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

### `execute_bash`

- Label: Execute Bash
- Category: `exec`
- Permission scope: `Bash`
- Read only: `false`
- Destructive: `true`
- Search hint: `bash shell command test build git`

Run a shell command through the permission gate. Use it for builds, tests, git inspection, and other command-line workflows. Avoid destructive commands unless the user explicitly asked and the permission prompt allows it. Prefer tool-native file APIs for reading or editing files; bash path access is still checked and audited as command execution.

Parameters:

```json
{
  "properties": {
    "command": {
      "description": "Shell command to execute.",
      "type": "string"
    },
    "cwd": {
      "description": "Optional working directory. Use the project cwd when the user asks to run in the current project.",
      "type": "string"
    }
  },
  "required": [
    "command"
  ],
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

Read the current value of an allowed pi configuration key. The tool is constrained by CONFIG_READ_ALLOWLIST and CONFIG_HARDCODED_READ_DENY: workspace.*, agent.id, primitive.path_rules, primitive.bash_*, llm.default_model and similar non-sensitive fields are readable; llm.api_key*, llm.api_base, security.*, storage.* and other sensitive fields are denied. Missing dot-path keys return not_set.

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

Append to or update an allowed pi configuration key. Every call shows the user a unified diff and requires confirmation. CONFIG_WRITE_ALLOWLIST and CONFIG_HARDCODED_WRITE_DENY protect sensitive or self-escalating fields.

Semantics: array fields such as workspace.workspace_roots, workspace.entries, primitive.path_rules, primitive.bash_forbidden, and primitive.bash_approval_required accept value as one JSON element string and append it only. Scalar fields such as llm.default_model, log.level, and context.compaction_turns accept value as the replacement string. Deleting or arbitrary mutation is not supported; return an error that guides the user to pi config edit.

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

