| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| tomcat | 2026-10-10 20:50 +0800 | ACTIVE | feature/transcript-rich-render | - |

### DONE

- [✓] Added bilingual product controls, actionable messages and CLI output, with a gear entry to Settings and a shared language preference. Rust renders its own text and TS its own UI; user/model content and historical text are not rewritten.
- [✓] Added icon-only session pin/unpin/delete actions with localized tooltip/ARIA. Busy sessions cannot be deleted; existing occupancy locks protect deletion, late title rewrites cannot recreate deleted transcripts, and cleanup failures remain visible even if opening the fallback session also fails.
- [✓] Plan Review requires an explicit choice and does not start on a timeout. Answers and ready replay preserve the create_plan parent card. The existing question-card interaction is retained; the proposed Revert-style dialog was withdrawn without code changes.
- [✓] General adopts a new connection's language snapshot while retaining successful saves within the same connection. Reconnecting disables saving; existing drafts, model overrides and environment-override semantics are preserved.
- [✓] Removed model/developer-only translation entries and unnecessary mechanisms while preserving safety checks. Extended the shared anti-overengineering guidance with concise reader, ownership and evidence rules rather than an implementation diary.
- [✓] Retained prior branch Files/Keep/Undo, tool-owned media, volatile queue, rewind, provider routing and MCP work. Earlier implementation history remains in Git; accepted scope and retention boundaries were not broadened.
- [✓] Ran the project version script once: CLI `0.1.70 → 0.1.71`, extension `0.1.84 → 0.1.85`, bundled CLI declaration `0.1.70 → 0.1.71`. Only the five version files' version fields changed relative to the pre-bump workspace; dependency contents, private GUI version and existing work were preserved.

### INTERFACE

- Configuration adds `ui.language`; Serve initialization exposes `uiPreferences`, and `set_ui_language` saves through the existing configuration workflow. The saving window updates immediately; other processes read on restart. Environment overrides remain authoritative, and VS Code manifest text follows VS Code's language.
- Serve `delete_session` reports cleanup warnings after committed deletion. Pin preferences stay in extension workspace state. Session deletion does not erase workspace files, plans, shared checkpoints, audit trails or remote uploads; sidecar cleanup remains best-effort and empty lock-file residue is allowed.
- Review uses the existing question request/response and `allowCustom: false`; stable option IDs, not translated strings, decide whether to run the reviewer. Stop/disconnect and parent-tool identity protections remain intact; no separate modal protocol was added.
- Rust schema/d.ts fixtures and extension wire types include the new capabilities. Existing Files/Keep and tool-media contracts remain intact.
- Release metadata now declares CLI/bundled CLI `0.1.71` and extension `0.1.85`. This commit does not produce, install or publish those versions. The previously verified pure VSIX remains a historical `0.1.84` artifact.

### BLOCKED

- Whole-project gates are not all green. Four existing Clippy findings remain: `large_enum_variant` in chat command parsing, `type_complexity`/`redundant_closure` in checkpoint session files, and `items_after_test_module` in multimodal code. No allow attributes, removed assertions or unrelated refactors were used to hide them.
- Original failed runs remain failures: the Rust library run exposed two preheat-cache fixture omissions, fixed and verified separately; extension integration initially timed out warming Cargo for three cases, which passed on focused continuation. The four historical proxy-sensitive web-search cases passed this round without changing system proxy settings; the proxy root cause is not claimed fixed.
- Two UI recapture attempts failed during CDP/startup before screenshots. The subsequent affected journey passed; startup flakiness is not claimed resolved. Windows real-host cancellation/locking and paid real-LLM plan-review quality remain unverified. Cov% remains unmeasured (`-`), so the commit message omits coverage.
- Prior boundaries remain: remote file_id upload previously returned HTTP 404; same-turn cached-HEAD and wall-clock Keep ordering risks remain accepted; legacy backup compatibility and stable public VS Code fragment-preview tab identity are not added here.
- Local screenshots, logs, disposable host profiles, acceptance reports and VSIX files remain ignored. This is a local source/status commit only: no test-suite rerun, build, packaging, installation, restart, publication or push during version/commit preparation.

### VERIFICATION

- Reused the completed product/remediation acceptance documented in `tomcat-vscode-ext/docs/architecture/product-i18n.md`; detailed local logs remain in `.agents/acceptance/product-plan-rereview.md`. This reuse is not a claim that newly bumped binaries were built or tested.
- Extension acceptance: typechecks and 717 Host + 795 GUI tests passed; integration had 180 initial passes and three successful focused continuations. The original `gate:full` command remains exit 1.
- Rust acceptance: library 3168 passed / two fixture failures / three ignored; the corrected two fixtures passed 2/2 with original assertions retained. Offline integration passed 371 parallel + 35 serial cases, with 26 skipped. The long transcript-read cases completed successfully; zero discovered doctests are not behavioral coverage. The original gate remains exit 1.
- Installed UI: 23 passed with five conditional pending; the separate distribution matrix passed 10 cases. Final evidence contains 36 PNG/ARIA/console triplets, including actual light-theme rendering, preserved Review parent cards and open-General restart. Captured webview error arrays were empty; fake Serve is UI evidence, not disk-safety evidence.
- Version consistency passed before and after the single bump (`1791636586660-6b1pzs`). Byte/content comparison confirmed only the five allowed version files changed during that operation, with no dependency or GUI version changes. Version-file whitespace check passed.
- Commit preparation checks all three Git regions, the complete related staged set, status/message format and whitespace. Historical gate failures and unverified boundaries are retained rather than represented as a fresh all-green run.
