### Metadata

| Field | Value |
| :--- | :--- |
| Updated | 2026-10-02 19:17 +0800 |
| State | ACTIVE |
| Branch | feature/transcript-rich-render |
| Scope | OpenAI Fast/Ultrafast service speeds and Max reasoning; end-to-end Speed protocol and model configuration preservation; Settings example placeholder; CLI/extension release metadata |
| Cov% | - |

### DONE

- Added independent OpenAI service speed and reasoning controls: Fast maps to `service_tier: "priority"`, Ultrafast to `"ultrafast"`, and Max remains a reasoning effort; Ultra multi-agent execution and WebSocket transport changes are excluded.
- Declared accelerated capabilities per complete catalog model ID, inherited matching built-in declarations, persisted model speed preferences, and resolved unsupported saved speeds to Standard without implicitly enabling a paid option.
- Unified the Rust/extension/chat/plan Speed request and receipt field as `speed`, regenerated wire/schema fixtures, required the intermediate GUI callbacks, and aligned Composer menu closing with its sibling controls.
- Degraded non-OpenAI speed declarations during user catalog loading while keeping admin/built-in validation strict; preserved unknown top-level model fields across model add/edit/remove, including edits to the same ID.
- Kept comma-separated effort/speed drafts intact while typing, normalized only save payloads, and changed the empty Supported speeds placeholder to `e.g. fast, ultrafast`; the example never becomes a saved value.
- Unified all smoke probe tiers on bounded non-streaming JSON with dry-run, no retries, and report overwrite protection; retained capability declarations when upstream tier echoes are unconfirmed rather than treating that as proof of no acceleration.
- Applied this request's one-time repository-script patch bump: CLI `0.1.62 -> 0.1.63`, extension `0.1.76 -> 0.1.77`, and bundled CLI `0.1.62 -> 0.1.63`; no dependency versions, builds, tests, installations or published artifacts changed during the bump/commit operation.

- Isolated unreadable Skill/plugin entries so one bad file no longer freezes the category; healthy additions/removals continue, the reconciled inventory matches an independent new session, and repaired entries return on reload.
- Shared `SkillSet::discovery_failed()` between `/reload` and `/skill reload` to preserve the old category only when root discovery fails, rather than for a single unreadable file.
- Made `call_timeout_ms = 0` disable plugin startup waiting limits as well as call timeouts; retained initialization-error reporting, cleanup, and same-key retry.
- Protected Serve turn handle installation and completion with the existing `run_task` mutex so an old completion cannot erase a following task's handle or release busy before cleanup is synchronized.
- Parsed shared command arguments into `SharedInvocation` before execution, removed the corresponding `unwrap()`/`unreachable!`, and corrected slash `--force`, existing-VM reuse, and shutdown documentation.
- Synchronized release metadata to CLI `0.1.61`, VS Code extension `0.1.75`, and bundled CLI pin `0.1.61`; the separately built pure `0.1.75` VSIX remains an ignored local artifact.

- Added explicit session resource synchronization through `/reload`, shared with `/install` and `/uninstall` in terminal and VS Code; external changes require reload rather than a disk scan before every prompt.
- Matched effective plugin IDs by source/content fingerprint, skipped staging directories, preserved the last good inventory after root/registry discovery failures, and retired revoked capabilities without aborting other sessions' current calls.
- Kept VM birth manifests for permission checks, rejected stale registry writes, and made session/plugin initialization single-flight with readiness/error receipts and retry after failed initialization.
- Moved slow Serve maintenance commands into session jobs so another session can continue; rehydrated terminal memory after `/restore` and resumed restored pending questions.
- Added shared-table composer suggestions and pending-command controls, real external-uninstall/request-boundary regressions, multi-session checks, and responsive browser capture coverage.
- Repaired the existing planner-wording assertion, default-provider credential isolation, and serial-group/nextest contract; kept request-source tickets and in-flight counters out of the trimmed implementation.

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

- `ModelEntry`/`ModelView` expose `supported_speeds` / `supportedSpeeds` and the effective `selectedSpeed`; Standard is implicit and an empty accelerated list hides Speed options.
- `set_speed { model, speed, sessionId? }` and its receipt use `speed`; the old `tier` key is rejected. Chat, plan preview, Settings and the generated Serve contracts carry the same capability data.
- OpenAI adapters alone translate Speed into `service_tier`; `reasoning.effort` remains independent. Anthropic request bodies do not gain OpenAI speed fields.
- User catalog reads can clear invalid API/speed combinations with a warning; writes still reject them. Unknown model-top-level TOML values are preserved, not arbitrary comments/formatting or unknown nested capability fields.

- `SkillSet::discovery_failed()` distinguishes root discovery failure from per-file warnings; both reload paths use the same predicate, and unreadable entries stop appearing in the effective inventory until repaired.
- `PluginEngineConfig::call_timeout_ms = 0` means unlimited startup readiness waiting; nonzero startup deadlines and initialization-failure cleanup remain in place.
- Shared slash command names, layer aliases, and `SlashReply`/Serve wire contracts are unchanged; slash `/install` does not accept `--force`, while the outer CLI retains its own force-install option.

- Serve initialization advertises `slashCommands` and `run_slash_command`; `run_slash_command` accepts a session ID plus raw text and returns shared `SlashReply { ok, text }` without interactive prompts or model requests.
- `/uninstall` takes a package-ledger name, not a tool name; manually copied resources must be deleted from their layer's `plugins/` or `skills/` directory before `/reload`.
- Resource reload updates the current session and shared inventory; other sessions finish the current round and synchronize at their next epoch boundary. MCP, builtin tools, and model configuration are outside this reload contract.
- Command-pending state disables Send, Compact, and Build until the maintenance reply; terminal restore shares durable-context rehydration with Compact and Serve.

- The VS Code composer continues to show the active session's own unsent draft; switching sessions does not alter either draft.
- Release metadata now declares CLI `0.1.63`, VS Code extension `0.1.77`, and bundled CLI pin `0.1.63`; this metadata-only bump does not rebuild or replace the previously deployed `0.1.62` / `0.1.76` runtime.
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

- The latest Speed implementation's original Rust/extension aggregate gates exited 1 under the conditions recorded during acceptance; failed cases passed focused rechecks, Rust subsequently passed all 2979 library tests in a separate single-thread run, and the remaining installed-E2E/VSIX stages passed. These staged results do not retroactively make the original aggregate commands green or prove default-concurrency stability.
- Six bounded JSON probes were accepted with HTTP 200, but all four accelerated requests echoed `default`; upstream acceleration eligibility, actual tier and billing remain unconfirmed. This is not proof that Fast/Ultrafast provides no acceleration, and no declarations were removed.
- The precise historical writer/event that dropped model speed declarations remains unverified. Preservation tests and restored runtime/UI data do not identify the original cause.
- No test, build, package, install, restart, coverage run or push is part of this requested bump/commit. Existing installation and implementation evidence describe the earlier runtime, not newly built `0.1.63` / `0.1.77` artifacts.

- The latest remediation's complete `gate-fast` run exited 1 because the new error-path test used a Clippy-rejected assertion. After fixing it, full all-targets Clippy and both affected regressions passed; the complete library/doc/integration results remain valid. The initial exit 1 is retained, not relabeled as one all-green command.
- Earlier resource-inventory gate failures and focused closure results remain historical evidence; the later complete remediation run passed all library and integration tests.
- Earlier implementation acceptance deployed and checked a remediation CLI/extension pair; this metadata-only commit does not install or verify a new `0.1.63` / `0.1.77` artifact in the user's daily profile.
- Large fonts, very short windows, or long questions may still require scrolling within the current question; preserving the height cap does not guarantee every possible question fits fully above the fold.
- The unchanged Git large-output timing assertion failed under load and passed alone in 3.25s; load sensitivity remains a risk. Browser acceptance used the production App/TipTap with a simulated webview host plus separately verified real Serve/host boundaries, not an installed-profile end-to-end manual walkthrough.
- Per-turn disk reconciliation remains disabled: the debug/test-profile no-change medians were about 13ms for 30 Skills/5 plugins and 351ms for 1000 Skills/20 plugins, above the plan's auto-sync thresholds. The isolated-HOME real-project PTY observed project MCP 401 warnings; these are retained as environment evidence, not claimed as MCP reload acceptance.
- Settings connection state is snapshot/poll based rather than pushed live from Serve; a newly opened connector route may briefly display its previous/default state until `list_connectors` and the subsequent tool-catalog request settle.
- A legacy orphan HTTP fixture process was observed during acceptance but could not be attributed to the current run and was intentionally not terminated; current controlled fixture processes were verified to exit.

### VERIFICATION

- This bump/commit request explicitly skips tests, builds and coverage. Commit-time validation is limited to repository version consistency, the five-file version-only delta, Git scope/whitespace, status metadata and commit-message format; Cov% remains unmeasured (`-`).
- Earlier Speed acceptance passed Rust declaration/unknown-field/Speed-wire regressions, generated schema checks, extension host checks and GUI tests. Its later independent library run passed 2979 tests / 0 failed / 3 ignored (`1790936321885-i644nb`); valid Clippy/doc/integration results and extension core 563 / GUI 654 passes were reused, while five init-fixture failures passed their focused recheck and remaining installed E2E/VSIX stages passed. These are historical results, not reruns for this commit.
- Earlier visual/runtime checks covered desktop/narrow PNG/ARIA/console states with zero capture-scope errors, nine effective model declarations through the connected Settings list/form, and real shared Model Edit menus for dual-speed and Fast-only capabilities. No user model/speed/effort preference changed during UI inspection.
- The final example-placeholder change passed all 27 SettingsApp tests plus GUI/extension typechecks (`1790938762393-ipek8n`, `1790938969819-yert3w`), and local production Settings desktop/narrow browser captures. This local fixture page is not a claim that the installed VSIX was rebuilt for the new placeholder.
- This request's version operation passed pre/post consistency and allowed-field checks (`1790939801036-ybpv7o`): CLI `0.1.63`, extension `0.1.77`, bundled CLI `0.1.63`; dependencies and private GUI metadata were unchanged.

- Remediation acceptance completed the full `gate-fast` command (`1790851903945-ucldnd`): 2967 library tests passed / 3 ignored, doctest had 0 cases, parallel integration had 363 passed / 26 skipped, and serial integration had 32 passed. Its exit 1 came from the initial Clippy assertion issue described above.
- After fixing that assertion, formatting, complete `cargo clippy --all-targets -- -D warnings`, both zero-timeout regressions, and whitespace checks passed (`1790853571921-23wfnj`). Earlier focused checks also covered bad-file isolation/recovery, controlled turn completion, session jobs, and unchanged shared-command behavior.
- The complete extension `npm test` passed 530 host tests and 644 GUI tests (`1790851903982-yz2g4s`); these are implementation acceptance results, not reruns for this commit.
- Final version/scope checks passed (`1790854496587-ud369l`). The three unrelated gate-configuration fixes were already committed in `11d61f60`; this remediation leaves those files unchanged and does not rewrite that history.
- The pure `tomcat-vscode-ext-0.1.75.vsix` was built and checked before this commit request (`1790846968883-2ufpc1`, `1790847479940-67seee`): 220 files, no bundled CLI, archive integrity passed, SHA-256 `d8a538748565816d4c7112725d4ebc66650c8d3745e63b09ec099eb674f9b1d4`.

- The resource-inventory implementation acceptance recorded the real process regression red to green: external CLI uninstall, same live session `/reload`, then actual model system/tools without the removed resources. Task `1790819140533-2osppy` also recorded the initial full gate failures, 2962 passing lib tests, 359 passing parallel integration tests, and 32 passing serial integration tests; its failure result is preserved.
- Focused closure passed all-targets Clippy, shared/session-job tests, the four-scenario timing matrix (`1790821426978-em3rel`), the unchanged Git timeout test (`1790823581755-itk8fd`), and 5 reminder plus 5 integration-gate configuration tests with lib/tests Clippy (`1790823725702-ambbw9`).
- Extension acceptance passed 530 host tests, 644 GUI tests, lint and 2 real Serve integration tests (`1790817201495-ap1ww5`); the final UI production edits passed GUI lint and 50 Composer tests (`1790818326722-e0koku`). Eight latest PNG/ARIA/console captures passed with zero console errors (`1790824012649-2hpbio`).
- Real PTY acceptance passed all three resource commands and verified that the actual post-restore request excluded later conversation rounds (`1790823725739-ywndxz`). Ten reloads of the real project directory with isolated HOME had median 2.9ms, maximum 5.1ms, and zero model requests (`1790823867158-e32zqm`).
- Wire/version and final diff/console checks passed before this commit request (`1790824127537-ych0ml`, `1790829374515-25c8sr`). Detailed local evidence and timing data remain in `.agents/shots/resource-slash/PR-description.md`; generated captures, request logs, temporary profiles and backups stay ignored. CLI/extension versions were not changed by resource reload.

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
