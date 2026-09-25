### Metadata

| Field | Value |
| :--- | :--- |
| Updated | 2026-09-25 10:04 +0800 |
| State | ACTIVE |
| Branch | feature/transcript-rich-render |
| Scope | MCP concurrent-call lifecycle, recovery and project trust; connector Settings management and acceptance; tolerant MCP configuration migration; composer layout documentation; CLI/extension release metadata |
| Cov% | - |

### DONE

- Preserved independent unsent drafts, references, and attachments for every chat session across switching and webview reload.
- Prevented late storage/state results and send acknowledgements from deleting newer edits.
- Made real-service test fixtures use disposable storage without inheriting the parent agent-session guard.
- Updated release metadata through one repository-script patch bump: CLI `0.1.58 -> 0.1.59`, VS Code extension `0.1.72 -> 0.1.73`, and bundled CLI pin `0.1.58 -> 0.1.59`; only the five release-version mirror files changed.
- Synchronized planner and plan-reviewer explanation blocks verbatim with the updated engineering standards, including the Test case checklist, solution subsections, small gray decision details, and paths of tests to add or modify; kept core identity and code-reviewer role boundaries unchanged.
- Updated prompt contract tests to protect the complete wording, subsection order, nesting, and single-copy parity between planner and plan reviewer; replaced the stale core-identity assertion with the current beginner-friendly explanation requirement without weakening the check or restoring obsolete prompt text.
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
- Upgraded `rmcp` to 3.4.0 and replaced serialized MCP calls with per-source admission/execution quotas, request-scoped cancellation, progress-renewed idle windows, bounded cancellation delivery, and explicit HTTP ownership cleanup without replaying side effects.
- Split MCP lifecycle, call timing, connection, failure, login, recovery, and scoped HTTP responsibilities into focused modules; preserved shared clients across Serve sessions while isolating request results, stop actions, recovery budgets, and workspace scope identities.
- Replaced per-server command/integrity trust with one-time project-root trust: Global MCP remains available, Workspace MCP waits without spawning or starting OAuth, and CLI/VS Code can approve the project once.
- Made legacy `startupTimeoutMs`, `callTimeoutMs`, `maxConcurrentCalls`, `trusted`, and `integrity` keys warning-only; tolerated unknown file/server fields during reads, retained strict `toolFilter`/OAuth validation, and documented that the next successful save removes unrecognized keys.
- Expanded connector Settings with project-trust grouping, Add-and-Trust, per-tool toggles, login/logout, deterministic Reload receipts, recovery states, responsive detail views, state polling, and cached tool-catalog identity checks.
- Added controlled stdio/HTTP fault fixtures, real dual-Serve-session coverage, generated wire/schema updates, Settings frame drivers, installed VSIX acceptance, Reload/recovery/tool-management scenarios, and MCP concurrency evidence without conflating SDK probes with product acceptance.
- Documented the Composer input-height design and retained the connector Settings responsive shell behavior for narrow windows and long error/configuration text.
- Updated release metadata to CLI `0.1.60`, VS Code extension `0.1.74`, and bundled CLI `0.1.60`; built a pure `0.1.74` VSIX without an embedded CLI binary during delivery.

### INTERFACE

- The VS Code composer continues to show the active session's own unsent draft; switching sessions does not alter either draft.
- Release metadata declares VS Code extension `0.1.74` with bundled CLI `0.1.60`; changing these values is not evidence of publishing or installing the artifact, while the separately built pure VSIX intentionally contains no bundled CLI.
- Plan authoring and plan review now share the explicit Test case checklist contract; permissions, advisory review output, runtime protocols, and the core/code-review explanation scope are unchanged. Prompt templates remain compile-time embedded and require a rebuilt CLI/new process to take effect.
- CLI and serve model deletion now use `remove_user_model_with_config_path`; model selection and deletion share `with_current_model_catalog` to reject stale catalog choices.
- The remove-model response and settings state distinguish the deletion outcome from catalog-refresh feedback; matching `tomcat.plan.buildModel` is cleared before deletion.
- Pending question `N of M` denotes the current page, not the answered count; page navigation does not submit answers, and `Continue` still submits the collected answers together. The answer protocol is unchanged.
- MCP JSON runtime policy now lives under `[connector.mcp]`; per-server legacy keys are ignored with warnings, reads are byte-preserving, and a later successful mutation rewrites only recognized fields.
- `ProjectTrustStore` persists approved project roots in `{work_dir}/project-trust.json`; `get_project_trust` and `trust_project` replace per-service trust mutations, while Global sources remain independent of project approval.
- `ConnectorRegistry::create_and_start` starts eligible configured sources in background tasks; Chat and Settings resolve through the same process identity cache keyed by configuration path and explicit workspace context.
- `set_connector_tool_enabled`, connector Reload/login/logout commands, connection generation/attempt/recovery fields, and management tool catalogs are exposed through the Serve wire contract and consumed by Settings.
- MCP calls use the process-level `[connector.mcp]` startup/call timeout and per-source concurrency policy; cancellation and disconnect errors do not imply safe replay or guaranteed remote rollback.
- Settings reads connector state from local Serve snapshots and polls while visible; it fetches a tool catalog only for a connected source and validates config key, generation, and attempt before applying it.

### BLOCKED

- The earlier Rust full integration run was not fully green: `completion_flow_test` expects Chinese acceptance wording, and `missing_live_credentials_is_not_a_successful_skip` has an exit-code mismatch. These previously recorded failures are not repaired by this commit.
- A pure extension VSIX was built during implementation acceptance, but this commit does not verify installation in the user's daily profile or the artifact loaded by its current window.
- Large fonts, very short windows, or long questions may still require scrolling within the current question; preserving the height cap does not guarantee every possible question fits fully above the fold.
- Full Rust gate evidence retains two pre-existing failures: the stale planner acceptance-wording assertion and the missing-live-credentials preflight that sees the configured default provider key. Focused MCP, Clippy, schema, real-LLM, and extension batches are recorded in the architecture acceptance notes rather than treating these baselines as new regressions.
- Settings connection state is snapshot/poll based rather than pushed live from Serve; a newly opened connector route may briefly display its previous/default state until `list_connectors` and the subsequent tool-catalog request settle.
- A legacy orphan HTTP fixture process was observed during acceptance but could not be attributed to the current run and was intentionally not terminated; current controlled fixture processes were verified to exit.

### VERIFICATION

- The 2026-09-20 implementation acceptance for `plan_add_model_id_edit_id_aee801f9` recorded a passing extension `npm run gate:full`, focused Rust/GUI/extension checks, schema/wire checks, and desktop/mobile browser checks with zero console errors. These are earlier results, not new runs for this commit.
- The 2026-09-21 acceptance for `plan_verify_4885bbfb` recorded five passing focused Rust contract tests (tasks `1789964332987-e8fgyh` and `1789964544981-u8z0jk`) and prompt-source alignment checks. Those runs preceded the later Markdown formatting and paragraph relocation; they are not new test runs for this commit.
- Subsequent static checks confirmed unchanged non-whitespace skill content apart from the exact paragraph relocation, byte-for-byte preserved ASCII/command blocks, valid version-command structure, and synchronized release metadata.
- The question-panel implementation acceptance recorded 66 focused GUI tests; after the final Other-focus change, its 12 owning component tests and GUI typecheck passed again (task `1789995584792-ibytf5`). These precede this commit request.
- The final browser matrix passed nine scenarios with PNG/ARIA/console evidence (task `1789995584849-ibdcbp`); ordinary short-question pages passed full visibility checks, while narrow/large-font and long-question cases explicitly exercised scrolling rather than claiming full first-screen visibility.
- The existing real-host suite passed its seven checks (task `1789995181374-ee1dfl`); the final native mouse/keyboard acceptance also passed pagination, Other focus, and submission at 120% zoom after waiting for both frame layouts to settle (task `1789995953920-a2543y`). Both pages measured a 665px webview and a 150.44px panel, with each prompt and all three options visible; captured console reports contained zero errors.
- The pure `tomcat-vscode-ext-0.1.72-pagination.vsix` was built before this commit request; ZIP integrity, exclusion of bundled CLI binaries, and byte-for-byte agreement of its CSS/JS with the accepted dist were checked. Generated screenshots, host profiles, and VSIX files remain ignored local artifacts.
- Prompt implementation acceptance initially reproduced the stale core-identity wording failure (task `1789997511479-etozf7`), then recorded 27 passing prompt tests plus that same failure (task `1789998173938-uafahn`). After the user requested its repair, `cargo test --manifest-path tomcat/Cargo.toml --lib core::prompts::tests::load_test` passed all 28 tests (task `1789998744359-mrbnfr`); this run preceded the version-only bump and this commit request.
- Source-wording parity, unchanged surrounding prompt contracts, and role boundaries were checked during implementation; the repaired test file passed `rustfmt --check` and `git diff --check` (task `1789998762644-mtxqmt`). These are earlier checks, not new runs for this commit.
- The one-time version bump passed both pre/post `release-version.mjs check` calls (task `1789999059484-ecdt8q`), and the five-file version diff and whitespace check passed (task `1789999073438-uf5ula`); no dependencies, private GUI version, builds, installations, or published artifacts were changed by that operation.
- MCP concurrency/recovery implementation evidence includes passing focused stdio/HTTP manager tests, real shared-Serve-session checks, Clippy, generated schema verification, release CLI builds, extension core/GUI/integration gates, installed Settings acceptance, and a real DeepWiki dual-session field run. Detailed task IDs, hashes, failures, and evidence boundaries are preserved in `tomcat/docs/architecture/connector-mcp/mcp-concurrency-{sdk-evidence,acceptance}.md`.
- The MCP configuration compatibility acceptance passed 14 config tests, 2 builtin tests, 10 chat-context tests, 3 project-trust CLI tests, 8 connector MCP integration tests, Clippy with all targets and the HTTP test feature, formatting, and whitespace checks.
- A pure `tomcat-vscode-ext-0.1.74.vsix` was built successfully with 220 files; archive extraction and required entrypoints passed, and SHA-256 `6f63e5abc2d8248d0b715ea14f2c0b43f6f3079cce32110aa78d5b3cb18e6dd8` confirmed no bundled `bin/tomcat` or `bin/tomcat.exe`.
- This commit intentionally does not rerun tests, builds, or coverage, as explicitly requested by the user. Commit-time checks are limited to Git scope, whitespace, status metadata, and commit-message format; no new coverage percentage is claimed.
