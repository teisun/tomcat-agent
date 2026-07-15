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

Read a UTF-8 text file. Read a file before editing it. Use list_dir for directories; binary or non-UTF-8 files return a structured hint with the detected first bytes instead of a raw decode error.

Guidelines:
- Use read to inspect a file before editing it.
- When you point to code in a reply, cite it as a clickable `path:line` reference.


Parameters:

```json
{
  "properties": {
    "hashline": {
      "description": "Render each line as `{line}#{2-char hash}:{content}` for use with hashline_edit. Display-only prefix — do not paste into edit.old_content. Mutually exclusive with line_numbers (hashline wins). Default false.",
      "type": "boolean"
    },
    "limit": {
      "description": "Optional max lines to return (default 2000). On overflow the result appends a resume hint with the next offset.",
      "maximum": 10000,
      "minimum": 1,
      "type": "integer"
    },
    "line_numbers": {
      "description": "Render `cat -n` style line numbers (default true). These prefixes are display-only — do not paste `  N\\t...` into edit.old_content.",
      "type": "boolean"
    },
    "offset": {
      "description": "Optional 1-based line to start from. Defaults to 1.",
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

### `load_skill`

- Label: Load Skill
- Category: `filesystem`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `load skill body attachment by name`

Load one skill body by its declared name instead of guessing a file path. Use this after reading `<available_skills>` when a skill's full instructions are needed. Required `name` selects the skill; optional `file` reads a relative attachment inside the same skill directory. The read still goes through the permission gate, and reviewer/verifier contexts may reject this tool.

Parameters:

```json
{
  "properties": {
    "file": {
      "description": "技能目录下的相对附件路径；省略或 null 表示读取主 SKILL.md 正文。",
      "type": [
        "string",
        "null"
      ]
    },
    "name": {
      "description": "Skill 名称（来自 <available_skills> 的 name 字段）。",
      "type": "string"
    }
  },
  "required": [
    "name"
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

Create or overwrite a file at an authorized path. Use this for new files or complete rewrites when the final content is known; prefer edit for small surgical changes. Writes may require user confirmation and are audited.

Guidelines:
- Use write only for new files or complete rewrites; prefer edit for small changes to existing files.
- Make file changes with the edit/write tools directly; never print a code block pretending to edit a file.


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

Edit an existing text file by replacing exact text. Two input shapes:
  Shape A (single): { path, old_content, new_content, replace_all? }
  Shape B (preferred, multiple): { path, edits: [ { old_content, new_content, replace_all? }, ... ] }
When both appear, `edits` wins. Each segment matches the file's ORIGINAL snapshot (no chained matching). Without `replace_all: true` a segment must match exactly once, else the call returns an Ambiguous error. Read the file first (a fresh read stamp is required; mtime/size mismatch returns a Stale error). Do NOT include `cat -n`/hashline display prefixes (`  N\t...` or `N#XX:...`) in `old_content`. Use write for new files; do not edit binary files.

Guidelines:
- Default file-edit workflow: read -> edit; for repeated short snippets or line-anchored edits, use read(hashline=true) -> hashline_edit.
- When copying from read output, never include display prefixes like `  N\t` or `N#XX:` in edit.old_content.
- Make file changes with the edit/write tools directly; never print a code block pretending to edit a file.


Parameters:

```json
{
  "description": "Edit a file (read -> edit). Provide Shape A (top-level old_content/new_content) or Shape B (edits[]); when both appear, `edits` wins. All segments match the file's ORIGINAL snapshot (no chained matching). Do not include read display prefixes (`  N\\t...` or `N#XX:...`) in old_content.",
  "properties": {
    "edits": {
      "description": "Shape B (preferred): edit segments applied to the file's ORIGINAL snapshot. Overlapping spans are rejected.",
      "items": {
        "additionalProperties": false,
        "properties": {
          "new_content": {
            "description": "Replacement text for this segment.",
            "type": "string"
          },
          "old_content": {
            "description": "Exact existing text to replace in this segment (real file text, no display prefixes).",
            "type": "string"
          },
          "replace_all": {
            "description": "Replace every occurrence of `old_content` in this segment. Defaults to false.",
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
      "description": "Shape A: replacement text.",
      "type": "string"
    },
    "old_content": {
      "description": "Shape A: exact existing text to replace; include enough context to be unique unless `replace_all: true`. Copy real file text, not display prefixes.",
      "type": "string"
    },
    "path": {
      "description": "Absolute or relative file path to edit.",
      "type": "string"
    },
    "replace_all": {
      "description": "Shape A: replace every occurrence of `old_content` instead of failing on multiple matches. Defaults to false.",
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

Edit a file using line-number + 2-char content-hash anchors. Call `read(hashline=true)` first, then pass the returned `<line>#<2char>` anchors here. Each segment's anchor must match the file's CURRENT content; if the line changed, the anchor no longer matches and the call returns HashMismatch (no write). Operations: `replace` (anchor -> lines), `insert` (insert `lines` BEFORE the anchor line), `delete` (anchor[..end] -> empty). Use this when substring `edit` would be ambiguous (repeated short snippets) or when you need strong line-level consistency. A fresh read stamp is still required.

Parameters:

```json
{
  "description": "Line-anchored edit. Call `read(hashline=true)` first, then pass the returned `<line>#<2char>` anchors here. Anchors are validated against the file's current content before any write; mismatches return HashMismatch.",
  "properties": {
    "edits": {
      "description": "Line-anchored operations applied against the CURRENT file content.",
      "items": {
        "additionalProperties": false,
        "properties": {
          "end": {
            "description": "Optional inclusive end-line anchor (replace/delete only). Defaults to `pos`.",
            "type": "string"
          },
          "lines": {
            "description": "Replacement / insertion text (end multi-line text with a newline). Ignored by `delete`.",
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
            "description": "Start-line anchor `<1-based-line>#<2char-hash>` (e.g. `42#Ab`). For `insert`, content goes BEFORE this line.",
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

List the immediate contents of an authorized directory (no recursion). Use it to discover nearby files before choosing read or edit; call it on subdirectories instead of guessing paths.

Guidelines:
- Only claim you can access directories you have successfully listed or read with tools; if unsure, verify with list_dir. Do not guess or fabricate accessible paths.


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

Search authorized files by content regex or file-path glob. Use target=content to search inside files and target=files to find file paths; target=files only uses pattern/path/head_limit/offset/include_hidden. Use list_dir for a single directory level and read when you already know the path.

Uses system `rg` (content) / `fd` (files), falling back to an in-process Rust regex engine when they are missing (no lookaround/back-references; large/binary files skipped). Both honour .gitignore/.ignore.

Guidelines:
- Use search_files to find file paths or content; prefer it over bash with grep/find/ls -R.


Parameters:

```json
{
  "properties": {
    "case_insensitive": {
      "description": "[content only] Ignore case, equivalent to ripgrep -i. Defaults to false.",
      "type": "boolean"
    },
    "context": {
      "description": "[content only] Surrounding context lines when output_mode=content. Ignored otherwise.",
      "minimum": 0,
      "type": "integer"
    },
    "glob": {
      "description": "[content only] Optional file glob filter such as `*.rs` or `**/*.md`. Omit when unused; do not pass an empty string.",
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
      "description": "[both] Max returned items after offset. Defaults to 64 for content and 128 for files. null = unlimited; 0 is rejected."
    },
    "include_hidden": {
      "description": "[both] Include hidden files and directories. Defaults to false; .gitignore is still respected.",
      "type": "boolean"
    },
    "offset": {
      "description": "[both] Skip this many items before head_limit. Use next_offset when truncated=true.",
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
      "description": "[both] Optional file or directory to search. Defaults to the workspace; must pass Read permission checks.",
      "type": "string"
    },
    "pattern": {
      "description": "[both] Search expression. target=content: ripgrep regex over file contents. target=files: file-path glob such as `*.rs` or `src/**/*.rs`.",
      "type": "string"
    },
    "target": {
      "description": "[both] `content` searches inside files; `files` searches file paths by glob. Defaults to `content`.",
      "enum": [
        "content",
        "files"
      ],
      "type": "string"
    },
    "type": {
      "description": "[content only] Optional ripgrep file type filter such as `rust`, `js`, or `py`. Omit when unused.",
      "type": "string"
    }
  },
  "required": [
    "pattern"
  ],
  "type": "object"
}
```

### `create_plan`

- Label: Create Plan
- Category: `filesystem`
- Permission scope: `Write`
- Read only: `false`
- Destructive: `false`
- Search hint: `plan create planning goal draft todos reviewer`

Create a new plan file under `~/.tomcat/plans/<slug>_<hash>.plan.md` (PLAN mode only). Pass `goal` (short objective), `draft` (plan-body content), and an initial flat `todos` list; the runtime derives `plan_id` from goal (do NOT pass plan_id), normalizes `draft` into the `## Plan` section, writes frontmatter under an advisory lock, then runs an advisory reviewer whose summary rides back on this tool's result `review` field. Reviewer output is advisory only and does NOT gate `/plan build`. Calling outside Planning returns a tool error.

Parameters:

```json
{
  "description": "Create a plan file under ~/.tomcat/plans/. Only callable when PlanRuntime mode == Planning. plan_id is derived by runtime from goal; do NOT pass plan_id.",
  "properties": {
    "draft": {
      "description": "Markdown for the plan body `## Plan` section (approach, key decisions, constraints; <= ~2000 chars). Do NOT include the `## Goal` / `## Plan` / `## Todos Board` headings yourself.",
      "type": "string"
    },
    "goal": {
      "description": "Concise plan objective (1-3 sentences). Becomes frontmatter `goal` and seeds the derived `plan_id`.",
      "type": "string"
    },
    "todos": {
      "description": "Initial flat todo list (>= 1 item). `status` defaults to `pending`.",
      "items": {
        "properties": {
          "content": {
            "description": "Single-sentence imperative todo description.",
            "type": "string"
          },
          "id": {
            "description": "Stable kebab-case todo id, unique within the plan.",
            "type": "string"
          },
          "status": {
            "description": "Initial status. Defaults to `pending`; at most one todo may be `in_progress`.",
            "enum": [
              "pending",
              "in_progress",
              "completed",
              "cancelled"
            ],
            "type": "string"
          }
        },
        "required": [
          "id",
          "content"
        ],
        "type": "object"
      },
      "minItems": 1,
      "type": "array"
    }
  },
  "required": [
    "goal",
    "draft",
    "todos"
  ],
  "type": "object"
}
```

### `update_plan`

- Label: Update Plan
- Category: `filesystem`
- Permission scope: `Write`
- Read only: `false`
- Destructive: `false`
- Search hint: `plan update todos upsert set_status remove replace`

Apply incremental todo-only ops (`upsert` / `set_status` / `remove`) to the active plan, persisted to its `.plan.md` frontmatter under an advisory lock. Visible in CHAT / PLAN / EXEC. `plan_id` and `path` target the plan; `replace=true` swaps the entire todo list with the provided upsert results. When all todos reach `completed` in EXEC, the runtime auto-derives state=completed and resets the reminder/catalog/visible labels. Only frontmatter.todos is mutated; plan body markdown is left untouched.

Parameters:

```json
{
  "description": "Apply incremental todo-only ops to the active plan. Callable in CHAT / PLAN / EXEC; requires an active plan. `replace=true` swaps the whole todo list with the upsert results; each op is tagged by `kind` (`upsert` / `set_status` / `remove`).",
  "properties": {
    "ops": {
      "description": "Ordered mutations applied atomically (one frontmatter write under advisory lock).",
      "items": {
        "oneOf": [
          {
            "additionalProperties": false,
            "description": "`upsert` creates a todo if id is new, else updates the provided fields.",
            "properties": {
              "content": {
                "description": "Todo content. Required when creating a brand-new todo.",
                "type": "string"
              },
              "id": {
                "description": "Target todo id (kebab-case).",
                "type": "string"
              },
              "kind": {
                "const": "upsert",
                "description": "Operation kind.",
                "type": "string"
              },
              "status": {
                "description": "For `upsert` (optional) and `set_status` (required). At most one todo may be `in_progress`; `in_progress` only allowed when plan.state == executing.",
                "enum": [
                  "pending",
                  "in_progress",
                  "completed",
                  "cancelled"
                ],
                "type": "string"
              }
            },
            "required": [
              "kind",
              "id"
            ],
            "type": "object"
          },
          {
            "additionalProperties": false,
            "description": "`set_status` only changes status for an existing todo.",
            "properties": {
              "id": {
                "description": "Target todo id (kebab-case).",
                "type": "string"
              },
              "kind": {
                "const": "set_status",
                "description": "Operation kind.",
                "type": "string"
              },
              "status": {
                "description": "For `upsert` (optional) and `set_status` (required). At most one todo may be `in_progress`; `in_progress` only allowed when plan.state == executing.",
                "enum": [
                  "pending",
                  "in_progress",
                  "completed",
                  "cancelled"
                ],
                "type": "string"
              }
            },
            "required": [
              "kind",
              "id",
              "status"
            ],
            "type": "object"
          },
          {
            "additionalProperties": false,
            "description": "`remove` deletes a todo by id.",
            "properties": {
              "id": {
                "description": "Target todo id (kebab-case).",
                "type": "string"
              },
              "kind": {
                "const": "remove",
                "description": "Operation kind.",
                "type": "string"
              }
            },
            "required": [
              "kind",
              "id"
            ],
            "type": "object"
          }
        ]
      },
      "minItems": 1,
      "type": "array"
    },
    "path": {
      "description": "Alternative target path under ~/.tomcat/plans/. If both `plan_id` and `path` are given, `plan_id` wins.",
      "type": "string"
    },
    "plan_id": {
      "description": "Target plan_id. Optional in EXEC (defaults to the active plan); REQUIRED in CHAT / PLAN / Pending / Completed.",
      "type": "string"
    },
    "replace": {
      "description": "If true, replace the entire todos[] list with the upsert results in `ops`. Default false.",
      "type": "boolean"
    }
  },
  "required": [
    "ops"
  ],
  "type": "object"
}
```

### `todos`

- Label: Todos
- Category: `filesystem`
- Permission scope: `Write`
- Read only: `false`
- Destructive: `false`
- Search hint: `todos upsert set_status remove scratchpad new_todos replace`

Manage a session-local todo scratchpad and return a full snapshot of all items after each call. It NEVER writes the active PlanFile (advance plan todos via `update_plan`); when persistence is configured it is stored at `~/.tomcat/agents/<id>/todos/<session_id>.todo.md`. Use `new_todos=true` to clear the scratchpad and start fresh; use `replace=true` to replace the whole list with the provided upsert results. At most one todo may be `in_progress`.

Parameters:

```json
{
  "description": "Session-local todo scratchpad (any plan mode). Returns the full items snapshot after each call. It never writes the active PlanFile (advance plan todos via `update_plan`). `new_todos=true` clears the scratchpad and starts fresh; `replace=true` swaps the whole list with the upsert results.",
  "properties": {
    "new_todos": {
      "description": "If true, clear the current scratchpad before applying ops (same session file is overwritten). Default false.",
      "type": "boolean"
    },
    "ops": {
      "description": "Ordered list of mutations applied in order.",
      "items": {
        "oneOf": [
          {
            "additionalProperties": false,
            "description": "`upsert` creates a todo if id is new, else updates the provided fields.",
            "properties": {
              "content": {
                "description": "Todo content. Required when creating a brand-new todo.",
                "type": "string"
              },
              "id": {
                "description": "Target todo id (kebab-case).",
                "type": "string"
              },
              "kind": {
                "const": "upsert",
                "description": "Operation kind.",
                "type": "string"
              },
              "status": {
                "description": "For `upsert` (optional) and `set_status` (required). At most one todo may be `in_progress`.",
                "enum": [
                  "pending",
                  "in_progress",
                  "completed",
                  "cancelled"
                ],
                "type": "string"
              }
            },
            "required": [
              "kind",
              "id"
            ],
            "type": "object"
          },
          {
            "additionalProperties": false,
            "description": "`set_status` only changes status for an existing todo.",
            "properties": {
              "id": {
                "description": "Target todo id (kebab-case).",
                "type": "string"
              },
              "kind": {
                "const": "set_status",
                "description": "Operation kind.",
                "type": "string"
              },
              "status": {
                "description": "For `upsert` (optional) and `set_status` (required). At most one todo may be `in_progress`.",
                "enum": [
                  "pending",
                  "in_progress",
                  "completed",
                  "cancelled"
                ],
                "type": "string"
              }
            },
            "required": [
              "kind",
              "id",
              "status"
            ],
            "type": "object"
          },
          {
            "additionalProperties": false,
            "description": "`remove` deletes a todo by id.",
            "properties": {
              "id": {
                "description": "Target todo id (kebab-case).",
                "type": "string"
              },
              "kind": {
                "const": "remove",
                "description": "Operation kind.",
                "type": "string"
              }
            },
            "required": [
              "kind",
              "id"
            ],
            "type": "object"
          }
        ]
      },
      "minItems": 1,
      "type": "array"
    },
    "replace": {
      "description": "If true, replace the entire todo list with the upsert results in `ops`. Default false.",
      "type": "boolean"
    },
    "title": {
      "description": "Optional title stored in the new .todo.md frontmatter when `new_todos=true`.",
      "type": "string"
    }
  },
  "required": [
    "ops"
  ],
  "type": "object"
}
```

### `ask_question`

- Label: Ask Question
- Category: `filesystem`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `plan ask question single choice recommended custom skip`

Ask the user 1-4 structured single-choice questions. Each question has 2-4 `options` (stable `id` + `label`); exactly one option must carry `recommended: true` (UI renders it with an `— 推荐` suffix). The UI auto-appends a `__custom__` free-text slot (do NOT declare it) and a per-question `skip`. The tool blocks until the user answers, skips, or cancels (cancel -> `{ cancelled: true }`, not a ToolError). Visible in CHAT / PLAN / Pending / Completed; hidden in EXEC to avoid blocking the execution loop.

Parameters:

```json
{
  "description": "Block-await structured single-choice answers from the user. Each question has 2-4 options with stable ids; exactly one option must carry `recommended: true`. The UI auto-appends a `__custom__` slot and a `skip` action — do not declare `__custom__` yourself.",
  "properties": {
    "questions": {
      "description": "1-4 questions presented in one panel turn.",
      "items": {
        "properties": {
          "id": {
            "description": "Stable question id (kebab-case), unique within the panel turn.",
            "type": "string"
          },
          "options": {
            "description": "2-4 options. Exactly one option must carry `recommended: true`.",
            "items": {
              "properties": {
                "id": {
                  "description": "Stable option id (kebab-case), unique within this question. Reserved id `__custom__` is forbidden — the UI appends it automatically.",
                  "type": "string"
                },
                "label": {
                  "description": "Human-readable option label (max 200 chars).",
                  "type": "string"
                },
                "recommended": {
                  "description": "Mark exactly one option per question as recommended; the UI suffixes it with `— 推荐`.",
                  "type": "boolean"
                }
              },
              "required": [
                "id",
                "label"
              ],
              "type": "object"
            },
            "maxItems": 4,
            "minItems": 2,
            "type": "array"
          },
          "prompt": {
            "description": "Question text shown to the user (max 500 chars).",
            "type": "string"
          }
        },
        "required": [
          "id",
          "prompt",
          "options"
        ],
        "type": "object"
      },
      "maxItems": 4,
      "minItems": 1,
      "type": "array"
    }
  },
  "required": [
    "questions"
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

Run a shell command through the permission gate (builds, tests, git inspection, other CLI workflows). Avoid destructive commands unless the user explicitly asked and the permission prompt allows it. Prefer tool-native file APIs over bash for reading or editing files; bash path access is still checked and audited.

Set `run_in_background: true` for long-running commands (builds, watchers, dev servers): the call returns immediately with `task_id` + `log_path`, driven via `task_output` / `task_stop` / `task_list`. A trailing `&` still runs inside the same foreground call, so prefer `run_in_background: true` to outlive the current tool round.

Guidelines:
- Prefer tool-native file APIs over bash for reading or editing files.


Parameters:

```json
{
  "properties": {
    "args": {
      "description": "Optional argv elements appended to `command`; when present the command runs argv-style (no shell) — safer for paths with spaces or quotes.",
      "items": {
        "type": "string"
      },
      "type": "array"
    },
    "command": {
      "description": "Shell command to execute. With `args` set, runs argv-style (no shell); otherwise via `sh -c` (Unix) / `cmd /C` (Windows).",
      "type": "string"
    },
    "cwd": {
      "description": "Optional working directory. Empty means unset. Use an absolute path or `~/...`; shell vars like `$HOME` are NOT expanded here. Defaults to the agent process cwd.",
      "type": "string"
    },
    "run_in_background": {
      "description": "When true, spawn as a background task and return { task_id, log_path } immediately; pair with task_output/task_stop/task_list. Defaults to false.",
      "type": "boolean"
    },
    "timeout_ms": {
      "description": "Optional wall-clock timeout in ms (default 120000, capped at 600000). On timeout the child is killed and `timed_out=true`. Ignored when run_in_background=true.",
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

Read incremental output from a background `bash` task (started with run_in_background=true). Returns a UTF-8 lossy chunk from `since` plus `finished` and `exit_code`; pass the previous response's `next_offset` as the next `since` to tail across turns (first call may omit `since`). `block=false` (default) returns immediately; `block=true` waits until new output, the task finishes, or `timeout_ms` elapses (default 5000, max 30000, `0` == block=false) and adds a `wakeReason` of `new_output` | `finished` | `timeout`. A `timeout` wakeReason is NOT a failure. Do not busy-poll. See the background bash tasks section in the system prompt for the full workflow.

Parameters:

```json
{
  "properties": {
    "block": {
      "description": "If true, wait until new output arrives, the task finishes, or `timeout_ms` elapses, and return a `wakeReason`. Default false.",
      "type": "boolean"
    },
    "since": {
      "description": "Byte offset to start from; pass the previous response's `next_offset` to tail. Defaults to 0.",
      "minimum": 0,
      "type": "integer"
    },
    "task_id": {
      "description": "The task_id returned by a previous `bash` call with run_in_background=true.",
      "type": "string"
    },
    "timeout_ms": {
      "description": "Wait slice in ms for block=true (default 5000, max 30000; `0` == block=false). A timeout is not a failure — inspect `content`/`finished` before waiting again.",
      "maximum": 30000,
      "minimum": 0,
      "type": "integer"
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

Stop a background `bash` task by its `task_id` (SIGKILL to the whole process group on Unix). Subsequent `task_output` calls return `finished=true` with `exit_code=-1`.

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

List every background `bash` task in the current session with its status (`Running`, `Stopped`, or `Finished{exit_code}`), originating command, started_at timestamp, and log path. Use it to discover task ids to follow up on.

Parameters:

```json
{
  "properties": {},
  "required": [],
  "type": "object"
}
```

### `web_search`

- Label: Web Search
- Category: `exec`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `web search internet tavily brave serper query`

Search the web and return normalized search hits. Use this to discover candidate URLs/snippets; use `web_fetch` when you need one URL body afterward. Required `query`, plus optional `count`, `freshness`, `country`, `language`, and `domain_filter`. Results are normalized across hosted OpenAI search plus Tavily / Brave / Serper backends with automatic fallback in `auto` mode. Preserve source attribution when citing, and mind the current date for time-sensitive queries.

Parameters:

```json
{
  "properties": {
    "count": {
      "description": "Number of hits to request. Defaults to 5, capped at 20.",
      "maximum": 20,
      "minimum": 1,
      "type": "integer"
    },
    "country": {
      "description": "Optional ISO 3166-1 alpha-2 country hint such as `us` or `cn`.",
      "type": [
        "string",
        "null"
      ]
    },
    "domain_filter": {
      "description": "Optional allowlist of bare-host domains such as `github.com`.",
      "items": {
        "description": "One allowed domain suffix.",
        "type": "string"
      },
      "type": "array"
    },
    "freshness": {
      "description": "Optional recency filter (`day`/`week`/`month`/`year`); omit or null for none.",
      "enum": [
        "day",
        "week",
        "month",
        "year",
        null
      ],
      "type": [
        "string",
        "null"
      ]
    },
    "language": {
      "description": "Optional ISO 639-1 language hint such as `en` or `zh`.",
      "type": [
        "string",
        "null"
      ]
    },
    "query": {
      "description": "Search query text (required); prefer natural-language keywords.",
      "type": "string"
    }
  },
  "required": [
    "query"
  ],
  "type": "object"
}
```

### `web_fetch`

- Label: Web Fetch
- Category: `exec`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `web fetch url markdown html pdf redirect`

Fetch one specific URL and return cleaned page content. Use this after `web_search` when you already have a candidate URL. Unsafe hosts (embedded credentials, single-label / IP-literal, private/loopback) are rejected; off-host redirects are not auto-followed and instead return structured redirect info so you can decide whether to refetch. Small text/html returns inline; large text and binary payloads (PDF/images) are persisted with a head preview plus `persisted_output_path`. Required `url`, plus optional `prompt` (warning-only) and `format` (`markdown` or `text`).

Parameters:

```json
{
  "properties": {
    "format": {
      "description": "Output format for textual pages. Defaults to `markdown`; use `text` for plain text.",
      "enum": [
        "markdown",
        "text"
      ],
      "type": "string"
    },
    "prompt": {
      "description": "Optional extraction intent (MVP: recorded as a warning only, does not change fetched content).",
      "type": [
        "string",
        "null"
      ]
    },
    "url": {
      "description": "Target URL (required). Must be an http(s) URL without embedded credentials or private/IP-literal hosts.",
      "type": "string"
    }
  },
  "required": [
    "url"
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

Read the current value of an allowed tomcat configuration key. Non-sensitive fields (workspace.*, agent.id, primitive.*, llm.default_model, and similar) are readable; sensitive fields (llm.api_key*, security.*, storage.*) are denied. Missing dot-path keys return not_set.

Parameters:

```json
{
  "properties": {
    "key": {
      "description": "Configuration dot-path, e.g. workspace.workspace_roots, primitive.path_rules, or agent.id.",
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

Append to or update an allowed tomcat configuration key. Every call shows a unified diff and requires confirmation. Array fields (workspace_roots, path_rules, bash_*, etc.) take `value` as one JSON element string and append only; scalar fields (llm.default_model, log.level, context.*) take `value` as the replacement. Deletion or arbitrary mutation is unsupported; sensitive fields (llm.api_key*, security.*, storage.*, agent.id, primitive.auto_confirm) are denied.

Parameters:

```json
{
  "properties": {
    "key": {
      "description": "Allowed configuration dot-path to update.",
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

