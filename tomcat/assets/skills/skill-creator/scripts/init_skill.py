#!/usr/bin/env python3
"""Create a portable Tomcat skill skeleton without overwriting existing work."""
import argparse
import re
import sys
from pathlib import Path

NAME = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
TEMPLATE = """---
name: {name}
description: "TODO: say what this skill does and when Tomcat should use it."
---

# {title}

## Quick start

TODO: Give the shortest safe path.

## Workflow

1. TODO: Inspect the inputs.
2. TODO: Produce the result.
3. TODO: Verify the result.

## References

Add a `references/` file only when detailed material is needed.
"""

def main():
    parser = argparse.ArgumentParser(description="Create a Tomcat SKILL.md skeleton.")
    parser.add_argument("name")
    parser.add_argument("--path", required=True, help="Parent directory for the new skill")
    args = parser.parse_args()
    if not NAME.fullmatch(args.name):
        raise SystemExit("name must be lowercase hyphen-case and at most 64 characters")
    root = Path(args.path).expanduser().resolve() / args.name
    if root.exists():
        raise SystemExit(f"Refusing to overwrite existing skill: {root}")
    root.mkdir(parents=True)
    (root / "SKILL.md").write_text(
        TEMPLATE.format(name=args.name, title=args.name.replace("-", " ").title()),
        encoding="utf-8",
    )
    print(root)

if __name__ == "__main__":
    main()
