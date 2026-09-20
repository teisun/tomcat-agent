### Metadata

| Field | Value |
| --- | --- |
| Updated | 2026-09-21 07:22 +0800 |
| State | ACTIVE |
| Branch | feature/transcript-rich-render |
| Scope | Safe model removal, model ID editing, and compact question navigation |

### DONE

- Preserved independent unsent drafts, references, and attachments for every chat session across switching and webview reload.
- Prevented late storage/state results and send acknowledgements from deleting newer edits.
- Made real-service test fixtures use disposable storage without inheriting the parent agent-session guard.
- Bumped the CLI to `0.1.53`, the VS Code extension to `0.1.67`, and the bundled CLI pin to `0.1.53`.
- Synchronized planner and plan-reviewer prompt contracts so every solution includes an ASCII-and-text explanation plus a plain-language key-decisions checklist with concrete scope details.
- Cleared persisted model references before deleting a user model, kept session reads write-free, and serialized same-store writes so stale selections cannot restore deleted references.
- Routed main chat, compaction, and new sub-agents through the current model selection; surfaced deletion progress, partial failures, and catalog-refresh failures accurately.
- Kept suggested model IDs editable during creation and locked the original ID while editing an existing model.
- Added a height-limited question dock with active `N of M`, scroll/arrow navigation, icon-only collapse, fixed actions, and preserved answer drafts.

### INTERFACE

- The VS Code composer continues to show the active session's own unsent draft; switching sessions does not alter either draft.
- The extension packages CLI `0.1.53` inside VS Code extension `0.1.67`.
- CLI and serve model deletion now use `remove_user_model_with_config_path`; model selection and deletion share `with_current_model_catalog` to reject stale catalog choices.
- The remove-model response and settings state distinguish the deletion outcome from catalog-refresh feedback; matching `tomcat.plan.buildModel` is cleared before deletion.
- Question navigation does not submit answers; `Continue` submits the collected answers together.

### BLOCKED

- The earlier Rust full integration run was not fully green: `completion_flow_test` expects Chinese acceptance wording, and `missing_live_credentials_is_not_a_successful_skip` has an exit-code mismatch. These previously recorded failures are not repaired by this commit.
- The proposed verify-skill prompt revision remains in its separate plan and is not included in this commit.

### VERIFICATION

- The 2026-09-20 implementation acceptance for `plan_add_model_id_edit_id_aee801f9` recorded a passing extension `npm run gate:full`, focused Rust/GUI/extension checks, schema/wire checks, and desktop/mobile browser checks with zero console errors. These are earlier results, not new runs for this commit.
- This commit intentionally does not rerun tests, builds, or coverage, as explicitly requested by the user. Commit-time checks are limited to Git scope, whitespace, status metadata, and commit-message format; no new coverage percentage is claimed.
