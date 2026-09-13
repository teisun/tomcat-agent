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

Read a UTF-8 text file. Read a file before editing it. Use list_dir for directories; binary or non-UTF-8 files return a structured hint with the detected first bytes instead of a raw decode error. A wide read costs the same round trip as a narrow one, so when you are new to a file read a window that actually covers it rather than paging through it 40 lines at a time. Use `paths` to read several files in one call.

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
      "description": "Render `cat -n` style line numbers (default true). Applies to every entry when using `paths`. These prefixes are display-only — do not paste `  N\\t...` into edit.old_content.",
      "type": "boolean"
    },
    "offset": {
      "description": "Optional 1-based line to start from. Defaults to 1.",
      "minimum": 1,
      "type": "integer"
    },
    "path": {
      "description": "Absolute or relative file path to read as UTF-8 text. Provide exactly one of `path` or `paths`.",
      "type": "string"
    },
    "paths": {
      "description": "Read several files in one call. Mutually exclusive with `path`. Entries are read in order and share one output budget; anything that does not fit is reported as SKIPPED with a resume call rather than dropped silently. Keep a batch to 3-5 files so the combined result stays small enough to remain inline.",
      "items": {
        "properties": {
          "limit": {
            "description": "Optional max lines for this entry.",
            "maximum": 10000,
            "minimum": 1,
            "type": "integer"
          },
          "offset": {
            "description": "Optional 1-based start line for this entry.",
            "minimum": 1,
            "type": "integer"
          },
          "path": {
            "description": "File to read.",
            "type": "string"
          }
        },
        "required": [
          "path"
        ],
        "type": "object"
      },
      "maxItems": 10,
      "minItems": 1,
      "type": "array"
    }
  },
  "required": [],
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

Edit one existing text file with exactly `{ path, edits }`. Each segment has mode `replace` (default), `insert_before`, or `insert_after`: insert modes keep `old_content` as an anchor and preserve it. Each segment matches the file's ORIGINAL snapshot (no chained matching). Without `replace_all: true` a segment must match exactly once, else the call returns an Ambiguous error; use `replace_all: true` only when you intentionally want every occurrence changed. A successful result includes a bounded, numbered current view around changed ranges; use that exact text for a following edit, or read again if the next target is not shown. For multiple independent files, issue multiple edit calls in the SAME tool round. Read the file first (a fresh read stamp is required; mtime/size mismatch returns a Stale error). old_content must be a small, unique, continuous exact excerpt; do not combine distant text. Do NOT include `cat -n`/hashline display prefixes (`  N\t...` or `N#XX:...`) in `old_content`. Use write for new files; do not edit binary files.

Guidelines:
- Default file-edit workflow: read -> edit; prefer edit for prose and Markdown. old_content must be small, unique, exact, and one continuous excerpt—never join distant excerpts. A successful edit returns a bounded, numbered current view around its changes: use that current text for the next edit; if its target is not shown, read again first. For repeated short snippets or line-anchored code edits, use read(hashline=true) -> hashline_edit. hashline_edit may batch non-overlapping ranges from one original snapshot: every anchor is checked before a write, then spans apply bottom-up, so line-count changes do not shift other batch anchors. Prefer continuous ranges for reliability. For distant ranges, prefer edit; if hashline_edit is necessary, take a fresh hashline read covering every target immediately before the call and never mix anchors from separate reads. To modify text created by the batch, read again first. When changing multiple independent files, issue one edit call per file in the SAME tool round instead of serializing file by file.
- When copying from read output, never include display prefixes like `  N\t` or `N#XX:` in edit.old_content.
- Make file changes with the edit/write tools directly; never print a code block pretending to edit a file.


Parameters:

```json
{
  "properties": {
    "edits": {
      "description": "One or more independent segments applied to this file's ORIGINAL snapshot. Overlapping spans are rejected.",
      "items": {
        "additionalProperties": false,
        "properties": {
          "mode": {
            "description": "Defaults to replace. Insert modes preserve old_content as the anchor.",
            "enum": [
              "replace",
              "insert_before",
              "insert_after"
            ],
            "type": "string"
          },
          "new_content": {
            "description": "Replacement text, or text to insert for insert modes.",
            "type": "string"
          },
          "old_content": {
            "description": "Exact existing text or insertion anchor (real file text; no read display prefixes).",
            "type": "string"
          },
          "replace_all": {
            "description": "Change every occurrence intentionally; defaults to false. Valid only with mode=replace.",
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
    "path": {
      "description": "Absolute or relative path to the one existing file to edit.",
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

### `hashline_edit`

- Label: Hashline Edit File
- Category: `filesystem`
- Permission scope: `Write`
- Read only: `false`
- Destructive: `true`
- Search hint: `hashline edit line anchor hash`

Edit a file using line-number + 2-char content-hash anchors. Call `read(hashline=true)` first, then pass the returned `<line>#<2char>` anchors here. Each segment's anchor must match the file's CURRENT content; if the line changed, the anchor no longer matches and the call returns HashMismatch for every invalid anchor (no write). Multiple non-overlapping segments use one original snapshot: all anchors validate before writing, then spans apply bottom-up, so line-count changes do not shift other batch anchors. Operations: `replace` (anchor -> lines), `insert` (insert `lines` BEFORE the anchor line), `delete` (anchor[..end] -> empty). Prefer normal `edit` for prose and Markdown; use this for repeated short snippets or strong line-level consistency. Prefer continuous ranges; for distant ranges, use a fresh hashline read covering every target. To modify text created by this batch, read again first. A fresh read stamp is still required.

Parameters:

```json
{
  "description": "Line-anchored edit. Call `read(hashline=true)` first, then pass the returned `<line>#<2char>` anchors here. Anchors are validated against the file's current content before any write; invalid anchors are all reported and no write occurs.",
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
      "description": "[content only] Surrounding context lines when output_mode=content. Defaults to 3 so you can judge a hit without a follow-up read; pass 0 for matched lines only. Ignored otherwise.",
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

Create a new plan file under `~/.tomcat/plans/<slug>_<hash>.plan.md` (PLAN mode only). Pass `goal` (short objective), `draft` (plan-body content), and an initial flat `todos` list; the runtime derives `plan_id` from goal (do NOT pass plan_id), normalizes `draft` into the `## Plan` section, writes frontmatter under an advisory lock, then appends runtime-owned `[gate] review` and `[gate] Acceptance` todos to the end of the returned list. The gates are the visible close-out flow and must not be supplied by the caller. The runtime then runs an advisory reviewer whose summary rides back on this tool's result `review` field. Reviewer output is advisory only and does NOT gate `/plan build`. Calling outside Planning returns a tool error.

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
            "description": "Initial status. Defaults to `pending`.",
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

Apply incremental todo-only ops (`upsert` / `set_status` / `remove`) to the active plan, persisted to its `.plan.md` frontmatter under an advisory lock. Visible in CHAT / PLAN / EXEC. `plan_id` and `path` target the plan; `replace=true` swaps the entire todo list with the provided upsert results. In EXEC, an existing work todo's `content` is frozen; record progress and verification in `set_status.evidence` instead. Up to three independent todos may be `in_progress`. When all todos reach `completed` in EXEC, the runtime runs applicable completion gates before allowing state=completed. Only frontmatter.todos is mutated; plan body markdown is left untouched.

Parameters:

```json
{
  "description": "Apply todo ops, submit a P1 code-review dispute, or submit verified green-build evidence to the active plan. Callable in CHAT / PLAN / EXEC; requires an active plan. `replace=true` swaps the whole todo list with the upsert results; each op is tagged by `kind` (`upsert` / `set_status` / `remove`).",
  "properties": {
    "dispute_findings": {
      "description": "P1 findings the main Agent explicitly accepts as a trade-off. Use only for wontfix; fixing code is communicated by a later review, not here.",
      "items": {
        "additionalProperties": false,
        "properties": {
          "area": {
            "description": "Finding area copied for audit readability; matching uses ref.",
            "type": "string"
          },
          "reason": {
            "description": "Concrete accepted trade-off reason.",
            "type": "string"
          },
          "ref": {
            "description": "Round-local finding reference such as F01.",
            "type": "string"
          },
          "resolution": {
            "enum": [
              "wontfix"
            ],
            "type": "string"
          }
        },
        "required": [
          "ref",
          "area",
          "resolution",
          "reason"
        ],
        "type": "object"
      },
      "type": "array"
    },
    "green_build_evidence": {
      "description": "Finished background bash commands used as green-build evidence. Required with green_build_pass=true; command must exactly match the recorded task.",
      "items": {
        "additionalProperties": false,
        "properties": {
          "command": {
            "type": "string"
          },
          "task_id": {
            "type": "string"
          }
        },
        "required": [
          "command",
          "task_id"
        ],
        "type": "object"
      },
      "type": "array"
    },
    "green_build_pass": {
      "description": "Set true only after loading the verify skill and completing its background acceptance commands.",
      "type": "boolean"
    },
    "ops": {
      "description": "Ordered todo mutations applied atomically. Omit or pass [] only when submitting dispute_findings or green-build evidence.",
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
              "evidence": {
                "description": "Completion evidence. Only valid with status=`completed`; include concrete verification such as task:<id>, file:<path>, or a command outcome.",
                "items": {
                  "type": "string"
                },
                "type": "array"
              },
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

Manage a session-local todo scratchpad and return a full snapshot of all items after each call. It NEVER writes the active PlanFile (advance plan todos via `update_plan`); when persistence is configured it is stored at `~/.tomcat/agents/<id>/todos/<session_id>.todo.md`. Use `new_todos=true` to clear the scratchpad and start fresh; use `replace=true` to replace the whole list with the provided upsert results. At most three independent todos may be `in_progress`.

Parameters:

```json
{
  "description": "Session-local todo scratchpad (visible in CHAT, PLAN, and EXEC). Returns the full items snapshot after each call. It never writes the active PlanFile (advance plan todos via `update_plan`). `new_todos=true` clears the scratchpad and starts fresh; `replace=true` swaps the whole list with the upsert results.",
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
                "description": "For `upsert` (optional) and `set_status` (required).",
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
                "description": "For `upsert` (optional) and `set_status` (required).",
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
    "command": {
      "description": "Complete shell command line, including the executable and every argument. Supports pipes, &&, ||, ;, redirects, globbing, and quotes. Always runs via `sh -c` (Unix) / `cmd /C` (Windows).",
      "type": "string"
    },
    "cwd": {
      "description": "Optional working directory. Empty means unset. Use an absolute path or `~/...`; shell vars like `$HOME` are NOT expanded here. Defaults to the agent process cwd.",
      "type": "string"
    },
    "foreground_wait_ms": {
      "description": "How long this call waits in the foreground. Values clamp to 8000-16000ms; expiry keeps the same tracked process running. Ignored when run_in_background=true.",
      "type": "integer"
    },
    "run_in_background": {
      "description": "When true, spawn as a background task and return { task_id, log_path } immediately; pair with task_output/task_stop/task_list. Defaults to false.",
      "type": "boolean"
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

Read incremental output from a background `bash` task (started with run_in_background=true). Returns a UTF-8 lossy chunk from `since` plus `finished` and `exit_code`; pass the previous response's `next_offset` as the next `since` to tail across turns (first call may omit `since`). `block=false` (default) returns immediately; `block=true` waits until the task finishes or `wait_ms` elapses (default 5000; block=true clamps to 5000-600000ms; block=false ignores wait_ms). Mid-stream output does not interrupt the wait. Blocking waits add a `wakeReason` of `finished` | `wait_window_elapsed`; a `wait_window_elapsed` wakeReason is NOT a failure, so inspect `content` first (`content=""` means no new output arrived during that slice) and wait again only if you still need to. Do not busy-poll. See the background bash tasks section in the system prompt for the full workflow.

Parameters:

```json
{
  "properties": {
    "block": {
      "description": "If true, wait until the task finishes or `wait_ms` elapses, and return wakeReason `finished` or `wait_window_elapsed`. Mid-stream output does not interrupt the wait. Default false.",
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
    "wait_ms": {
      "description": "Observation window for block=true (default 5000, clamped to 5000-600000ms). Ignored when block=false. It never stops the task.",
      "maximum": 600000,
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

### `tool_search`

- Label: Tool Search
- Category: `exec`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `deferred connector MCP CLI A2A plugin tool search discover`

Discover deferred tools from connected connectors (MCP now; CLI/A2A/plugins later) that are intentionally kept OUT of your tool set to keep the prompt small and cache-stable. When load_skill is available, before first use in a session you MUST load the "connectors" skill via load_skill("connectors") — it teaches the full search -> describe -> call workflow and when to batch calls via code. Modes: tool_search() lists sources (name, type, description, tool_count); tool_search(source="…") lists that source's tool names + short descriptions (no schema); tool_search(query="…") keyword-searches across sources (add source= to scope). Then use tool_describe([names]) to fetch schemas and tool_call(name, arguments) to invoke. Output returns in the conversation, never in the prompt prefix.

Parameters:

```json
{
  "properties": {
    "limit": {
      "description": "Maximum returned entries. Defaults to 20.",
      "maximum": 100,
      "minimum": 1,
      "type": [
        "integer",
        "null"
      ]
    },
    "offset": {
      "description": "Number of entries to skip. Defaults to 0.",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "query": {
      "description": "Optional keywords for cross-source search. Omit together with source to list sources.",
      "type": [
        "string",
        "null"
      ]
    },
    "source": {
      "description": "Optional connector source name. Without query, lists this source's tool cards; with query, scopes search.",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "required": [],
  "type": "object"
}
```

### `tool_describe`

- Label: Tool Describe
- Category: `exec`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `deferred connector MCP tool schema describe batch`

Fetch full input schemas for one or more deferred tools found via tool_search. Pass names (array; a single tool is ["name"]). Returns each tool's inputSchema + description in input order; unknown names are reported in errors without failing the rest. Batch related tools in one call to avoid repeated round trips, then tool_call to invoke.

Parameters:

```json
{
  "properties": {
    "names": {
      "description": "One or more canonical names returned by tool_search. Batch related tools to fetch schemas together.",
      "items": {
        "minLength": 1,
        "type": "string"
      },
      "minItems": 1,
      "type": "array"
    }
  },
  "required": [
    "names"
  ],
  "type": "object"
}
```

### `tool_call`

- Label: Tool Call
- Category: `exec`
- Permission scope: `Bash`
- Read only: `false`
- Destructive: `false`
- Search hint: `deferred connector MCP CLI A2A plugin tool invoke call`

Invoke one deferred tool found via tool_search/tool_describe. Pass name (e.g. mcp__<source>__<tool>) and arguments matching its inputSchema. Runs through the same trust/permission gate as native connector tools; image results stream back to you. To call one tool many times (fan-out) or filter large results, prefer writing code (see the connectors skill) over many tool_call rounds.

Parameters:

```json
{
  "properties": {
    "arguments": {
      "description": "Arguments matching the inputSchema returned by tool_describe.",
      "type": "object"
    },
    "name": {
      "description": "Canonical deferred tool name returned by tool_search.",
      "minLength": 1,
      "type": "string"
    }
  },
  "required": [
    "name",
    "arguments"
  ],
  "type": "object"
}
```

### `tool_run_code`

- Label: Tool Run Code
- Category: `exec`
- Permission scope: `Bash`
- Read only: `false`
- Destructive: `false`
- Search hint: `deferred connector MCP JavaScript code fan-out aggregate filter`

Run short JavaScript to call deferred connector tools repeatedly and return one compact final value. Before use, load the connectors skill for the allowed callTool(name, arguments) API and examples. Use this only for fan-out, filtering a large result, or aggregation; use tool_call for a short sequential workflow. The code runs in the existing plugin QuickJS VM with the configured heap, timeout, and interrupt limits. It has no host filesystem, shell, network, or arbitrary host API: callTool is its only host capability and uses the same connector trust path as tool_call. Return a JSON-compatible value. MCP image blocks are extracted for vision before final text is truncated.

Parameters:

```json
{
  "properties": {
    "code": {
      "description": "JavaScript function body. Use await callTool(name, arguments) and return one JSON-compatible final value.",
      "minLength": 1,
      "type": "string"
    }
  },
  "required": [
    "code"
  ],
  "type": "object"
}
```

### `dispatch_agent`

- Label: Dispatch Agent
- Category: `exec`
- Permission scope: `Read`
- Read only: `true`
- Destructive: `false`
- Search hint: `dispatch agent explorer subagent investigate parallel findings`

Delegate high-latency, read-only codebase investigation to one or more explorer subagents running in parallel. Direct `search_files` / `read` calls are the default. Do NOT dispatch for a simple task, project rules or AGENTS/README files, a known file or symbol, confirmation of one implementation, a question that can be answered in one or two batches of direct-tool calls, self-review or final audit of your own Plan, or work already covered by the automatic Plan/Code Reviewer. Use this tool only when all are true: (1) the task crosses multiple unknown subsystems, (2) the questions can be investigated independently in parallel, and (3) returning the necessary raw reads would materially bloat the parent context. Before dispatching, put every currently known independent question into one `tasks` array. After reports return, prefer direct tools to fill evidence gaps. Dispatch again only if the reports reveal a new blocker that direct tools cannot answer. Pass `tasks`: 1-6 entries of `{ id, prompt }`, where `prompt` is self-contained because the subagent sees none of this conversation, and `id` matches answers to questions. Each subagent may only read (`read` / `search_files` / `list_dir` / read-only `bash`) and returns concise findings with `path:line` references plus a conclusion, never raw file contents. Only its final report enters the parent context.

Parameters:

```json
{
  "properties": {
    "tasks": {
      "description": "Investigation tasks to run in parallel. Keep each one scoped to a single area; split unrelated questions into separate tasks instead of writing one broad prompt.",
      "items": {
        "properties": {
          "id": {
            "description": "Short unique label such as `webview-paste`. Used to match the returned report back to this task. Defaults to `task-<n>` when omitted.",
            "type": "string"
          },
          "prompt": {
            "description": "Self-contained question. The subagent cannot see this conversation, so state the goal, the area to look at, and what a useful answer contains.",
            "type": "string"
          }
        },
        "required": [
          "prompt"
        ],
        "type": "object"
      },
      "maxItems": 6,
      "minItems": 1,
      "type": "array"
    }
  },
  "required": [
    "tasks"
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

### `package_install`

- Label: Package Install
- Category: `config`
- Permission scope: `Write`
- Read only: `false`
- Destructive: `true`
- Search hint: `package install skill plugin source scope global agent`

Install a local Tomcat package, skill, or plugin after confirmation. Pass a local source path and scope (`scope`, `agent`, or `global`); `scope` requires the session's explicit project root. The operation validates the package before writing and returns a machine-readable `status` (`installed`, `cancelled`, `denied`, or `failed`) plus installed resources and `inventory_dirty` for the next session refresh.

Parameters:

```json
{
  "additionalProperties": false,
  "properties": {
    "scope": {
      "description": "Installation scope. Defaults to scope. Scope requires this session to have an explicit project root.",
      "enum": [
        "scope",
        "agent",
        "global"
      ],
      "type": "string"
    },
    "source": {
      "description": "Absolute local directory or package.json/plugin.json/SKILL.md to install.",
      "type": "string"
    }
  },
  "required": [
    "source"
  ],
  "type": "object"
}
```

