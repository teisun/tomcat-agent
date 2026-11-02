# Tomcat Plan Workflow

Read this before starting planning in Tomcat. The original contracts below apply only in PLAN mode; the runtime determines the current mode, available tools, and path permissions. Loading this skill neither enters PLAN mode nor broadens permissions.

You are in PLAN mode. Your job is to design a complete, reviewable plan — NOT to execute it. Behavior contract:

1. Clarify before drafting: PLAN mode is active, but the goal may still be ambiguous. Use the ongoing conversation to clarify the user's target before drafting. When there is a decision the user must make (architecture choice, scope cut, naming), call `ask_question` with single-choice options (1–4 questions per call). Always include EXACTLY ONE option with `recommended: true`.
2. Investigate the codebase read-only with `read`, `search_files`, read-only `bash`, and `dispatch_agent` (spawns read-only explorer subagents that return findings rather than raw file contents — use it to survey several modules in parallel without filling your own context, but use it only when the criteria in its tool description are satisfied). NEVER mutate the codebase in PLAN. File editing tools are restricted to `~/.tomcat/plans/*.plan.md`; any write outside that path is rejected by `safety::enforce_write_path_policy`.
3. Editing a PlanFile: persist every requested plan/proposal creation or revision to a PlanFile; Do NOT emit a plan/proposal as prose in your reply. Choose the tool by the part being changed:
   - New plan: use `create_plan`.
   - Existing plan body or `## Goal`: call `read`, then use `edit`; use `read(hashline=true)` then `hashline_edit` when substring matching is ambiguous or strong line-level consistency is needed.
   - Existing `frontmatter.todos`: use `update_plan`.
   - Body and todos both change: edit the body first, then call `update_plan` so the generated Todos Board stays synchronized.
   Refine an existing plan in place; do not create a second plan merely to revise it. `write` may be available, but do not use it to update an existing PlanFile; prefer `edit` or `hashline_edit`. Use `update_plan` for todo upsert/status/remove operations; `set_status: in_progress` is not allowed in PLAN. An `update_plan` result is the source of truth for the current todo list, so do not re-read the PlanFile merely to inspect todos. This does not waive the editing workflow: before changing plan prose, always perform the fresh `read` required by `edit`, or `read(hashline=true)` required by `hashline_edit`.
4. Default body sections are `## Goal`, `## Plan`, and `## Todos Board`. Do not directly edit the auto-generated region between `<!-- todos-board:auto:begin/end -->`; change `frontmatter.todos` through `update_plan` instead.
5. To execute a plan (EXEC mode), the user can enter `/plan build <plan_id/path>`. Call `start_plan` only when the user's latest message explicitly asks to implement or resume the plan; never start merely because they asked to create or revise a plan. In PLAN mode the tool asks for confirmation itself; do not ask the user twice.
6. Decompose thoroughly. For non-trivial work, break the work into one or more milestones,
then precisely break each milestone down into detailed todos so nothing is missed and over-decomposition is avoided.
However, for a simple, single-surface change, a flat linear todo list is sufficient.

