# Tomcat Plan Workflow

This file adds only what no other source already states. The plan-mode reminder declares the
mode, each tool schema defines its own arguments and mechanics, and [Plan Structure]
(plan-structure.md) defines the chapters. Rules that only make sense across tools are below.

## Which tool for which need

| Need | Tool |
|---|---|
| A new plan | `create_plan`, with the whole draft in one call |
| Existing body / `## Goal` | `read`, then `edit`; switch to `read(hashline=true)` then `hashline_edit` only when the target text is not unique |
| `frontmatter.todos` | `update_plan` |
| A decision the user owns | `ask_question`; every question needs exactly one recommended option, format in its schema |
| A wide read-only survey | `dispatch_agent`, only under the conditions its description lists |

- Changing body and todos together: edit the body first, then call `update_plan` so the Todos
  Board is regenerated from the new state.
- `update_plan` returns the current todos snapshot; do not re-read the plan file just to inspect
  status.
- Revise a plan in place: never create a second plan to express a revision, and never rewrite an
  existing plan file with `write`.
- The todos-board auto region (the pair of `todos-board:auto` marker comments) is generated;
  `update_plan` overwrites anything you put between the markers.
- Investigation in PLAN mode is read-only. `bash` is **not** path-guarded, so it must not modify
  repository files; only the write tools are restricted to `~/.tomcat/plans/*.plan.md` by
  `safety::enforce_write_path_policy`.

## Gates

- Clarify the user's target before drafting. When a decision belongs to the user (architecture,
  scope, naming), ask first instead of writing and revising afterwards.
- Call `start_plan` only when the user asks to implement or resume; asking for a plan to be
  written or revised does not start it.

## Decomposition

Decompose thoroughly. For non-trivial work, break the work into one or more milestones,
then precisely break each milestone down into detailed todos so nothing is missed and over-decomposition is avoided.
However, for a simple, single-surface change, a flat linear todo list is sufficient.
