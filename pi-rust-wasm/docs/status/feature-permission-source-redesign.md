# feature/permission-source-redesign

| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| - | 2026-05-01 23:01 | PENDING_INTEGRATION | feature/permission-source-redesign | - |

## Status

- Branch: `feature/permission-source-redesign`
- State: `PENDING_INTEGRATION`
- Scope: permission naming, grant trace audit schema, workspace root config rename, dragged path authorization model, confirmation UI, and agent trail naming.

### 2026-05-01 Stage 1-6

DONE:

- Renamed `[workspace] extra_roots` to `workspace_roots` across config schema, CLI, tests, docs, and the local config files.
- Removed the `DraggedPaths` permission bucket; drag menu `[a]` now grants session scope with `DraggedPathMenu` trigger.
- Replaced `GrantSource` with `GrantType` + `GrantTrigger` via `GrantTrace`.
- Replaced primitive audit fields `grant_source` / `in_working_dir` with `grant_type` / `grant_trigger`.
- Implemented path `NeedConfirm` as `[s]/[w]/[c]`; bash approval remains `[y/N]`.
- Renamed runtime readonly terminology to `agent_trail_dir` / `agent_trail_readonly_dirs`.

INTERFACE:

- Config schema is intentionally breaking: use `workspace.workspace_roots`.
- Audit JSONL schema is intentionally breaking: use `grant_type` and `grant_trigger`.
- System prompt no longer renders `[dragged_path]` read/write roots.

VALIDATION:

- `cargo check --all-targets` passed during implementation.
- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --all-targets` passed.
