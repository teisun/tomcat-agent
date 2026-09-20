### Metadata

| Field | Value |
| --- | --- |
| Updated | 2026-09-20 11:47 +0800 |
| State | ACTIVE |
| Scope | Single-list context refactor and aligned CLI/extension patch release |

### DONE

- Preserved independent unsent drafts, references, and attachments for every chat session across switching and webview reload.
- Prevented late storage/state results and send acknowledgements from deleting newer edits.
- Made real-service test fixtures use disposable storage without inheriting the parent agent-session guard.
- Bumped the CLI to `0.1.53`, the VS Code extension to `0.1.67`, and the bundled CLI pin to `0.1.53`.
- Synchronized planner and plan-reviewer prompt contracts so every solution includes an ASCII-and-text explanation plus a plain-language key-decisions checklist with concrete scope details.

### INTERFACE

- The VS Code composer continues to show the active session's own unsent draft; switching sessions does not alter either draft.
- The extension packages CLI `0.1.53` inside VS Code extension `0.1.67`.

### BLOCKED

- None.
