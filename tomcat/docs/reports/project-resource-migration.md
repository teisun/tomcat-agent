# Project resource-directory migration record

Date: 2026-09-08

## Result

No project-managed skill or plugin data was moved in this repository.

| Location checked | Observed contents | Action |
| --- | --- | --- |
| `.agents/skills/` | `source-command-commit-with-status`, `source-command-release-cli-ext` | Left unchanged; this is the configured/default project resource location. |
| `.tomcat/` | `.DS_Store`, `shots/` | Left unchanged; it contains no managed `skills/`, `plugins/`, `packages/`, or `mcp.json` data to migrate. |

## Safety rules applied

- The new resolver defaults to `.agents` and rejects legacy `.tomcat` as a configured project resource directory.
- No existing `.agents` content was overwritten, deleted, or renamed.
- No backup was needed because there was no old managed resource to move. If a future repository contains legacy managed files, create a timestamped backup first and move only into an empty destination after listing both sides.
