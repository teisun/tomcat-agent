### Metadata

| Field | Value |
| --- | --- |
| Updated | 2026-09-14 16:36 +0800 |
| State | DONE |
| Scope | Per-session composer draft persistence, acceptance-test isolation, and release version bump |

### DONE

- Preserved independent unsent drafts, references, and attachments for every chat session across switching and webview reload.
- Prevented late storage/state results and send acknowledgements from deleting newer edits.
- Made real-service test fixtures use disposable storage without inheriting the parent agent-session guard.
- Bumped the CLI to `0.1.52`, the VS Code extension to `0.1.66`, and the bundled CLI pin to `0.1.52`.

### INTERFACE

- The VS Code composer continues to show the active session's own unsent draft; switching sessions does not alter either draft.
- The extension packages CLI `0.1.52` inside VS Code extension `0.1.66`.

### BLOCKED

- None.
