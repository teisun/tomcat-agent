#!/usr/bin/env python3
"""Create a portable Tomcat skill source tree without overwriting work."""
import argparse
import json
import re
import sys
from pathlib import Path

NAME = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
RESOURCES = {"scripts", "references", "assets"}
TEMPLATE = """---
name: {name}
description: "TODO: describe what this skill does and when it applies."
---

# {title}

TODO: Add only the task-specific guidance an agent needs.
"""


def normalize(raw: str) -> str:
    return re.sub(r"-{2,}", "-", re.sub(r"[^a-z0-9]+", "-", raw.strip().lower())).strip("-")


def main() -> None:
    parser = argparse.ArgumentParser(description="Create a portable Tomcat SKILL.md source tree.")
    parser.add_argument("name", help="Skill name; it is normalized to lowercase hyphen-case")
    parser.add_argument("--path", required=True, help="Parent directory for the new source skill")
    parser.add_argument("--resources", default="", help="Comma-separated: scripts,references,assets")
    parser.add_argument("--examples", action="store_true", help="Create removable example files")
    args = parser.parse_args()

    name = normalize(args.name)
    if not NAME.fullmatch(name):
        raise SystemExit("name must contain 1–64 lowercase letters, digits, or hyphens")
    requested = [item.strip() for item in args.resources.split(",") if item.strip()]
    invalid = sorted(set(requested) - RESOURCES)
    if invalid:
        raise SystemExit(f"unknown resource type(s): {', '.join(invalid)}")
    if args.examples and not requested:
        raise SystemExit("--examples requires --resources")

    root = Path(args.path).expanduser().resolve() / name
    if root.exists():
        raise SystemExit(f"refusing to overwrite existing skill: {root}")
    root.mkdir(parents=True)
    title = " ".join(word.capitalize() for word in name.split("-"))
    # JSON strings are valid quoted YAML scalars and keep YAML booleans as strings.
    frontmatter = TEMPLATE.format(name=json.dumps(name), title=title)
    (root / "SKILL.md").write_text(frontmatter, encoding="utf-8")
    for resource in dict.fromkeys(requested):
        directory = root / resource
        directory.mkdir()
        if args.examples:
            if resource == "scripts":
                (directory / "example.py").write_text("#!/usr/bin/env python3\nprint('replace or remove this example')\n", encoding="utf-8")
            elif resource == "references":
                (directory / "example.md").write_text("# Example reference\n\nReplace or remove this file.\n", encoding="utf-8")
            else:
                (directory / "example.txt").write_text("Replace or remove this file.\n", encoding="utf-8")
    print(root)


if __name__ == "__main__":
    main()
