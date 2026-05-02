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

