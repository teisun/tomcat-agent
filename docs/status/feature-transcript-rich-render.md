### Metadata

| Field | Value |
| :--- | :--- |
| Updated | 2026-09-21 21:23 +0800 |
| State | ACTIVE |
| Branch | feature/transcript-rich-render |
| Scope | Single-question pagination and responsive dock, model/draft safety, verification contracts, and CLI/extension release metadata |
| Cov% | - |

### DONE

- Preserved independent unsent drafts, references, and attachments for every chat session across switching and webview reload.
- Prevented late storage/state results and send acknowledgements from deleting newer edits.
- Made real-service test fixtures use disposable storage without inheriting the parent agent-session guard.
- Updated release metadata through the repository script to CLI `0.1.58`, VS Code extension `0.1.72`, and bundled CLI pin `0.1.58`.
- Aligned core identity, planner, and plan-reviewer explanations with the engineering-standards source: visuals first, with each key decision followed by concrete targets, before/after behavior, and scope boundaries.
- Cleared persisted model references before deleting a user model, kept session reads write-free, and serialized same-store writes so stale selections cannot restore deleted references.
- Routed main chat, compaction, and new sub-agents through the current model selection; surfaced deletion progress, partial failures, and catalog-refresh failures accurately.
- Kept suggested model IDs editable during creation and locked the original ID while editing an existing model.
- Replaced stacked pending questions and scroll-derived page counts with one mounted question at a time, explicit previous/next navigation, and preserved answer drafts.
- Restored a complete question-panel border, removed inter-question dividers, and made typography and control spacing font-relative while keeping the existing `min(30vh, 20rem)` height cap and original corner radii.
- Kept the question dock next to the composer without remounting or clearing its draft; Other now focuses and reveals its input through native browser behavior.
- Added a deterministic browser capture harness with per-option visibility assertions and updated GUI/native-host regressions for pagination, draft retention, and ordered batch submission.
- Simplified the verify skill to review, impact-sized verification, and UI acceptance; removed the repeated review loop and replaced the widen-only rule with reuse of still-valid results, with matching contract assertions.
- Joined hard-wrapped skill prose, normalized spacing, and moved the existing screenshot-output-directory instruction into UI capture step 3 without changing its wording or command blocks.
- Added `/bump-cli-ext-version` to upgrade CLI and extension patch versions together through the project script, without implicitly building, committing, or publishing.

### INTERFACE

- The VS Code composer continues to show the active session's own unsent draft; switching sessions does not alter either draft.
- Release metadata declares VS Code extension `0.1.72` with bundled CLI `0.1.58`; changing these values is not evidence of a rebuilt or installed artifact.
- CLI and serve model deletion now use `remove_user_model_with_config_path`; model selection and deletion share `with_current_model_catalog` to reject stale catalog choices.
- The remove-model response and settings state distinguish the deletion outcome from catalog-refresh feedback; matching `tomcat.plan.buildModel` is cleared before deletion.
- Pending question `N of M` denotes the current page, not the answered count; page navigation does not submit answers, and `Continue` still submits the collected answers together. The answer protocol is unchanged.

### BLOCKED

- The earlier Rust full integration run was not fully green: `completion_flow_test` expects Chinese acceptance wording, and `missing_live_credentials_is_not_a_successful_skip` has an exit-code mismatch. These previously recorded failures are not repaired by this commit.
- A pure extension VSIX was built during implementation acceptance, but this commit does not verify installation in the user's daily profile or the artifact loaded by its current window.
- Large fonts, very short windows, or long questions may still require scrolling within the current question; preserving the height cap does not guarantee every possible question fits fully above the fold.

### VERIFICATION

- The 2026-09-20 implementation acceptance for `plan_add_model_id_edit_id_aee801f9` recorded a passing extension `npm run gate:full`, focused Rust/GUI/extension checks, schema/wire checks, and desktop/mobile browser checks with zero console errors. These are earlier results, not new runs for this commit.
- The 2026-09-21 acceptance for `plan_verify_4885bbfb` recorded five passing focused Rust contract tests (tasks `1789964332987-e8fgyh` and `1789964544981-u8z0jk`) and prompt-source alignment checks. Those runs preceded the later Markdown formatting and paragraph relocation; they are not new test runs for this commit.
- Subsequent static checks confirmed unchanged non-whitespace skill content apart from the exact paragraph relocation, byte-for-byte preserved ASCII/command blocks, valid version-command structure, and synchronized release metadata.
- The question-panel implementation acceptance recorded 66 focused GUI tests; after the final Other-focus change, its 12 owning component tests and GUI typecheck passed again (task `1789995584792-ibytf5`). These precede this commit request.
- The final browser matrix passed nine scenarios with PNG/ARIA/console evidence (task `1789995584849-ibdcbp`); ordinary short-question pages passed full visibility checks, while narrow/large-font and long-question cases explicitly exercised scrolling rather than claiming full first-screen visibility.
- The existing real-host suite passed its seven checks (task `1789995181374-ee1dfl`); the final native mouse/keyboard acceptance also passed pagination, Other focus, and submission at 120% zoom after waiting for both frame layouts to settle (task `1789995953920-a2543y`). Both pages measured a 665px webview and a 150.44px panel, with each prompt and all three options visible; captured console reports contained zero errors.
- The pure `tomcat-vscode-ext-0.1.72-pagination.vsix` was built before this commit request; ZIP integrity, exclusion of bundled CLI binaries, and byte-for-byte agreement of its CSS/JS with the accepted dist were checked. Generated screenshots, host profiles, and VSIX files remain ignored local artifacts.
- This commit intentionally does not rerun tests, builds, or coverage, as explicitly requested by the user. Commit-time checks are limited to Git scope, whitespace, status metadata, and commit-message format; no new coverage percentage is claimed.
