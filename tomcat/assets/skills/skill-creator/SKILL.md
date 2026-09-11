---
name: skill-creator
description: Create and refine Codex skills (folders containing `SKILL.md`). Use when you need help naming a skill, writing effective `name`/`description`, structuring the on-disk instructions, or adding scripts/references/assets for repeatable workflows.
license: Complete terms in LICENSE.txt
---

# Skill Creator

This skill provides guidance for creating effective **Codex** skills.

## How Codex Skills Work

- Ensure the experimental `skills` feature is enabled (config: `[features] skills = true`, or launch with `codex --enable skills`).
- Codex discovers skills from `~/.codex/skills/**/SKILL.md` (recursive). Only files named exactly `SKILL.md` count.
- Hidden entries and symlinks are skipped.
- Codex injects **only** `name`, `description`, and the file path into runtime context.
- The SKILL.md **body stays on disk** until you explicitly insert it (via `/skills` or by mentioning `$<skill-name>`).
- Skills are loaded once at startup; restart Codex after edits if you don’t see changes.

### What Skills Are For

- Reusable workflows: “how we do X here”
- Tooling playbooks: exact commands, flags, scripts, and verification loops
- Local reference: schemas, conventions, checklists, templates
- Bundled resources: scripts/assets that are better stored on disk than re-generated

## Core Principles

### Keep `name`/`description` High-Signal

Codex validates only two required frontmatter fields:

- `name`: non-empty, ≤100 characters, single line
- `description`: non-empty, ≤500 characters, single line

Extra YAML keys are ignored by Codex, but keep frontmatter minimal to avoid confusion.

Write descriptions so the right skill is obvious from the skills list:
- Include “what it does” **and** “when to use it” (triggers, file types, tools, domains).
- Prefer concrete keywords over generic phrasing (“BigQuery SQL”, “OpenAPI”, “Terraform”, “.docx”, “invoice PDFs”, “React”, “Postgres”).

### Keep The Body Skimmable

The body is only loaded when explicitly inserted, but once inserted it consumes context. Prefer:

- Short “Quick start” + “Decision tree” sections
- Links to on-disk `references/` docs for depth
- Scripts for repeated, deterministic work

### Match Guardrails To Fragility

Use more structure when mistakes are costly or workflows are brittle:

- High freedom: heuristics, checklists, examples
- Medium freedom: pseudocode, templates, scripts with parameters
- Low freedom: exact commands, strict templates, “do-not-skip” verification steps

## Recommended Skill Layout (Not Required)

Codex only cares about `SKILL.md`, but this structure keeps skills maintainable:

```
skill-name/
├── SKILL.md (required)
├── scripts/          # runnable helpers (optional)
├── references/       # docs you may paste/insert (optional)
└── assets/           # templates / binaries to copy (optional)
```

## Skill Creation Workflow

### Step 1: Collect Concrete Triggers

Start from real prompts you expect to see. Capture:
- “User says …” examples (3–8)
- Inputs involved (file types, repos, APIs, tools)
- Outputs expected (formats, quality bar, constraints)

### Step 2: Write Frontmatter (`name`, `description`)

Keep both fields single-line.

Template:

```markdown
---
name: your-skill-name
description: what it does + when to use it (<=500 chars, single line)
---
```

Recommendation: use lowercase hyphen-case names so `$your-skill-name` is easy to type.

### Step 3: Scaffold The Folder

Create a folder under `~/.codex/skills/`:

- Manual: `mkdir -p ~/.codex/skills/<skill-name>/`
- Scripted (from `~/.codex/skills`): `python3 skill-creator/scripts/init_skill.py <skill-name>` (see script help)

### Step 4: Write The Body (What To Do When Invoked)

Default structure that works for most skills:

- `## Quick start` (fastest safe path)
- `## Decision tree` (how to choose workflow)
- `## Workflows` (numbered steps; include verification loops)
- `## References` (what to open when)

Use these reference guides when helpful:
- Multi-step / branching processes: `skill-creator/references/workflows.md`
- Strict output formats / examples: `skill-creator/references/output-patterns.md`

### Step 5: Add On-Disk Resources

Add only what you’ll actually reuse:
- `scripts/`: runnable helpers you don’t want to re-create (make executable; document usage in SKILL.md)
- `references/`: deeper docs, schemas, checklists, examples
- `assets/`: templates/binaries to copy into outputs

### Step 6: Validate And Iterate

- Run `python3 ~/.codex/skills/skill-creator/scripts/quick_validate.py ~/.codex/skills/<skill-name>` for a fast check.
- Restart Codex and confirm the skill appears in `/skills`.
- Try a real task by invoking `$<skill-name>` and refine the skill based on what was missing.

## Tomcat adaptation

Tomcat discovers managed built-ins from its global work directory. To distribute a new skill, create a local package, validate it, confirm the requested scope, then call `package_install`. Do not copy files directly into a project resource directory.
