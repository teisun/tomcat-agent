### Metadata

| Field | Value |
| :--- | :--- |
| Updated | 2026-10-08 13:55 +0800 |
| State | ACTIVE |
| Branch | feature/transcript-rich-render |
| Scope | Agent Box queue remediation: authoritative session configuration, busy configuration locks across all Composers, preserved historical draft on Stop, and strict steering without starting a new task; local commit only |
| Cov% | - |

### DONE

- Removed the Host's nextConfig/composerConfig copy and prompt configuration fields so Build and other backend mode changes remain authoritative; queued prompts carry content only instead of restoring stale Plan/model selections.
- Locked mode/model/effort/speed while the real session is busy across normal, queued and historical Composers; closed already-open menus using existing controls while keeping content/attachments editable and submit/save/resend semantics unchanged.
- Preserved the historical editor and dirty draft when the existing Stop button receives an actual pointerdown/click; configuration remains locked while stopping and unlocks only after backend idle, not at click time.
- Kept strict steering in the existing inbox branch even if busy changes during preparation, preventing an implicit new task and duplicate queue execution; retained non-strict legacy behavior and strengthened the existing no-start/no-history endpoint assertions.
- Migrated existing queue integration tests and made the planned Build regression explicitly exercise the queue; no extra F05 ordering cases, production test hooks, menu framework, new dependencies or state authority were introduced.

- Added an extension-owned, per-session in-memory queue distinct from Rust follow-up delivery: busy submissions enqueue, completed runs drain FIFO, and explicit send inserts at an existing safe point using the current task's mode/model. An idle new submission starts directly without replacing older queued items.
- Closed steering admission atomically at convergence and on every run outcome. Rich attachments/instruction snapshots use the existing message builder, remain unarchived until consumed, and emit stable-ID consumption receipts; Stop discards unconsumed steering while retaining and pausing ordinary queued items.
- Kept queue state content-only and volatile: Serve replacement discards pending items without deleting the main draft, chat history or shared blobs. Ordinary prompts now use the backend session's current configuration; idle configuration changes use the existing setter RPCs.
- Split provider reuse into shared route resources and isolated per-model instances so connection pools/concurrency gates remain shared without leaking thinking format, capabilities or learned provider fallback between models.
- Replaced Files/Todos tabs with compact independent Messages → Files → Todos Docks below Ask Question; kept shared scrolling and existing content typography, moved Undo All to the persistent Files header, and scoped plan-preview table styling locally.
- Reused one complete Composer surface for ordinary input and queued-message editing: Tip/notices → attachments → body → bottom controls. Editing adds only its title/Cancel, preserves the independent main draft/attachments, and restores focus after Save/Cancel; SVG preview uses the existing typed blob URL lifecycle.
- Enforced a single primary action: ordinary busy input with content shows only Send/enqueue, completely empty busy input shows only Stop, and idle shows Send disabled when empty. Queued editing only shows Save, never Stop; preparation/missing-attachment validity does not accidentally switch the action, and empty Enter never interrupts.
- Extended Planner and Plan Reviewer reuse requirements to each new mechanism and reviewer-proposed addition, requiring existing capability, gap and evidence rather than introducing avoidable abstractions.
- Retained the already-committed versions: CLI `0.1.69`, extension `0.1.83`, bundled CLI `0.1.69`. This remediation commit changes no release, dependency or private GUI version fields.
- Retained earlier branch features: native full/fragment diff and file restore, historical message editing, isolated session drafts/attachments, Commands/Rules and slash resources, EOF recovery, independent speed/reasoning controls, MCP lifecycle/project trust, and requirement-driven planning. Detailed historical changes and evidence remain in Git rather than accumulating duplicate status blocks.

### INTERFACE

- Serve still advertises `message_queue`, and `steer` retains optional `onlyIfRunning`. `ServeMessageParams` no longer exposes `agentMode/model`; the shared flattened `RewindMessage` representation and now-unused generated `AgentMode` definition disappear accordingly. Existing generators synchronized extension wire and Rust schema fixtures.
- `agent_idle` carries `outcome` (`completed`, `interrupted`, `failed`); `steering_consumed` identifies `userMessageIds` only after archival/context insertion. Rust events, generated Serve schema/fixtures and extension wire types are synchronized.
- Host exposes per-session queue snapshots and queue actions, uses one dispatch arbiter, and checks draft revision/send attempt identity before clearing or reconciling asynchronous results. Queue storage is not durable or a new backend draft authority.
- Current-task insertion does not hot-switch its model; subsequent fresh turns use the backend session's current mode/model. The GUI derives configuration permission from real session busy, independent of editor-specific save/resend presentation; Host ignores busy/starting/in-flight mode/model changes. Runtime model hot-switching remains deferred.
- Existing historical rewind, file-source identity, native restore protection and shared attachment ownership contracts remain intact. Stop remains run-directed, not bound to a particular queued task.
- Release metadata now declares CLI `0.1.69`, extension `0.1.83`, and bundled CLI pin `0.1.69`. Metadata is not a rebuilt binary or installed/published package; prompt templates remain compile-time embedded and require a later rebuild/new process to take effect.

### BLOCKED

- Historical whole-repository gate failures are not relabeled green: strict Clippy large_enum_variant and five Rust integration failures remain documented in `.agents/acceptance/agent-box/delivery.md:67-74`. The gateway SSE fixture and old skill XML assertion are identified; three search timeouts still lack a clean-baseline reproduction. This remediation did not rerun or repair those gates and does not exempt new failures by count.
- This local commit reuses the just-completed implementation checks and performs scope/status/whitespace validation only; it does not repeat tests or build, package, install, restart or push. Implementation used a generated CLI for targeted tests and Vite for light UI acceptance, not a new installed release. Cov% remains unmeasured (`-`).
- Pending messages are deliberately lost on Serve restart; the main draft/history/blobs remain. Short windows may require the existing outer/Dock scrolling rather than displaying every panel at once.
- Earlier branch limitations remain: Files source selection assumes stable manifest timestamps and cleanup is best-effort; fragment previews have no stable public VS Code tab identity; upstream gateway/cache/service-tier behavior is not an SLA; previously recorded external-agent/network/legacy-fixture failures are not silently resolved by this commit.
- Generated screenshots, disposable host profiles, local replay scripts, reports and VSIX files remain ignored artifacts. All 24 maintained source/test/generated-wire files from this remediation, plus this status update, belong in the commit; version metadata is unchanged.

### VERIFICATION

- Completed plan `plan_09290499_2_build_plan_8af24132` before this commit request. Evidence and limitations are recorded in `.agents/acceptance/agent-box/queue-remediation.md`; earlier branch history remains available in Git.
- Rust: 193 Serve unit tests, project gen:wire and regenerated schema fixture test passed (`1791435698106-aovlvr`); strict control flow cannot fall through to persistence/start_turn. This is not a deterministic reproduction of the narrow race window.
- Host: 100 queue/provider unit tests and 58 provider-flow integration tests passed (`1791435962220-e3jczi`); four real Serve queue integration cases passed (`1791437023084-igtuwz`), including unchanged FIFO, Stop, rich-input and restart behavior.
- GUI: 173 owning cases plus 587 remaining cases passed (`1791436511112-aifc3o`), covering 760 unique tests. After the native-pointer Stop fix, the affected 98 App/Inline cases and GUI typecheck passed again (`1791437218474-quk73r`); unchanged results were reused rather than rerunning the package.
- Extension typecheck against new wire passed (`1791437071975-yelned`); focused Rust formatting/whitespace passed (`1791436907088-mt6c9m`), and final scope/whitespace passed (`1791437545454-2songc`). The initial formatting failure and wrong-working-directory lint ENOENT are retained in the report, not treated as functional regressions.
- Six saved PNG/ARIA pairs across 960×800 and 390×844, dark/light, were read under `.agents/shots/queue-remediation/`; browser console reported 0 errors and 0 warnings. Production GUI used controlled Webview state injection, including real pointer Stop and subsequent idle; devhost/installed E2E was deliberately not rerun.
- This commit introduces no further application changes after that acceptance. Commit-time checks cover all Git regions, complete staging, status metadata, whitespace and the what/why message; no new test run or coverage percentage is claimed.
