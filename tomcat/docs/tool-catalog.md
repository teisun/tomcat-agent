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

Read a file from the local filesystem. Use this before editing or when the user asks to inspect file contents. Default workflow: `read` -> `edit`. For repeated short snippets or line-anchored edits, use `read(hashline=true)` -> `hashline_edit`. Use list_dir for directories; binary or non-UTF-8 files return a structured hint with the detected first bytes instead of raw decode errors.

Parameters:

```json
{
  "properties": {
    "hashline": {
      "description": "When true, render each line as `{:>6}#{2-char hash}:{content}` (xxh32 over whitespace-stripped content). Use with `hashline_edit` when you need line-number + content-hash anchors. The `N#XX:` prefix is display-only, so do not paste it into `edit.old_content`. Mutually exclusive with line_numbers — hashline takes priority. Defaults to false.",
      "type": "boolean"
    },
    "limit": {
      "description": "Optional max number of lines to return; defaults to 2000. When the file has more lines, the result includes a `... [N more lines truncated; resume with offset=<next>, limit=<same>]` hint so you can paginate.",
      "maximum": 10000,
      "minimum": 1,
      "type": "integer"
    },
    "line_numbers": {
      "description": "Render output with `cat -n` style line numbers (`{:>6}\\t{content}`); defaults to true. These prefixes are display-only, so do not paste `  N\\t...` into `edit.old_content`. Set false only when piping the content into a tool that itself parses line numbers (e.g. diff).",
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

### `load_skill`

- Label: Load Skill
- Category: `filesystem`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `load skill body attachment by name`

Load one skill body by its declared name instead of guessing a file path. Use this after reading `<available_skills>` when a skill's full instructions are needed. Required `name` selects the skill; optional `file` reads a relative attachment inside the same skill directory (for example `references/COMMIT_CONVENTION.md`). The read still goes through the existing permission gate, and reviewer/verifier contexts may reject this tool.

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
When both shapes appear, `edits` wins. Each segment matches against the file's ORIGINAL snapshot (no chained / incremental matching), so multi-segment edits are safe to compose. Set `replace_all: true` to replace every occurrence; otherwise the segment must match exactly once or the call returns an Ambiguous error. Read the file first (default workflow: `read` -> `edit`; the tool requires a fresh read stamp, and mtime/size mismatch returns a Stale error). Do NOT include `cat -n` line-number prefixes (`  N\t...`) or hashline prefixes (`N#XX:...`) in `old_content` — those are display prefixes, not file content. If repeated short snippets make substring edit ambiguous, prefer `read(hashline=true)` + `hashline_edit`. Use write for new files or complete rewrites; do not use edit on binary files.

Parameters:

```json
{
  "description": "Edit a file. Default workflow: `read` -> `edit`. Provide either Shape A (top-level old_content/new_content) or Shape B (edits[]); when both appear, `edits` wins. All segments match the file's ORIGINAL snapshot (no chained matching). Do not include read display prefixes (`  N\\t...` or `N#XX:...`) in `old_content`; for repeated short snippets or line-anchored edits, prefer `read(hashline=true)` + `hashline_edit`.",
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
            "description": "Exact existing text to replace within this segment. Copy the real file text, not read-only display prefixes like `  N\\t...` or `N#XX:...`.",
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
      "description": "Shape A only: exact existing text to replace; include enough context to make it unique unless `replace_all: true`. Copy the real file text, not read-only display prefixes like `  N\\t...` or `N#XX:...`.",
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

Edit a file with line-number + 2-char content hash anchors. Call `read` with `hashline: true` first, then pass those anchors to `hashline_edit`. Each edit segment carries an anchor `<line>#<2char>` that must match the file's CURRENT content; if the line content changed, the anchor stops matching and the call returns HashMismatch (no write). Operations: `replace` (anchor → lines), `insert` (insert `lines` BEFORE anchor line), `delete` (anchor[..end] → empty). Use this when sub-string `edit` would be ambiguous (repeated short snippets) or when you need strong line-level consistency. Reads are still required first; the file's read stamp is checked.

Parameters:

```json
{
  "description": "Line-anchored edit. Call `read(hashline=true)` first, then pass the returned `<line>#<2char>` anchors here. Anchors are validated against the file's current hashline before any write; mismatches return HashMismatch.",
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
      "description": "[content only] Optional file glob filter such as `*.rs` or `**/*.md`. Omit when not used; do not pass an empty string.",
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
      "description": "[content only] Optional ripgrep file type filter such as `rust`, `js`, or `py`. Omit when not used; do not pass an empty string.",
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

Create a new plan file under `~/.tomcat/plans/<slug>_<hash>.plan.md` (PLAN mode only). Caller passes `goal` (short objective), `draft` (plan-body content), and an initial flat `todos` list; the runtime derives `plan_id` from goal (caller does NOT supply plan_id), normalizes `draft` into the plan body's `## Plan` section, and writes frontmatter (`plan_id`, `goal`, `state=planning`, `todos`, `schema_version=1`) under an exclusive advisory lock, then synchronously dispatches an internal reviewer sub-agent whose `ReviewSummary` rides back on this tool's result `review` field. Reviewer output is advisory only and does NOT gate `/plan build` — the user must call `/plan build <plan_id/path>` to enter EXEC. Visible only when `mode == Planning`; calling outside Planning returns a tool error.

Parameters:

```json
{
  "description": "Create a plan file under ~/.tomcat/plans/. Only callable when PlanRuntime mode == Planning. plan_id is derived by runtime from goal; do NOT pass plan_id.",
  "properties": {
    "draft": {
      "description": "Markdown content for the plan body's `## Plan` section: ordered bullet points or short paragraphs covering the approach, key decisions, and constraints (≤ ~2000 chars). The runtime wraps it with `## Goal` / `## Plan` / `## Todos Board`; do NOT include those headings yourself. If you accidentally include legacy headings such as `## Draft` or `## Notes`, runtime will normalize them.",
      "type": "string"
    },
    "goal": {
      "description": "Concise plan objective (1–3 sentences) — what success looks like. Becomes the frontmatter `goal` field and the seed for the derived `plan_id`.",
      "type": "string"
    },
    "todos": {
      "description": "Initial flat todo list (≥ 1 item). `status` defaults to `pending`.",
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

Apply incremental todo-only ops (`upsert` / `set_status` / `remove`) to the active plan, persisted to its `.plan.md` frontmatter under the same advisory lock. Visible in CHAT / PLAN / EXEC modes — model uses it to refine the todo list during PLAN or to advance todos during EXEC. `plan_id` and `path` are plan-specific targeting fields; `replace=true` swaps the entire todo list with the provided upsert results. When all todos transition to `completed` in EXEC, runtime auto-derives `state=completed` and resets system reminder / catalog / visible prompt labels. The tool only mutates frontmatter.todos; plan body markdown is left untouched.

Parameters:

```json
{
  "description": "Apply incremental todo-only ops to the active plan. Callable in CHAT / PLAN / EXEC; requires an active plan. `plan_id` and `path` target the plan, `replace=true` replaces the whole todo list with the provided upsert results, and each op is tagged by `kind` (`upsert` / `set_status` / `remove`).",
  "properties": {
    "ops": {
      "description": "Ordered list of mutations applied atomically (one frontmatter write under advisory lock).",
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
      "description": "Alternative target path under ~/.tomcat/plans/. If both `plan_id` and `path` are provided, `plan_id` wins.",
      "type": "string"
    },
    "plan_id": {
      "description": "Target plan_id. Optional in EXEC mode (defaults to the active plan); REQUIRED in CHAT / PLAN / Pending / Completed.",
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

Manage a session-local todo scratchpad and return a full snapshot of all items after each call. The list is persisted under `~/.tomcat/agents/<id>/sessions/<session_key>/todos/<todos_id>.todo.md` when persistence is configured, and it NEVER writes the active PlanFile. Use `new_todos=true` to rotate to a new scratchpad file; use `replace=true` to replace the whole list with the provided upsert results. Only one todo may be `in_progress` at a time — attempting to mark a second `in_progress` returns a structured error. The full items snapshot in the response lets the model self-orient between rounds without re-listing.

Parameters:

```json
{
  "description": "Session-local todo scratchpad (any plan mode). Returns the full items snapshot after each call. It never writes the active PlanFile; advance plan todos via `update_plan`. Use `new_todos=true` to rotate to a new scratchpad file; use `replace=true` to replace the whole list with the provided upsert results.",
  "properties": {
    "new_todos": {
      "description": "If true, create a new active todos file for this session before applying ops. Default false.",
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

Ask the user 1–4 structured single-choice questions. Each question has 2–4 `options` (each with a stable `id` and `label`); exactly one option must carry `recommended: true` (UI renders it with an `— 推荐` suffix). The UI panel automatically appends a synthetic `__custom__` slot (do NOT declare it manually) where the user can type free-form text up to 500 chars, and also supports `skip` to skip only the current question. The tool blocks until the user answers, skips, or cancels (cancel → `{ cancelled: true }`, not a ToolError). Visible in CHAT / PLAN / Pending / Completed; hidden in EXEC to avoid blocking the execution loop.

Parameters:

```json
{
  "description": "Block-await structured single-choice answers from the user (PLAN mode only). Each question must have 2–4 options with stable ids; exactly one option must carry `recommended: true`. The UI auto-appends a `__custom__` slot and a `skip` action for the current question — do not declare `__custom__` yourself.",
  "properties": {
    "questions": {
      "description": "1–4 questions presented to the user in one panel turn.",
      "items": {
        "properties": {
          "id": {
            "description": "Stable question id (kebab-case), unique within the panel turn.",
            "type": "string"
          },
          "options": {
            "description": "2–4 options. Exactly one option must carry `recommended: true`.",
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
      "description": "Optional working directory. Omit when not needed. Empty strings are treated as missing. Pass a real absolute path or `~/...`; shell env vars like `$HOME/...` are NOT expanded here. When omitted, falls back to the agent process working directory.",
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

Waiting modes:
- `block=false` (default): non-blocking; returns immediately with whatever bytes are already on disk. Good for an occasional progress glance — but **do not busy-poll**.
- `block=true`: blocks until any of {new output appears | the task finishes | `timeout_ms` elapses}. Returns an extra `wakeReason` field with one of `"new_output" | "finished" | "timeout"`. **`timeout` is NOT a failure** — when `wakeReason="timeout" && finished=false` the response is just an empty wait slice (`content=""`, `next_offset == since`); call `task_output(block=true)` again to keep waiting.

When to use which:
1. The current todo cannot proceed without the shell result → `task_output(block=true, timeout_ms=...)`. Loop on `wakeReason="timeout"`.
2. The current todo can do other independent work first → spawn `bash(run_in_background=true)` and immediately do other tools/edits/reads. The runtime will inject a synthetic `<background-task-finished task_id="..." exit_code="..." log_path="...">tail</background-task-finished>` user message **automatically** when the shell finishes; you do not need to poll.
3. Just want a peek at progress → one-shot `task_output(block=false)`.

When you see the `<background-task-finished ...>` tag, treat it as a system signal that a previously blocked todo can now proceed (NOT as new user input); pull the full log with `task_output(task_id, since=...)` if the tail body is insufficient.

`timeout_ms` defaults to 5000, is capped at 30000, and `0` is equivalent to `block=false`.

Parameters:

```json
{
  "properties": {
    "block": {
      "description": "If true, wait until new output arrives, the task finishes, or `timeout_ms` elapses. Returns an extra `wakeReason` field. Default false (non-blocking).",
      "type": "boolean"
    },
    "since": {
      "description": "Byte offset to start reading from; pass the previous response's `next_offset` to tail. Defaults to 0 (read from start).",
      "minimum": 0,
      "type": "integer"
    },
    "task_id": {
      "description": "The task_id returned by a previous `bash` call with run_in_background=true.",
      "type": "string"
    },
    "timeout_ms": {
      "description": "Wait slice in milliseconds for `block=true`. Default 5000, max 30000 (values above are capped). `0` is equivalent to `block=false`. Timeout is NOT a failure: when `wakeReason=\"timeout\" && finished=false`, you may call `task_output(block=true)` again to continue waiting.",
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

### `web_search`

- Label: Web Search
- Category: `exec`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `web search internet tavily brave serper query`

Search the web and return normalized search hits. Use this to discover candidate URLs and snippets for a query; use `web_fetch` when you need to fetch one URL body afterward. Input fields align with the architecture doc: required `query`, plus optional `count`, `freshness`, `country`, `language`, and `domain_filter`.

Results are normalized across hosted OpenAI search plus Tavily / Brave / Serper backends, with automatic fallback in `auto` mode. Preserve source attribution when citing results, and pay attention to the current date for time-sensitive queries.

Parameters:

```json
{
  "properties": {
    "count": {
      "description": "Optional number of hits to request. Defaults to 5 and is capped at 20.",
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
      "description": "Optional allowlist of domains to constrain results to. Each item should be a bare host like `github.com`.",
      "items": {
        "description": "One allowed domain suffix.",
        "type": "string"
      },
      "type": "array"
    },
    "freshness": {
      "description": "Optional recency filter. Use `day`, `week`, `month`, or `year`; omit / pass null when no freshness constraint is needed.",
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
      "description": "Search query text. Required; prefer natural-language keywords that describe what the user wants to find.",
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

Fetch one specific URL and return cleaned page content. Use this after `web_search` when you already have a candidate URL and need the actual page body. Private/authenticated URLs, URLs with embedded credentials, single-label hosts, IP literal hosts, and private/loopback targets are rejected before any request; off-host redirects are not auto-followed and instead return structured redirect info so the model can decide whether to refetch with the new URL.

Small text/html pages are returned inline as markdown or plain text. Large text responses are persisted to `tool-results` and return a head preview plus `persisted_output_path`; PDF/images and other binary payloads are persisted instead of being inlined. Input fields align with the architecture doc: required `url`, plus optional `prompt` (MVP warning-only hint) and `format` (`markdown` or `text`).

Parameters:

```json
{
  "properties": {
    "format": {
      "description": "Optional output format for textual pages. Defaults to `markdown`; use `text` when you want plain text without markdown syntax.",
      "enum": [
        "markdown",
        "text"
      ],
      "type": "string"
    },
    "prompt": {
      "description": "Optional extraction intent. In the current MVP this is recorded as a warning only and does not change the fetched content.",
      "type": [
        "string",
        "null"
      ]
    },
    "url": {
      "description": "Target URL to fetch. Required; must be an http(s) URL without embedded credentials, localhost-style hosts, or private/IP-literal targets.",
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

Semantics: array fields such as workspace.workspace_roots, workspace.entries, primitive.path_rules, primitive.bash_forbidden, and primitive.bash_approval_required accept value as one JSON element string and append it only. Scalar fields such as llm.default_model, log.level, context.keep_recent_turns, context.current_tail_compactable_min_chars, context.current_tail_single_result_max_chars, and context.compaction_max_tokens accept value as the replacement string. Deleting or arbitrary mutation is not supported; return an error that guides the user to tomcat config edit.

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

