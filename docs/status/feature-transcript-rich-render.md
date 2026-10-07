### Metadata

| Field | Value |
| :--- | :--- |
| Updated | 2026-10-08 07:37 +0800 |
| State | ACTIVE |
| Branch | feature/transcript-rich-render |
| Scope | Agent Box pending-message queue, rich safe-point steering, provider isolation, compact Docks, unified Composer and single primary action; CLI/extension patch bump and local commit |
| Cov% | - |

### DONE

- Added an extension-owned, per-session in-memory queue distinct from Rust follow-up delivery: busy submissions enqueue, completed runs drain FIFO, and explicit send inserts at an existing safe point using the current task's mode/model. An idle new submission starts directly without replacing older queued items.
- Closed steering admission atomically at convergence and on every run outcome. Rich attachments/instruction snapshots use the existing message builder, remain unarchived until consumed, and emit stable-ID consumption receipts; Stop discards unconsumed steering while retaining and pausing ordinary queued items.
- Kept queue state content-only and volatile: Serve replacement discards pending items without deleting the main draft, chat history or shared blobs. Prompt mode/model changes validate and apply in the same command before starting a new run.
- Split provider reuse into shared route resources and isolated per-model instances so connection pools/concurrency gates remain shared without leaking thinking format, capabilities or learned provider fallback between models.
- Replaced Files/Todos tabs with compact independent Messages → Files → Todos Docks below Ask Question; kept shared scrolling and existing content typography, moved Undo All to the persistent Files header, and scoped plan-preview table styling locally.
- Reused one complete Composer surface for ordinary input and queued-message editing: Tip/notices → attachments → body → bottom controls. Editing adds only its title/Cancel, preserves the independent main draft/attachments, and restores focus after Save/Cancel; SVG preview uses the existing typed blob URL lifecycle.
- Enforced a single primary action: ordinary busy input with content shows only Send/enqueue, completely empty busy input shows only Stop, and idle shows Send disabled when empty. Queued editing only shows Save, never Stop; preparation/missing-attachment validity does not accidentally switch the action, and empty Enter never interrupts.
- Extended Planner and Plan Reviewer reuse requirements to each new mechanism and reviewer-proposed addition, requiring existing capability, gap and evidence rather than introducing avoidable abstractions.
- Applied exactly one repository-script patch bump: CLI `0.1.68 → 0.1.69`, extension `0.1.82 → 0.1.83`, bundled CLI `0.1.68 → 0.1.69`. Only the five approved version mirrors changed; dependency versions and private GUI metadata are unchanged.
- Retained earlier branch features: native full/fragment diff and file restore, historical message editing, isolated session drafts/attachments, Commands/Rules and slash resources, EOF recovery, independent speed/reasoning controls, MCP lifecycle/project trust, and requirement-driven planning. Detailed historical changes and evidence remain in Git rather than accumulating duplicate status blocks.

### INTERFACE

- Serve advertises `message_queue`; `steer` accepts optional `onlyIfRunning`, and `prompt` accepts optional `agentMode`/`model`. Existing callers remain valid, and unsupported queue capability keeps the older busy-input behavior.
- `agent_idle` carries `outcome` (`completed`, `interrupted`, `failed`); `steering_consumed` identifies `userMessageIds` only after archival/context insertion. Rust events, generated Serve schema/fixtures and extension wire types are synchronized.
- Host exposes per-session queue snapshots and queue actions, uses one dispatch arbiter, and checks draft revision/send attempt identity before clearing or reconciling asynchronous results. Queue storage is not durable or a new backend draft authority.
- Current-task insertion does not hot-switch its model; the next fresh run uses the composer's selected configuration. Runtime model hot-switching is explicitly deferred.
- Existing historical rewind, file-source identity, native restore protection and shared attachment ownership contracts remain intact. Stop remains run-directed, not bound to a particular queued task.
- Release metadata now declares CLI `0.1.69`, extension `0.1.83`, and bundled CLI pin `0.1.69`. Metadata is not a rebuilt binary or installed/published package; prompt templates remain compile-time embedded and require a later rebuild/new process to take effect.

### BLOCKED

- Whole-repository gates are not all green: the existing strict Clippy `large_enum_variant` finding and five unresolved Rust integration failures remain recorded in `.agents/acceptance/agent-box/delivery.md`. Focused retries do not turn the original aggregate failures into passing runs.
- This request explicitly skips tests, compilation, packaging, installation, restart, coverage measurement and push. Earlier implementation acceptance is historical evidence, not verification of rebuilt `0.1.69` / `0.1.83` artifacts. Cov% remains unmeasured (`-`).
- Pending messages are deliberately lost on Serve restart; the main draft/history/blobs remain. Short windows may require the existing outer/Dock scrolling rather than displaying every panel at once.
- Earlier branch limitations remain: Files source selection assumes stable manifest timestamps and cleanup is best-effort; fragment previews have no stable public VS Code tab identity; upstream gateway/cache/service-tier behavior is not an SLA; previously recorded external-agent/network/legacy-fixture failures are not silently resolved by this commit.
- Generated screenshots, disposable host profiles, local replay scripts, reports and VSIX files remain ignored artifacts. Maintained source, tests, generated wire fixtures, release metadata and this status belong in the commit.

### VERIFICATION

- This bump/commit request performs only Git-region/scope review, pre/post repository version consistency, the five-file version-only diff, whitespace, status metadata and what/why message checks. It does not run a test suite or build and does not fabricate a coverage percentage.
- Earlier Agent Box implementation checks covered 431 LLM cases, steering/schema boundaries, Host arbitration/state tests and real Serve FIFO/current-model/Stop/restart/rich-input behavior; precise task IDs and remaining aggregate failures are preserved in `.agents/acceptance/agent-box/delivery.md`.
- Before the final Tip reorder, valid GUI batches covered 757 unique cases; GUI/extension typechecks passed. Dev Host queue/Files/native-diff coverage and three installed scenarios passed, with PNG/ARIA/console evidence in `.agents/acceptance/agent-box/composer-unified.md`.
- The final Tip correction passed 73 owning Composer/queue tests and whitespace (`1791415321976-qdauiu`), plus a rebuilt real Dev Host queue scenario with actual notice/attachment/input coordinate assertions (`1791415338239-yv1ls2`). Desktop/narrow dark/light PNGs, ARIA and clean frame console reports were read in `.agents/shots/gui-IKwEFL/`; the earlier installed captures predate this reorder.
- The one-time version operation passed both `release-version.mjs check` calls and the five-file diff/whitespace review. No dependency, private GUI version or business code was changed by the bump itself.
