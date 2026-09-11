---
name: plugin-creator
description: Create or improve a Tomcat plugin, validate its manifest and permissions, package it, and install it with package_install at a user-approved scope.
---

# Plugin Creator

Use this skill when a user asks for a new Tomcat plugin, a change to an existing plugin, or a reusable package containing plugins and skills.

## Safe workflow

1. Start with the smallest plugin that can prove the needed behavior.
2. Give it a lowercase hyphen-case `id`; it is a directory name, so never use slashes, `..`, or an absolute path.
3. List only the permissions and network hosts the plugin really needs. A network permission requires an explicit host list.
4. Validate `plugin.json`, run the plugin’s focused test, then package it.
5. Ask the user which scope should receive the package. Use `package_install`; do not manually copy plugin files to managed directories.
6. Verify that the next session inventory sees the plugin. A currently loaded plugin is not hot-replaced.

## Files

```text
my-plugin/
├── plugin.json
└── main.js
```

Use `scripts/init_plugin.mjs <plugin-id> --out <directory>` to create a small valid skeleton. Replace the placeholder tool before packaging.

## Package forms

- Plugin only: package a directory with `plugin.json`.
- Skill only: package a directory with `SKILL.md`.
- Combined: use `package.json` with `tomcat.plugins` and `tomcat.skills`, each pointing to a directory under the package root.

Never make a package resource reference leave its package root or cross a symbolic link.
