# Upstream attribution and adaptation record

The starting point is the `skill-creator` sample in the Codex source tree at
commit `6478a751fde8884b2fdc76486fe23175a8e795d4`:
`codex-rs/skills/src/assets/samples/skill-creator/`.

The upstream source is licensed under Apache License 2.0. `LICENSE.txt` is the
verbatim license from that source. The port keeps the upstream `SKILL.md`,
`scripts/init_skill.py`, and `scripts/quick_validate.py` as its documented
basis, then changes only the target-environment instructions and portable
skill format rules described below.

## Included files

| File | Purpose | Tomcat adaptation |
| --- | --- | --- |
| `SKILL.md` | Guidance for creating and maintaining skills | Uses Tomcat discovery, `load_skill`, and `package_install` rather than another product's home directory or UI metadata. |
| `scripts/init_skill.py` | Creates a named skill directory and optional resource directories | Does not create product-specific UI metadata; output is a portable Tomcat skill. |
| `scripts/quick_validate.py` | Checks the portable `SKILL.md` contract | Applies Tomcat's discovery-byte and YAML-value rules. |
| `references/` | Details that are loaded only when relevant | Contains only Tomcat-specific package and portable-skill guidance. |

## Deliberately omitted upstream attachments

The upstream `agents/openai.yaml`, `references/openai_yaml.md`,
`scripts/generate_openai_yaml.py`, and two icon assets are UI-specific and are
not part of the portable Tomcat skill format. They are omitted rather than
silently repurposed. No generated UI configuration is required to create or
use a Tomcat skill.

## Adaptation boundaries

- A skill is created in a user-chosen source directory. The initializer never
  installs it or overwrites an existing directory.
- Installing is a separate user-authorized operation. `scope` needs an
  explicit project root; `agent` and `global` require their own confirmation.
- An explorer is read-only. It can inspect and validate artifacts but cannot
  claim to have executed a change.
- The public format remains one `SKILL.md` file with YAML frontmatter. The
  initializer uses quoted string values and the validator checks parsed YAML,
  not merely text that resembles YAML.
