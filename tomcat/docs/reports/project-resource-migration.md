# Project resource-directory migration record

Date: 2026-09-13

## 2026-09-13 verified screenshot migration

The repository-root legacy `.tomcat/` directory contained 23 files: 22 files below
`shots/` plus `.DS_Store`. It contained no managed `skills/`, `plugins/`,
`packages/`, or `mcp.json` data.

- Inventory: each source file was recorded with relative path, byte size, and SHA-256.
- Recoverable backup: `/Users/yankeben/.tomcat/backups/tomcat-agent-20260913-022229/`
  contains all 23 files and `SHA256SUMS.json`. All backup hashes and sizes were
  verified before migration.
- Migration: the 22 `shots/` files were copied without overwriting into
  repository-root `.agents/shots/`, retaining their relative paths. Their hashes and
  sizes were verified against the backup manifest after the copy.
- Cleanup: the 22 verified source screenshots and the legacy `.DS_Store` were
  removed one by one; the now-empty `.tomcat/shots/remediation/`,
  `.tomcat/shots/`, and `.tomcat/` directories were removed with `rmdir`.

The existing `.agents/skills/` content was not overwritten, deleted, or renamed.

## Historical result (2026-09-08, not independently revalidated)

No project-managed skill or plugin data was moved in this repository.

| Location checked | Observed contents | Action |
| --- | --- | --- |
| `.agents/skills/` | `source-command-commit-with-status`, `source-command-release-cli-ext` | Left unchanged; this is the configured/default project resource location. |
| `.tomcat/` | `.DS_Store`, `shots/` | Left unchanged; it contains no managed `skills/`, `plugins/`, `packages/`, or `mcp.json` data to migrate. |

## Historical safety rules (2026-09-08)

- The new resolver defaults to `.agents` and rejects legacy `.tomcat` as a configured project resource directory.
- No existing `.agents` content was overwritten, deleted, or renamed.
- No backup was needed because there was no old managed resource to move. If a future repository contains legacy managed files, create a timestamped backup first and move only into an empty destination after listing both sides.

## Current safety rules

- The resolver defaults to `.agents` and rejects legacy `.tomcat` as a configured project resource directory.
- The migration preflight rejects every destination collision before copying any file.
- Future migrations must create and verify a timestamped backup before deleting a source file.
