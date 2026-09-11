#!/usr/bin/env python3
"""Validate the portable Tomcat SKILL.md header."""
import re
import sys
from pathlib import Path

NAME = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")

def fail(message):
    print(message)
    raise SystemExit(1)

def main():
    if len(sys.argv) != 2:
        fail("Usage: quick_validate.py <skill-directory>")
    skill = Path(sys.argv[1]) / "SKILL.md"
    try:
        text = skill.read_text(encoding="utf-8")
    except OSError as error:
        fail(f"Cannot read {skill}: {error}")
    if not text.startswith("---\n"):
        fail("SKILL.md must begin with YAML frontmatter")
    end = text.find("\n---\n", 4)
    if end < 0:
        fail("SKILL.md frontmatter is not closed")
    if len(text[:end + 5].encode("utf-8")) > 4 * 1024:
        fail("SKILL.md frontmatter must fit in Tomcat's 4 KiB discovery window")
    header = {}
    for line in text[4:end].splitlines():
        if not line or line.lstrip().startswith("#"):
            continue
        if ":" not in line:
            fail(f"Invalid frontmatter line: {line!r}")
        key, value = line.split(":", 1)
        key = key.strip()
        value = value.strip()
        if not key or key in header:
            fail(f"Frontmatter keys must be non-empty and unique: {key!r}")
        if value.startswith('"') != value.endswith('"'):
            fail(f"Unclosed quoted value for {key}")
        header[key] = value.strip('"')
    name = header.get("name", "")
    description = header.get("description", "")
    if not NAME.fullmatch(name):
        fail("name must be lowercase hyphen-case and at most 64 characters")
    if not description or "\n" in description:
        fail("description must be a non-empty single line")
    print("Skill is valid for Tomcat.")

if __name__ == "__main__":
    main()
